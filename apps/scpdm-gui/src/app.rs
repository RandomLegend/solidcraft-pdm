//! The window: folders and filters on the left, the designs in the middle, the selected design's
//! properties, links and history on the right.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Local, TimeZone};
use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use serde::{Deserialize, Serialize};
use vault::{Entry, Status, Vault, Version};

use crate::launcher::{self, Launcher};
use crate::scan::{self, ImageFile, Info, Scan};
use crate::search::{Haystack, Search};

/// How often the vault is rescanned while the window has focus (cheap: unchanged files are
/// recognised by size and modification time).
const RESCAN_EVERY: Duration = Duration::from_secs(3);
/// SolidCraft's portable thumbnail size.
const THUMB: egui::Vec2 = egui::vec2(240.0, 150.0);
/// Width/height of SolidCraft's thumbnails.
const THUMB_ASPECT: f32 = 1.6;
const ROW_THUMB_WIDTHS: std::ops::RangeInclusive<f32> = 48.0..=240.0;
const HISTORY_THUMB: egui::Vec2 = egui::vec2(80.0, 50.0);
const MAX_RECENT: usize = 8;

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    last_vault: Option<PathBuf>,
    recent: Vec<PathBuf>,
    /// Program (and arguments) that opens a design; empty: SolidCraft as installed.
    open_command: String,
    /// Width of the thumbnails in the list.
    thumb_width: f32,
    /// List columns the user switched off.
    hidden_columns: Vec<ListColumn>,
    /// Interface size (egui zoom factor; Ctrl + and Ctrl - change it too). 150 % by default:
    /// egui's 100 % is small on large or high-resolution screens.
    zoom: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            last_vault: None,
            recent: Vec::new(),
            open_command: String::new(),
            thumb_width: 160.0,
            hidden_columns: Vec::new(),
            zoom: 1.5,
        }
    }
}

/// The list's optional columns (the thumbnail and the design are always shown).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
enum ListColumn {
    Status,
    Version,
    PartNumber,
    Revision,
    Modified,
}

impl ListColumn {
    const ALL: [ListColumn; 5] =
        [ListColumn::Status, ListColumn::Version, ListColumn::PartNumber, ListColumn::Revision, ListColumn::Modified];

    fn title(self) -> &'static str {
        match self {
            ListColumn::Status => "Status",
            ListColumn::Version => "Ver",
            ListColumn::PartNumber => "Part number",
            ListColumn::Revision => "Rev",
            ListColumn::Modified => "Modified",
        }
    }

    fn menu_label(self) -> &'static str {
        match self {
            ListColumn::Version => "Version",
            ListColumn::Revision => "Revision",
            c => c.title(),
        }
    }

    fn width(self) -> f32 {
        match self {
            ListColumn::Status => 96.0,
            ListColumn::Version => 40.0,
            ListColumn::PartNumber => 110.0,
            ListColumn::Revision => 40.0,
            ListColumn::Modified => 118.0,
        }
    }
}

const ZOOM_PRESETS: [f32; 6] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.5];

#[derive(Clone, PartialEq)]
enum View {
    All,
    Status(Status),
    /// Designs directly in the vault folder.
    TopLevel,
    /// Everything below this folder.
    Folder(String),
}

/// A design being dragged onto a folder.
struct DraggedDesign(String);

/// What a row of the list shows: a design (index into `entries`) or a reference image (index
/// into `images`).
#[derive(Clone, Copy, PartialEq)]
enum Item {
    Design(usize),
    Image(usize),
}

/// A row of the list, and the file name's letters that matched the search.
struct Row {
    item: Item,
    matches: Vec<usize>,
}

enum Dialog {
    Restore { path: String, number: u32 },
    Rename { from: String, to: String },
    CheckInAll { comment: String, untracked: bool },
    MakeVault { dir: PathBuf },
    Settings { open_command: String },
    NewFolder { parent: String, name: String },
    NewDesign { folder: String, name: String, part_number: String, description: String, open: bool },
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
    AskNewFolder(String),
    CreateFolder(String),
    DeleteFolder(String),
    ShowFolder(String),
    MoveToFolder { path: String, folder: String },
    ToggleColumn(ListColumn),
    /// Opens a file (a reference image) with the system's program for it.
    OpenFile(String),
    /// Opens a version: the working file when it has that content, else a read-only copy.
    OpenVersion { path: String, version: Version },
    AskNewDesign(String),
    CreateDesign { path: String, part_number: String, description: String, open: bool },
}

pub struct App {
    settings: Settings,
    vault: Option<Vault>,
    entries: Vec<Entry>,
    /// Working files' properties, by content hash.
    infos: HashMap<String, Info>,
    /// Thumbnail PNGs by key (content hash for working files, object hash for versions).
    thumbs: HashMap<String, Option<Arc<[u8]>>>,
    /// Every folder of the vault, also empty ones.
    folders: Vec<String>,
    /// Reference images, and their sizes in pixels by thumbnail key.
    images: Vec<ImageFile>,
    image_sizes: HashMap<String, (u32, u32)>,
    scan: Option<Receiver<Result<Scan, String>>>,
    rescan: bool,
    last_scan: Instant,
    view: View,
    search: String,
    focus_search: bool,
    /// The list as shown, and what it was computed for (`None`: recompute).
    rows: Vec<Row>,
    rows_for: Option<(String, View)>,
    /// SolidCraft as installed (desktop entry or PATH).
    detected: Option<Launcher>,
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
        // Only with settings storage, which the app always has (even on the first run); tests
        // have none and lay out at 100 %.
        if cc.storage.is_some() {
            cc.egui_ctx.set_zoom_factor(settings.zoom.clamp(0.5, 3.0));
        }
        let mut app = App {
            settings,
            vault: None,
            entries: Vec::new(),
            infos: HashMap::new(),
            thumbs: HashMap::new(),
            folders: Vec::new(),
            images: Vec::new(),
            image_sizes: HashMap::new(),
            scan: None,
            rescan: false,
            last_scan: Instant::now(),
            view: View::All,
            search: String::new(),
            focus_search: false,
            rows: Vec::new(),
            rows_for: None,
            detected: launcher::detect(),
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

    /// The program that opens designs (as in Settings; empty: SolidCraft as installed).
    pub fn set_open_command(&mut self, command: &str) {
        self.settings.open_command = command.trim().to_owned();
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
                self.folders.clear();
                self.images.clear();
                self.image_sizes.clear();
                self.rows_for = None;
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
        let known = self.infos.keys().chain(self.image_sizes.keys()).cloned().collect();
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
                for (key, thumb) in scan.image_thumbs {
                    if let Some(t) = &thumb {
                        self.image_sizes.insert(key.clone(), (t.width, t.height));
                    } else {
                        self.image_sizes.insert(key.clone(), (0, 0));
                    }
                    self.thumbs.insert(key, thumb.map(|t| t.png));
                }
                let live_images: HashSet<&str> = scan.images.iter().map(|i| i.key.as_str()).collect();
                self.image_sizes.retain(|k, _| live_images.contains(k.as_str()));
                self.thumbs.retain(|k, _| !k.starts_with("img:") || live_images.contains(k.as_str()));
                self.entries = scan.entries;
                self.folders = scan.folders;
                self.images = scan.images;
                self.rows_for = None;
                if self.selected.as_ref().is_some_and(|s| self.entry(s).is_none() && self.image_file(s).is_none()) {
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

    fn image_file(&self, path: &str) -> Option<&ImageFile> {
        self.images.iter().find(|i| i.file.path == path)
    }

    fn item_path(&self, item: Item) -> &str {
        match item {
            Item::Design(i) => &self.entries[i].path,
            Item::Image(i) => &self.images[i].file.path,
        }
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

    /// Recomputes the list when the search, the view or the vault changed. A search looks in
    /// the whole vault, best matches first; otherwise the view's designs are listed by path.
    fn refilter(&mut self) {
        let key = (self.search.clone(), self.view.clone());
        if self.rows_for.as_ref() == Some(&key) {
            return;
        }
        let designs = (0..self.entries.len()).map(Item::Design);
        let images = (0..self.images.len()).map(Item::Image);
        self.rows = match Search::new(&self.search) {
            Some(mut search) => {
                let mut hits: Vec<(u32, Row)> = designs
                    .chain(images)
                    .filter_map(|item| {
                        let path = self.item_path(item);
                        let (folder, file_name) = path.rsplit_once('/').unwrap_or(("", path));
                        let mut fields = vec![folder];
                        if let Item::Design(index) = item {
                            let e = &self.entries[index];
                            let latest = e.latest.as_ref();
                            match self.info(e) {
                                Some(i) => {
                                    let m = &i.metadata;
                                    fields.extend(
                                        [&i.name, &m.part_number, &m.description, &m.project, &m.revision]
                                            .into_iter()
                                            .flatten()
                                            .map(String::as_str),
                                    );
                                }
                                None => fields.extend(
                                    [latest.and_then(|v| v.name.as_deref()), latest.and_then(|v| v.part_number.as_deref())]
                                        .into_iter()
                                        .flatten(),
                                ),
                            }
                        }
                        let hit = search.score(&Haystack { file_name, fields })?;
                        Some((hit.score, Row { item, matches: hit.file_name_matches }))
                    })
                    .collect();
                hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| self.item_path(a.1.item).cmp(self.item_path(b.1.item))));
                hits.into_iter().map(|(_, row)| row).collect()
            }
            None => {
                // Reference images show in the folders, next to their designs.
                let in_folder = |path: &str| match &self.view {
                    View::TopLevel => !path.contains('/'),
                    View::Folder(f) => path.starts_with(&format!("{f}/")),
                    _ => false,
                };
                let mut rows: Vec<Row> = designs
                    .filter(|&item| {
                        let Item::Design(i) = item else { return false };
                        let e = &self.entries[i];
                        match &self.view {
                            View::All => true,
                            View::Status(s) => e.status == *s,
                            _ => in_folder(&e.path),
                        }
                    })
                    .chain(images.filter(|&item| in_folder(self.item_path(item))))
                    .map(|item| Row { item, matches: Vec::new() })
                    .collect();
                rows.sort_by(|a, b| self.item_path(a.item).cmp(self.item_path(b.item)));
                rows
            }
        };
        self.rows_for = Some(key);
    }

    fn launcher(&self) -> Option<Launcher> {
        launcher::custom(&self.settings.open_command).or_else(|| self.detected.clone())
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
            Action::ShowInFolder(path) | Action::ShowFolder(path) => {
                let Some(vault) = &self.vault else { return };
                if let Err(e) = launcher::show_in_folder(&vault.absolute(&path)) {
                    self.error(format!("Could not open the file manager: {e}"));
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
                let to = typed_path(&to);
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
            Action::SetOpenCommand(cmd) => {
                self.settings.open_command = cmd.trim().to_owned();
                self.detected = launcher::detect();
            }
            Action::AskNewFolder(parent) => self.dialog = Some(Dialog::NewFolder { parent, name: String::new() }),
            Action::CreateFolder(folder) => {
                let Some(vault) = &self.vault else { return };
                match vault.create_folder(&folder) {
                    Ok(()) => {
                        self.info_msg(format!("Created folder {folder}"));
                        self.folders.push(folder.clone());
                        self.folders.sort();
                        self.view = View::Folder(folder);
                        self.start_scan(ctx);
                    }
                    Err(e) => self.error(e),
                }
            }
            Action::DeleteFolder(folder) => {
                let Some(vault) = &self.vault else { return };
                match vault.remove_folder(&folder) {
                    Ok(()) => {
                        self.info_msg(format!("Deleted folder {folder}"));
                        self.folders.retain(|f| *f != folder);
                        if matches!(&self.view, View::Folder(f) if *f == folder || f.starts_with(&format!("{folder}/"))) {
                            self.view = View::All;
                        }
                        self.start_scan(ctx);
                    }
                    Err(e) => self.error(e),
                }
            }
            Action::OpenFile(path) => {
                let Some(vault) = &self.vault else { return };
                if let Err(e) = launcher::open_with_system(&vault.absolute(&path)) {
                    self.error(format!("Could not open {path}: {e}"));
                }
            }
            Action::ToggleColumn(column) => {
                let hidden = &mut self.settings.hidden_columns;
                if let Some(i) = hidden.iter().position(|c| *c == column) {
                    hidden.remove(i);
                } else {
                    hidden.push(column);
                }
            }
            Action::OpenVersion { path, version } => {
                if self.entry(&path).and_then(|e| e.hash.as_deref()) == Some(version.hash.as_str()) {
                    self.open_design(&path);
                } else {
                    self.open_version_copy(&path, &version);
                }
            }
            Action::AskNewDesign(folder) => {
                self.dialog = Some(Dialog::NewDesign {
                    folder,
                    name: String::new(),
                    part_number: String::new(),
                    description: String::new(),
                    open: true,
                });
            }
            Action::CreateDesign { path, part_number, description, open } => {
                let Some(vault) = &mut self.vault else { return };
                let (folder, file) = path.rsplit_once('/').unwrap_or(("", &path));
                let name = file.strip_suffix(".solidcraft").unwrap_or(file);
                let bytes = scformat::new_design(name, &[("part_number", &part_number), ("description", &description)]);
                match vault.add_new(&path, &bytes, "New design") {
                    Ok(_) => {
                        self.info_msg(format!("Created {path} (v1)"));
                        self.view = if folder.is_empty() { View::TopLevel } else { View::Folder(folder.to_owned()) };
                        self.search.clear();
                        self.select(&path);
                        self.start_scan(ctx);
                        if open {
                            self.open_design(&path);
                        }
                    }
                    Err(e) => self.error(e),
                }
            }
            Action::MoveToFolder { path, folder } => {
                let users = self.used_in(&path).len();
                let Some(vault) = &mut self.vault else { return };
                match vault.move_to_folder(&path, &folder) {
                    Ok(to) if to == path => {}
                    Ok(to) => {
                        let place = if folder.is_empty() { "the top level".to_owned() } else { folder };
                        let mut msg = format!("Moved {path} to {place}");
                        if users > 0 {
                            msg.push_str(&format!(" ({users} designs link to it by path; reopen and relink them in SolidCraft)"));
                        }
                        self.info_msg(msg);
                        if self.selected.as_deref() == Some(path.as_str()) {
                            self.selected = Some(to);
                        }
                        self.load_history();
                        self.start_scan(ctx);
                    }
                    Err(e) => self.error(e),
                }
            }
        }
    }

    fn open_design(&mut self, path: &str) {
        let Some(vault) = &self.vault else { return };
        let file = vault.absolute(path);
        let Some(launcher) = self.launcher() else {
            self.error("SolidCraft was not found. Choose its AppImage in Settings.");
            self.dialog = Some(Dialog::Settings { open_command: self.settings.open_command.clone() });
            return;
        };
        if let Err(e) = launcher.open(&file) {
            self.error(format!("Could not start {}: {e} (change it in Vault > Settings)", launcher.command[0]));
        }
    }

    /// Opens an earlier version in SolidCraft as a copy in a read-only folder, so that
    /// SolidCraft cannot save over it; the working file and the vault stay as they are.
    fn open_version_copy(&mut self, path: &str, version: &Version) {
        let Some(vault) = &self.vault else { return };
        let file = path.rsplit_once('/').map_or(path, |(_, f)| f);
        let (stem, ext) = file.rsplit_once('.').unwrap_or((file, "solidcraft"));
        let dir = std::env::temp_dir().join("solidcraft-pdm").join(&version.hash[..16]);
        let copy = dir.join(format!("{stem} (v{}, read-only).{ext}", version.number));
        let result = (|| -> Result<(), String> {
            if !copy.is_file() {
                let bytes = vault.version_bytes(version).map_err(|e| e.to_string())?;
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                std::fs::write(&copy, bytes).map_err(|e| e.to_string())?;
                for p in [&copy, &dir] {
                    let mut perms = std::fs::metadata(p).map_err(|e| e.to_string())?.permissions();
                    perms.set_readonly(true);
                    std::fs::set_permissions(p, perms).map_err(|e| e.to_string())?;
                }
            }
            Ok(())
        })();
        match result {
            Err(e) => self.error(format!("Could not prepare v{}: {e}", version.number)),
            Ok(()) => match self.launcher() {
                Some(launcher) => match launcher.open(&copy) {
                    Ok(()) => self.info_msg(format!(
                        "Opened a read-only copy of v{} (to work on it, Restore it first)",
                        version.number
                    )),
                    Err(e) => self.error(format!("Could not start {}: {e}", launcher.command[0])),
                },
                None => self.error("SolidCraft was not found. Choose its AppImage in Settings."),
            },
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
                    self.detected = launcher::detect();
                    self.dialog = Some(Dialog::Settings { open_command: self.settings.open_command.clone() });
                }
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            // Stays open while columns are ticked.
            let keep_open = egui::containers::menu::MenuConfig::new()
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
            egui::containers::menu::MenuButton::new("View").config(keep_open).ui(ui, |ui| {
                ui.label(RichText::new("Columns").small().weak());
                column_toggles(ui, &self.settings.hidden_columns, actions);
                ui.separator();
                ui.label(RichText::new(format!("Interface size: {:.0} %", ui.ctx().zoom_factor() * 100.0)).small().weak());
                egui::gui_zoom::zoom_menu_buttons(ui);
            });
            if self.vault.is_none() {
                return;
            }
            ui.separator();
            let folder = match &self.view {
                View::Folder(f) => f.clone(),
                _ => String::new(),
            };
            if ui.button("+ New design").on_hover_text("Create an empty design and open it in SolidCraft").clicked() {
                actions.push(Action::AskNewDesign(folder));
            }
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
            if self.scan.is_some() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.spinner());
            }
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

    fn left_panel(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
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
            ui.horizontal(|ui| {
                ui.label(RichText::new("Folders").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let parent = match &view {
                        View::Folder(f) => f.clone(),
                        _ => String::new(),
                    };
                    let hint = if parent.is_empty() {
                        "New folder in the vault".to_owned()
                    } else {
                        format!("New folder in {parent}")
                    };
                    if ui.small_button("+ New folder").on_hover_text(hint).clicked() {
                        actions.push(Action::AskNewFolder(parent));
                    }
                });
            });
            let top = self.entries.iter().filter(|e| !e.path.contains('/')).count();
            let response = ui.selectable_value(&mut view, View::TopLevel, format!("Top level  ({top})"));
            drop_target(ui, &response, "", actions);
            response.context_menu(|ui| {
                if ui.button("New design here…").clicked() {
                    actions.push(Action::AskNewDesign(String::new()));
                    ui.close();
                }
                if ui.button("New folder…").clicked() {
                    actions.push(Action::AskNewFolder(String::new()));
                    ui.close();
                }
            });
            let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
            for f in &self.folders {
                children.entry(f.rsplit_once('/').map_or("", |(p, _)| p)).or_default().push(f);
            }
            self.folder_tree(ui, "", &children, &mut view, actions);
            ui.add_space(6.0);
            ui.label(RichText::new("Drag designs onto a folder to move them.").small().weak());
            if view != self.view {
                self.view = view;
                self.search.clear();
            }
        });
    }

    fn folder_tree(
        &self,
        ui: &mut egui::Ui,
        parent: &str,
        children: &BTreeMap<&str, Vec<&str>>,
        view: &mut View,
        actions: &mut Vec<Action>,
    ) {
        for &folder in children.get(parent).map(Vec::as_slice).unwrap_or_default() {
            if children.contains_key(folder) {
                let id = ui.make_persistent_id(("folder", folder));
                egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
                    .show_header(ui, |ui| self.folder_item(ui, folder, children, view, actions))
                    .body(|ui| self.folder_tree(ui, folder, children, view, actions));
            } else {
                ui.horizontal(|ui| {
                    ui.add_space(ui.spacing().indent);
                    self.folder_item(ui, folder, children, view, actions);
                });
            }
        }
    }

    /// A folder in the tree: selects it, takes dropped designs, and has a context menu.
    fn folder_item(
        &self,
        ui: &mut egui::Ui,
        folder: &str,
        children: &BTreeMap<&str, Vec<&str>>,
        view: &mut View,
        actions: &mut Vec<Action>,
    ) {
        let name = folder.rsplit_once('/').map_or(folder, |(_, n)| n);
        let prefix = format!("{folder}/");
        let n = self.entries.iter().filter(|e| e.path.starts_with(&prefix)).count();
        let response = ui.selectable_value(view, View::Folder(folder.to_owned()), format!("{name}  ({n})"));
        drop_target(ui, &response, folder, actions);
        let empty = n == 0 && !children.contains_key(folder) && !self.images.iter().any(|i| i.file.path.starts_with(&prefix));
        response.context_menu(|ui| {
            if ui.button("New design here…").clicked() {
                actions.push(Action::AskNewDesign(folder.to_owned()));
                ui.close();
            }
            if ui.button("New subfolder…").clicked() {
                actions.push(Action::AskNewFolder(folder.to_owned()));
                ui.close();
            }
            if ui.button("Show in file manager").clicked() {
                actions.push(Action::ShowFolder(folder.to_owned()));
                ui.close();
            }
            if ui
                .add_enabled(empty, egui::Button::new("Delete folder"))
                .on_disabled_hover_text("Only empty folders can be deleted")
                .clicked()
            {
                actions.push(Action::DeleteFolder(folder.to_owned()));
                ui.close();
            }
        });
    }

    fn search_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let slider_width = 140.0;
            let clear_width = if self.search.is_empty() { 0.0 } else { 28.0 };
            let edit = egui::TextEdit::singleline(&mut self.search)
                .hint_text("Search all designs: file, name, part number, description…   (Ctrl+F)")
                .margin(egui::vec2(8.0, 6.0))
                .desired_width(ui.available_width() - slider_width - clear_width - 3.0 * ui.spacing().item_spacing.x);
            let response = ui.add(edit);
            if std::mem::take(&mut self.focus_search) {
                response.request_focus();
            }
            // The field gives up focus as Escape is pressed.
            if (response.has_focus() || response.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.search.clear();
            }
            if !self.search.is_empty() && ui.button("×").on_hover_text("Clear the search (Esc)").clicked() {
                self.search.clear();
            }
            ui.add_sized(
                [slider_width, 20.0],
                egui::Slider::new(&mut self.settings.thumb_width, ROW_THUMB_WIDTHS).show_value(false),
            )
            .on_hover_text("Thumbnail size");
        });
    }

    /// The folder new designs go into from the list: the one shown ("" for the top level).
    fn current_folder(&self) -> String {
        match &self.view {
            View::Folder(f) => f.clone(),
            _ => String::new(),
        }
    }

    fn list(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        self.search_bar(ui);
        self.refilter();
        let searching = !self.search.trim().is_empty();
        if searching {
            let n = self.rows.len();
            let noun = if n == 1 { "match" } else { "matches" };
            ui.label(RichText::new(format!("{n} {noun} in the whole vault")).small().weak());
        }
        ui.add_space(4.0);
        let here = self.current_folder();
        if self.rows.is_empty() {
            let response = ui
                .centered_and_justified(|ui| {
                    ui.weak(if searching {
                        "Nothing matches".to_owned()
                    } else if self.entries.is_empty() && self.scan.is_some() {
                        "Scanning…".to_owned()
                    } else {
                        "No designs here. Right-click for a new design, or drag designs onto a folder on the left.".to_owned()
                    });
                })
                .response
                .interact(egui::Sense::click());
            response.context_menu(|ui| new_here_menu(ui, &here, actions));
            return;
        }
        // Thumbnails are looked up (and version thumbnails loaded) before the table borrows self.
        let mut thumbs = Vec::with_capacity(self.rows.len());
        for i in 0..self.rows.len() {
            thumbs.push(match self.rows[i].item {
                Item::Design(d) => {
                    let e = self.entries[d].clone();
                    self.entry_thumb(&e)
                }
                Item::Image(m) => self.image(&self.images[m].key),
            });
        }
        let width = self.settings.thumb_width.clamp(*ROW_THUMB_WIDTHS.start(), *ROW_THUMB_WIDTHS.end());
        let thumb = egui::vec2(width, (width / THUMB_ASPECT).round());
        let text_height = ui.text_style_height(&egui::TextStyle::Body);
        let row_height = (thumb.y + 8.0).max(text_height * 2.6);
        let show_description = row_height >= text_height * 4.0;
        let mut columns: Vec<ListColumn> =
            ListColumn::ALL.into_iter().filter(|c| !self.settings.hidden_columns.contains(c)).collect();
        // Without room for every column, the date moves into the design's second line.
        let spacing = ui.spacing().item_spacing.x;
        let needed = thumb.x + 8.0 + 160.0 + columns.iter().map(|c| c.width() + spacing).sum::<f32>() + spacing;
        if needed > ui.available_width() {
            columns.retain(|c| *c != ListColumn::Modified);
        }
        let date_in_name = !self.settings.hidden_columns.contains(&ListColumn::Modified) && !columns.contains(&ListColumn::Modified);
        let hidden = &self.settings.hidden_columns;
        let folders = &self.folders;
        let mut table = TableBuilder::new(ui)
            // Widths are kept per column position: another column set gets its own.
            .id_salt(("list", &columns))
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click_and_drag())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(thumb.x + 8.0))
            .column(Column::remainder().at_least(160.0).clip(true));
        for c in &columns {
            table = table.column(Column::initial(c.width()).at_least(c.width().min(60.0)).clip(true));
        }
        table
            .header(text_height + 8.0, |mut h| {
                let titles = ["", "Design"].into_iter().chain(columns.iter().map(|c| c.title()));
                for title in titles {
                    let (_, response) = h.col(|ui| {
                        ui.strong(title);
                    });
                    let response = response.on_hover_text("Right-click to choose the columns");
                    egui::Popup::context_menu(&response)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .show(|ui| column_toggles(ui, hidden, actions));
                }
            })
            .body(|body| {
                body.rows(row_height, self.rows.len(), |mut row| {
                    let n = row.index();
                    let r = &self.rows[n];
                    let path = self.item_path(r.item);
                    let (folder, file) = path.rsplit_once('/').unwrap_or(("", path));
                    row.set_selected(self.selected.as_deref() == Some(path));
                    row.col(|ui| {
                        thumbnail(ui, thumbs[n].clone(), thumb, egui::Sense::hover());
                    });

                    // The texts of the row: second line, third line, and each column.
                    let (sub, description, cells): (Vec<String>, Option<&str>, Vec<RichText>) = match r.item {
                        Item::Design(d) => {
                            let e = &self.entries[d];
                            let info = self.info(e);
                            let latest = e.latest.as_ref();
                            let props = info.map(|i| &i.metadata);
                            let name = info.and_then(|i| i.name.clone()).or_else(|| latest.and_then(|v| v.name.clone()));
                            let date = if date_in_name { e.modified.map(system_time) } else { None };
                            let sub = [name, Some(folder.to_owned()), date].into_iter().flatten().collect();
                            let description = info.and_then(|i| i.metadata.description.as_deref()).filter(|_| show_description);
                            let cells = columns
                                .iter()
                                .map(|c| match c {
                                    ListColumn::Status => {
                                        let (label, color) = status_style(e.status);
                                        RichText::new(label).color(color)
                                    }
                                    ListColumn::Version => latest.map_or("-".into(), |v| format!("v{}", v.number)).into(),
                                    ListColumn::PartNumber => props
                                        .and_then(|m| m.part_number.clone())
                                        .or_else(|| latest.and_then(|v| v.part_number.clone()))
                                        .unwrap_or_else(|| "-".into())
                                        .into(),
                                    ListColumn::Revision => props
                                        .and_then(|m| m.revision.clone())
                                        .or_else(|| latest.and_then(|v| v.revision.clone()))
                                        .unwrap_or_else(|| "-".into())
                                        .into(),
                                    ListColumn::Modified => e.modified.map_or("-".into(), system_time).into(),
                                })
                                .collect();
                            (sub, description, cells)
                        }
                        Item::Image(m) => {
                            let image = &self.images[m];
                            let size = self.image_sizes.get(&image.key).filter(|(w, _)| *w > 0).map(|(w, h)| format!("{w} × {h}"));
                            let date = if date_in_name { image.file.modified.map(system_time) } else { None };
                            let sub = [Some("Reference image".to_owned()), size, Some(folder.to_owned()), date]
                                .into_iter()
                                .flatten()
                                .collect();
                            let cells = columns
                                .iter()
                                .map(|c| match c {
                                    ListColumn::Status => RichText::new("Image").weak(),
                                    ListColumn::Modified => image.file.modified.map_or("-".into(), system_time).into(),
                                    _ => RichText::new("-").weak(),
                                })
                                .collect();
                            (sub, None, cells)
                        }
                    };
                    row.col(|ui| {
                        let lines = if description.is_some() { 3.0 } else { 2.0 };
                        let block = text_height * (lines + 0.4);
                        ui.vertical(|ui| {
                            ui.add_space(((ui.available_height() - block) / 2.0).max(0.0));
                            ui.add(egui::Label::new(highlighted(ui, file, &r.matches)).truncate().selectable(false));
                            let sub = sub.iter().filter(|s| !s.is_empty()).map(String::as_str).collect::<Vec<_>>().join("  ·  ");
                            ui.add(egui::Label::new(RichText::new(sub).weak().small()).truncate().selectable(false));
                            if let Some(d) = description {
                                ui.add(egui::Label::new(RichText::new(d).small()).truncate().selectable(false));
                            }
                        });
                    });
                    for text in cells {
                        row.col(|ui| {
                            ui.add(egui::Label::new(text).truncate().selectable(false));
                        });
                    }

                    let response = row.response();
                    let is_image = matches!(r.item, Item::Image(_));
                    let present = match r.item {
                        Item::Design(d) => self.entries[d].status != Status::Missing,
                        Item::Image(_) => true,
                    };
                    if present {
                        response.dnd_set_drag_payload(DraggedDesign(path.to_owned()));
                    }
                    if response.clicked() {
                        actions.push(Action::Select(path.to_owned()));
                    }
                    if response.double_clicked() && present {
                        actions.push(if is_image { Action::OpenFile(path.to_owned()) } else { Action::Open(path.to_owned()) });
                    }
                    response.context_menu(|ui| {
                        actions.push(Action::Select(path.to_owned()));
                        if present {
                            let open = if is_image { "Open" } else { "Open in SolidCraft" };
                            if ui.button(open).clicked() {
                                actions.push(if is_image { Action::OpenFile(path.to_owned()) } else { Action::Open(path.to_owned()) });
                                ui.close();
                            }
                            if ui.button("Show in folder").clicked() {
                                actions.push(Action::ShowInFolder(path.to_owned()));
                                ui.close();
                            }
                            ui.menu_button("Move to", |ui| {
                                for target in std::iter::once("").chain(folders.iter().map(String::as_str)) {
                                    let label = if target.is_empty() { "Top level" } else { target };
                                    if ui.add_enabled(target != folder, egui::Button::new(label)).clicked() {
                                        actions.push(Action::MoveToFolder { path: path.to_owned(), folder: target.to_owned() });
                                        ui.close();
                                    }
                                }
                            });
                        } else {
                            ui.weak("The file is missing");
                        }
                        ui.separator();
                        // In a filter or search the row's own folder; in a folder view, that folder.
                        new_here_menu(ui, folder, actions);
                    });
                });
            });
        // Right-click below the list.
        let rest = ui.allocate_response(ui.available_size().max(egui::vec2(0.0, 0.0)), egui::Sense::click());
        rest.context_menu(|ui| new_here_menu(ui, &here, actions));
    }

    fn details(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        if let Some(image) = self.selected.as_deref().and_then(|p| self.image_file(p)).cloned() {
            self.image_details(ui, &image, actions);
            return;
        }
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
            let preview = thumbnail(ui, image, THUMB, egui::Sense::click());
            if entry.status != Status::Missing && preview.on_hover_text("Double-click to open in SolidCraft").double_clicked() {
                actions.push(Action::Open(entry.path.clone()));
            }
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
                    // The newest version with the working file's content is the working file; an
                    // older one with the same content (before a restore) is only the same.
                    let working = history.iter().find(|v| entry.hash.as_deref() == Some(v.hash.as_str())).map(|v| v.number);
                    for v in &history {
                        let same = if working == Some(v.number) {
                            Sameness::Working
                        } else if entry.hash.as_deref() == Some(v.hash.as_str()) {
                            Sameness::SameContent
                        } else {
                            Sameness::Different
                        };
                        let image = self.version_thumb(v);
                        self.version_row(ui, &entry, v, same, image, actions);
                    }
                });
        });
    }

    fn image_details(&mut self, ui: &mut egui::Ui, image: &ImageFile, actions: &mut Vec<Action>) {
        let path = image.file.path.clone();
        let file = path.rsplit_once('/').map_or(path.as_str(), |(_, f)| f).to_owned();
        let picture = self.image(&image.key);
        let pixels = self.image_sizes.get(&image.key).copied().filter(|(w, _)| *w > 0);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.add_space(6.0);
            let width = ui.available_width();
            let height = match pixels {
                Some((w, h)) => (width * h as f32 / w as f32).min(360.0),
                None => width / THUMB_ASPECT,
            };
            let preview = thumbnail(ui, picture, egui::vec2(width, height), egui::Sense::click());
            if preview.on_hover_text("Double-click to open").double_clicked() {
                actions.push(Action::OpenFile(path.clone()));
            }
            ui.add_space(6.0);
            ui.heading(&file);
            ui.weak(&path);
            ui.label(RichText::new("Reference image (shown, not versioned)").weak());
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button("Open").clicked() {
                    actions.push(Action::OpenFile(path.clone()));
                }
                if ui.button("Show in folder").clicked() {
                    actions.push(Action::ShowInFolder(path.clone()));
                }
                if ui.button("Move / rename…").clicked() {
                    self.dialog = Some(Dialog::Rename { from: path.clone(), to: path.clone() });
                }
            });
            ui.add_space(10.0);
            egui::Grid::new("image-properties").num_columns(2).spacing([12.0, 4.0]).striped(true).show(ui, |ui| {
                let mut row = |label: &str, value: Option<String>| {
                    if let Some(v) = value {
                        ui.weak(label);
                        ui.label(v);
                        ui.end_row();
                    }
                };
                row("Dimensions", pixels.map(|(w, h)| format!("{w} × {h} px")));
                row("Size", Some(human_size(image.file.size)));
                row("Modified", image.file.modified.map(system_time));
            });
            if pixels.is_none() {
                ui.label(RichText::new("This image could not be read.").color(ui.visuals().warn_fg_color));
            }
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
        same: Sameness,
        image: Option<egui::Image<'static>>,
        actions: &mut Vec<Action>,
    ) {
        let is_working = same == Sameness::Working;
        let mut frame = egui::Frame::group(ui.style());
        if is_working {
            frame = frame.stroke(egui::Stroke::new(1.5, ui.visuals().selection.stroke.color));
        }
        let mut buttons_top = f32::INFINITY;
        let shown = frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                thumbnail(ui, image, HISTORY_THUMB, egui::Sense::hover());
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("v{}", v.number)).strong());
                        ui.weak(format!("{} · {}", unix_time(v.created_at), v.author));
                        match same {
                            Sameness::Working => {
                                ui.label(RichText::new("working file").small().color(ui.visuals().selection.stroke.color));
                            }
                            Sameness::SameContent => {
                                ui.label(RichText::new("same as working file").small().weak());
                            }
                            Sameness::Different => {}
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
                    let buttons = ui.horizontal(|ui| {
                        if ui
                            .add_enabled(same == Sameness::Different, egui::Button::new("Restore…").small())
                            .on_hover_text("Put this version back into the working file")
                            .on_disabled_hover_text("The working file already has this content")
                            .clicked()
                        {
                            actions.push(Action::AskRestore { path: entry.path.clone(), number: v.number });
                        }
                        if ui.add(egui::Button::new("Save copy…").small()).clicked() {
                            actions.push(Action::SaveCopy { path: entry.path.clone(), version: v.clone() });
                        }
                    });
                    buttons_top = buttons.response.rect.top();
                });
            });
        });
        // Double-click anywhere above the buttons: an area over the buttons would take their clicks.
        let mut area = shown.response.rect;
        area.max.y = area.max.y.min(buttons_top - 2.0);
        let hint = if is_working { "Double-click to open in SolidCraft" } else { "Double-click to open a read-only copy" };
        let response = ui.interact(area, ui.id().with(("version", v.number)), egui::Sense::click()).on_hover_text(hint);
        if response.double_clicked() && entry.status != Status::Missing {
            actions.push(Action::OpenVersion { path: entry.path.clone(), version: v.clone() });
        }
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
                Dialog::NewFolder { parent, name } => {
                    ui.heading("New folder");
                    ui.label(if parent.is_empty() { "In the top level of the vault".to_owned() } else { format!("In {parent}") });
                    let edit = ui.add(egui::TextEdit::singleline(name).hint_text("Folder name").desired_width(f32::INFINITY));
                    if ui.memory(|m| m.focused().is_none()) {
                        edit.request_focus();
                    }
                    let typed = typed_path(name);
                    let trimmed = typed.as_str();
                    let target = if parent.is_empty() { trimmed.to_owned() } else { format!("{parent}/{trimmed}") };
                    let exists = self.folders.contains(&target);
                    if exists {
                        ui.colored_label(ui.visuals().warn_fg_color, "A folder with this name exists already.");
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let ok = !trimmed.is_empty() && !exists;
                        if ui.add_enabled(ok, egui::Button::new("Create")).clicked()
                            || (ok && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        {
                            actions.push(Action::CreateFolder(target.clone()));
                            close = true;
                        }
                        close |= ui.button("Cancel").clicked();
                    });
                }
                Dialog::NewDesign { folder, name, part_number, description, open } => {
                    ui.heading("New design");
                    ui.add_space(4.0);
                    egui::Grid::new("new-design").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                        ui.label("Folder");
                        let shown = if folder.is_empty() { "Top level".to_owned() } else { folder.clone() };
                        egui::ComboBox::from_id_salt("new-design-folder").selected_text(shown).width(260.0).show_ui(ui, |ui| {
                            ui.selectable_value(folder, String::new(), "Top level");
                            for f in &self.folders {
                                ui.selectable_value(folder, f.clone(), f.as_str());
                            }
                        });
                        ui.end_row();
                        ui.label("Name");
                        let edit = ui.add(egui::TextEdit::singleline(name).hint_text("e.g. wall-bracket").desired_width(260.0));
                        if ui.memory(|m| m.focused().is_none()) {
                            edit.request_focus();
                        }
                        ui.end_row();
                        ui.label("Part number");
                        ui.add(egui::TextEdit::singleline(part_number).hint_text("optional").desired_width(260.0));
                        ui.end_row();
                        ui.label("Description");
                        ui.add(egui::TextEdit::singleline(description).hint_text("optional").desired_width(260.0));
                        ui.end_row();
                    });
                    let stem = typed_path(name).trim_end_matches(".solidcraft").to_owned();
                    let file = format!("{stem}.solidcraft");
                    let path = if folder.is_empty() { file.clone() } else { format!("{folder}/{file}") };
                    let valid = !stem.is_empty() && !stem.contains('/') && !stem.starts_with('.');
                    let exists = valid && self.vault.as_ref().is_some_and(|v| v.absolute(&path).exists());
                    if exists {
                        ui.colored_label(ui.visuals().warn_fg_color, format!("{file} exists already in this folder."));
                    } else if valid {
                        ui.label(RichText::new(format!("Creates {path} as v1")).small().weak());
                    }
                    ui.checkbox(open, "Open it in SolidCraft");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let ok = valid && !exists;
                        if ui.add_enabled(ok, egui::Button::new("Create")).clicked()
                            || (ok && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        {
                            actions.push(Action::CreateDesign {
                                path: path.clone(),
                                part_number: part_number.clone(),
                                description: description.clone(),
                                open: *open,
                            });
                            close = true;
                        }
                        close |= ui.button("Cancel").clicked();
                    });
                }
                Dialog::Settings { open_command } => {
                    ui.heading("Settings");
                    ui.add_space(4.0);
                    ui.label(RichText::new("Opening designs").strong());
                    match &self.detected {
                        Some(found) => {
                            ui.label(format!("SolidCraft found: {}", found.command[0]));
                            ui.label(RichText::new(format!("from {}", found.source)).small().weak());
                        }
                        None => {
                            ui.label("SolidCraft was not found (no desktop entry, not on PATH). Choose its AppImage below.");
                        }
                    }
                    ui.add_space(6.0);
                    ui.label("Use this program instead (optional):");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(open_command)
                                .hint_text("e.g. ~/Applications/SolidCraft.AppImage")
                                .desired_width(ui.available_width() - 90.0),
                        );
                        if ui.button("Browse…").clicked() {
                            if let Some(file) = rfd::FileDialog::new().set_title("SolidCraft program or AppImage").pick_file() {
                                *open_command = file.display().to_string();
                            }
                        }
                    });
                    ui.label(
                        RichText::new("Leave empty to use SolidCraft as found. %f stands for the design; without it, the design is added at the end.")
                            .small()
                            .weak(),
                    );
                    ui.add_space(10.0);
                    ui.label(RichText::new("Interface size").strong());
                    ui.horizontal_wrapped(|ui| {
                        let current = ui.ctx().zoom_factor();
                        for zoom in ZOOM_PRESETS {
                            let selected = (current - zoom).abs() < 0.01;
                            if ui.selectable_label(selected, format!("{:.0} %", zoom * 100.0)).clicked() {
                                ui.ctx().set_zoom_factor(zoom);
                            }
                        }
                    });
                    ui.label(RichText::new("Also Ctrl + and Ctrl - anywhere, Ctrl 0 to reset.").small().weak());
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
        if focused {
            // Unfocused, nothing is scheduled: regaining focus rescans anyway.
            ctx.request_repaint_after(RESCAN_EVERY);
        }
        self.settings.zoom = ctx.zoom_factor();
        if ctx.input(|i| i.key_pressed(egui::Key::F5)) {
            self.start_scan(ctx);
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            self.focus_search = true;
        }

        let mut actions = Vec::new();
        egui::TopBottomPanel::top("top").show(ctx, |ui| self.top_bar(ui, &mut actions));
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| self.status_bar(ui));
        if self.vault.is_some() {
            egui::SidePanel::left("folders")
                .default_width(220.0)
                .min_width(150.0)
                .show(ctx, |ui| self.left_panel(ui, &mut actions));
            egui::SidePanel::right("details")
                .default_width(360.0)
                .min_width(280.0)
                .show(ctx, |ui| self.details(ui, &mut actions));
            egui::CentralPanel::default().show(ctx, |ui| self.list(ui, &mut actions));
        } else {
            egui::CentralPanel::default().show(ctx, |ui| self.welcome(ui, &mut actions));
        }
        self.dialogs(ctx, &mut actions);
        drag_label(ctx);
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
fn thumbnail(ui: &mut egui::Ui, image: Option<egui::Image<'static>>, size: egui::Vec2, sense: egui::Sense) -> egui::Response {
    match image {
        Some(image) => ui.add(image.fit_to_exact_size(size).corner_radius(4.0).sense(sense)),
        None => {
            let (rect, response) = ui.allocate_exact_size(size, sense);
            let visuals = ui.visuals();
            ui.painter().rect_filled(rect, 4.0, visuals.faint_bg_color);
            ui.painter().rect_stroke(rect, 4.0, visuals.widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);
            let font = if size.y > 80.0 { egui::FontId::proportional(13.0) } else { egui::FontId::proportional(18.0) };
            let text = if size.y > 80.0 { "No saved preview" } else { "⬚" };
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, font, visuals.weak_text_color());
            response
        }
    }
}

/// How a version in the history relates to the working file.
#[derive(Clone, Copy, PartialEq)]
enum Sameness {
    /// The newest version with the working file's content.
    Working,
    /// An older version with that content (the one a restore brought back).
    SameContent,
    Different,
}

/// "New design here…" and "New folder here…" for `folder` ("" is the top level).
fn new_here_menu(ui: &mut egui::Ui, folder: &str, actions: &mut Vec<Action>) {
    let place = if folder.is_empty() { "the top level".to_owned() } else { folder.to_owned() };
    if ui.button("New design here…").on_hover_text(format!("In {place}")).clicked() {
        actions.push(Action::AskNewDesign(folder.to_owned()));
        ui.close();
    }
    if ui.button("New folder here…").on_hover_text(format!("In {place}")).clicked() {
        actions.push(Action::AskNewFolder(folder.to_owned()));
        ui.close();
    }
}

/// Checkboxes for the list's optional columns.
fn column_toggles(ui: &mut egui::Ui, hidden: &[ListColumn], actions: &mut Vec<Action>) {
    for column in ListColumn::ALL {
        let mut shown = !hidden.contains(&column);
        if ui.checkbox(&mut shown, column.menu_label()).changed() {
            actions.push(Action::ToggleColumn(column));
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

/// A path as typed in a dialog: trimmed, `\` (as on Windows) read as `/`, no leading or
/// trailing `/`.
fn typed_path(text: &str) -> String {
    text.trim().replace('\\', "/").trim_matches('/').to_owned()
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

/// Makes a folder entry take dragged designs (moving them into `folder`; "" is the top level).
fn drop_target(ui: &egui::Ui, response: &egui::Response, folder: &str, actions: &mut Vec<Action>) {
    if response.dnd_hover_payload::<DraggedDesign>().is_some() {
        let stroke = egui::Stroke::new(2.0, ui.visuals().selection.stroke.color);
        ui.painter().rect_stroke(response.rect.expand(2.0), 4.0, stroke, egui::StrokeKind::Outside);
    }
    if let Some(design) = response.dnd_release_payload::<DraggedDesign>() {
        actions.push(Action::MoveToFolder { path: design.0.clone(), folder: folder.to_owned() });
    }
}

/// Shows which design is being dragged, next to the pointer.
fn drag_label(ctx: &egui::Context) {
    let Some(design) = egui::DragAndDrop::payload::<DraggedDesign>(ctx) else { return };
    let Some(pos) = ctx.pointer_interact_pos() else { return };
    let file = design.0.rsplit_once('/').map_or(design.0.as_str(), |(_, f)| f).to_owned();
    egui::Area::new(egui::Id::new("dragged-design"))
        .order(egui::Order::Tooltip)
        .fixed_pos(pos + egui::vec2(14.0, 10.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(format!("Move {file} to…"));
            });
        });
}

/// `text` in the strong text colour, with the chars at `matches` highlighted.
fn highlighted(ui: &egui::Ui, text: &str, matches: &[usize]) -> egui::text::LayoutJob {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let normal = egui::TextFormat::simple(font.clone(), ui.visuals().strong_text_color());
    let hit_color = ui.visuals().warn_fg_color;
    let hit = egui::TextFormat { underline: egui::Stroke::new(1.0, hit_color), ..egui::TextFormat::simple(font, hit_color) };
    let mut job = egui::text::LayoutJob::default();
    for (i, c) in text.chars().enumerate() {
        let format = if matches.binary_search(&i).is_ok() { hit.clone() } else { normal.clone() };
        job.append(c.encode_utf8(&mut [0; 4]), 0.0, format);
    }
    job
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
    fn typed_paths_accept_windows_separators() {
        assert_eq!(super::typed_path("  archive\\2026\\ "), "archive/2026");
        assert_eq!(super::typed_path("/parts/bracket.solidcraft/"), "parts/bracket.solidcraft");
    }

    #[test]
    fn links_resolve_relative_to_the_linking_design() {
        assert_eq!(resolve_link("asm/box.solidcraft", "lid.solidcraft").as_deref(), Some("asm/lid.solidcraft"));
        assert_eq!(resolve_link("asm/box.solidcraft", "../parts/lid.solidcraft").as_deref(), Some("parts/lid.solidcraft"));
        assert_eq!(resolve_link("box.solidcraft", "./lid.solidcraft").as_deref(), Some("lid.solidcraft"));
        assert_eq!(resolve_link("box.solidcraft", "../outside.solidcraft"), None);
    }
}
