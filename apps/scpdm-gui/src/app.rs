//! The window: folders and filters on the left, the designs in the middle, the selected design's
//! properties, links and history on the right.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Local, TimeZone};
use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use serde::{Deserialize, Serialize};
use vault::{Entry, Status, Vault, Version};

use crate::scan::{self, Info, Scan};

/// How often the vault is rescanned while the window has focus (cheap: unchanged files are
/// recognised by size and modification time).
const RESCAN_EVERY: Duration = Duration::from_secs(3);
/// SolidCraft's portable thumbnail size.
const THUMB: egui::Vec2 = egui::vec2(240.0, 150.0);
const ROW_THUMB: egui::Vec2 = egui::vec2(64.0, 40.0);
const HISTORY_THUMB: egui::Vec2 = egui::vec2(80.0, 50.0);
const MAX_RECENT: usize = 8;

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    last_vault: Option<PathBuf>,
    recent: Vec<PathBuf>,
    /// Program and arguments that open a design (the file is appended); empty for automatic.
    open_command: String,
}

#[derive(Clone, PartialEq)]
enum View {
    All,
    Status(Status),
    /// Everything below this folder.
    Folder(String),
}

enum Dialog {
    Restore { path: String, number: u32 },
    Rename { from: String, to: String },
    CheckInAll { comment: String, untracked: bool },
    MakeVault { dir: PathBuf },
    Settings { open_command: String },
}

/// What the user asked for while the frame was drawn; applied after drawing.
enum Action {
    Select(String),
    Open(String),
    ShowInFolder(String),
    CheckIn { path: String, comment: String },
    CheckInAll { comment: String, untracked: bool },
    AskRestore { path: String, number: u32 },
    Restore { path: String, number: u32 },
    SaveCopy { path: String, version: Version },
    Rename { from: String, to: String },
    Relink { missing: String, to: String },
    PickVault,
    NewVault,
    OpenVault(PathBuf),
    MakeVault(PathBuf),
    Refresh,
    SetOpenCommand(String),
}

pub struct App {
    settings: Settings,
    vault: Option<Vault>,
    entries: Vec<Entry>,
    /// Working files' properties, by content hash.
    infos: HashMap<String, Info>,
    /// Thumbnail PNGs by key (content hash for working files, object hash for versions).
    thumbs: HashMap<String, Option<Arc<[u8]>>>,
    scan: Option<Receiver<Result<Scan, String>>>,
    rescan: bool,
    last_scan: Instant,
    view: View,
    search: String,
    selected: Option<String>,
    history: Vec<Version>,
    comment: String,
    message: Option<(String, bool)>,
    dialog: Option<Dialog>,
    focused: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext, start: Option<PathBuf>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let settings: Settings = cc.storage.and_then(|s| eframe::get_value(s, eframe::APP_KEY)).unwrap_or_default();
        let mut app = App {
            settings,
            vault: None,
            entries: Vec::new(),
            infos: HashMap::new(),
            thumbs: HashMap::new(),
            scan: None,
            rescan: false,
            last_scan: Instant::now(),
            view: View::All,
            search: String::new(),
            selected: None,
            history: Vec::new(),
            comment: String::new(),
            message: None,
            dialog: None,
            focused: false,
        };
        let ctx = &cc.egui_ctx;
        match start {
            Some(dir) => app.open_vault(ctx, dir),
            None => {
                if let Some(last) = app.settings.last_vault.clone().filter(|p| p.join(vault::PDM_DIR).is_dir()) {
                    app.open_vault(ctx, last);
                }
            }
        }
        app
    }

    /// Selects a design by vault path.
    pub fn select(&mut self, path: &str) {
        if self.selected.as_deref() != Some(path) {
            self.selected = Some(path.to_owned());
            self.comment.clear();
            self.load_history();
        }
    }

    /// Whether a background scan is running.
    pub fn is_scanning(&self) -> bool {
        self.scan.is_some()
    }

    // ---- state ----

    fn open_vault(&mut self, ctx: &egui::Context, dir: PathBuf) {
        match Vault::discover(&dir) {
            Ok(vault) => {
                let root = vault.root().to_path_buf();
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                    "{} — SolidCraft PDM",
                    root.file_name().map_or(root.display().to_string(), |n| n.to_string_lossy().into_owned())
                )));
                self.settings.recent.retain(|p| *p != root);
                self.settings.recent.insert(0, root.clone());
                self.settings.recent.truncate(MAX_RECENT);
                self.settings.last_vault = Some(root);
                self.vault = Some(vault);
                self.entries.clear();
                self.infos.clear();
                self.thumbs.clear();
                self.scan = None;
                self.view = View::All;
                self.selected = None;
                self.history.clear();
                self.message = None;
                self.start_scan(ctx);
            }
            Err(vault::Error::NotAVault(_)) => self.dialog = Some(Dialog::MakeVault { dir }),
            Err(e) => self.error(e),
        }
    }

    fn start_scan(&mut self, ctx: &egui::Context) {
        let Some(vault) = &self.vault else { return };
        self.last_scan = Instant::now();
        if self.scan.is_some() {
            self.rescan = true;
            return;
        }
        let known = self.infos.keys().cloned().collect();
        self.scan = Some(scan::spawn(vault.root().to_path_buf(), known, ctx.clone()));
    }

    fn poll_scan(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.scan else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.scan = None;
        match result {
            Ok(scan) => {
                for (hash, info) in scan.infos {
                    let png = info.as_ref().as_ref().ok().and_then(|i| i.thumbnail_png.as_deref()).map(Arc::from);
                    self.thumbs.insert(hash.clone(), png);
                    self.infos.insert(hash, info);
                }
                let live: HashSet<&str> = scan.entries.iter().filter_map(|e| e.hash.as_deref()).collect();
                self.infos.retain(|h, _| live.contains(h.as_str()));
                self.entries = scan.entries;
                if self.selected.as_ref().is_some_and(|s| !self.entries.iter().any(|e| &e.path == s)) {
                    self.selected = None;
                }
                self.load_history();
            }
            Err(e) => self.error(e),
        }
        if std::mem::take(&mut self.rescan) {
            self.start_scan(ctx);
        }
    }

    fn load_history(&mut self) {
        self.history = match (&self.vault, &self.selected) {
            (Some(vault), Some(path)) => vault.history(path).unwrap_or_default(),
            _ => Vec::new(),
        };
    }

    fn entry(&self, path: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.path == path)
    }

    fn info(&self, entry: &Entry) -> Option<&scformat::DesignInfo> {
        self.infos.get(entry.hash.as_deref()?)?.as_ref().as_ref().ok()
    }

    /// The thumbnail of a working file, or of its latest version when the file is missing.
    fn entry_thumb(&mut self, entry: &Entry) -> Option<egui::Image<'static>> {
        match &entry.hash {
            Some(hash) => self.image(hash),
            None => self.version_thumb(entry.latest.as_ref()?),
        }
    }

    fn version_thumb(&mut self, version: &Version) -> Option<egui::Image<'static>> {
        let key = version.thumbnail.as_ref()?;
        if !self.thumbs.contains_key(key) {
            let png = self.vault.as_ref().and_then(|v| v.thumbnail(version).ok().flatten()).map(Arc::from);
            self.thumbs.insert(key.clone(), png);
        }
        self.image(key)
    }

    fn image(&self, key: &str) -> Option<egui::Image<'static>> {
        let png = self.thumbs.get(key)?.clone()?;
        Some(egui::Image::from_bytes(format!("bytes://{key}.png"), egui::load::Bytes::Shared(png)))
    }

    fn visible(&self) -> Vec<usize> {
        let needle = self.search.to_lowercase();
        (0..self.entries.len())
            .filter(|&i| {
                let e = &self.entries[i];
                let in_view = match &self.view {
                    View::All => true,
                    View::Status(s) => e.status == *s,
                    View::Folder(f) => e.path.starts_with(&format!("{f}/")),
                };
                in_view && (needle.is_empty() || self.searchable(e).to_lowercase().contains(&needle))
            })
            .collect()
    }

    fn searchable(&self, e: &Entry) -> String {
        let mut s = e.path.clone();
        if let Some(i) = self.info(e) {
            for v in [&i.name, &i.metadata.part_number, &i.metadata.description, &i.metadata.project].into_iter().flatten() {
                s.push('\n');
                s.push_str(v);
            }
        } else if let Some(v) = &e.latest {
            for v in [&v.name, &v.part_number].into_iter().flatten() {
                s.push('\n');
                s.push_str(v);
            }
        }
        s
    }

    fn count(&self, status: Status) -> usize {
        self.entries.iter().filter(|e| e.status == status).count()
    }

    fn info_msg(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), false));
    }

    fn error(&mut self, e: impl std::fmt::Display) {
        self.message = Some((e.to_string(), true));
    }

    // ---- actions ----

    fn apply(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::Select(path) => self.select(&path),
            Action::Open(path) => self.open_design(&path),
            Action::ShowInFolder(path) => {
                let Some(vault) = &self.vault else { return };
                let dir = vault.absolute(&path).parent().map(Path::to_path_buf).unwrap_or_default();
                if let Err(e) = std::process::Command::new("xdg-open").arg(dir).spawn() {
                    self.error(format!("Could not open the folder: {e}"));
                }
            }
            Action::CheckIn { path, comment } => {
                let Some(vault) = &mut self.vault else { return };
                match vault.check_in(&path, &comment) {
                    Ok(v) => {
                        self.comment.clear();
                        self.info_msg(format!("Checked in {path} as v{}", v.number));
                        self.load_history();
                        self.start_scan(ctx);
                    }
                    Err(e) => self.error(e),
                }
            }
            Action::CheckInAll { comment, untracked } => {
                let Some(vault) = &mut self.vault else { return };
                let paths: Vec<String> = self
                    .entries
                    .iter()
                    .filter(|e| e.status == Status::Modified || (untracked && e.status == Status::Untracked))
                    .map(|e| e.path.clone())
                    .collect();
                let mut done = 0;
                let mut failed = Vec::new();
                for path in &paths {
                    match vault.check_in(path, &comment) {
                        Ok(_) => done += 1,
                        Err(vault::Error::Unchanged(_)) => {}
                        Err(e) => failed.push(format!("{path}: {e}")),
                    }
                }
                if failed.is_empty() {
                    self.info_msg(format!("Checked in {done} designs"));
                } else {
                    self.error(format!("Checked in {done}; failed: {}", failed.join("; ")));
                }
                self.load_history();
                self.start_scan(ctx);
            }
            Action::AskRestore { path, number } => self.dialog = Some(Dialog::Restore { path, number }),
            Action::Restore { path, number } => {
                let Some(vault) = &mut self.vault else { return };
                match vault.restore(&path, number) {
                    Ok(v) => {
                        self.info_msg(format!("Restored v{number} of {path} (now v{})", v.number));
                        self.load_history();
                        self.start_scan(ctx);
                    }
                    Err(e) => self.error(e),
                }
            }
            Action::SaveCopy { path, version } => {
                let Some(vault) = &self.vault else { return };
                let file = vault.absolute(&path);
                let stem = file.file_stem().map_or("design".into(), |s| s.to_string_lossy().into_owned());
                let ext = file.extension().map_or("solidcraft".into(), |s| s.to_string_lossy().into_owned());
                let mut dialog = rfd::FileDialog::new().set_file_name(format!("{stem}-v{}.{ext}", version.number));
                if let Some(dir) = file.parent() {
                    dialog = dialog.set_directory(dir);
                }
                let Some(out) = dialog.save_file() else { return };
                match vault.version_bytes(&version).map_err(|e| e.to_string()).and_then(|b| {
                    std::fs::write(&out, b).map_err(|e| e.to_string())
                }) {
                    Ok(()) => {
                        self.info_msg(format!("Saved v{} as {}", version.number, out.display()));
                        self.start_scan(ctx);
                    }
                    Err(e) => self.error(e),
                }
            }
            Action::Rename { from, to } => {
                let Some(vault) = &mut self.vault else { return };
                let to = to.trim().trim_matches('/').to_owned();
                match vault.rename(&from, &to) {
                    Ok(()) => {
                        self.info_msg(format!("Moved {from} to {to}"));
                        self.selected = Some(to);
                        self.load_history();
                        self.start_scan(ctx);
                    }
                    Err(e) => self.error(e),
                }
            }
            Action::Relink { missing, to } => {
                let Some(vault) = &mut self.vault else { return };
                match vault.relink(&missing, &to) {
                    Ok(()) => {
                        self.info_msg(format!("{to} now has the history of {missing}"));
                        self.load_history();
                        self.start_scan(ctx);
                    }
                    Err(e) => self.error(e),
                }
            }
            Action::PickVault => {
                if let Some(dir) = rfd::FileDialog::new().set_title("Open vault").pick_folder() {
                    self.open_vault(ctx, dir);
                }
            }
            Action::NewVault => {
                if let Some(dir) = rfd::FileDialog::new().set_title("Folder for the new vault").pick_folder() {
                    self.apply(ctx, Action::MakeVault(dir));
                }
            }
            Action::OpenVault(dir) => self.open_vault(ctx, dir),
            Action::MakeVault(dir) => match Vault::init(&dir) {
                Ok(_) | Err(vault::Error::AlreadyAVault(_)) => self.open_vault(ctx, dir),
                Err(e) => self.error(e),
            },
            Action::Refresh => self.start_scan(ctx),
            Action::SetOpenCommand(cmd) => self.settings.open_command = cmd.trim().to_owned(),
        }
    }

    fn open_design(&mut self, path: &str) {
        let Some(vault) = &self.vault else { return };
        let file = vault.absolute(path);
        let command: Vec<String> = if self.settings.open_command.is_empty() {
            vec![if on_path("solidcraft") { "solidcraft" } else { "xdg-open" }.to_owned()]
        } else {
            self.settings.open_command.split_whitespace().map(str::to_owned).collect()
        };
        let Some((program, args)) = command.split_first() else { return };
        if let Err(e) = std::process::Command::new(program).args(args).arg(&file).spawn() {
            self.error(format!("Could not run {program}: {e} (set the program in Vault > Settings)"));
        }
    }

    // ---- drawing ----

    fn top_bar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("Vault", |ui| {
                if ui.button("Open vault…").clicked() {
                    actions.push(Action::PickVault);
                }
                if ui.button("New vault…").clicked() {
                    actions.push(Action::NewVault);
                }
                ui.menu_button("Recent", |ui| {
                    if self.settings.recent.is_empty() {
                        ui.weak("No recent vaults");
                    }
                    for p in &self.settings.recent {
                        if ui.button(p.display().to_string()).clicked() {
                            actions.push(Action::OpenVault(p.clone()));
                        }
                    }
                });
                ui.separator();
                if ui.button("Settings…").clicked() {
                    self.dialog = Some(Dialog::Settings { open_command: self.settings.open_command.clone() });
                }
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            if self.vault.is_none() {
                return;
            }
            ui.separator();
            if ui.button("⟳ Refresh").on_hover_text("Rescan the vault (F5)").clicked() {
                actions.push(Action::Refresh);
            }
            let changes = self.count(Status::Modified) + self.count(Status::Untracked);
            if ui
                .add_enabled(changes > 0, egui::Button::new("⬆ Check in all…"))
                .on_hover_text("Record every modified (and optionally untracked) design as a new version")
                .clicked()
            {
                self.dialog = Some(Dialog::CheckInAll { comment: String::new(), untracked: true });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search name, part no…").desired_width(220.0));
                if self.scan.is_some() {
                    ui.spinner();
                }
            });
        });
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(vault) = &self.vault {
                ui.weak(vault.root().display().to_string());
                ui.separator();
                ui.label(format!(
                    "{} designs · {} modified · {} untracked · {} missing",
                    self.entries.len(),
                    self.count(Status::Modified),
                    self.count(Status::Untracked),
                    self.count(Status::Missing)
                ));
            }
            if let Some((text, is_error)) = &self.message {
                ui.separator();
                if *is_error {
                    ui.colored_label(ui.visuals().error_fg_color, text);
                } else {
                    ui.label(text);
                }
            }
        });
    }

    fn left_panel(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(4.0);
            let total = self.entries.len();
            let mut view = self.view.clone();
            ui.selectable_value(&mut view, View::All, format!("All designs  ({total})"));
            for status in [Status::Modified, Status::Untracked, Status::Missing] {
                let n = self.count(status);
                if n > 0 || self.view == View::Status(status) {
                    let (label, color) = status_style(status);
                    let text = RichText::new(format!("{label}  ({n})")).color(color);
                    ui.selectable_value(&mut view, View::Status(status), text);
                }
            }
            ui.add_space(8.0);
            ui.label(RichText::new("Folders").strong());
            let folders = vault::design_folders(&self.entries);
            let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
            for f in &folders {
                children.entry(f.rsplit_once('/').map_or("", |(p, _)| p)).or_default().push(f);
            }
            if folders.is_empty() {
                ui.weak("No subfolders");
            }
            self.folder_tree(ui, "", &children, &mut view);
            self.view = view;
        });
    }

    fn folder_tree(&self, ui: &mut egui::Ui, parent: &str, children: &BTreeMap<&str, Vec<&str>>, view: &mut View) {
        for &folder in children.get(parent).map(Vec::as_slice).unwrap_or_default() {
            let name = folder.rsplit_once('/').map_or(folder, |(_, n)| n);
            let prefix = format!("{folder}/");
            let n = self.entries.iter().filter(|e| e.path.starts_with(&prefix)).count();
            let this = View::Folder(folder.to_owned());
            let label = format!("{name}  ({n})");
            if children.contains_key(folder) {
                let id = ui.make_persistent_id(("folder", folder));
                egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
                    .show_header(ui, |ui| ui.selectable_value(view, this, label))
                    .body(|ui| self.folder_tree(ui, folder, children, view));
            } else {
                ui.horizontal(|ui| {
                    ui.add_space(ui.spacing().indent);
                    ui.selectable_value(view, this, label);
                });
            }
        }
    }

    fn table(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let visible = self.visible();
        if visible.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.weak(if self.entries.is_empty() {
                    if self.scan.is_some() { "Scanning…" } else { "No .solidcraft files in this vault yet" }
                } else {
                    "Nothing matches"
                });
            });
            return;
        }
        // Thumbnails are looked up (and version thumbnails loaded) before the table borrows self.
        let mut thumbs = Vec::with_capacity(visible.len());
        for &i in &visible {
            let e = self.entries[i].clone();
            thumbs.push(self.entry_thumb(&e));
        }
        let text_height = ui.text_style_height(&egui::TextStyle::Body);
        let row_height = (ROW_THUMB.y + 4.0).max(text_height * 2.4);
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(ROW_THUMB.x + 8.0))
            .column(Column::initial(230.0).at_least(120.0).clip(true))
            .column(Column::initial(96.0))
            .column(Column::initial(40.0))
            .column(Column::initial(110.0).clip(true))
            .column(Column::initial(40.0))
            .column(Column::remainder().at_least(118.0))
            .header(text_height + 8.0, |mut h| {
                for title in ["", "Design", "Status", "Ver", "Part number", "Rev", "Modified"] {
                    h.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(row_height, visible.len(), |mut row| {
                    let n = row.index();
                    let e = &self.entries[visible[n]];
                    let info = self.info(e);
                    let latest = e.latest.as_ref();
                    row.set_selected(self.selected.as_ref() == Some(&e.path));
                    row.col(|ui| thumbnail(ui, thumbs[n].clone(), ROW_THUMB));
                    row.col(|ui| {
                        ui.vertical(|ui| {
                            let file = e.path.rsplit_once('/').map_or(e.path.as_str(), |(_, f)| f);
                            ui.add(egui::Label::new(RichText::new(file).strong()).truncate().selectable(false));
                            let name = info.and_then(|i| i.name.clone()).or_else(|| latest.and_then(|v| v.name.clone()));
                            let folder = e.path.rsplit_once('/').map_or("", |(d, _)| d);
                            let sub = [name.as_deref().unwrap_or(""), folder]
                                .into_iter()
                                .filter(|s| !s.is_empty())
                                .collect::<Vec<_>>()
                                .join("  ·  ");
                            ui.add(egui::Label::new(RichText::new(sub).weak().small()).truncate().selectable(false));
                        });
                    });
                    row.col(|ui| {
                        let (label, color) = status_style(e.status);
                        ui.add(egui::Label::new(RichText::new(label).color(color)).selectable(false));
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(latest.map_or("-".into(), |v| format!("v{}", v.number))).selectable(false));
                    });
                    let props = info.map(|i| &i.metadata);
                    let part = props.and_then(|m| m.part_number.clone()).or_else(|| latest.and_then(|v| v.part_number.clone()));
                    let rev = props.and_then(|m| m.revision.clone()).or_else(|| latest.and_then(|v| v.revision.clone()));
                    row.col(|ui| {
                        ui.add(egui::Label::new(part.unwrap_or_else(|| "-".into())).truncate().selectable(false));
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(rev.unwrap_or_else(|| "-".into())).selectable(false));
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(e.modified.map_or("-".into(), system_time)).selectable(false));
                    });
                    let response = row.response();
                    if response.clicked() {
                        actions.push(Action::Select(e.path.clone()));
                    }
                    if response.double_clicked() && e.status != Status::Missing {
                        actions.push(Action::Open(e.path.clone()));
                    }
                    response.context_menu(|ui| {
                        actions.push(Action::Select(e.path.clone()));
                        if e.status != Status::Missing {
                            if ui.button("Open in SolidCraft").clicked() {
                                actions.push(Action::Open(e.path.clone()));
                                ui.close();
                            }
                            if ui.button("Show in folder").clicked() {
                                actions.push(Action::ShowInFolder(e.path.clone()));
                                ui.close();
                            }
                        }
                    });
                });
            });
    }

    fn details(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(entry) = self.selected.as_deref().and_then(|p| self.entry(p)).cloned() else {
            ui.centered_and_justified(|ui| {
                ui.weak("Select a design to see its properties and history");
            });
            return;
        };
        let info = self.info(&entry).cloned();
        let latest = entry.latest.clone();
        let image = self.entry_thumb(&entry);
        let file = entry.path.rsplit_once('/').map_or(entry.path.as_str(), |(_, f)| f).to_owned();

        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.add_space(6.0);
            thumbnail(ui, image, THUMB);
            ui.add_space(6.0);
            ui.heading(&file);
            ui.weak(&entry.path);
            ui.horizontal(|ui| {
                let (label, color) = status_style(entry.status);
                ui.label(RichText::new(label).color(color).strong());
                if let Some(v) = &latest {
                    ui.label(format!("· v{} of {}", v.number, self.history.len()));
                }
            });

            if let Some(from) = &entry.moved_from {
                ui.add_space(6.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.label(format!("This looks like {from}, moved here outside the PDM."));
                    if ui.button("Keep its history (relink)").clicked() {
                        actions.push(Action::Relink { missing: from.clone(), to: entry.path.clone() });
                    }
                });
            }

            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                let present = entry.status != Status::Missing;
                if ui.add_enabled(present, egui::Button::new("Open in SolidCraft")).clicked() {
                    actions.push(Action::Open(entry.path.clone()));
                }
                if ui.add_enabled(present, egui::Button::new("Show in folder")).clicked() {
                    actions.push(Action::ShowInFolder(entry.path.clone()));
                }
                if ui.add_enabled(present, egui::Button::new("Move / rename…")).clicked() {
                    self.dialog = Some(Dialog::Rename { from: entry.path.clone(), to: entry.path.clone() });
                }
            });

            if matches!(entry.status, Status::Modified | Status::Untracked) {
                ui.add_space(10.0);
                ui.label(RichText::new(if entry.status == Status::Untracked { "Add to vault" } else { "Check in" }).strong());
                ui.add(
                    egui::TextEdit::multiline(&mut self.comment)
                        .hint_text("What changed?")
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                let label = if entry.status == Status::Untracked { "Add as v1" } else { "Check in as new version" };
                let next = latest.as_ref().map_or(1, |v| v.number + 1);
                if ui.button(format!("⬆ {label}")).on_hover_text(format!("Records the file's current content as v{next}")).clicked() {
                    actions.push(Action::CheckIn { path: entry.path.clone(), comment: self.comment.clone() });
                }
            }

            ui.add_space(10.0);
            egui::CollapsingHeader::new(RichText::new("Properties").strong()).default_open(true).show(ui, |ui| {
                properties(ui, &entry, info.as_ref(), latest.as_ref());
            });

            if let Some(info) = &info {
                if !info.links.is_empty() {
                    egui::CollapsingHeader::new(RichText::new(format!("Uses ({})", info.links.len())).strong())
                        .default_open(true)
                        .show(ui, |ui| {
                            for link in &info.links {
                                let target = resolve_link(&entry.path, &link.path);
                                let component =
                                    info.components.iter().find(|c| c.id == link.component).map_or("?", |c| c.name.as_str());
                                self.link_row(ui, component, &link.path, target, actions);
                            }
                        });
                }
            }
            let used_in = self.used_in(&entry.path);
            if !used_in.is_empty() {
                egui::CollapsingHeader::new(RichText::new(format!("Used in ({})", used_in.len())).strong())
                    .default_open(true)
                    .show(ui, |ui| {
                        for path in used_in {
                            if ui.link(&path).clicked() {
                                actions.push(Action::Select(path));
                            }
                        }
                    });
            }

            egui::CollapsingHeader::new(RichText::new(format!("History ({})", self.history.len())).strong())
                .default_open(true)
                .show(ui, |ui| {
                    if self.history.is_empty() {
                        ui.weak("Not in the vault yet. Add it to start its history.");
                    }
                    let history = self.history.clone();
                    for v in &history {
                        let is_working = entry.hash.as_deref() == Some(v.hash.as_str());
                        let image = self.version_thumb(v);
                        self.version_row(ui, &entry, v, is_working, image, actions);
                    }
                });
        });
    }

    fn link_row(&self, ui: &mut egui::Ui, component: &str, shown: &str, target: Option<String>, actions: &mut Vec<Action>) {
        ui.horizontal_wrapped(|ui| {
            ui.weak(format!("{component}:"));
            match target.clone().filter(|t| self.entry(t).is_some_and(|e| e.status != Status::Missing)) {
                Some(t) => {
                    if ui.link(&t).on_hover_text(format!("Stored as {shown}")).clicked() {
                        actions.push(Action::Select(t));
                    }
                }
                None => {
                    let text = target.unwrap_or_else(|| shown.to_owned());
                    ui.colored_label(ui.visuals().error_fg_color, format!("{text} (missing)"))
                        .on_hover_text(format!("Stored as {shown}"));
                }
            }
        });
    }

    /// Designs in the vault whose links point at `path`.
    fn used_in(&self, path: &str) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| {
                self.info(e)
                    .is_some_and(|i| i.links.iter().any(|l| resolve_link(&e.path, &l.path).as_deref() == Some(path)))
            })
            .map(|e| e.path.clone())
            .collect()
    }

    fn version_row(
        &self,
        ui: &mut egui::Ui,
        entry: &Entry,
        v: &Version,
        is_working: bool,
        image: Option<egui::Image<'static>>,
        actions: &mut Vec<Action>,
    ) {
        let mut frame = egui::Frame::group(ui.style());
        if is_working {
            frame = frame.stroke(egui::Stroke::new(1.5, ui.visuals().selection.stroke.color));
        }
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                thumbnail(ui, image, HISTORY_THUMB);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("v{}", v.number)).strong());
                        ui.weak(format!("{} · {}", unix_time(v.created_at), v.author));
                        if is_working {
                            ui.label(RichText::new("working file").small().color(ui.visuals().selection.stroke.color));
                        }
                    });
                    if !v.comment.is_empty() {
                        ui.label(&v.comment);
                    }
                    let props: Vec<String> = [
                        v.part_number.clone(),
                        v.revision.as_ref().map(|r| format!("rev {r}")),
                        (v.path != entry.path).then(|| format!("as {}", v.path)),
                    ]
                    .into_iter()
                    .flatten()
                    .collect();
                    if !props.is_empty() {
                        ui.label(RichText::new(props.join(" · ")).small().weak());
                    }
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!is_working, egui::Button::new("Restore…").small())
                            .on_hover_text("Put this version back into the working file")
                            .clicked()
                        {
                            actions.push(Action::AskRestore { path: entry.path.clone(), number: v.number });
                        }
                        if ui.add(egui::Button::new("Save copy…").small()).clicked() {
                            actions.push(Action::SaveCopy { path: entry.path.clone(), version: v.clone() });
                        }
                    });
                });
            });
        });
    }

    fn welcome(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.25);
            ui.heading("SolidCraft PDM");
            ui.add_space(6.0);
            ui.label("A vault is a folder of SolidCraft designs whose versions are kept in a hidden .pdm folder.");
            ui.label("Your design files stay where they are and open in SolidCraft as before.");
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                let width = 2.0 * 160.0 + ui.spacing().item_spacing.x;
                ui.add_space((ui.available_width() - width).max(0.0) / 2.0);
                if ui.add_sized([160.0, 32.0], egui::Button::new("Open vault…")).clicked() {
                    actions.push(Action::PickVault);
                }
                if ui.add_sized([160.0, 32.0], egui::Button::new("New vault…")).clicked() {
                    actions.push(Action::NewVault);
                }
            });
            if !self.settings.recent.is_empty() {
                ui.add_space(20.0);
                ui.weak("Recent");
                for p in &self.settings.recent {
                    if ui.link(p.display().to_string()).clicked() {
                        actions.push(Action::OpenVault(p.clone()));
                    }
                }
            }
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(dialog) = &mut self.dialog else { return };
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("dialog")).show(ctx, |ui| {
            ui.set_max_width(460.0);
            match dialog {
                Dialog::Restore { path, number } => {
                    ui.heading(format!("Restore v{number}?"));
                    ui.add_space(4.0);
                    ui.label(format!("{path} is replaced by the content of v{number}, recorded as a new version."));
                    ui.label("Changes not checked in yet are checked in first, so nothing is lost.");
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("If the design is open in SolidCraft, close it first or reopen it afterwards, or a later save there would overwrite the restored file.")
                            .color(ui.visuals().warn_fg_color),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(format!("Restore v{number}")).clicked() {
                            actions.push(Action::Restore { path: path.clone(), number: *number });
                            close = true;
                        }
                        close |= ui.button("Cancel").clicked();
                    });
                }
                Dialog::Rename { from, to } => {
                    ui.heading("Move / rename");
                    ui.label("New path in the vault (folders are created as needed):");
                    let edit = ui.add(egui::TextEdit::singleline(to).desired_width(f32::INFINITY));
                    ui.label(
                        RichText::new("Designs that link to this file by path are not updated yet.")
                            .small()
                            .color(ui.visuals().warn_fg_color),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let ok = to.trim() != from.as_str() && !to.trim().is_empty();
                        if ui.add_enabled(ok, egui::Button::new("Move")).clicked()
                            || (ok && edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        {
                            actions.push(Action::Rename { from: from.clone(), to: to.clone() });
                            close = true;
                        }
                        close |= ui.button("Cancel").clicked();
                    });
                }
                Dialog::CheckInAll { comment, untracked } => {
                    ui.heading("Check in all changes");
                    let modified = self.entries.iter().filter(|e| e.status == Status::Modified).count();
                    let new = self.entries.iter().filter(|e| e.status == Status::Untracked).count();
                    ui.label(format!("{modified} modified designs get a new version."));
                    ui.add_enabled(new > 0, egui::Checkbox::new(untracked, format!("Also add {new} untracked designs")));
                    ui.add(egui::TextEdit::multiline(comment).hint_text("What changed?").desired_width(f32::INFINITY));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let n = modified + if *untracked { new } else { 0 };
                        if ui.add_enabled(n > 0, egui::Button::new(format!("Check in {n}"))).clicked() {
                            actions.push(Action::CheckInAll { comment: comment.clone(), untracked: *untracked });
                            close = true;
                        }
                        close |= ui.button("Cancel").clicked();
                    });
                }
                Dialog::MakeVault { dir } => {
                    ui.heading("Not a vault yet");
                    ui.label(format!("{} is not a vault.", dir.display()));
                    ui.label("Creating one adds a hidden .pdm folder there; your files are not changed.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Create vault here").clicked() {
                            actions.push(Action::MakeVault(dir.clone()));
                            close = true;
                        }
                        close |= ui.button("Cancel").clicked();
                    });
                }
                Dialog::Settings { open_command } => {
                    ui.heading("Settings");
                    ui.label("Program that opens a design (the file is added at the end):");
                    ui.add(
                        egui::TextEdit::singleline(open_command)
                            .hint_text("automatic: solidcraft if installed, else xdg-open")
                            .desired_width(f32::INFINITY),
                    );
                    ui.label(RichText::new("e.g. ~/Apps/SolidCraft.AppImage, or snap run solidcraft").small().weak());
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            actions.push(Action::SetOpenCommand(open_command.clone()));
                            close = true;
                        }
                        close |= ui.button("Cancel").clicked();
                    });
                }
            }
        });
        if close || modal.should_close() {
            self.dialog = None;
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_scan(ctx);
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if self.vault.is_some() && focused && (!self.focused || self.last_scan.elapsed() >= RESCAN_EVERY) {
            self.start_scan(ctx);
        }
        self.focused = focused;
        ctx.request_repaint_after(RESCAN_EVERY);
        if ctx.input(|i| i.key_pressed(egui::Key::F5)) {
            self.start_scan(ctx);
        }

        let mut actions = Vec::new();
        egui::TopBottomPanel::top("top").show(ctx, |ui| self.top_bar(ui, &mut actions));
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| self.status_bar(ui));
        if self.vault.is_some() {
            egui::SidePanel::left("folders").default_width(220.0).min_width(150.0).show(ctx, |ui| self.left_panel(ui));
            egui::SidePanel::right("details")
                .default_width(360.0)
                .min_width(280.0)
                .show(ctx, |ui| self.details(ui, &mut actions));
            egui::CentralPanel::default().show(ctx, |ui| self.table(ui, &mut actions));
        } else {
            egui::CentralPanel::default().show(ctx, |ui| self.welcome(ui, &mut actions));
        }
        self.dialogs(ctx, &mut actions);
        for action in actions {
            self.apply(ctx, action);
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.settings);
    }
}

fn properties(ui: &mut egui::Ui, entry: &Entry, info: Option<&scformat::DesignInfo>, latest: Option<&Version>) {
    egui::Grid::new("properties").num_columns(2).spacing([12.0, 4.0]).striped(true).show(ui, |ui| {
        let mut row = |label: &str, value: Option<String>| {
            if let Some(v) = value.filter(|v| !v.is_empty()) {
                ui.weak(label);
                ui.add(egui::Label::new(v).wrap());
                ui.end_row();
            }
        };
        match info {
            Some(i) => {
                let m = &i.metadata;
                row("Name", i.name.clone());
                row("Part number", m.part_number.clone());
                row("Revision", m.revision.clone());
                row("Project", m.project.clone());
                row("Description", m.description.clone());
                row("Author", m.author.clone());
                row("Organization", m.organization.clone());
                row("License", m.license.clone());
                row("Units", i.units.clone());
                row("Features", Some(i.feature_count.to_string()));
                row("Parameters", Some(i.param_count.to_string()));
                row("Components", (!i.components.is_empty()).then(|| i.components.len().to_string()));
                row(
                    "Stored as",
                    Some(match i.encoding {
                        scformat::Encoding::Container => "Container".into(),
                        scformat::Encoding::Json => "Plain JSON".into(),
                    }),
                );
                row("Format", Some(i.format.clone().unwrap_or_else(|| "legacy".into())));
                for key in m.extensions.keys() {
                    row("Tool data", Some(key.clone()));
                }
            }
            None => {
                if let Some(v) = latest {
                    row("Name", v.name.clone());
                    row("Part number", v.part_number.clone());
                    row("Revision", v.revision.clone());
                    row("Format", v.format.clone());
                }
                if entry.status != Status::Missing {
                    row("Note", Some("Not a readable SolidCraft design".into()));
                }
            }
        }
        row("Size", entry.size.map(human_size));
        row("Modified", entry.modified.map(system_time));
    });
}

/// Draws a thumbnail, or a placeholder of the same size.
fn thumbnail(ui: &mut egui::Ui, image: Option<egui::Image<'static>>, size: egui::Vec2) {
    match image {
        Some(image) => {
            ui.add(image.fit_to_exact_size(size).corner_radius(4.0));
        }
        None => {
            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            let visuals = ui.visuals();
            ui.painter().rect_filled(rect, 4.0, visuals.faint_bg_color);
            ui.painter().rect_stroke(rect, 4.0, visuals.widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);
            let font = if size.y > 80.0 { egui::FontId::proportional(13.0) } else { egui::FontId::proportional(18.0) };
            let text = if size.y > 80.0 { "No saved preview" } else { "⬚" };
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, font, visuals.weak_text_color());
        }
    }
}

fn status_style(status: Status) -> (&'static str, Color32) {
    match status {
        Status::Current => ("Up to date", Color32::from_rgb(80, 170, 100)),
        Status::Modified => ("Modified", Color32::from_rgb(225, 145, 40)),
        Status::Untracked => ("Untracked", Color32::from_rgb(120, 145, 185)),
        Status::Missing => ("Missing", Color32::from_rgb(215, 75, 75)),
    }
}

/// The vault path a link stored in `from` points to (`None` when it leaves the vault).
fn resolve_link(from: &str, link: &str) -> Option<String> {
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for part in link.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    Some(parts.join("/"))
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
}

fn unix_time(secs: i64) -> String {
    Local.timestamp_opt(secs, 0).single().map_or("?".into(), |t| t.format("%Y-%m-%d %H:%M").to_string())
}

fn system_time(t: SystemTime) -> String {
    DateTime::<Local>::from(t).format("%Y-%m-%d %H:%M").to_string()
}

fn human_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} KB", b as f64 / (1 << 10) as f64),
        b => format!("{b} B"),
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_link;

    #[test]
    fn links_resolve_relative_to_the_linking_design() {
        assert_eq!(resolve_link("asm/box.solidcraft", "lid.solidcraft").as_deref(), Some("asm/lid.solidcraft"));
        assert_eq!(resolve_link("asm/box.solidcraft", "../parts/lid.solidcraft").as_deref(), Some("parts/lid.solidcraft"));
        assert_eq!(resolve_link("box.solidcraft", "./lid.solidcraft").as_deref(), Some("lid.solidcraft"));
        assert_eq!(resolve_link("box.solidcraft", "../outside.solidcraft"), None);
    }
}
