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

use crate::backend::{Backend, OpenError};
use crate::launcher::{self, Launcher};
use crate::scan::{self, ImageFile, Info, RemoteInfo, Scan};
use crate::search::{Haystack, Search};
use crate::icons::{self, Icon};
use crate::theme::{self, size};
use crate::update;

mod settings_page;
use settings_page::Section;
use scformat::compare::{Change, Comparison, FeatureChangeKind};

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
/// How long a local vault keeps deleted designs.
const KEEP_DELETED: Duration = Duration::from_secs(30 * 24 * 3600);
/// How long a saved design must stay unchanged before it is checked in automatically (so a
/// save in progress is not caught half-way).
const AUTO_CHECKIN_AFTER: Duration = Duration::from_secs(4);
/// How often the vault is looked at while the window is in the background, with automatic
/// check-in (the user is working in SolidCraft meanwhile).
const AUTO_SCAN_EVERY: Duration = Duration::from_secs(6);

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
    /// Servers connected to before (address, user); passwords are never kept.
    servers: Vec<(String, String)>,
    /// Look for a new release once a day (the AppImage and the Windows installation only).
    check_updates: bool,
    /// Dark, light, or as the system is.
    appearance: Appearance,
    /// One of SolidCraft's colour themes instead (its id; empty: none).
    theme: String,
    /// Theme files imported in the settings, as they were read.
    imported_themes: Vec<String>,
    /// Saved designs become versions by themselves (one person's work: no manual check-in).
    auto_checkin: bool,
    /// Thumbnails drawn by SolidCraft with outlined edges (else the ones saved in the files).
    draw_thumbnails: bool,
    /// Production files made when a design is released (extensions, see `export::FORMATS`).
    release_files: Vec<String>,
    /// The program each kind of production file opens in, by extension (none: the system's).
    file_apps: BTreeMap<String, launcher::FileApp>,
    /// When that was last done (Unix seconds), and a version not to point out again.
    last_update_check: i64,
    skipped_update: String,
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
            servers: Vec::new(),
            check_updates: true,
            appearance: Appearance::System,
            theme: String::new(),
            imported_themes: Vec::new(),
            auto_checkin: false,
            draw_thumbnails: true,
            release_files: crate::export::FORMATS.iter().map(|(ext, _)| (*ext).to_owned()).collect(),
            file_apps: BTreeMap::new(),
            last_update_check: 0,
            skipped_update: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Appearance {
    System,
    Dark,
    Light,
}

impl Appearance {
    fn preference(self) -> egui::ThemePreference {
        match self {
            Appearance::System => egui::ThemePreference::System,
            Appearance::Dark => egui::ThemePreference::Dark,
            Appearance::Light => egui::ThemePreference::Light,
        }
    }
}

/// The list's optional columns (the thumbnail and the design are always shown).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
enum ListColumn {
    Status,
    State,
    Version,
    PartNumber,
    Revision,
    Modified,
}

impl ListColumn {
    const ALL: [ListColumn; 6] = [
        ListColumn::Status,
        ListColumn::State,
        ListColumn::Version,
        ListColumn::PartNumber,
        ListColumn::Revision,
        ListColumn::Modified,
    ];

    fn title(self) -> &'static str {
        match self {
            ListColumn::Status => "Status",
            ListColumn::State => "State",
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

    /// `remote`: a server vault's statuses are longer ("Checked out by anna").
    fn width(self, remote: bool) -> f32 {
        match self {
            ListColumn::Status if remote => 150.0,
            ListColumn::Status => 96.0,
            ListColumn::State => 104.0,
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
    /// Released or obsolete designs.
    Lifecycle(vault::State),
    /// Designs directly in the vault folder.
    TopLevel,
    /// Everything below this folder.
    Folder(String),
    /// A local vault's deleted designs (kept 30 days).
    Deleted,
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
    NewFolder { parent: String, name: String },
    NewDesign { folder: String, name: String, part_number: String, description: String, open: bool },
    Compare { path: String, from: Side, to: Side, result: Option<Result<Comparison, String>>, look: CompareLook },
    /// The workspace's login expired.
    Login { password: String, error: Option<String> },
    /// Connect to a server and make a workspace of one of its vaults.
    Connect(ConnectForm),
    /// A newer release: its notes, and installing it.
    Update { error: Option<String> },
    /// Deleting a design: which designs use it, and whether on a server.
    Delete { path: String, used_in: Vec<String>, remote: bool },
    /// Removing a deleted design for good.
    Purge { id: i64, path: String },
    /// `unreleased`: designs the assembly uses that are not released (or changed since), and why.
    Release { path: String, revision: String, note: String, unreleased: Vec<(String, &'static str)> },
    Properties { path: String, name: String, values: [String; 7], check_in: bool, comment: String },
}

/// The design properties the PDM edits, in the order shown, with their labels.
const EDITABLE: [(&str, &str); 7] = [
    ("part_number", "Part number"),
    ("revision", "Revision"),
    ("project", "Project"),
    ("description", "Description"),
    ("author", "Author"),
    ("organization", "Organization"),
    ("license", "License"),
];

/// Connecting to a server: the login, then a vault and a folder for its workspace.
struct ConnectForm {
    url: String,
    user: String,
    password: String,
    /// Logged in: the session and the vaults this user may use.
    client: Option<remote::Client>,
    vaults: Vec<protocol::VaultInfo>,
    vault: String,
    folder: String,
    error: Option<String>,
}

/// How a comparison shows the two sides' pictures.
#[derive(Clone, Copy, PartialEq)]
struct CompareLook {
    overlay: bool,
    /// Overlaid: how much of the "To" side shows over the "From" side.
    blend: f32,
    view: crate::render::View,
}

impl Default for CompareLook {
    fn default() -> Self {
        CompareLook { overlay: false, blend: 0.5, view: crate::render::View::Iso }
    }
}

/// A side's picture for comparing, and whether a larger one is being drawn.
#[derive(Clone)]
struct SidePicture {
    image: Option<egui::Image<'static>>,
    drawing: bool,
}

/// One side of a comparison.
#[derive(Clone, Copy, PartialEq)]
enum Side {
    Working,
    Version(u32),
}

/// What the user asked for while the frame was drawn; applied after drawing.
enum Action {
    Select(String),
    Open(String),
    ShowInFolder(String),
    CheckIn { path: String, comment: String },
    CheckInAll { comment: String, untracked: bool },
    AskRestore { path: String, number: u32 },
    /// Compares a version with the working file (the working version with the one before).
    AskCompare { path: String, number: u32, is_working: bool },
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
    AskRelease(String),
    AskConnect,
    ConnectServer,
    JoinVault,
    Relogin(String),
    Get(String),
    GetAll,
    Checkout(String),
    UndoCheckout { path: String, discard: bool },
    RemoteCheckin { path: String, comment: String, keep: bool },
    AddToServer { path: String, comment: String },
    UploadImage(String),
    /// A conflict: keep a copy of the local file elsewhere, then get the latest version.
    KeepCopyAndGet(String),
    Release { path: String, revision: String, note: String },
    Revise(String),
    SetObsolete { path: String, obsolete: bool },
    AskEditProperties(String),
    SaveProperties { path: String, name: String, values: [String; 7], check_in: bool, comment: String },
    SetScheme(vault::RevisionScheme),
    /// Adds the app to the application menu (`true`) or takes it out.
    #[cfg_attr(windows, allow(dead_code))]
    MenuEntry(bool),
    CreateDesign { path: String, part_number: String, description: String, open: bool },
    /// Look for a newer release now (and say so if there is none).
    CheckForUpdates,
    SetUpdateChecks(bool),
    SetAppearance(Appearance),
    /// One of SolidCraft's colour themes, by id.
    SetTheme(String),
    ImportTheme,
    RemoveTheme(String),
    SetAutoCheckin(bool),
    SetDrawThumbnails(bool),
    /// Asks before deleting a design, then deletes it.
    AskDelete(String),
    Delete(String),
    RestoreDeleted(i64),
    AskPurge { id: i64, path: String },
    Purge(i64),
    InstallUpdate,
    SkipUpdate,
    /// Makes the production files of a release (again).
    MakeReleaseFiles { path: String, version: u32, revision: String },
    SaveReleaseFile(vault::ReleaseFile),
    SaveReleaseFiles(Vec<vault::ReleaseFile>),
    SetReleaseFormat { extension: String, on: bool },
    SetFileApp { extension: String, app: Option<launcher::FileApp> },
    /// Opens a production file in its program (Settings > Production files).
    OpenReleaseFile(vault::ReleaseFile),
    /// Saves an assembly's bill of materials as a CSV file.
    SaveBom(String),
    /// A zip of an assembly with every design it uses, in their vault folders.
    PackAndGo(String),
}

pub struct App {
    settings: Settings,
    /// The open local vault or workspace of a server vault.
    backend: Option<Backend>,
    entries: Vec<Entry>,
    /// In a workspace: each file and the server, by path; why the server is not reachable.
    remote: HashMap<String, RemoteInfo>,
    offline: Option<String>,
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
    /// The selected design's lifecycle, and the revision proposed for its next release.
    lifecycle: Option<(vault::Lifecycle, String)>,
    comment: String,
    message: Option<(String, bool)>,
    dialog: Option<Dialog>,
    focused: bool,
    /// Updates: how this copy is installed, a check under way (asked for by the user: `true`),
    /// the newer release found, an installation under way.
    install_kind: update::Kind,
    /// Automatic check-ins that failed, by path and content: not tried again until it changes.
    auto_failed: HashSet<(String, Option<String>)>,
    /// SolidCraft drawing thumbnails, and its pictures by content hash (`None`: could not).
    renderer: Option<crate::render::Renderer>,
    /// A local vault's deleted designs.
    deleted: Vec<vault::Deleted>,
    rendered: HashMap<String, Option<Arc<[u8]>>>,
    update_check: Option<(Receiver<Result<update::Release, String>>, bool)>,
    update: Option<update::Release>,
    installing: Option<(Receiver<Result<PathBuf, String>>, Arc<update::Progress>)>,
    /// Production files being made (for this design), and the releases waiting for theirs.
    exporting: Option<(String, Receiver<crate::export::Finished>)>,
    export_queue: Vec<crate::export::Request>,
    /// SolidCraft's command line program, when set (the tests); else looked for.
    solidcraft_cli: Option<PathBuf>,
    /// The settings, while they are shown.
    settings_page: Option<settings_page::SettingsPage>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext, start: Option<PathBuf>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let settings: Settings = cc.storage.and_then(|s| eframe::get_value(s, eframe::APP_KEY)).unwrap_or_default();
        theme::install(&cc.egui_ctx, settings.appearance.preference(), chosen_theme(&settings).map(|t| t.tokens()));
        // Only with settings storage, which the app always has (even on the first run); tests
        // have none and lay out at 100 %.
        if cc.storage.is_some() {
            cc.egui_ctx.set_zoom_factor(settings.zoom.clamp(0.5, 3.0));
        }
        let mut app = App {
            settings,
            backend: None,
            entries: Vec::new(),
            remote: HashMap::new(),
            offline: None,
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
            lifecycle: None,
            comment: String::new(),
            message: None,
            dialog: None,
            focused: false,
            install_kind: update::Kind::detect(),
            auto_failed: HashSet::new(),
            renderer: None,
            deleted: Vec::new(),
            rendered: HashMap::new(),
            update_check: None,
            update: None,
            installing: None,
            exporting: None,
            export_queue: Vec::new(),
            solidcraft_cli: None,
            settings_page: None,
        };
        // Not without stored settings (the tests): they show the thumbnails saved in the files.
        if cc.storage.is_some() && app.settings.draw_thumbnails {
            app.start_renderer(&cc.egui_ctx);
        }
        let due = now_secs() - app.settings.last_update_check >= 24 * 3600;
        if app.settings.check_updates && app.install_kind.checks() && due {
            app.check_for_updates(&cc.egui_ctx, false);
        }
        let ctx = &cc.egui_ctx;
        match start {
            Some(dir) => app.open_vault(ctx, dir),
            None => {
                let openable = |p: &PathBuf| p.join(vault::PDM_DIR).is_dir() || remote::Workspace::is_workspace(p);
                if let Some(last) = app.settings.last_vault.clone().filter(openable) {
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
    /// Automatic check-in (as in the settings).
    pub fn set_auto_checkin(&mut self, on: bool) {
        self.settings.auto_checkin = on;
    }

    pub fn set_open_command(&mut self, command: &str) {
        self.settings.open_command = command.trim().to_owned();
    }

    /// Whether a background scan is running.
    pub fn is_scanning(&self) -> bool {
        self.scan.is_some()
    }

    /// Draws thumbnails and comparison pictures with SolidCraft, as the app does when it has
    /// settings storage (the tests have none; the README's screenshots ask for it).
    pub fn start_drawing(&mut self, ctx: &egui::Context) {
        self.start_renderer(ctx);
    }

    /// Whether SolidCraft is drawing pictures.
    pub fn is_drawing(&self) -> bool {
        self.renderer.as_ref().is_some_and(|r| r.busy())
    }

    /// The status line's message, and whether it is an error.
    pub fn message(&self) -> Option<(&str, bool)> {
        self.message.as_ref().map(|(text, error)| (text.as_str(), *error))
    }

    /// Whether production files are being made.
    pub fn is_exporting(&self) -> bool {
        self.exporting.is_some() || !self.export_queue.is_empty()
    }

    /// SolidCraft's command line program to make production files with (instead of looking).
    pub fn use_solidcraft_cli(&mut self, cli: PathBuf) {
        self.solidcraft_cli = Some(cli);
    }

    /// The program production files with `extension` open in (Settings > Production files).
    pub fn set_file_app(&mut self, extension: &str, name: &str, command: &str) {
        self.settings.file_apps.insert(extension.to_owned(), launcher::FileApp { name: name.to_owned(), command: command.to_owned() });
    }

    /// Which production files releasing makes (extensions: step, stl, 3mf).
    pub fn set_release_formats(&mut self, formats: &[&str]) {
        self.settings.release_files = formats.iter().map(|f| (*f).to_owned()).collect();
    }

    // ---- state ----

    fn open_vault(&mut self, ctx: &egui::Context, dir: PathBuf) {
        match Backend::open(&dir) {
            Ok(backend) => {
                let root = backend.root();
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{} — SolidCraft PDM", backend.title())));
                self.settings.recent.retain(|p| *p != root);
                self.settings.recent.insert(0, root.clone());
                self.settings.recent.truncate(MAX_RECENT);
                self.settings.last_vault = Some(root);
                self.backend = Some(backend);
                if let Some(vault) = self.backend.as_mut().and_then(Backend::local) {
                    let before = now_secs() - KEEP_DELETED.as_secs() as i64;
                    if let Err(e) = vault.purge_deleted(before) {
                        self.error(format!("Cleaning up deleted designs: {e}"));
                    }
                }
                self.deleted.clear();
                self.entries.clear();
                self.remote.clear();
                self.offline = None;
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
            Err(OpenError::NotAVault) => self.dialog = Some(Dialog::MakeVault { dir }),
            Err(OpenError::Other(e)) => self.error(e),
        }
    }

    fn start_scan(&mut self, ctx: &egui::Context) {
        let Some(backend) = &self.backend else { return };
        self.last_scan = Instant::now();
        if self.scan.is_some() {
            self.rescan = true;
            return;
        }
        let known = self.infos.keys().chain(self.image_sizes.keys()).cloned().collect();
        self.scan = Some(match backend.shared_workspace() {
            Some(ws) => scan::spawn_remote(ws, known, ctx.clone()),
            None => scan::spawn(backend.root(), known, ctx.clone()),
        });
    }

    /// Makes the production files of a release on a worker thread (after the one under way).
    fn make_release_files(&mut self, ctx: &egui::Context, path: &str, version: u32, revision: &str) {
        let Some(backend) = &self.backend else { return };
        let history = if self.selected.as_deref() == Some(path) { self.history.clone() } else { backend.history(path).unwrap_or_default() };
        let Some(v) = history.iter().find(|v| v.number == version) else {
            self.error(format!("{path} has no v{version}"));
            return;
        };
        let bytes = match backend.version_bytes(v) {
            Ok(b) => b,
            Err(e) => {
                self.error(format!("Production files of {path}: {e}"));
                return;
            }
        };
        let formats = match self.settings.release_files.clone() {
            f if f.is_empty() => crate::export::FORMATS.iter().map(|(ext, _)| (*ext).to_owned()).collect(),
            f => f,
        };
        let request = crate::export::Request { path: path.to_owned(), version, revision: revision.to_owned(), bytes, file: backend.absolute(path), formats };
        self.export_queue.push(request);
        self.next_export(ctx);
    }

    fn next_export(&mut self, ctx: &egui::Context) {
        if self.exporting.is_some() || self.export_queue.is_empty() {
            return;
        }
        let request = self.export_queue.remove(0);
        let path = request.path.clone();
        let solidcraft = self.launcher().and_then(|l| l.command.first().map(PathBuf::from));
        self.exporting = Some((path, crate::export::start(ctx, request, self.solidcraft_cli.clone(), solidcraft)));
    }

    fn poll_exports(&mut self, ctx: &egui::Context) {
        let Some((_, rx)) = &self.exporting else { return };
        let finished = match rx.try_recv() {
            Ok(f) => f,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.exporting = None;
                self.error("Making the production files stopped");
                return;
            }
        };
        self.exporting = None;
        let crate::export::Finished { path, version, revision, result } = finished;
        let stored = result.and_then(|files| {
            let backend = self.backend.as_mut().ok_or("no vault")?;
            backend.add_release_files(&path, version, &files)?;
            Ok(files.into_iter().map(|(name, _)| name).collect::<Vec<_>>())
        });
        match stored {
            Ok(names) => self.info_msg(format!("Stored {} with revision {revision} of {path}", names.join(", "))),
            Err(e) => self.error(format!("Production files of {path}, revision {revision}: {e}")),
        }
        self.load_history();
        self.start_scan(ctx);
        self.next_export(ctx);
    }

    fn check_for_updates(&mut self, ctx: &egui::Context, asked: bool) {
        self.settings.last_update_check = now_secs();
        let kind = self.install_kind.clone();
        self.update_check = Some((update::spawn(ctx, move || update::latest(&update::api(), &kind)), asked));
    }

    fn poll_updates(&mut self, ctx: &egui::Context) {
        if let Some((rx, asked)) = &self.update_check {
            let asked = *asked;
            match rx.try_recv() {
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.update_check = None,
                Ok(result) => {
                    self.update_check = None;
                    match result {
                        Ok(release) if release.version > update::current() => {
                            let skipped = update::version_text(release.version) == self.settings.skipped_update;
                            self.update = Some(release);
                            if asked {
                                self.dialog = Some(Dialog::Update { error: None });
                            } else if skipped {
                                self.update = None;
                            }
                        }
                        Ok(_) if asked => self.info_msg(format!("SolidCraft PDM {} is the latest version", env!("CARGO_PKG_VERSION"))),
                        Err(e) if asked => self.error(e),
                        // A check of its own accord stays quiet (offline, say).
                        _ => {}
                    }
                }
            }
        }
        if let Some((rx, _)) = &self.installing {
            let result = match rx.try_recv() {
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(100));
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("The installation stopped".to_owned()),
                Ok(result) => result,
            };
            self.installing = None;
            match result.and_then(|program| update::start(&self.install_kind, &program).map_err(|e| format!("Cannot start {}: {e}", program.display()))) {
                Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Err(e) => self.dialog = Some(Dialog::Update { error: Some(e) }),
            }
        }
    }

    /// With automatic check-in: every saved design that has not changed for a moment becomes
    /// a version (new designs too). In a server vault: changes the vault takes without a
    /// check-out, and designs checked out by this user (which stay checked out).
    fn auto_check_in(&mut self, ctx: &egui::Context) {
        const COMMENT: &str = "Saved (checked in automatically)";
        let settled = |modified: Option<SystemTime>| modified.and_then(|m| m.elapsed().ok()).is_some_and(|age| age >= AUTO_CHECKIN_AFTER);
        let mut done = Vec::new();
        let mut failed = Vec::new();
        match self.backend.as_mut() {
            Some(Backend::Local(vault)) => {
                let due: Vec<(String, Option<String>)> = self
                    .entries
                    .iter()
                    .filter(|e| matches!(e.status, Status::Modified | Status::Untracked) && settled(e.modified))
                    .map(|e| (e.path.clone(), e.hash.clone()))
                    .filter(|key| !self.auto_failed.contains(key))
                    .collect();
                for (path, hash) in due {
                    match vault.check_in(&path, COMMENT) {
                        Ok(v) => done.push(format!("{path} (v{})", v.number)),
                        Err(vault::Error::Unchanged(_)) => {}
                        Err(e) => {
                            failed.push(format!("{path}: {e}"));
                            self.auto_failed.insert((path, hash));
                        }
                    }
                }
            }
            Some(Backend::Remote(_)) => {
                use remote::RemoteStatus as R;
                let due: Vec<(String, Option<String>, R)> = self
                    .entries
                    .iter()
                    .filter(|e| settled(e.modified) && !self.auto_failed.contains(&(e.path.clone(), e.hash.clone())))
                    .filter_map(|e| self.remote.get(&e.path).map(|r| (e.path.clone(), e.hash.clone(), r.status.clone())))
                    .filter(|(_, _, s)| matches!(s, R::Changed | R::CheckedOut { modified: true } | R::LocalOnly))
                    .collect();
                let Some(mut ws) = self.backend.as_ref().and_then(|b| b.workspace()) else { return };
                for (path, hash, status) in due {
                    let result = match status {
                        R::LocalOnly => ws.add(&path, COMMENT),
                        R::CheckedOut { .. } => ws.checkin(&path, COMMENT, true),
                        _ => ws.checkin(&path, COMMENT, false),
                    };
                    match result {
                        Ok(v) => done.push(format!("{path} (v{})", v.number)),
                        Err(e) => {
                            failed.push(format!("{path}: {e}"));
                            self.auto_failed.insert((path, hash));
                        }
                    }
                }
            }
            None => {}
        }
        if !failed.is_empty() {
            self.error(format!("Not checked in automatically: {}", failed.join("; ")));
        } else if !done.is_empty() {
            self.info_msg(format!("Checked in automatically: {}", done.join(", ")));
        }
        if !done.is_empty() {
            self.load_history();
            self.rescan = true;
            self.start_scan(ctx);
        }
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
                self.remote = scan.remote;
                self.offline = scan.offline;
                self.deleted = self.backend.as_mut().and_then(Backend::local).and_then(|v| v.deleted().ok()).unwrap_or_default();
                if !scan.followed_deletes.is_empty() {
                    self.info_msg(format!("Deleted on the server: {}", scan.followed_deletes.join(", ")));
                }
                if !scan.followed_moves.is_empty() {
                    let moves: Vec<String> = scan.followed_moves.iter().map(|(a, b)| format!("{a} to {b}")).collect();
                    self.info_msg(format!("Moved on the server: {}", moves.join(", ")));
                }
                self.rows_for = None;
                if self.selected.as_ref().is_some_and(|s| self.entry(s).is_none() && self.image_file(s).is_none()) {
                    self.selected = None;
                }
                self.load_history();
                if self.settings.auto_checkin {
                    self.auto_check_in(ctx);
                }
            }
            Err(e) if e == scan::LOGIN_NEEDED => {
                if !matches!(self.dialog, Some(Dialog::Login { .. })) {
                    self.dialog = Some(Dialog::Login { password: String::new(), error: None });
                }
            }
            Err(e) => self.error(e),
        }
        if std::mem::take(&mut self.rescan) {
            self.start_scan(ctx);
        }
    }

    fn load_history(&mut self) {
        let Some(path) = self.selected.clone() else {
            self.history.clear();
            self.lifecycle = None;
            return;
        };
        let untracked = self.entry(&path).is_none_or(|e| e.latest.is_none());
        self.history = match &self.backend {
            Some(b) if !untracked => b.history(&path).unwrap_or_default(),
            _ => Vec::new(),
        };
        self.lifecycle = match &self.backend {
            Some(b) if !untracked => b.lifecycle(&path, self.entry(&path)),
            _ => None,
        };
    }

    /// A design's links, as vault paths (one per linked component); `None` when it is missing
    /// or not readable.
    fn links_of(&self, path: &str) -> Option<Vec<String>> {
        let entry = self.entry(path).filter(|e| e.status != Status::Missing)?;
        let info = self.info(entry)?;
        Some(info.links.iter().filter_map(|l| resolve_link(path, &l.path)).collect())
    }

    /// The designs an assembly uses that are not released as they are, and why.
    fn unreleased_parts(&self, path: &str) -> Vec<(String, &'static str)> {
        crate::bom::used(path, |p| self.links_of(p))
            .into_iter()
            .filter_map(|part| {
                let why = match self.entry(&part) {
                    None => "missing",
                    Some(e) if e.status == Status::Missing => "missing",
                    Some(e) if e.state == Some(vault::State::Obsolete) => "obsolete",
                    Some(e) if e.state != Some(vault::State::Released) => "in progress",
                    Some(e) if e.status == Status::Modified => "changed since its release",
                    Some(_) => return None,
                };
                Some((part, why))
            })
            .collect()
    }

    /// The bill of materials' rows as shown and saved: quantity, part number, name,
    /// description, revision, state, file.
    fn bom_rows(&self, path: &str) -> Vec<(crate::bom::Line, [String; 5])> {
        crate::bom::lines(path, |p| self.links_of(p))
            .into_iter()
            .filter(|l| l.path != path)
            .map(|line| {
                let entry = self.entry(&line.path);
                let info = entry.and_then(|e| self.info(e));
                let stem = line.path.rsplit_once('/').map_or(line.path.as_str(), |(_, n)| n);
                let stem = stem.rsplit_once('.').map_or(stem, |(s, _)| s);
                let part_number = info.and_then(|i| i.metadata.part_number.clone()).unwrap_or_default();
                let name = info.and_then(|i| i.name.clone()).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| stem.to_owned());
                let description = info.and_then(|i| i.metadata.description.clone()).unwrap_or_default();
                let revision = info.and_then(|i| i.metadata.revision.clone()).unwrap_or_default();
                let state = match entry.map(|e| (e.state, e.status)) {
                    _ if line.missing => "Missing".to_owned(),
                    Some((Some(vault::State::Released), Status::Current)) => "Released".to_owned(),
                    Some((Some(vault::State::Released), _)) => "Released · changed".to_owned(),
                    Some((Some(vault::State::Obsolete), _)) => "Obsolete".to_owned(),
                    Some((_, Status::Untracked)) => "Not in the vault".to_owned(),
                    _ => "In progress".to_owned(),
                };
                (line, [part_number, name, description, revision, state])
            })
            .collect()
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
    fn start_renderer(&mut self, ctx: &egui::Context) {
        // The program designs open with (an AppImage, or solidcraft next to solidcraft-cli).
        let solidcraft = self.launcher().and_then(|l| l.command.first().map(PathBuf::from));
        self.renderer = crate::render::Renderer::start(ctx, solidcraft);
    }

    /// SolidCraft's own picture of the content `hash` (with outlined edges), if drawn; asks
    /// for it otherwise (`source`: what to draw).
    fn drawn(&mut self, hash: &str, source: impl FnOnce(&Self) -> Option<crate::render::Source>) -> Option<egui::Image<'static>> {
        self.drawn_shot(hash, crate::render::Shot::Thumbnail, source)
    }

    fn drawn_shot(&mut self, hash: &str, shot: crate::render::Shot, source: impl FnOnce(&Self) -> Option<crate::render::Source>) -> Option<egui::Image<'static>> {
        let key = shot.key(hash);
        if !self.rendered.contains_key(&key) {
            let renderer = self.renderer.as_ref()?;
            match renderer.cached(hash, shot) {
                Some(png) => {
                    self.rendered.insert(key.clone(), Some(png));
                }
                None => {
                    if !renderer.asked(hash, shot) {
                        let source = source(self);
                        self.renderer.as_mut()?.request(hash, shot, || source);
                    }
                    return None;
                }
            }
        }
        let png = self.rendered.get(&key)?.clone()?;
        Some(egui::Image::from_bytes(format!("bytes://drawn-{key}.png"), egui::load::Bytes::Shared(png)))
    }

    fn entry_thumb(&mut self, entry: &Entry) -> Option<egui::Image<'static>> {
        match &entry.hash {
            Some(hash) => {
                let file = self.backend.as_ref().map(|b| b.absolute(&entry.path));
                if let Some(image) = self.drawn(hash, |_| file.map(crate::render::Source::File)) {
                    return Some(image);
                }
                self.image(hash)
            }
            None => self.version_thumb(entry.latest.as_ref()?),
        }
    }

    fn version_thumb(&mut self, version: &Version) -> Option<egui::Image<'static>> {
        let v = version.clone();
        if let Some(image) = self.drawn(&version.hash, |app| app.backend.as_ref()?.version_bytes(&v).ok().map(crate::render::Source::Bytes)) {
            return Some(image);
        }
        let key = version.thumbnail.as_ref()?;
        if !self.thumbs.contains_key(key) {
            let png = self.backend.as_ref().and_then(|b| b.thumbnail(version)).map(Arc::from);
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
                            View::Lifecycle(s) => e.state == Some(*s),
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
            Action::Open(path) => {
                if self.remote.get(&path).is_some_and(|r| r.status == remote::RemoteStatus::NotDownloaded)
                    && self.remote_op(ctx, |ws| ws.get(&path)).is_none()
                {
                    return;
                }
                self.open_design(&path);
            }
            Action::AskConnect => {
                let (url, user) = self.settings.servers.first().cloned().unwrap_or_default();
                self.dialog = Some(Dialog::Connect(ConnectForm {
                    url: if url.is_empty() { "http://".into() } else { url },
                    user,
                    password: String::new(),
                    client: None,
                    vaults: Vec::new(),
                    vault: String::new(),
                    folder: String::new(),
                    error: None,
                }));
            }
            Action::ConnectServer => {
                let Some(Dialog::Connect(form)) = &mut self.dialog else { return };
                let mut client = remote::Client::new(form.url.trim(), None);
                match client.login(form.user.trim(), &form.password).and_then(|_| client.vaults().map(|v| (client, v))) {
                    Ok((client, vaults)) => {
                        form.password.clear();
                        form.error = if vaults.is_empty() { Some(format!("{} may use no vault on this server yet", form.user.trim())) } else { None };
                        form.vault = vaults.first().map(|v| v.name.clone()).unwrap_or_default();
                        form.folder = default_workspace(&form.vault);
                        form.vaults = vaults;
                        form.client = Some(client);
                        let server = (form.url.trim().to_owned(), form.user.trim().to_owned());
                        self.settings.servers.retain(|s| *s != server);
                        self.settings.servers.insert(0, server);
                    }
                    Err(e) => form.error = Some(e.to_string()),
                }
            }
            Action::JoinVault => {
                let Some(Dialog::Connect(form)) = &mut self.dialog else { return };
                let Some(client) = form.client.clone() else { return };
                let folder = PathBuf::from(form.folder.trim());
                let result = if remote::Workspace::is_workspace(&folder) {
                    Ok(())
                } else {
                    remote::Workspace::create(&folder, client, form.user.trim(), &form.vault).map(drop)
                };
                match result {
                    Ok(()) => {
                        self.dialog = None;
                        self.open_vault(ctx, folder);
                    }
                    Err(e) => form.error = Some(e.to_string()),
                }
            }
            Action::Relogin(password) => {
                let Some(ws) = self.backend.as_ref().and_then(Backend::shared_workspace) else { return };
                let mut ws = ws.lock().unwrap_or_else(|e| e.into_inner());
                let user = ws.user().to_owned();
                let mut client = remote::Client::new(ws.client().url(), None);
                let result = client.login(&user, &password).and_then(|_| ws.set_client(client, &user));
                drop(ws);
                match result {
                    Ok(()) => {
                        self.dialog = None;
                        self.info_msg(format!("Logged in as {user}"));
                        self.start_scan(ctx);
                    }
                    Err(e) => {
                        if let Some(Dialog::Login { error, .. }) = &mut self.dialog {
                            *error = Some(e.to_string());
                        }
                    }
                }
            }
            Action::Get(path) => {
                if self.remote_op(ctx, |ws| ws.get(&path)).is_some() {
                    self.info_msg(format!("Got {path}"));
                }
            }
            Action::GetAll => {
                let paths: Vec<String> = self
                    .remote
                    .iter()
                    .filter(|(_, r)| matches!(r.status, remote::RemoteStatus::NotDownloaded | remote::RemoteStatus::NewerOnServer))
                    .map(|(p, _)| p.clone())
                    .collect();
                let mut failed = Vec::new();
                let got = self
                    .remote_op(ctx, |ws| {
                        let mut got = 0;
                        for p in &paths {
                            match ws.get(p) {
                                Ok(()) => got += 1,
                                Err(e) if e.needs_login() => return Err(e),
                                Err(e) => failed.push(format!("{p}: {e}")),
                            }
                        }
                        Ok(got)
                    })
                    .unwrap_or(0);
                if failed.is_empty() {
                    self.info_msg(format!("Got {got} files"));
                } else {
                    self.error(format!("Got {got}; not: {}", failed.join("; ")));
                }
            }
            Action::Checkout(path) => {
                if self.remote_op(ctx, |ws| ws.checkout(&path)).is_some() {
                    self.info_msg(format!("{path} is checked out to you"));
                }
            }
            Action::UndoCheckout { path, discard } => {
                if self.remote_op(ctx, |ws| ws.undo_checkout(&path, discard)).is_some() {
                    self.info_msg(if discard { format!("{path}: check-out undone, changes discarded") } else { format!("{path} is no longer checked out") });
                }
            }
            Action::RemoteCheckin { path, comment, keep } => {
                if let Some(v) = self.remote_op(ctx, |ws| ws.checkin(&path, &comment, keep)) {
                    self.comment.clear();
                    self.info_msg(format!("Checked in {path} as v{}{}", v.number, if keep { " (still checked out)" } else { "" }));
                }
            }
            Action::AddToServer { path, comment } => {
                if let Some(v) = self.remote_op(ctx, |ws| ws.add(&path, &comment)) {
                    self.comment.clear();
                    self.info_msg(format!("Added {path} to the server as v{}", v.number));
                }
            }
            Action::UploadImage(path) => {
                if self.remote_op(ctx, |ws| ws.put_image(&path)).is_some() {
                    self.info_msg(format!("Uploaded {path}"));
                }
            }
            Action::KeepCopyAndGet(path) => {
                let Some(backend) = &self.backend else { return };
                let file = backend.absolute(&path);
                let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let mut dialog = rfd::FileDialog::new().set_title("Keep a copy of your version").set_file_name(format!("my copy of {name}"));
                if let Some(dir) = dirs_home() {
                    dialog = dialog.set_directory(dir);
                }
                let Some(out) = dialog.save_file() else { return };
                if let Err(e) = std::fs::copy(&file, &out).and_then(|_| {
                    let mut perms = std::fs::metadata(&file)?.permissions();
                    #[allow(clippy::permissions_set_readonly_false)]
                    perms.set_readonly(false);
                    std::fs::set_permissions(&file, perms)?;
                    std::fs::remove_file(&file)
                }) {
                    self.error(format!("Could not keep the copy: {e}"));
                    return;
                }
                if self.remote_op(ctx, |ws| ws.get(&path)).is_some() {
                    self.info_msg(format!("Your version is in {}; {path} is the latest from the server now", out.display()));
                }
            }
            Action::ShowInFolder(path) | Action::ShowFolder(path) => {
                let Some(backend) = &self.backend else { return };
                if let Err(e) = launcher::show_in_folder(&backend.absolute(&path)) {
                    self.error(format!("Could not open the file manager: {e}"));
                }
            }
            Action::CheckIn { path, comment } => {
                let Some(vault) = self.backend.as_mut().and_then(Backend::local) else { return };
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
                let Some(vault) = self.backend.as_mut().and_then(Backend::local) else { return };
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
            Action::AskCompare { path, number, is_working } => {
                let (from, to) = if is_working && number > 1 {
                    (Side::Version(number - 1), Side::Version(number))
                } else {
                    (Side::Version(number), Side::Working)
                };
                let look = match &self.dialog {
                    Some(Dialog::Compare { look, .. }) => *look,
                    _ => CompareLook::default(),
                };
                self.dialog = Some(Dialog::Compare { path, from, to, result: None, look });
            }
            Action::Restore { path, number } => {
                let Some(backend) = &mut self.backend else { return };
                match backend.restore(&path, number) {
                    Ok(v) => {
                        self.info_msg(format!("Restored v{number} of {path} (now v{})", v.number));
                        self.load_history();
                        self.start_scan(ctx);
                    }
                    Err(e) => self.error(e),
                }
            }
            Action::SaveCopy { path, version } => {
                let Some(backend) = &self.backend else { return };
                let file = backend.absolute(&path);
                let stem = file.file_stem().map_or("design".into(), |s| s.to_string_lossy().into_owned());
                let ext = file.extension().map_or("solidcraft".into(), |s| s.to_string_lossy().into_owned());
                let mut dialog = rfd::FileDialog::new().set_file_name(format!("{stem}-v{}.{ext}", version.number));
                if let Some(dir) = file.parent() {
                    dialog = dialog.set_directory(dir);
                }
                let Some(out) = dialog.save_file() else { return };
                match backend.version_bytes(&version).map_err(|e| e.to_string()).and_then(|b| {
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
                let Some(backend) = &mut self.backend else { return };
                let to = typed_path(&to);
                match backend.rename(&from, &to) {
                    Ok(report) => {
                        self.report_move(&from, &to, &report);
                        self.selected = Some(to);
                        self.load_history();
                        self.start_scan(ctx);
                    }
                    Err(e) => self.error(e),
                }
            }
            Action::Relink { missing, to } => {
                let Some(vault) = self.backend.as_mut().and_then(Backend::local) else { return };
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
            Action::CheckForUpdates => self.check_for_updates(ctx, true),
            Action::SetUpdateChecks(on) => self.settings.check_updates = on,
            Action::AskDelete(path) => {
                let used_in = self.used_in(&path);
                let remote = self.backend.as_ref().is_some_and(Backend::is_remote);
                self.dialog = Some(Dialog::Delete { path, used_in, remote });
            }
            Action::Delete(path) => {
                let Some(backend) = self.backend.as_mut() else { return };
                match backend.delete(&path) {
                    Ok(()) => {
                        let remote = backend.is_remote();
                        self.info_msg(if remote {
                            format!("Deleted {path} (the server keeps it a while: an administrator can restore it)")
                        } else {
                            format!("Deleted {path} (in Recently deleted for 30 days)")
                        });
                        if self.selected.as_deref() == Some(path.as_str()) {
                            self.selected = None;
                            self.history.clear();
                        }
                        self.start_scan(ctx);
                    }
                    Err(e) => self.error(e),
                }
            }
            Action::RestoreDeleted(id) => {
                let Some(vault) = self.backend.as_mut().and_then(Backend::local) else { return };
                match vault.restore_deleted(id) {
                    Ok(path) => {
                        self.info_msg(format!("Restored {path}"));
                        self.deleted.retain(|d| d.id != id);
                        self.start_scan(ctx);
                    }
                    Err(e) => self.error(e),
                }
            }
            Action::AskPurge { id, path } => self.dialog = Some(Dialog::Purge { id, path }),
            Action::Purge(id) => {
                let Some(vault) = self.backend.as_mut().and_then(Backend::local) else { return };
                match vault.purge_one(id) {
                    Ok(()) => {
                        self.deleted.retain(|d| d.id != id);
                        self.info_msg("Deleted for good");
                    }
                    Err(e) => self.error(e),
                }
            }
            Action::SaveBom(path) => {
                let rows: Vec<Vec<String>> = self
                    .bom_rows(&path)
                    .into_iter()
                    .map(|(line, [part_number, name, description, revision, state])| vec![line.quantity.to_string(), part_number, name, description, revision, state, line.path])
                    .collect();
                let text = crate::bom::csv(&["Quantity", "Part number", "Name", "Description", "Revision", "State", "File"], &rows);
                let stem = path.rsplit_once('/').map_or(path.as_str(), |(_, n)| n);
                let stem = stem.rsplit_once('.').map_or(stem, |(s, _)| s);
                let Some(out) = rfd::FileDialog::new().set_file_name(format!("{stem}-bom.csv")).add_filter("CSV", &["csv"]).save_file() else { return };
                match std::fs::write(&out, text) {
                    Ok(()) => self.info_msg(format!("Saved the bill of materials as {}", out.display())),
                    Err(e) => self.error(format!("Saving {}: {e}", out.display())),
                }
            }
            Action::PackAndGo(path) => {
                let Some(backend) = &self.backend else { return };
                let stem = path.rsplit_once('/').map_or(path.as_str(), |(_, n)| n);
                let stem = stem.rsplit_once('.').map_or(stem, |(s, _)| s).to_owned();
                let Some(out) = rfd::FileDialog::new().set_file_name(format!("{stem}.zip")).add_filter("Zip", &["zip"]).save_file() else { return };
                let mut designs = vec![path.clone()];
                designs.extend(crate::bom::used(&path, |p| self.links_of(p)));
                match crate::bom::pack(&backend.root(), &stem, &designs, &out) {
                    Ok(missing) if missing.is_empty() => self.info_msg(format!("Packed {} designs into {}", designs.len(), out.display())),
                    Ok(missing) => self.error(format!("Packed into {}, without {} missing: {}", out.display(), missing.len(), missing.join(", "))),
                    Err(e) => self.error(format!("Pack and go: {e}")),
                }
            }
            Action::MakeReleaseFiles { path, version, revision } => self.make_release_files(ctx, &path, version, &revision),
            Action::SaveReleaseFile(file) => {
                let Some(backend) = &self.backend else { return };
                let Some(out) = rfd::FileDialog::new().set_file_name(&file.name).save_file() else { return };
                match backend.release_file_bytes(&file).and_then(|b| std::fs::write(&out, b).map_err(|e| e.to_string())) {
                    Ok(()) => self.info_msg(format!("Saved {}", out.display())),
                    Err(e) => self.error(format!("Saving {}: {e}", file.name)),
                }
            }
            Action::SaveReleaseFiles(files) => {
                let Some(backend) = &self.backend else { return };
                let Some(dir) = rfd::FileDialog::new().set_title("Folder for the production files").pick_folder() else { return };
                let saved = files.iter().try_for_each(|f| {
                    let bytes = backend.release_file_bytes(f)?;
                    std::fs::write(dir.join(&f.name), bytes).map_err(|e| format!("{}: {e}", f.name))
                });
                match saved {
                    Ok(()) => self.info_msg(format!("Saved {} files in {}", files.len(), dir.display())),
                    Err(e) => self.error(format!("Saving the production files: {e}")),
                }
            }
            Action::SetFileApp { extension, app } => match app {
                Some(app) => {
                    self.settings.file_apps.insert(extension, app);
                }
                None => {
                    self.settings.file_apps.remove(&extension);
                }
            },
            Action::OpenReleaseFile(file) => {
                let Some(backend) = &self.backend else { return };
                let extension = file.name.rsplit_once('.').map_or(String::new(), |(_, e)| e.to_ascii_lowercase());
                let app = self.settings.file_apps.get(&extension).cloned();
                let opened = backend.release_file_bytes(&file).and_then(|bytes| {
                    // A copy under its own name (as the program shows it), in the temporary folder.
                    let dir = std::env::temp_dir().join("solidcraft-pdm-files").join(&file.hash[..16]);
                    let path = dir.join(&file.name);
                    if !path.is_file() {
                        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
                    }
                    match &app {
                        Some(app) => app.open(&path).map_err(|e| format!("{}: {e}", app.name)),
                        None => launcher::open_with_system(&path).map_err(|e| e.to_string()),
                    }
                });
                match opened {
                    Ok(()) => self.info_msg(format!("Opened {}{}", file.name, app.map_or(String::new(), |a| format!(" in {}", a.name)))),
                    Err(e) => self.error(format!("Could not open {}: {e}", file.name)),
                }
            }
            Action::SetReleaseFormat { extension, on } => {
                self.settings.release_files.retain(|e| *e != extension);
                if on {
                    self.settings.release_files.push(extension);
                }
                // In the order SolidCraft's formats are listed.
                let order = |e: &String| crate::export::FORMATS.iter().position(|(f, _)| f == e);
                self.settings.release_files.sort_by_key(order);
            }
            Action::SetDrawThumbnails(on) => {
                self.settings.draw_thumbnails = on;
                self.rendered.clear();
                self.renderer = None;
                if on {
                    self.start_renderer(ctx);
                }
            }
            Action::SetAutoCheckin(on) => {
                self.settings.auto_checkin = on;
                if on {
                    self.start_scan(ctx);
                }
            }
            Action::SetAppearance(choice) => {
                self.settings.appearance = choice;
                self.settings.theme.clear();
                theme::apply(ctx, choice.preference(), None);
            }
            Action::SetTheme(id) => {
                self.settings.theme = id;
                theme::apply(ctx, self.settings.appearance.preference(), chosen_theme(&self.settings).map(|t| t.tokens()));
            }
            Action::ImportTheme => {
                let Some(file) = rfd::FileDialog::new().set_title("A SolidCraft theme file").add_filter("Theme", &["json"]).pick_file() else { return };
                let read = std::fs::read_to_string(&file).map_err(|e| e.to_string()).and_then(|text| crate::themes::parse(&text).map(|t| (t, text)));
                match read {
                    Ok((theme, text)) => {
                        // A theme of the same name is replaced.
                        self.settings.imported_themes.retain(|t| crate::themes::parse(t).is_ok_and(|old| old.id != theme.id));
                        self.settings.imported_themes.push(text);
                        self.settings.imported_themes.truncate(crate::themes::MAX_IMPORTED);
                        self.info_msg(format!("Imported the theme {}", theme.name));
                        self.apply(ctx, Action::SetTheme(imported_id(&theme.id)));
                    }
                    Err(e) => self.error(format!("{}: {e}", file.display())),
                }
            }
            Action::RemoveTheme(id) => {
                self.settings.imported_themes.retain(|t| crate::themes::parse(t).is_ok_and(|old| imported_id(&old.id) != id));
                if self.settings.theme == id {
                    self.apply(ctx, Action::SetAppearance(self.settings.appearance));
                }
            }
            Action::SkipUpdate => {
                if let Some(release) = self.update.take() {
                    self.settings.skipped_update = update::version_text(release.version);
                }
            }
            Action::InstallUpdate => {
                let Some(release) = self.update.clone() else { return };
                let progress = Arc::new(update::Progress::default());
                let shared = progress.clone();
                let kind = self.install_kind.clone();
                let rx = update::spawn(ctx, move || match &kind {
                    update::Kind::AppImage(current) => update::install_appimage(&release, current, &shared),
                    update::Kind::Installer => update::download_installer(&release, &shared),
                    update::Kind::Other => Err("Download it from the release page".into()),
                });
                self.installing = Some((rx, progress));
                self.dialog = Some(Dialog::Update { error: None });
            }
            Action::SetOpenCommand(cmd) => {
                self.settings.open_command = cmd.trim().to_owned();
                self.detected = launcher::detect();
            }
            Action::AskNewFolder(parent) => self.dialog = Some(Dialog::NewFolder { parent, name: String::new() }),
            Action::CreateFolder(folder) => {
                let Some(backend) = &mut self.backend else { return };
                match backend.create_folder(&folder) {
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
                let Some(backend) = &mut self.backend else { return };
                match backend.remove_folder(&folder) {
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
                let Some(backend) = &self.backend else { return };
                if let Err(e) = launcher::open_with_system(&backend.absolute(&path)) {
                    self.error(format!("Could not open {path}: {e}"));
                }
            }
            Action::AskRelease(path) => {
                let revision = self.lifecycle.as_ref().map(|(_, suggested)| suggested.clone()).unwrap_or_default();
                let unreleased = self.unreleased_parts(&path);
                self.dialog = Some(Dialog::Release { path, revision, note: String::new(), unreleased });
            }
            Action::Release { path, revision, note } => {
                let Some(backend) = &mut self.backend else { return };
                match backend.release_as(&path, &revision, &note) {
                    Ok(r) => {
                        self.info_msg(format!("Released {path} as revision {} (v{})", r.revision, r.version));
                        self.load_history();
                        self.start_scan(ctx);
                        if !self.settings.release_files.is_empty() {
                            self.make_release_files(ctx, &path, r.version, &r.revision);
                        }
                    }
                    Err(e) => self.error(e),
                }
            }
            Action::Revise(path) => {
                let Some(backend) = &mut self.backend else { return };
                match backend.revise(&path) {
                    Ok(()) => {
                        self.info_msg(format!("{path} is work in progress again"));
                        self.load_history();
                        self.start_scan(ctx);
                    }
                    Err(e) => self.error(e),
                }
            }
            Action::SetObsolete { path, obsolete } => {
                let Some(backend) = &mut self.backend else { return };
                match backend.set_obsolete(&path, obsolete) {
                    Ok(()) => {
                        self.info_msg(if obsolete { format!("{path} is obsolete") } else { format!("{path} is work in progress again") });
                        self.load_history();
                        self.start_scan(ctx);
                    }
                    Err(e) => self.error(e),
                }
            }
            Action::AskEditProperties(path) => {
                let Some(backend) = &self.backend else { return };
                match scformat::read_file(backend.absolute(&path)) {
                    Ok(info) => {
                        let m = &info.metadata;
                        let values = [&m.part_number, &m.revision, &m.project, &m.description, &m.author, &m.organization, &m.license]
                            .map(|v| v.clone().unwrap_or_default());
                        let name = info.name.clone().unwrap_or_default();
                        self.dialog = Some(Dialog::Properties { path, name, values, check_in: true, comment: "Edited properties".into() });
                    }
                    Err(e) => self.error(format!("{path}: {e}")),
                }
            }
            Action::SaveProperties { path, name, values, check_in, comment } => {
                let Some(backend) = &mut self.backend else { return };
                let properties: Vec<(&str, Option<&str>)> =
                    EDITABLE.iter().zip(&values).map(|((key, _), v)| (*key, Some(v.as_str()))).collect();
                let result = backend.save_properties(&path, &name, &properties, check_in.then_some(comment.as_str()));
                match result {
                    Ok(Some(n)) => self.info_msg(format!("Saved the properties of {path} (v{n})")),
                    Ok(None) => self.info_msg(format!("Saved the properties of {path}")),
                    Err(e) => self.error(e),
                }
                self.load_history();
                self.start_scan(ctx);
            }
            Action::MenuEntry(add) => {
                #[cfg(not(windows))]
                {
                    let result = match (crate::menu_entry::data_home(), crate::menu_entry::program()) {
                        (Some(data), Some(program)) if add => crate::menu_entry::install(&data, &program),
                        (Some(data), _) if !add => crate::menu_entry::remove(&data),
                        _ => Err("no home folder or program path".to_owned()),
                    };
                    match result {
                        Ok(()) if add => self.info_msg("SolidCraft PDM is in the application menu"),
                        Ok(()) => self.info_msg("SolidCraft PDM is out of the application menu"),
                        Err(e) => self.error(e),
                    }
                }
                #[cfg(windows)]
                let _ = add;
            }
            Action::SetScheme(scheme) => {
                let Some(vault) = self.backend.as_mut().and_then(Backend::local) else { return };
                if let Err(e) = vault.set_revision_scheme(scheme) {
                    self.error(e);
                }
                self.load_history();
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
                let Some(backend) = &mut self.backend else { return };
                let (folder, file) = path.rsplit_once('/').unwrap_or(("", &path));
                let name = vault::design_stem(file);
                let bytes = scformat::new_design(name, &[("part_number", &part_number), ("description", &description)]);
                match backend.add_new(&path, &bytes, "New design") {
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
                let Some(backend) = &mut self.backend else { return };
                match backend.move_to_folder(&path, &folder) {
                    Ok(report) if report.to == path => {}
                    Ok(report) => {
                        let place = if folder.is_empty() { "the top level".to_owned() } else { folder };
                        self.report_move(&path, &place, &report);
                        if self.selected.as_deref() == Some(path.as_str()) {
                            self.selected = Some(report.to);
                        }
                        self.load_history();
                        self.start_scan(ctx);
                    }
                    Err(e) => self.error(e),
                }
            }
        }
    }

    /// Runs a workspace operation (it talks to the server); on success the history and the list
    /// are refreshed, an expired login asks for the password, other errors go to the status line.
    fn remote_op<T>(&mut self, ctx: &egui::Context, op: impl FnOnce(&mut remote::Workspace) -> remote::Result<T>) -> Option<T> {
        let ws = self.backend.as_ref()?.shared_workspace()?;
        let result = {
            let mut ws = ws.lock().unwrap_or_else(|e| e.into_inner());
            op(&mut ws)
        };
        match result {
            Ok(value) => {
                self.load_history();
                self.start_scan(ctx);
                Some(value)
            }
            Err(e) if e.needs_login() => {
                self.dialog = Some(Dialog::Login { password: String::new(), error: None });
                None
            }
            Err(e) => {
                self.error(e);
                None
            }
        }
    }

    /// What can be done with a workspace design: get, check out, check in, add.
    fn remote_panel(&mut self, ui: &mut egui::Ui, entry: &Entry, r: &RemoteInfo, actions: &mut Vec<Action>) {
        use remote::RemoteStatus as R;
        let path = entry.path.clone();
        let me = self.backend.as_ref().and_then(|b| b.workspace().map(|w| w.user().to_owned())).unwrap_or_default();
        let t = theme::tokens(ui);
        ui.add_space(10.0);
        theme::card(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                icons::show(ui, Icon::Server, 16.0, t.icon);
                ui.label(RichText::new("On the server").font(theme::semibold(size::HEADING)));
            });
            if let Some(lock) = &r.lock {
                let who = if lock.user == me { "you".to_owned() } else { lock.user.clone() };
                ui.horizontal(|ui| {
                    icons::show(ui, Icon::Lock, 14.0, if lock.user == me { t.accent } else { t.other });
                    ui.label(format!("Checked out by {who} since {}", unix_time(lock.since)));
                });
            }
            if let (Some(server), Some(base)) = (r.server_version, r.base_version) {
                if server != base {
                    ui.label(RichText::new(format!("v{server} on the server; you have v{base}")).color(t.accent));
                }
            }
            let required = self.backend.as_ref().and_then(|b| b.workspace().map(|w| w.checkout_required())).unwrap_or(true);
            let comment_box = |ui: &mut egui::Ui, comment: &mut String| {
                ui.add(egui::TextEdit::multiline(comment).hint_text("What changed?").desired_rows(2).margin(egui::vec2(8.0, 6.0)).desired_width(f32::INFINITY));
            };
            ui.horizontal_wrapped(|ui| match &r.status {
                R::NotDownloaded => {
                    if ui.add(theme::primary("Get", t)).on_hover_text("Fetch it into the workspace").clicked() {
                        actions.push(Action::Get(path.clone()));
                    }
                }
                R::NewerOnServer => {
                    if ui.add(theme::primary("Get latest", t)).clicked() {
                        actions.push(Action::Get(path.clone()));
                    }
                    if ui.button("Check out").on_hover_text("Fetches the latest version and locks it for you").clicked() {
                        actions.push(Action::Checkout(path.clone()));
                    }
                }
                R::UpToDate if required => {
                    if ui.add(theme::primary("Check out", t)).on_hover_text("Lock it for you, to change it").clicked() {
                        actions.push(Action::Checkout(path.clone()));
                    }
                }
                R::UpToDate => {
                    // Saving it is enough here; a check-out only keeps others out meanwhile.
                    if ui.add(theme::secondary("Check out")).on_hover_text("Not needed in this vault: only keeps others out while you work").clicked() {
                        actions.push(Action::Checkout(path.clone()));
                    }
                }
                R::CheckedOutByOther(user) => {
                    ui.weak(format!("{user} is working on it; you can open it read-only."));
                }
                R::ChangedWithoutCheckout => {
                    ui.colored_label(ui.visuals().warn_fg_color, "You changed this without checking it out.");
                    if ui.add(theme::primary("Check out now", t)).clicked() {
                        actions.push(Action::Checkout(path.clone()));
                    }
                }
                R::Conflict => {
                    ui.colored_label(ui.visuals().error_fg_color, "Changed here and on the server.");
                    if ui.button("Keep my copy, get the latest…").on_hover_text("Saves your version elsewhere first").clicked() {
                        actions.push(Action::KeepCopyAndGet(path.clone()));
                    }
                }
                R::RemovedOnServer => {
                    ui.weak("No longer on the server (deleted or moved by someone).");
                }
                R::CheckedOut { .. } | R::LocalOnly | R::Changed => {}
            });
            match &r.status {
                R::CheckedOut { modified } => {
                    comment_box(ui, &mut self.comment);
                    ui.horizontal_wrapped(|ui| {
                        if ui.add_enabled(*modified, theme::primary("⬆ Check in", t)).on_disabled_hover_text("Nothing changed yet").clicked() {
                            actions.push(Action::RemoteCheckin { path: path.clone(), comment: self.comment.clone(), keep: false });
                        }
                        if ui.add_enabled(*modified, egui::Button::new("Check in, keep checked out")).clicked() {
                            actions.push(Action::RemoteCheckin { path: path.clone(), comment: self.comment.clone(), keep: true });
                        }
                        if ui.button("Undo check-out").on_hover_text("Keeps your changes here").clicked() {
                            actions.push(Action::UndoCheckout { path: path.clone(), discard: false });
                        }
                        if *modified && ui.button("Undo and discard changes").clicked() {
                            actions.push(Action::UndoCheckout { path: path.clone(), discard: true });
                        }
                    });
                }
                R::Changed => {
                    comment_box(ui, &mut self.comment);
                    if ui.add(theme::primary("⬆ Check in", t)).on_hover_text("Uploads it as the next version").clicked() {
                        actions.push(Action::RemoteCheckin { path: path.clone(), comment: self.comment.clone(), keep: false });
                    }
                }
                R::LocalOnly => {
                    ui.label("Not on the server yet.");
                    comment_box(ui, &mut self.comment);
                    if ui.add(theme::primary("⬆ Add to the server", t)).clicked() {
                        actions.push(Action::AddToServer { path: path.clone(), comment: self.comment.clone() });
                    }
                }
                _ => {}
            }
        });
    }

    /// The picture of a side of a comparison.
    /// A side's large picture from `view`, drawn by SolidCraft; the thumbnail while it is drawn,
    /// or when it cannot be.
    fn side_picture(&mut self, path: &str, side: Side, view: crate::render::View) -> SidePicture {
        let shot = crate::render::Shot::Large(view);
        let (hash, version) = match side {
            Side::Working => (self.entry(path).and_then(|e| e.hash.clone()), None),
            Side::Version(n) => match self.history.iter().find(|v| v.number == n) {
                Some(v) => (Some(v.hash.clone()), Some(v.clone())),
                None => (None, None),
            },
        };
        if let Some(hash) = &hash {
            let file = self.backend.as_ref().map(|b| b.absolute(path));
            let large = self.drawn_shot(hash, shot, |app| match &version {
                Some(v) => app.backend.as_ref()?.version_bytes(v).ok().map(crate::render::Source::Bytes),
                None => file.map(crate::render::Source::File),
            });
            if large.is_some() {
                return SidePicture { image: large, drawing: false };
            }
        }
        let drawing = hash.as_ref().is_some_and(|h| self.renderer.is_some() && !self.rendered.contains_key(&shot.key(h)));
        SidePicture { image: self.side_image(path, side), drawing }
    }

    fn side_image(&mut self, path: &str, side: Side) -> Option<egui::Image<'static>> {
        match side {
            Side::Working => {
                let entry = self.entry(path)?.clone();
                self.entry_thumb(&entry)
            }
            Side::Version(n) => {
                let version = self.history.iter().find(|v| v.number == n)?.clone();
                self.version_thumb(&version)
            }
        }
    }

    /// The status line after a move: where to, and what happened to links.
    fn report_move(&mut self, from: &str, place: &str, report: &vault::MoveReport) {
        let mut msg = format!("Moved {from} to {place}");
        if !report.updated.is_empty() {
            let names: Vec<String> = report
                .updated
                .iter()
                .map(|u| match u.version {
                    Some(v) => format!("{} (v{v})", u.path),
                    None => format!("{} (with your unsaved changes)", u.path),
                })
                .collect();
            msg.push_str(&format!("; links updated in {}", names.join(", ")));
        }
        if report.failed.is_empty() {
            self.info_msg(msg);
        } else {
            let failed: Vec<String> = report.failed.iter().map(|(p, e)| format!("{p}: {e}")).collect();
            self.error(format!("{msg}; links NOT updated in {}", failed.join("; ")));
        }
    }

    fn open_design(&mut self, path: &str) {
        let Some(backend) = &self.backend else { return };
        let file = backend.absolute(path);
        let Some(launcher) = self.launcher() else {
            self.error("SolidCraft was not found. Choose its AppImage in Settings.");
            self.open_settings(Section::Opening);
            return;
        };
        if let Err(e) = launcher.open(&file) {
            self.error(format!("Could not start {}: {e} (change it in Vault > Settings)", launcher.command[0]));
        }
    }

    /// Opens an earlier version in SolidCraft as a copy in a read-only folder, so that
    /// SolidCraft cannot save over it; the working file and the vault stay as they are.
    fn open_version_copy(&mut self, path: &str, version: &Version) {
        let Some(backend) = &self.backend else { return };
        let file = path.rsplit_once('/').map_or(path, |(_, f)| f);
        let (stem, ext) = file.rsplit_once('.').unwrap_or((file, "solidcraft"));
        let dir = std::env::temp_dir().join("solidcraft-pdm").join(&version.hash[..16]);
        let copy = dir.join(format!("{stem} (v{}, read-only).{ext}", version.number));
        let result = (|| -> Result<(), String> {
            if !copy.is_file() {
                let bytes = backend.version_bytes(version).map_err(|e| e.to_string())?;
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

    /// The app bar (dark in both themes, as SolidCraft's): the app, its menus, the open vault,
    /// and on the right the server connection, an update and the settings.
    fn top_bar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let t = theme::tokens(ui);
        // Light text and see-through buttons on the dark bar; menus keep the normal look.
        let v = ui.visuals_mut();
        v.override_text_color = Some(t.app_bar_text);
        for w in [&mut v.widgets.inactive, &mut v.widgets.noninteractive] {
            w.weak_bg_fill = Color32::TRANSPARENT;
            w.bg_fill = Color32::TRANSPARENT;
            w.bg_stroke = egui::Stroke::NONE;
        }
        for w in [&mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.weak_bg_fill = Color32::from_white_alpha(28);
            w.bg_fill = Color32::from_white_alpha(28);
            w.bg_stroke = egui::Stroke::NONE;
        }
        ui.horizontal(|ui| {
            ui.set_height(28.0);
            ui.add(egui::Image::new(egui::include_image!("../../../assets/solidcraft-pdm-256.png")).fit_to_exact_size(egui::vec2(22.0, 22.0)));
            ui.label(RichText::new("SolidCraft PDM").font(theme::semibold(size::HEADING)).color(t.app_bar_text));
            app_bar_divider(ui, t);
            let menus = |ui: &mut egui::Ui| {
                // The menus themselves in the normal look: rows, no button frames.
                ui.reset_style();
                ui.style_mut().visuals.button_frame = false;
                ui.spacing_mut().button_padding = egui::vec2(10.0, 6.0);
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.set_min_width(220.0);
            };
            ui.menu_button(RichText::new("Vault").color(t.app_bar_text), |ui| {
                menus(ui);
                if ui.button("Connect to a server…").clicked() {
                    actions.push(Action::AskConnect);
                }
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
                    self.open_settings(Section::Opening);
                }
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            // Stays open while columns are ticked.
            let keep_open = egui::containers::menu::MenuConfig::new()
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
            egui::containers::menu::MenuButton::new(RichText::new("View").color(t.app_bar_text)).config(keep_open).ui(ui, |ui| {
                menus(ui);
                theme::section(ui, "Columns");
                column_toggles(ui, &self.settings.hidden_columns, actions);
                ui.separator();
                theme::section(ui, &format!("Interface size: {:.0} %", ui.ctx().zoom_factor() * 100.0));
                egui::gui_zoom::zoom_menu_buttons(ui);
            });
            if let Some(backend) = &self.backend {
                app_bar_divider(ui, t);
                // The open vault, as a tab.
                let (icon, title) = (if backend.is_remote() { Icon::Server } else { Icon::Folder }, backend.title());
                egui::Frame::new().fill(t.app_bar_tab).corner_radius(egui::CornerRadius::same(theme::RADIUS)).inner_margin(egui::Margin::symmetric(10, 4)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        icons::show(ui, icon, 15.0, t.app_bar_text);
                        ui.label(RichText::new(title).font(theme::medium(size::BODY)).color(t.app_bar_text));
                    });
                });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let settings = ui.add(egui::Button::image(icons::image(ui, Icon::Settings, t.app_bar_text).fit_to_exact_size(egui::vec2(17.0, 17.0))).min_size(egui::vec2(30.0, 28.0)));
                settings.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Settings"));
                if settings.on_hover_text("Settings").clicked() {
                    self.open_settings(Section::Opening);
                }
                if let Some(release) = &self.update {
                    let label = format!("⬆ Update to {}", update::version_text(release.version));
                    let button = egui::Button::new(RichText::new(label).font(theme::medium(size::BODY)).color(t.on_primary)).fill(t.accent).corner_radius(egui::CornerRadius::same(theme::RADIUS));
                    if ui.add(button).on_hover_text("What is new, and installing it").clicked() {
                        self.dialog = Some(Dialog::Update { error: None });
                    }
                }
                if let Some(ws) = self.backend.as_ref().and_then(|b| b.workspace()) {
                    let (dot, text) = match &self.offline {
                        Some(_) => (t.warning, "Offline".to_owned()),
                        None => (Color32::from_rgb(0x4C, 0xC2, 0x7A), format!("{} · {}", ws.user(), host_of(ws.client().url()))),
                    };
                    // A chip: dot and text, drawn at its own size (the row runs right to left).
                    let galley = ui.painter().layout_no_wrap(text, theme::medium(size::SMALL + 0.5), t.app_bar_text);
                    let (rect, response) = ui.allocate_exact_size(galley.size() + egui::vec2(34.0, 10.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, egui::CornerRadius::same(theme::MENU_RADIUS), Color32::from_white_alpha(14));
                    ui.painter().circle_filled(egui::pos2(rect.left() + 14.0, rect.center().y), 4.0, dot);
                    ui.painter().galley(egui::pos2(rect.left() + 24.0, rect.center().y - galley.size().y / 2.0), galley, t.app_bar_text);
                    response
                    .on_hover_text(match &self.offline {
                        Some(why) => format!("The server is not reachable: {why}"),
                        None => format!("Connected to {} as {}", ws.client().url(), ws.user()),
                    });
                }
                if self.scan.is_some() {
                    ui.add(egui::Spinner::new().size(14.0).color(t.app_bar_dim));
                }
            });
        });
    }

    /// Under the app bar: what to do with the vault, and the search.
    fn toolbar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let t = theme::tokens(ui);
        ui.horizontal(|ui| {
            let folder = self.current_folder();
            let tool = |ui: &mut egui::Ui, icon: Icon, label: &str, enabled: bool| {
                let image = icons::image(ui, icon, if enabled { t.icon } else { t.text_dim }).fit_to_exact_size(egui::vec2(16.0, 16.0));
                ui.add_enabled(enabled, egui::Button::image_and_text(image, label).min_size(egui::vec2(0.0, theme::CONTROL_HEIGHT)))
            };
            let new = tool(ui, Icon::Plus, "New design", true);
            new.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "+ New design"));
            if new.on_hover_text("Create an empty design and open it in SolidCraft").clicked() {
                actions.push(Action::AskNewDesign(folder));
            }
            if tool(ui, Icon::Refresh, "Refresh", true).on_hover_text("Rescan the vault (F5)").clicked() {
                actions.push(Action::Refresh);
            }
            if self.backend.as_ref().is_some_and(Backend::is_remote) {
                let newer = self
                    .remote
                    .values()
                    .filter(|r| matches!(r.status, remote::RemoteStatus::NotDownloaded | remote::RemoteStatus::NewerOnServer))
                    .count();
                let label = format!("Get all new and newer ({newer})");
                if tool(ui, Icon::Download, &label, newer > 0)
                    .on_hover_text("Fetch what is new or newer on the server (never over your changes)")
                    .clicked()
                {
                    actions.push(Action::GetAll);
                }
            } else {
                let changes = self.count(Status::Modified) + self.count(Status::Untracked);
                if tool(ui, Icon::Upload, "Check in all…", changes > 0)
                    .on_hover_text("Record every modified (and optionally untracked) design as a new version")
                    .clicked()
                {
                    self.dialog = Some(Dialog::CheckInAll { comment: String::new(), untracked: true });
                }
            }
            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.scope(|ui| {
                    ui.spacing_mut().interact_size.y = 14.0;
                    ui.spacing_mut().slider_width = 100.0;
                    ui.add(egui::Slider::new(&mut self.settings.thumb_width, ROW_THUMB_WIDTHS).show_value(false)).on_hover_text("Thumbnail size");
                });
                icons::show(ui, Icon::Image, 15.0, t.text_dim);
                ui.add_space(10.0);
                self.search_bar(ui);
            });
        });
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        let t = theme::tokens(ui);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let small = |text: String| RichText::new(text).size(size::SMALL + 0.5).color(t.text_dim);
            if let Some(backend) = &self.backend {
                if let Some(ws) = backend.workspace() {
                    match &self.offline {
                        Some(why) => {
                            ui.label(RichText::new(format!("Offline: {why}")).size(size::SMALL + 0.5).color(t.warning));
                        }
                        None => {
                            ui.label(small(format!("{} on {} as {}", ws.vault(), ws.client().url(), ws.user())));
                        }
                    }
                    status_divider(ui, t);
                }
                ui.label(RichText::new(backend.root().display().to_string()).font(egui::FontId::monospace(size::SMALL)).color(t.text_dim));
                status_divider(ui, t);
                let (untracked, missing) = if backend.is_remote() { ("not on the server", "not downloaded") } else { ("untracked", "missing") };
                ui.label(small(format!(
                    "{} designs · {} modified · {} {untracked} · {} {missing}",
                    self.entries.len(),
                    self.count(Status::Modified),
                    self.count(Status::Untracked),
                    self.count(Status::Missing)
                )));
            }
            if let Some((text, is_error)) = &self.message {
                status_divider(ui, t);
                let color = if *is_error { t.error } else { t.text };
                ui.label(RichText::new(text).size(size::SMALL + 0.5).color(color));
            }
        });
    }

    fn left_panel(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            let t = theme::tokens(ui);
            let total = self.entries.len();
            let mut view = self.view.clone();
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                theme::section(ui, "Library");
            });
            ui.add_space(4.0);
            if theme::nav_row(ui, Some(Icon::Library), "All designs", Some(total), None, view == View::All).clicked() {
                view = View::All;
            }
            let remote = self.backend.as_ref().is_some_and(Backend::is_remote);
            for status in [Status::Modified, Status::Untracked, Status::Missing] {
                let n = self.count(status);
                if n > 0 || self.view == View::Status(status) {
                    let (label, color) = match (remote, status) {
                        // In a workspace these are designs not fetched yet, or new ones of yours.
                        (true, Status::Missing) => ("Not downloaded", t.accent),
                        (true, Status::Untracked) => ("Not on the server", t.accent),
                        _ => status_style(status),
                    };
                    let row = theme::nav_row(ui, Some(Icon::Filter), label, Some(n), Some(color), view == View::Status(status));
                    if row.clicked() {
                        view = View::Status(status);
                    }
                }
            }

            for state in [vault::State::Released, vault::State::Obsolete] {
                let n = self.entries.iter().filter(|e| e.state == Some(state)).count();
                if n > 0 || self.view == View::Lifecycle(state) {
                    let (label, icon) = if state == vault::State::Released { ("Released", Icon::Check) } else { ("Obsolete", Icon::Close) };
                    if theme::nav_row(ui, Some(icon), label, Some(n), None, view == View::Lifecycle(state)).clicked() {
                        view = View::Lifecycle(state);
                    }
                }
            }
            if !self.deleted.is_empty() || self.view == View::Deleted {
                if theme::nav_row(ui, Some(Icon::Delete), "Recently deleted", Some(self.deleted.len()), None, view == View::Deleted).clicked() {
                    view = View::Deleted;
                }
            }
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                theme::section(ui, "Folders");
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
                    let plus = egui::Button::image(icons::image(ui, Icon::Plus, t.text_dim).fit_to_exact_size(egui::vec2(13.0, 13.0))).frame(false);
                    let response = ui.add(plus);
                    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "+ New folder"));
                    if response.on_hover_text(hint).clicked() {
                        actions.push(Action::AskNewFolder(parent));
                    }
                });
            });
            ui.add_space(4.0);
            let top = self.entries.iter().filter(|e| !e.path.contains('/')).count();
            let response = theme::nav_row(ui, Some(Icon::Home), "Top level", Some(top), None, view == View::TopLevel);
            if response.clicked() {
                view = View::TopLevel;
            }
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
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.label(RichText::new("Drag designs onto a folder to move them.").size(size::SMALL).color(t.text_dim));
            });
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
        let selected = *view == View::Folder(folder.to_owned());
        let icon = if selected { Icon::FolderOpen } else { Icon::Folder };
        let response = theme::nav_row(ui, Some(icon), name, Some(n), None, selected);
        if response.clicked() {
            *view = View::Folder(folder.to_owned());
        }
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

    /// The search field (in a right-to-left row: it takes the room that is left).
    fn search_bar(&mut self, ui: &mut egui::Ui) {
        let t = theme::tokens(ui);
        let width = ui.available_width().clamp(200.0, 560.0);
        let focused = ui.memory(|m| m.focused()) == Some(egui::Id::new("search"));
        let frame = egui::Frame::new()
            .fill(t.field)
            .stroke(egui::Stroke::new(1.0, if focused { t.primary } else { t.control_border }))
            .corner_radius(egui::CornerRadius::same(theme::RADIUS))
            .inner_margin(egui::Margin::symmetric(8, 3));
        let shown = frame.show(ui, |ui| {
            ui.set_width(width - 18.0);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                icons::show(ui, Icon::Search, 15.0, t.text_dim);
                let keys = if self.search.is_empty() { 52.0 } else { 26.0 };
                let edit = egui::TextEdit::singleline(&mut self.search)
                    .id(egui::Id::new("search"))
                    .frame(egui::Frame::NONE)
                    .hint_text(RichText::new("Search all designs: file, name, part number, description…").color(t.text_dim))
                    .desired_width(ui.available_width() - keys);
                let response = ui.add(edit);
                if std::mem::take(&mut self.focus_search) {
                    response.request_focus();
                }
                // The field gives up focus as Escape is pressed.
                if (response.has_focus() || response.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.search.clear();
                }
                if self.search.is_empty() {
                    keycap(ui, "Ctrl F");
                } else if ui.add(egui::Button::image(icons::image(ui, Icon::Close, t.text_dim).fit_to_exact_size(egui::vec2(12.0, 12.0))).frame(false)).on_hover_text("Clear the search (Esc)").clicked() {
                    self.search.clear();
                }
            });
        });
        if focused {
            // SolidCraft's focus halo.
            let halo = shown.response.rect.expand(2.5);
            ui.painter().rect_stroke(halo, egui::CornerRadius::same(theme::RADIUS + 2), egui::Stroke::new(2.5, t.accent_soft), egui::StrokeKind::Outside);
        }
    }

    /// The folder new designs go into from the list: the one shown ("" for the top level).
    fn current_folder(&self) -> String {
        match &self.view {
            View::Folder(f) => f.clone(),
            _ => String::new(),
        }
    }

    fn list(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        if self.view == View::Deleted {
            self.deleted_list(ui, actions);
            return;
        }
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
            let text = if searching {
                "Nothing matches".to_owned()
            } else if self.entries.is_empty() && self.scan.is_some() {
                "Scanning…".to_owned()
            } else {
                "No designs here. Right-click for a new design or folder, or drag designs onto a folder on the left.".to_owned()
            };
            // The whole area takes the right click (a text widget on it would take it instead).
            let rect = ui.available_rect_before_wrap();
            let response = ui.allocate_rect(rect, egui::Sense::click());
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &text));
            let t = theme::tokens(ui);
            let galley = ui.painter().layout(text, egui::FontId::proportional(size::BODY), t.text_dim, (rect.width() - 40.0).max(100.0));
            ui.painter().galley(rect.center() - galley.size() / 2.0, galley, t.text_dim);
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
        let remote = !self.remote.is_empty();
        let needed = thumb.x + 8.0 + 160.0 + columns.iter().map(|c| c.width(remote) + spacing).sum::<f32>() + spacing;
        if needed > ui.available_width() {
            columns.retain(|c| *c != ListColumn::Modified);
        }
        let date_in_name = !self.settings.hidden_columns.contains(&ListColumn::Modified) && !columns.contains(&ListColumn::Modified);
        let hidden = &self.settings.hidden_columns;
        let folders = &self.folders;
        let t = theme::tokens(ui);
        let clip = ui.clip_rect();
        // Column handles show only while hovered.
        ui.visuals_mut().widgets.noninteractive.bg_stroke = egui::Stroke::NONE;
        ui.visuals_mut().selection.stroke = egui::Stroke::new(1.0, t.selected_ink);
        let mut table = TableBuilder::new(ui)
            // Widths are kept per column position: another column set gets its own.
            .id_salt(("list", &columns, remote))
            .striped(false)
            .resizable(true)
            .sense(egui::Sense::click_and_drag())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(thumb.x + 8.0))
            .column(Column::remainder().at_least(160.0).clip(true));
        for c in &columns {
            table = table.column(Column::initial(c.width(remote)).at_least(c.width(remote).min(60.0)).clip(true));
        }
        table
            .header(text_height + 12.0, |mut h| {
                let titles = ["", "Design"].into_iter().chain(columns.iter().map(|c| c.title()));
                for title in titles {
                    let (rect, response) = h.col(|ui| {
                        ui.add(egui::Label::new(RichText::new(title).font(theme::semibold(size::SMALL)).color(t.text_dim)).selectable(false));
                    });
                    ui_rule_under(&response, rect, t, clip);
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
                    let (sub, description, cells): (Vec<String>, Option<&str>, Vec<Cell>) = match r.item {
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
                                    ListColumn::Status => match self.remote.get(&e.path) {
                                        Some(r) => {
                                            let (label, color) = remote_style(&r.status);
                                            Cell::Status(label, color)
                                        }
                                        None => {
                                            let (label, color) = status_style(e.status);
                                            Cell::Status(label.to_owned(), color)
                                        }
                                    },
                                    ListColumn::State => state_cell(e),
                                    ListColumn::Version => Cell::Text(latest.map_or("-".into(), |v| format!("v{}", v.number)).into()),
                                    ListColumn::PartNumber => Cell::Text(
                                        RichText::new(
                                            props
                                                .and_then(|m| m.part_number.clone())
                                                .or_else(|| latest.and_then(|v| v.part_number.clone()))
                                                .unwrap_or_else(|| "-".into()),
                                        )
                                        .font(egui::FontId::monospace(size::SMALL + 0.5)),
                                    ),
                                    ListColumn::Revision => Cell::Text(
                                        props
                                            .and_then(|m| m.revision.clone())
                                            .or_else(|| latest.and_then(|v| v.revision.clone()))
                                            .unwrap_or_else(|| "-".into())
                                            .into(),
                                    ),
                                    ListColumn::Modified => Cell::Text(RichText::new(e.modified.map_or("-".into(), system_time)).color(theme::now().text_dim)),
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
                                    ListColumn::Status => Cell::Text(RichText::new("Image").weak()),
                                    ListColumn::Modified => Cell::Text(RichText::new(image.file.modified.map_or("-".into(), system_time)).color(theme::now().text_dim)),
                                    _ => Cell::Text(RichText::new("-").weak()),
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
                            // Designs without their extension (the file name is in the details).
                            let name = if matches!(r.item, Item::Design(_)) { vault::design_stem(file) } else { file };
                            ui.add(egui::Label::new(highlighted(ui, name, &r.matches, theme::medium(size::HEADING - 0.5))).truncate().selectable(false));
                            let sub = sub.iter().filter(|s| !s.is_empty()).map(String::as_str).collect::<Vec<_>>().join("  ·  ");
                            ui.add(egui::Label::new(RichText::new(sub).weak().small()).truncate().selectable(false));
                            if let Some(d) = description {
                                ui.add(egui::Label::new(RichText::new(d).small()).truncate().selectable(false));
                            }
                        });
                    });
                    for cell in cells {
                        row.col(|ui| match cell {
                            Cell::Text(text) => {
                                ui.add(egui::Label::new(text).truncate().selectable(false));
                            }
                            Cell::Status(text, color) => {
                                theme::status(ui, &text, color, egui::FontId::proportional(size::BODY));
                            }
                            Cell::Pill(text, ink, fill) => {
                                theme::pill(ui, &text, ink, fill);
                            }
                        });
                    }

                    let response = row.response();
                    // A hairline between rows.
                    response.ctx.layer_painter(response.layer_id).with_clip_rect(clip).hline(response.rect.x_range(), response.rect.bottom() - 0.5, egui::Stroke::new(1.0, theme::now().rule));
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
                            if is_image {
                                image_server_buttons(ui, self.remote.get(path).map(|r| &r.status), path, actions);
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
                        if !is_image {
                            ui.separator();
                            if ui.button(RichText::new("Delete…").color(theme::now().error)).clicked() {
                                actions.push(Action::AskDelete(path.to_owned()));
                                ui.close();
                            }
                        }
                    });
                });
            });
        // Right-click below the list.
        let rest = ui.allocate_response(ui.available_size().max(egui::vec2(0.0, 0.0)), egui::Sense::click());
        rest.context_menu(|ui| new_here_menu(ui, &here, actions));
    }

    /// A local vault's deleted designs: restore them, or delete them for good.
    fn deleted_list(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let t = theme::tokens(ui);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("Recently deleted").font(theme::semibold(size::TITLE)));
            ui.label(
                RichText::new("Deleted designs are kept here with their versions for 30 days, then removed for good. Restored, a design is back where it was, or beside a design that took its name.")
                    .color(t.text_dim),
            );
            ui.add_space(10.0);
            if self.deleted.is_empty() {
                ui.label(RichText::new("Nothing deleted.").color(t.text_dim));
            }
            let now = now_secs();
            for d in self.deleted.clone() {
                theme::card(t).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&d.path).font(theme::medium(size::HEADING)));
                            let days = ((d.deleted_at + KEEP_DELETED.as_secs() as i64 - now) / 86400).max(0);
                            let versions = d.latest.as_ref().map_or(0, |v| v.number);
                            ui.label(
                                RichText::new(format!("Deleted {} · {versions} version{} · removed for good in {days} days", unix_time(d.deleted_at), if versions == 1 { "" } else { "s" }))
                                    .size(size::SMALL + 0.5)
                                    .color(t.text_dim),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(theme::secondary("Delete for good…")).clicked() {
                                actions.push(Action::AskPurge { id: d.id, path: d.path.clone() });
                            }
                            if ui.add(theme::primary("Restore", t)).clicked() {
                                actions.push(Action::RestoreDeleted(d.id));
                            }
                        });
                    });
                });
            }
        });
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
        // On a server: who has it checked out ("you" for this user), which the server's rules
        // depend on.
        let me = self.backend.as_ref().and_then(|b| b.workspace().map(|w| w.user().to_owned()));
        let lock = self.remote.get(&entry.path).and_then(|r| r.lock.as_ref());
        let held = lock.map(|l| if Some(&l.user) == me.as_ref() { "you".to_owned() } else { l.user.clone() });
        let by_other = held.as_deref().filter(|h| *h != "you").map(|h| format!("Checked out by {h}"));

        let t = theme::tokens(ui);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            let width = ui.available_width() - 4.0;
            let height = (width / THUMB_ASPECT).min(THUMB.y * 1.15);
            let drawn = thumbnail(ui, image, egui::vec2(width.min(height * THUMB_ASPECT * 1.25), height), egui::Sense::hover());
            let preview = clickable(ui, drawn, "preview");
            if entry.status != Status::Missing && preview.on_hover_text("Double-click to open in SolidCraft").double_clicked() {
                actions.push(Action::Open(entry.path.clone()));
            }
            ui.add_space(6.0);
            ui.add(egui::Label::new(RichText::new(&file).font(theme::semibold(size::ARTICLE + 1.0)).color(t.text)).wrap());
            ui.add(egui::Label::new(RichText::new(&entry.path).font(egui::FontId::monospace(size::SMALL + 0.5)).color(t.text_dim)).wrap());
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                let (label, color) = match self.remote.get(&entry.path) {
                    Some(r) => remote_style(&r.status),
                    None => {
                        let (l, c) = status_style(entry.status);
                        (l.to_owned(), c)
                    }
                };
                theme::pill(ui, &label, color, color.gamma_multiply(0.16));
                if let Some(v) = &latest {
                    theme::pill(ui, &format!("v{} of {}", v.number, self.history.len()), t.label, t.panel_header);
                }
            });
            if let Some((lifecycle, next)) = self.lifecycle.clone() {
                lifecycle_ui(ui, &entry, &lifecycle, &next, held.as_deref(), actions);
            }

            if let Some(from) = &entry.moved_from {
                ui.add_space(6.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.label(format!("This looks like {from}, moved here outside the PDM."));
                    if ui.button("Keep its history (relink)").clicked() {
                        actions.push(Action::Relink { missing: from.clone(), to: entry.path.clone() });
                    }
                });
            }

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                let present = entry.status != Status::Missing;
                let open = egui::Button::image_and_text(
                    icons::image(ui, Icon::OpenDesign, t.on_primary).fit_to_exact_size(egui::vec2(16.0, 16.0)),
                    RichText::new("Open in SolidCraft").font(theme::medium(size::BUTTON)).color(t.on_primary),
                )
                .fill(t.primary)
                .stroke(egui::Stroke::new(1.0, t.primary))
                .min_size(egui::vec2(0.0, theme::CONTROL_HEIGHT));
                if ui.add_enabled(present, open).clicked() {
                    actions.push(Action::Open(entry.path.clone()));
                }
                let mut rename = false;
                // The server writes the properties as a new version: not while anyone has it.
                let readable = info.is_some();
                let why = match held.as_deref() {
                    Some("you") => "Check it in first".to_owned(),
                    Some(user) => format!("Checked out by {user}"),
                    None => "Not a readable SolidCraft design".to_owned(),
                };
                if ui
                    .add_enabled(present && readable && held.is_none(), theme::secondary("Edit properties…"))
                    .on_disabled_hover_text(why)
                    .clicked()
                {
                    actions.push(Action::AskEditProperties(entry.path.clone()));
                }
                // The rarer ones in a menu.
                let more = egui::Button::new(RichText::new("···").font(theme::bold(size::SUBTITLE))).min_size(egui::vec2(32.0, theme::CONTROL_HEIGHT));
                egui::containers::menu::MenuButton::from_button(more).ui(ui, |ui| {
                    ui.style_mut().visuals.button_frame = false;
                    ui.spacing_mut().button_padding = egui::vec2(10.0, 6.0);
                    ui.set_min_width(180.0);
                    if ui.add_enabled(present, egui::Button::new("Show in folder")).clicked() {
                        actions.push(Action::ShowInFolder(entry.path.clone()));
                    }
                    let movable = ui.add_enabled(present && by_other.is_none(), egui::Button::new("Move / rename…"));
                    if movable.on_disabled_hover_text(by_other.as_deref().unwrap_or_default()).clicked() {
                        rename = true;
                    }
                    ui.separator();
                    let delete = ui.add_enabled(by_other.is_none(), egui::Button::new(RichText::new("Delete…").color(t.error)));
                    if delete.on_disabled_hover_text(by_other.as_deref().unwrap_or_default()).clicked() {
                        actions.push(Action::AskDelete(entry.path.clone()));
                    }
                })
                .0
                .on_hover_text("Show in folder, move or rename");
                if rename {
                    self.dialog = Some(Dialog::Rename { from: entry.path.clone(), to: entry.path.clone() });
                }
            });

            if let Some(r) = self.remote.get(&entry.path).cloned() {
                self.remote_panel(ui, &entry, &r, actions);
            } else if matches!(entry.status, Status::Modified | Status::Untracked) {
                ui.add_space(10.0);
                theme::card(t).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let title = if entry.status == Status::Untracked { "Add to vault" } else { "Check in" };
                    ui.label(RichText::new(title).font(theme::semibold(size::HEADING)));
                    ui.add(
                        egui::TextEdit::multiline(&mut self.comment)
                            .hint_text("What changed?")
                            .desired_rows(2)
                            .margin(egui::vec2(8.0, 6.0))
                            .desired_width(f32::INFINITY),
                    );
                    let label = if entry.status == Status::Untracked { "Add as v1" } else { "Check in as new version" };
                    let next = latest.as_ref().map_or(1, |v| v.number + 1);
                    if ui.add(theme::primary(format!("⬆ {label}"), t)).on_hover_text(format!("Records the file's current content as v{next}")).clicked() {
                        actions.push(Action::CheckIn { path: entry.path.clone(), comment: self.comment.clone() });
                    }
                });
            }

            ui.add_space(12.0);
            theme::rule(ui);
            egui::CollapsingHeader::new(RichText::new("Properties").font(theme::semibold(size::HEADING))).default_open(true).show(ui, |ui| {
                properties(ui, &entry, info.as_ref(), latest.as_ref());
            });
            if let Some((lifecycle, _)) = &self.lifecycle {
                if !lifecycle.releases.is_empty() {
                    let exporting = self.exporting.as_ref().is_some_and(|(p, _)| *p == entry.path)
                        || self.export_queue.iter().any(|r| r.path == entry.path);
                    release_files_ui(ui, &entry.path, &lifecycle.releases, exporting, &self.settings.file_apps, actions);
                }
            }

            if let Some(info) = &info {
                if !info.links.is_empty() {
                    egui::CollapsingHeader::new(RichText::new(format!("Uses ({})", info.links.len())).font(theme::semibold(size::HEADING)))
                        .default_open(true)
                        .show(ui, |ui| {
                            for link in &info.links {
                                let target = resolve_link(&entry.path, &link.path);
                                let component =
                                    info.components.iter().find(|c| c.id == link.component).map_or("?", |c| c.name.as_str());
                                self.link_row(ui, component, &link.path, target, actions);
                            }
                        });
                    self.bom_ui(ui, &entry.path, actions);
                }
            }
            let used_in = self.used_in(&entry.path);
            if !used_in.is_empty() {
                egui::CollapsingHeader::new(RichText::new(format!("Used in ({})", used_in.len())).font(theme::semibold(size::HEADING)))
                    .default_open(true)
                    .show(ui, |ui| {
                        for path in used_in {
                            if ui.link(&path).clicked() {
                                actions.push(Action::Select(path));
                            }
                        }
                    });
            }

            egui::CollapsingHeader::new(RichText::new(format!("History ({})", self.history.len())).font(theme::semibold(size::HEADING)))
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
            let drawn = thumbnail(ui, picture, egui::vec2(width, height), egui::Sense::hover());
            let preview = clickable(ui, drawn, "image-preview");
            if preview.on_hover_text("Double-click to open").double_clicked() {
                actions.push(Action::OpenFile(path.clone()));
            }
            let t = theme::tokens(ui);
            ui.add_space(6.0);
            ui.add(egui::Label::new(RichText::new(&file).font(theme::semibold(size::ARTICLE + 1.0))).wrap());
            ui.add(egui::Label::new(RichText::new(&path).font(egui::FontId::monospace(size::SMALL + 0.5)).color(t.text_dim)).wrap());
            ui.add_space(2.0);
            ui.label(RichText::new("Reference image (shown, not versioned)").color(t.text_dim));
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                image_server_buttons(ui, self.remote.get(&path).map(|r| &r.status), &path, actions);
                if ui.add(theme::primary("Open", t)).clicked() {
                    actions.push(Action::OpenFile(path.clone()));
                }
                if ui.add(theme::secondary("Show in folder")).clicked() {
                    actions.push(Action::ShowInFolder(path.clone()));
                }
                if ui.add(theme::secondary("Move / rename…")).clicked() {
                    self.dialog = Some(Dialog::Rename { from: path.clone(), to: path.clone() });
                }
            });
            ui.add_space(10.0);
            ui.add_space(4.0);
            theme::rule(ui);
            ui.add_space(4.0);
            egui::Grid::new("image-properties").num_columns(2).spacing([16.0, 6.0]).min_col_width(96.0).show(ui, |ui| {
                let mut row = |label: &str, value: Option<String>| {
                    if let Some(v) = value {
                        theme::field_row(ui, label, &v);
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

    /// An assembly's bill of materials: every design it uses, with the quantity in all.
    fn bom_ui(&self, ui: &mut egui::Ui, path: &str, actions: &mut Vec<Action>) {
        let rows = self.bom_rows(path);
        let pieces: u32 = rows.iter().filter(|(l, _)| !l.assembly).map(|(l, _)| l.quantity).sum();
        let t = theme::tokens(ui);
        egui::CollapsingHeader::new(RichText::new("Bill of materials").font(theme::semibold(size::HEADING)))
            .id_salt("bom")
            .default_open(false)
            .show(ui, |ui| {
                ui.label(RichText::new(format!("{} designs, {pieces} parts in all (through sub-assemblies)", rows.len())).color(t.text_dim));
                egui::Grid::new("bom-table").num_columns(4).spacing([12.0, 4.0]).striped(true).show(ui, |ui| {
                    for title in ["Qty", "Part number", "Name", "Rev"] {
                        ui.label(RichText::new(title).small().color(t.label));
                    }
                    ui.end_row();
                    for (line, [part_number, name, _, revision, state]) in &rows {
                        ui.label(RichText::new(line.quantity.to_string()).strong());
                        ui.label(part_number);
                        let shown = if line.assembly { format!("{name} (assembly)") } else { name.clone() };
                        let text = if line.missing { RichText::new(format!("{name} (missing)")).color(t.error) } else { RichText::new(shown) };
                        let hint = format!("{}\n{state}{}", line.path, if line.assembly { " · an assembly: its parts are counted too" } else { "" });
                        if ui.link(text).on_hover_text(hint).clicked() && !line.missing {
                            actions.push(Action::Select(line.path.clone()));
                        }
                        ui.label(revision);
                        ui.end_row();
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Save as CSV…").on_hover_text("For a spreadsheet: quantity, part number, name, description, revision, state, file").clicked() {
                        actions.push(Action::SaveBom(path.to_owned()));
                    }
                    if ui.button("Pack and go…").on_hover_text("A zip of this assembly and every design it uses, in their folders, so the links work where it is unpacked").clicked() {
                        actions.push(Action::PackAndGo(path.to_owned()));
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
        same: Sameness,
        image: Option<egui::Image<'static>>,
        actions: &mut Vec<Action>,
    ) {
        let is_working = same == Sameness::Working;
        let t = theme::tokens(ui);
        let mut frame = theme::card(t).inner_margin(egui::Margin::same(8));
        if is_working {
            frame = frame.stroke(egui::Stroke::new(1.5, t.accent));
        }
        let mut buttons_top = f32::INFINITY;
        let shown = frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                thumbnail(ui, image, HISTORY_THUMB, egui::Sense::hover());
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("v{}", v.number)).font(theme::semibold(size::HEADING)));
                        ui.label(RichText::new(format!("{} · {}", unix_time(v.created_at), v.author)).color(t.text_dim));
                        match same {
                            Sameness::Working => {
                                theme::pill(ui, "working file", t.accent, t.accent_soft);
                            }
                            Sameness::SameContent => {
                                ui.label(RichText::new("same as working file").small().weak());
                            }
                            Sameness::Different => {}
                        }
                    });
                    let release = self.lifecycle.as_ref().and_then(|(l, _)| l.releases.iter().find(|r| r.version == v.number));
                    if let Some(r) = release {
                        ui.label(RichText::new(format!("Revision {}", r.revision)).color(added()).strong());
                    }
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
                        if ui.add(egui::Button::new("Compare…").small()).on_hover_text("What changed").clicked() {
                            actions.push(Action::AskCompare { path: entry.path.clone(), number: v.number, is_working });
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

    /// The start page (no vault open): open, make or connect to one, or a recent one.
    fn welcome(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let t = theme::tokens(ui);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let width = ui.available_width().min(940.0);
            let side = ((ui.available_width() - width) / 2.0).max(24.0);
            ui.horizontal(|ui| {
                ui.add_space(side);
                ui.vertical(|ui| {
                    ui.set_width(width - 48.0);
                    ui.add_space(40.0);
                    ui.label(RichText::new("Start").font(theme::semibold(size::DISPLAY + 4.0)).color(t.text));
                    ui.label(
                        RichText::new("A vault is a folder of SolidCraft designs whose versions are kept in a hidden .pdm folder. Your files stay where they are and open in SolidCraft as before.")
                            .size(size::SUBTITLE - 1.0)
                            .color(t.text_dim),
                    );
                    ui.add_space(24.0);
                    ui.label(RichText::new("Open").font(theme::semibold(size::SUBTITLE + 2.0)));
                    ui.add_space(4.0);
                    ui.columns(3, |cols| {
                        let choices = [
                            (Icon::FolderOpen, "Open vault…", "A folder of designs on this computer", Action::PickVault),
                            (Icon::NewDesign, "New vault…", "Make a folder a vault", Action::NewVault),
                            (Icon::Server, "Connect to a server…", "A vault your team shares", Action::AskConnect),
                        ];
                        for (col, (icon, title, text, action)) in cols.iter_mut().zip(choices) {
                            if start_card(col, icon, title, text).clicked() {
                                actions.push(action);
                            }
                        }
                    });
                    if !self.settings.recent.is_empty() {
                        ui.add_space(28.0);
                        ui.label(RichText::new("Recent").font(theme::semibold(size::SUBTITLE + 2.0)));
                        ui.add_space(4.0);
                        theme::card(t).inner_margin(egui::Margin::same(4)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.spacing_mut().item_spacing.y = 0.0;
                            for (i, p) in self.settings.recent.iter().enumerate() {
                                if i > 0 {
                                    theme::rule(ui);
                                }
                                if recent_row(ui, p).clicked() {
                                    actions.push(Action::OpenVault(p.clone()));
                                }
                            }
                        });
                    }
                    ui.add_space(32.0);
                });
            });
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        // A comparison's pictures are looked up first (that may load them from the store).
        let compare_images = match &self.dialog {
            Some(Dialog::Compare { path, from, to, look, .. }) => {
                let (path, from, to, view) = (path.clone(), *from, *to, look.view);
                Some((self.side_picture(&path, from, view), self.side_picture(&path, to, view)))
            }
            _ => None,
        };
        let Some(dialog) = &mut self.dialog else { return };
        let mut close = false;
        let t = theme::tokens_of(ctx);
        let frame = egui::Frame::new()
            .fill(t.card)
            .stroke(egui::Stroke::new(1.0, t.card_border))
            .corner_radius(egui::CornerRadius::same(theme::MENU_RADIUS))
            .inner_margin(egui::Margin::same(22))
            .shadow(ctx.global_style().visuals.window_shadow);
        let modal = egui::Modal::new(egui::Id::new("dialog")).frame(frame).show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 7.0;
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
                        if ui.add(theme::primary(format!("Restore v{number}"), t)).clicked() {
                            actions.push(Action::Restore { path: path.clone(), number: *number });
                            close = true;
                        }
                        close |= ui.add(theme::secondary("Cancel")).clicked();
                    });
                }
                Dialog::Rename { from, to } => {
                    ui.heading("Move / rename");
                    ui.label("New path in the vault (folders are created as needed):");
                    let edit = ui.add(egui::TextEdit::singleline(to).desired_width(f32::INFINITY));
                    ui.label(
                        RichText::new("Links to it from other designs are updated, and so are its own. If one of them is open in SolidCraft, reopen it there afterwards.")
                            .small()
                            .color(ui.visuals().warn_fg_color),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let ok = to.trim() != from.as_str() && !to.trim().is_empty();
                        if ui.add_enabled(ok, theme::primary("Move", t)).clicked()
                            || (ok && edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        {
                            actions.push(Action::Rename { from: from.clone(), to: to.clone() });
                            close = true;
                        }
                        close |= ui.add(theme::secondary("Cancel")).clicked();
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
                        if ui.add_enabled(n > 0, theme::primary(format!("Check in {n}"), t)).clicked() {
                            actions.push(Action::CheckInAll { comment: comment.clone(), untracked: *untracked });
                            close = true;
                        }
                        close |= ui.add(theme::secondary("Cancel")).clicked();
                    });
                }
                Dialog::MakeVault { dir } => {
                    ui.heading("Not a vault yet");
                    ui.label(format!("{} is not a vault.", dir.display()));
                    ui.label("Creating one adds a hidden .pdm folder there; your files are not changed.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.add(theme::primary("Create vault here", t)).clicked() {
                            actions.push(Action::MakeVault(dir.clone()));
                            close = true;
                        }
                        close |= ui.add(theme::secondary("Cancel")).clicked();
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
                        if ui.add_enabled(ok, theme::primary("Create", t)).clicked()
                            || (ok && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        {
                            actions.push(Action::CreateFolder(target.clone()));
                            close = true;
                        }
                        close |= ui.add(theme::secondary("Cancel")).clicked();
                    });
                }
                Dialog::Release { path, revision, note, unreleased } => {
                    ui.heading("Release");
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label("Revision");
                        ui.add(egui::TextEdit::singleline(revision).desired_width(80.0));
                    });
                    ui.label(format!("{path} gets this revision: it is written into the design's Revision property and checked in as a new version, which is then the released revision."));
                    if !unreleased.is_empty() {
                        ui.add_space(4.0);
                        egui::Frame::new().fill(t.warning_bg).corner_radius(4.0).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            let n = unreleased.len();
                            let what = if n == 1 { "1 design it uses is".to_owned() } else { format!("{n} designs it uses are") };
                            ui.label(RichText::new(format!("{what} not released:")).color(t.warning).strong());
                            for (part, why) in unreleased.iter().take(8) {
                                ui.label(RichText::new(format!("{part} ({why})")).font(egui::FontId::monospace(size::SMALL + 0.5)));
                            }
                            if n > 8 {
                                ui.label(format!("and {} more", n - 8));
                            }
                            ui.label(RichText::new("The release links to their working files as they are now. Release them first to have a fixed state.").small());
                        });
                    }
                    ui.add(egui::TextEdit::singleline(note).hint_text("Note (optional), e.g. what this release is for").desired_width(f32::INFINITY));
                    ui.label(
                        RichText::new("If the design is open in SolidCraft, reopen it there afterwards to see the new revision.")
                            .small()
                            .weak(),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let ok = !revision.trim().is_empty();
                        let label = if unreleased.is_empty() { format!("Release {}", revision.trim()) } else { format!("Release {} anyway", revision.trim()) };
                        if ui.add_enabled(ok, theme::primary(label, t)).clicked() {
                            actions.push(Action::Release { path: path.clone(), revision: revision.clone(), note: note.clone() });
                            close = true;
                        }
                        close |= ui.add(theme::secondary("Cancel")).clicked();
                    });
                }
                Dialog::Properties { path, name, values, check_in, comment } => {
                    ui.heading("Design properties");
                    ui.weak(path.as_str());
                    ui.add_space(4.0);
                    egui::Grid::new("properties-edit").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                        ui.label("Name");
                        ui.add(egui::TextEdit::singleline(name).desired_width(320.0));
                        ui.end_row();
                        for ((key, label), value) in EDITABLE.iter().zip(values.iter_mut()) {
                            ui.label(*label);
                            let multiline = *key == "description";
                            let edit = if multiline { egui::TextEdit::multiline(value).desired_rows(2) } else { egui::TextEdit::singleline(value) };
                            let response = ui.add(edit.desired_width(320.0));
                            if *key == "revision" {
                                response.on_hover_text("Releasing a design sets this itself");
                            }
                            ui.end_row();
                        }
                    });
                    ui.label(RichText::new("Written into the design file; SolidCraft shows them under File > Design Properties.").small().weak());
                    ui.checkbox(check_in, "Check in as a new version");
                    if *check_in {
                        ui.add(egui::TextEdit::singleline(comment).desired_width(f32::INFINITY));
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.add(theme::primary("Save", t)).clicked() {
                            actions.push(Action::SaveProperties {
                                path: path.clone(),
                                name: name.clone(),
                                values: values.clone(),
                                check_in: *check_in,
                                comment: comment.clone(),
                            });
                            close = true;
                        }
                        close |= ui.add(theme::secondary("Cancel")).clicked();
                    });
                }
                Dialog::Login { password, error } => {
                    ui.heading("Log in again");
                    let who = self.backend.as_ref().and_then(|b| b.workspace().map(|w| format!("{} on {}", w.user(), w.client().url()))).unwrap_or_default();
                    ui.label(format!("The login of {who} has expired."));
                    let edit = ui.add(egui::TextEdit::singleline(password).password(true).hint_text("Password").desired_width(f32::INFINITY));
                    if ui.memory(|m| m.focused().is_none()) {
                        edit.request_focus();
                    }
                    if let Some(e) = error {
                        ui.colored_label(ui.visuals().error_fg_color, e.as_str());
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.add(theme::primary("Log in", t)).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            actions.push(Action::Relogin(std::mem::take(password)));
                        }
                        close |= ui.add(theme::secondary("Cancel")).clicked();
                    });
                }
                Dialog::Connect(form) => {
                    ui.set_min_width(420.0);
                    ui.heading("Connect to a server");
                    ui.add_space(4.0);
                    egui::Grid::new("connect").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                        let logged_in = form.client.is_some();
                        ui.label("Server");
                        ui.add_enabled(!logged_in, egui::TextEdit::singleline(&mut form.url).hint_text("http://server:7380").desired_width(280.0));
                        ui.end_row();
                        ui.label("User");
                        ui.add_enabled(!logged_in, egui::TextEdit::singleline(&mut form.user).desired_width(280.0));
                        ui.end_row();
                        if !logged_in {
                            ui.label("Password");
                            ui.add(egui::TextEdit::singleline(&mut form.password).password(true).desired_width(280.0));
                            ui.end_row();
                        }
                    });
                    let plain = form.url.trim().starts_with("http://") && !form.url.contains("localhost") && !form.url.contains("127.0.0.1");
                    if plain && form.client.is_none() {
                        ui.label(RichText::new("Over http:// the password travels unencrypted: fine on your own network or a VPN.").small().color(ui.visuals().warn_fg_color));
                    }
                    if form.client.is_none() {
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let ok = !form.url.trim().is_empty() && !form.user.trim().is_empty() && !form.password.is_empty();
                            if ui.add_enabled(ok, theme::primary("Connect", t)).clicked() || (ok && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                                actions.push(Action::ConnectServer);
                            }
                            close |= ui.add(theme::secondary("Cancel")).clicked();
                        });
                    } else {
                        ui.add_space(8.0);
                        ui.label(RichText::new("Vault").font(theme::semibold(size::HEADING)));
                        let mut changed = false;
                        for v in &form.vaults {
                            let label = format!("{}  ({})", v.name, crate::app::access_label(v.access));
                            if ui.radio(form.vault == v.name, label).clicked() && form.vault != v.name {
                                form.vault = v.name.clone();
                                changed = true;
                            }
                        }
                        if changed {
                            form.folder = default_workspace(&form.vault);
                        }
                        ui.add_space(6.0);
                        ui.label("Workspace folder (your copy: SolidCraft opens and saves the designs here)");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut form.folder).desired_width(ui.available_width() - 90.0));
                            if ui.button("Browse…").clicked() {
                                if let Some(dir) = rfd::FileDialog::new().set_title("Workspace folder").pick_folder() {
                                    form.folder = dir.join(&form.vault).display().to_string();
                                }
                            }
                        });
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let ok = !form.vault.is_empty() && !form.folder.trim().is_empty();
                            if ui.add_enabled(ok, theme::primary("Open the vault", t)).clicked() {
                                actions.push(Action::JoinVault);
                            }
                            close |= ui.add(theme::secondary("Cancel")).clicked();
                        });
                    }
                    if let Some(e) = &form.error {
                        ui.colored_label(ui.visuals().error_fg_color, e.as_str());
                    }
                }
                Dialog::Update { error } => match &self.update {
                    None => close = true,
                    Some(release) => {
                        ui.set_min_width(520.0);
                        ui.heading(format!("SolidCraft PDM {}", update::version_text(release.version)));
                        ui.weak(format!("You have {}.", env!("CARGO_PKG_VERSION")));
                        ui.add_space(6.0);
                        egui::ScrollArea::vertical().max_height(ctx.content_rect().height() * 0.5).show(ui, |ui| release_notes(ui, &release.notes));
                        ui.add_space(8.0);
                        let can_install = release.asset.is_some() && release.checksums.is_some() && self.install_kind != update::Kind::Other;
                        if let Some((_, progress)) = &self.installing {
                            let bar = egui::ProgressBar::new(progress.fraction().unwrap_or(0.0)).show_percentage();
                            ui.add(bar.text("Downloading…"));
                        } else {
                            if let Some(e) = error {
                                ui.colored_label(ui.visuals().error_fg_color, e.as_str());
                            }
                            if !can_install {
                                let why = match self.install_kind {
                                    update::Kind::Other => "This copy was not installed from a release: download the new version from the release page.",
                                    _ => "This release has nothing to install here automatically: download it from the release page.",
                                };
                                ui.label(RichText::new(why).small().weak());
                            }
                            ui.horizontal(|ui| {
                                let restart = if self.install_kind == update::Kind::Installer { "Install (the app restarts)" } else { "Install and restart" };
                                if can_install && ui.add(theme::primary(restart, t)).clicked() {
                                    actions.push(Action::InstallUpdate);
                                }
                                if ui.add(theme::secondary("Release page")).clicked() {
                                    ui.ctx().open_url(egui::OpenUrl::new_tab(&release.page));
                                }
                                if ui.add(theme::secondary("Skip this version")).clicked() {
                                    actions.push(Action::SkipUpdate);
                                    close = true;
                                }
                                close |= ui.add(theme::secondary("Later")).clicked();
                            });
                        }
                    }
                },
                Dialog::Delete { path, used_in, remote } => {
                    ui.set_max_width(480.0);
                    ui.heading(format!("Delete {}?", path.rsplit_once('/').map_or(path.as_str(), |(_, f)| f)));
                    ui.label(if *remote {
                        "It is gone for everyone, also from the others' workspaces (their unchanged copies). The server keeps it with its versions for a while: an administrator can restore it on the server's page."
                    } else {
                        "It goes into Recently deleted with its versions, for 30 days. Changes not checked in yet are checked in first, so nothing is lost."
                    });
                    if !used_in.is_empty() {
                        ui.add_space(4.0);
                        ui.colored_label(t.warning, format!("Used by {}: their links to it break.", used_in.join(", ")));
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let delete = egui::Button::new(RichText::new("Delete").font(theme::medium(size::BUTTON)).color(Color32::WHITE))
                            .fill(t.error)
                            .min_size(egui::vec2(0.0, theme::CONTROL_HEIGHT));
                        if ui.add(delete).clicked() {
                            actions.push(Action::Delete(path.clone()));
                            close = true;
                        }
                        close |= ui.add(theme::secondary("Cancel")).clicked();
                    });
                }
                Dialog::Purge { id, path } => {
                    ui.heading(format!("Delete {} for good?", path.rsplit_once('/').map_or(path.as_str(), |(_, f)| f)));
                    ui.label("With all its versions. This cannot be undone.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let purge = egui::Button::new(RichText::new("Delete for good").font(theme::medium(size::BUTTON)).color(Color32::WHITE))
                            .fill(t.error)
                            .min_size(egui::vec2(0.0, theme::CONTROL_HEIGHT));
                        if ui.add(purge).clicked() {
                            actions.push(Action::Purge(*id));
                            close = true;
                        }
                        close |= ui.add(theme::secondary("Cancel")).clicked();
                    });
                }
                Dialog::Compare { path, from, to, result, look } => {
                    // Wide, for pictures large enough to see what changed.
                    let width = (ctx.content_rect().width() * 0.8).clamp(680.0, 1180.0);
                    ui.set_min_width(width);
                    ui.set_max_width(width);
                    ui.horizontal(|ui| {
                        ui.heading(format!("Compare {}", path.rsplit_once('/').map_or(path.as_str(), |(_, f)| f)));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // Other views need SolidCraft to draw them.
                            let drawn = self.renderer.as_ref().is_some_and(|r| r.found() != crate::render::Found::Missing);
                            ui.add_enabled_ui(drawn, |ui| {
                                egui::ComboBox::from_id_salt("compare-view").selected_text(look.view.label()).width(110.0).show_ui(ui, |ui| {
                                    for view in crate::render::View::ALL {
                                        ui.selectable_value(&mut look.view, view, view.label());
                                    }
                                });
                            })
                            .response
                            .on_disabled_hover_text("SolidCraft draws the other views: see Settings, Thumbnails");
                            ui.label("View");
                            ui.add_space(12.0);
                            ui.selectable_value(&mut look.overlay, true, "Overlay");
                            ui.selectable_value(&mut look.overlay, false, "Side by side");
                        });
                    });
                    ui.add_space(4.0);
                    let (from_picture, to_picture) = compare_images.clone().unwrap_or((SidePicture { image: None, drawing: false }, SidePicture { image: None, drawing: false }));
                    let before = (*from, *to);
                    if look.overlay {
                        let size = egui::vec2(width, (width / THUMB_ASPECT).min(ctx.content_rect().height() * 0.4));
                        overlay_pictures(ui, &from_picture, &to_picture, look.blend, size);
                        ui.horizontal(|ui| {
                            ui.label("From");
                            side_picker(ui, "From", from, &self.history);
                            ui.spacing_mut().slider_width = (width - 520.0).max(120.0);
                            ui.add(egui::Slider::new(&mut look.blend, 0.0..=1.0).show_value(false)).on_hover_text("How much of each side shows");
                            ui.label("To");
                            side_picker(ui, "To", to, &self.history);
                        });
                    } else {
                        let each = (width - 12.0) / 2.0;
                        let size = egui::vec2(each, (each / THUMB_ASPECT).min(ctx.content_rect().height() * 0.4));
                        ui.horizontal(|ui| {
                            for (label, side, picture) in [("From", &mut *from, from_picture), ("To", &mut *to, to_picture)] {
                                ui.vertical(|ui| {
                                    side_picture_ui(ui, &picture, size);
                                    ui.horizontal(|ui| {
                                        ui.label(label);
                                        side_picker(ui, label, side, &self.history);
                                    });
                                });
                                ui.add_space(12.0);
                            }
                        });
                    }
                    if (*from, *to) != before {
                        *result = None;
                    }
                    if result.is_none() {
                        *result = Some(self.backend.as_ref().map_or(Err("no vault".into()), |v| {
                            compare_sides(v, path, &self.history, *from, *to)
                        }));
                    }
                    ui.separator();
                    egui::ScrollArea::vertical().max_height(ctx.content_rect().height() * 0.28).show(ui, |ui| match result {
                        Some(Ok(c)) => comparison_ui(ui, c),
                        Some(Err(e)) => {
                            ui.colored_label(ui.visuals().error_fg_color, e.as_str());
                        }
                        None => {}
                    });
                    ui.add_space(8.0);
                    close |= ui.add(theme::secondary("Close")).clicked();
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
                    // `.sc` unless another design extension is typed.
                    let file = vault::new_design_name(&typed_path(name));
                    let stem = vault::design_stem(&file).to_owned();
                    let path = if folder.is_empty() { file.clone() } else { format!("{folder}/{file}") };
                    let valid = !stem.is_empty() && !stem.contains('/') && !stem.starts_with('.');
                    let exists = valid && self.backend.as_ref().is_some_and(|v| v.absolute(&path).exists());
                    if exists {
                        ui.colored_label(ui.visuals().warn_fg_color, format!("{file} exists already in this folder."));
                    } else if valid {
                        ui.label(RichText::new(format!("Creates {path} as v1")).small().weak());
                    }
                    ui.checkbox(open, "Open it in SolidCraft");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let ok = valid && !exists;
                        if ui.add_enabled(ok, theme::primary("Create", t)).clicked()
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
                        close |= ui.add(theme::secondary("Cancel")).clicked();
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
    /// Everything that is not drawing: scans, rescans, settings, shortcuts.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_scan(ctx);
        self.poll_updates(ctx);
        self.poll_exports(ctx);
        if let Some(renderer) = self.renderer.as_mut() {
            for (hash, png) in renderer.finished() {
                self.rendered.insert(hash, png);
            }
        }
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if self.backend.is_some() && focused && (!self.focused || self.last_scan.elapsed() >= RESCAN_EVERY) {
            self.start_scan(ctx);
        }
        self.focused = focused;
        if focused {
            // Unfocused, nothing is scheduled: regaining focus rescans anyway.
            ctx.request_repaint_after(RESCAN_EVERY);
        } else if self.settings.auto_checkin && self.backend.is_some() {
            // In the background too: saves in SolidCraft become versions without coming back.
            if self.last_scan.elapsed() >= AUTO_SCAN_EVERY {
                self.start_scan(ctx);
            }
            ctx.request_repaint_after(AUTO_SCAN_EVERY);
        }
        self.settings.zoom = ctx.zoom_factor();
        if ctx.input(|i| i.key_pressed(egui::Key::F5)) {
            self.start_scan(ctx);
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            self.focus_search = true;
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        theme::begin_frame(&ctx);
        let mut actions = Vec::new();
        let t = theme::tokens(ui);
        let bar = egui::Frame::new().fill(t.app_bar).inner_margin(egui::Margin::symmetric(12, 6));
        egui::Panel::top("app-bar").frame(bar).show(ui, |ui| self.top_bar(ui, &mut actions));
        let footer = egui::Frame::new().fill(t.footer).inner_margin(egui::Margin::symmetric(12, 5));
        egui::Panel::bottom("status").frame(footer).show(ui, |ui| self.status_bar(ui));
        if self.settings_page.is_some() {
            if self.dialog.is_none() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                self.close_settings(&mut actions);
            }
            let body = egui::Frame::new().fill(t.panel).inner_margin(egui::Margin { left: 24, right: 24, top: 18, bottom: 0 });
            egui::CentralPanel::default().frame(body).show(ui, |ui| self.settings_ui(ui, &mut actions));
        } else if self.backend.is_some() {
            let strip = egui::Frame::new().fill(t.toolbar).inner_margin(egui::Margin::symmetric(12, 8));
            egui::Panel::top("toolbar").frame(strip).show(ui, |ui| self.toolbar(ui, &mut actions));
            let nav = egui::Frame::new().fill(t.footer).inner_margin(egui::Margin { left: 8, right: 8, top: 10, bottom: 8 });
            egui::Panel::left("folders")
                .frame(nav)
                .default_size(232.0)
                .min_size(170.0)
                .show(ui, |ui| self.left_panel(ui, &mut actions));
            let side = egui::Frame::new().fill(t.panel).inner_margin(egui::Margin { left: 16, right: 12, top: 14, bottom: 8 });
            egui::Panel::right("details")
                .frame(side)
                .default_size(380.0)
                .min_size(300.0)
                .show(ui, |ui| self.details(ui, &mut actions));
            let body = egui::Frame::new().fill(t.panel).inner_margin(egui::Margin { left: 10, right: 10, top: 6, bottom: 0 });
            egui::CentralPanel::default().frame(body).show(ui, |ui| self.list(ui, &mut actions));
        } else {
            let body = egui::Frame::new().fill(t.panel).inner_margin(egui::Margin::same(0));
            egui::CentralPanel::default().frame(body).show(ui, |ui| self.welcome(ui, &mut actions));
        }
        self.dialogs(&ctx, &mut actions);
        drag_label(&ctx);
        for action in actions {
            self.apply(&ctx, action);
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.settings);
    }
}

fn properties(ui: &mut egui::Ui, entry: &Entry, info: Option<&scformat::DesignInfo>, latest: Option<&Version>) {
    egui::Grid::new("properties").num_columns(2).spacing([16.0, 6.0]).min_col_width(96.0).show(ui, |ui| {
        let mut row = |label: &str, value: Option<String>| {
            if let Some(v) = value.filter(|v| !v.is_empty()) {
                theme::field_row(ui, label, &v);
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
        Some(image) => {
            let response = ui.add(image.fit_to_exact_size(size).corner_radius(theme::RADIUS as f32).sense(sense));
            let t = theme::tokens(ui);
            ui.painter().rect_stroke(response.rect, theme::RADIUS as f32, egui::Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
            response
        }
        None => {
            let (rect, response) = ui.allocate_exact_size(size, sense);
            let visuals = ui.visuals();
            let t = theme::tokens(ui);
            ui.painter().rect_filled(rect, theme::RADIUS as f32, t.panel_header);
            ui.painter().rect_stroke(rect, theme::RADIUS as f32, egui::Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
            let font = if size.y > 80.0 { egui::FontId::proportional(13.0) } else { egui::FontId::proportional(18.0) };
            let text = if size.y > 80.0 { "No saved preview" } else { "⬚" };
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, font, visuals.weak_text_color());
            response
        }
    }
}

/// A side of a comparison, with a note while SolidCraft draws its large picture.
fn side_picture_ui(ui: &mut egui::Ui, picture: &SidePicture, size: egui::Vec2) {
    let response = thumbnail(ui, picture.image.clone(), size, egui::Sense::hover());
    if picture.drawing {
        drawing_note(ui, response.rect);
    }
}

fn drawing_note(ui: &egui::Ui, rect: egui::Rect) {
    let t = theme::tokens(ui);
    let at = rect.left_bottom() + egui::vec2(10.0, -10.0);
    ui.painter().text(at, egui::Align2::LEFT_BOTTOM, "SolidCraft is drawing it larger…", egui::FontId::proportional(size::SMALL), t.text_dim);
    ui.ctx().request_repaint_after(Duration::from_millis(250));
}

/// The "To" side over the "From" side, `blend` of it showing.
fn overlay_pictures(ui: &mut egui::Ui, from: &SidePicture, to: &SidePicture, blend: f32, size: egui::Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let t = theme::tokens(ui);
    ui.painter().rect_filled(rect, theme::RADIUS as f32, t.panel_header);
    // Both pictures are 1.6 : 1; centred in the box.
    let fit = egui::vec2(rect.width().min(rect.height() * THUMB_ASPECT), rect.height().min(rect.width() / THUMB_ASPECT));
    let place = egui::Rect::from_center_size(rect.center(), fit);
    let alpha = |a: f32| Color32::from_white_alpha((a.clamp(0.0, 1.0) * 255.0).round() as u8);
    if let Some(image) = from.image.clone() {
        // Opaque pictures cross-fade exactly; a see-through one (a saved thumbnail) still goes.
        image.tint(alpha(2.0 * (1.0 - blend))).paint_at(ui, place);
    }
    if let Some(image) = to.image.clone() {
        image.tint(alpha(blend)).paint_at(ui, place);
    }
    if from.image.is_none() && to.image.is_none() {
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "No saved preview", egui::FontId::proportional(13.0), ui.visuals().weak_text_color());
    }
    ui.painter().rect_stroke(rect, theme::RADIUS as f32, egui::Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    if from.drawing || to.drawing {
        drawing_note(ui, rect);
    }
}

/// A click area over an already drawn widget. Registered after it, so it is on top: the details
/// panel's scroll area would otherwise take the clicks of a widget that senses them itself.
fn clickable(ui: &egui::Ui, drawn: egui::Response, id: &str) -> egui::Response {
    ui.interact(drawn.rect, ui.id().with(id), egui::Sense::click())
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

/// Settings: adding the app to the application menu.
#[cfg(not(windows))]
fn menu_entry_settings(ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let (Some(data), Some(program)) = (crate::menu_entry::data_home(), crate::menu_entry::program()) else { return };
    ui.horizontal_wrapped(|ui| match crate::menu_entry::installed(&data) {
        Some(p) if p == program => {
            ui.label("SolidCraft PDM is in the application menu.");
            if ui.button("Remove").clicked() {
                actions.push(Action::MenuEntry(false));
            }
        }
        Some(p) => {
            ui.label(format!("The menu entry starts {}.", p.display()));
            if ui.button("Use this program").on_hover_text(program.display().to_string()).clicked() {
                actions.push(Action::MenuEntry(true));
            }
            if ui.button("Remove").clicked() {
                actions.push(Action::MenuEntry(false));
            }
        }
        None => {
            if ui.button("Add SolidCraft PDM to the application menu").on_hover_text(program.display().to_string()).clicked() {
                actions.push(Action::MenuEntry(true));
            }
        }
    });
    ui.label(RichText::new(format!("Starts {}", program.display())).small().weak());
    ui.label(RichText::new("With its icon, which also gives it the right icon in the dock on Wayland.").small().weak());
}

/// A table cell: text, a status (dot and text), or a pill.
enum Cell {
    Text(RichText),
    Status(String, Color32),
    Pill(String, Color32, Color32),
}

/// The State column: "In progress", "Released B" (a pill; changed since: orange), "Obsolete".
fn state_cell(e: &Entry) -> Cell {
    let t = theme::now();
    match (e.state, &e.release) {
        (None, _) => Cell::Text(RichText::new("-").weak()),
        (Some(vault::State::Released), release) => {
            let revision = release.as_ref().map_or("", |r| r.revision.as_str());
            if e.status == Status::Modified {
                Cell::Pill(format!("Released {revision} · changed"), t.warning, t.warning_bg)
            } else {
                Cell::Pill(format!("Released {revision}"), t.ok, t.ok_bg)
            }
        }
        (Some(vault::State::Obsolete), _) => Cell::Text(RichText::new("Obsolete").weak().strikethrough()),
        (Some(vault::State::Wip), _) => Cell::Text(RichText::new("In progress").color(t.label)),
    }
}

/// The table header's hairline under a cell.
fn ui_rule_under(response: &egui::Response, rect: egui::Rect, t: &theme::Tokens, clip: egui::Rect) {
    response.ctx.layer_painter(response.layer_id).with_clip_rect(clip).hline(rect.x_range(), rect.bottom() + 2.0, egui::Stroke::new(1.0, t.rule));
}

/// The selected design's lifecycle and what can be done with it. `held`: on a server, who has
/// it checked out ("you", or a name); the server releases only a design nobody has.
fn lifecycle_ui(ui: &mut egui::Ui, entry: &Entry, lifecycle: &vault::Lifecycle, next: &str, held: Option<&str>, actions: &mut Vec<Action>) {
    let path = entry.path.clone();
    let current = entry.status == Status::Current && held.is_none();
    let busy = match held {
        Some("you") => "Check it in first".to_owned(),
        Some(user) => format!("Checked out by {user}"),
        None => "Check in the changes first".to_owned(),
    };
    let t = theme::tokens(ui);
    let obsolete = lifecycle.state == vault::State::Obsolete;
    {
        ui.horizontal_wrapped(|ui| {
            match lifecycle.state {
                vault::State::Wip => {
                    theme::pill(ui, "In progress", t.label, t.hover);
                    if let Some(r) = lifecycle.releases.first() {
                        ui.label(RichText::new(format!("last released {}", r.revision)).color(t.text_dim));
                    }
                }
                vault::State::Released => {
                    let r = lifecycle.releases.first();
                    theme::pill(ui, &format!("Released {}", r.map_or("", |r| r.revision.as_str())), t.ok, t.ok_bg);
                    if let Some(r) = r {
                        ui.label(RichText::new(format!("v{} · {}", r.version, unix_time(r.released_at))).color(t.text_dim));
                    }
                }
                vault::State::Obsolete => {
                    theme::pill(ui, "Obsolete", t.text_dim, t.panel_header);
                }
            }
            match lifecycle.state {
                vault::State::Wip => {
                    if ui
                        .add_enabled(current, egui::Button::new(format!("Release as {next}…")))
                        .on_disabled_hover_text(&busy)
                        .clicked()
                    {
                        actions.push(Action::AskRelease(path.clone()));
                    }
                }
                vault::State::Released => {
                    if ui.add(egui::Button::new("Revise")).on_hover_text(format!("Work on revision {next}")).clicked() {
                        actions.push(Action::Revise(path.clone()));
                    }
                }
                vault::State::Obsolete => {}
            }
            let (label, hint) = if obsolete {
                ("Back to work in progress", "Use this design again")
            } else {
                ("Mark obsolete", "No longer used; kept with its history")
            };
            if ui.add(egui::Button::new(label)).on_hover_text(hint).clicked() {
                actions.push(Action::SetObsolete { path: path.clone(), obsolete: !obsolete });
            }
        });
    }
    if lifecycle.state == vault::State::Released && entry.status == Status::Modified {
        let revision = lifecycle.releases.first().map_or("", |r| r.revision.as_str());
        ui.colored_label(ui.visuals().warn_fg_color, format!("Changed since release {revision}: checking it in starts revision {next}."));
    }
}

/// A design's production files, by release (newest first), and making them.
fn release_files_ui(
    ui: &mut egui::Ui,
    path: &str,
    releases: &[vault::Release],
    exporting: bool,
    apps: &BTreeMap<String, launcher::FileApp>,
    actions: &mut Vec<Action>,
) {
    let count: usize = releases.iter().map(|r| r.files.len()).sum();
    let t = theme::tokens(ui);
    egui::CollapsingHeader::new(RichText::new(format!("Production files ({count})")).font(theme::semibold(size::HEADING)))
        .id_salt("release-files")
        .default_open(true)
        .show(ui, |ui| {
            let newest = &releases[0];
            if exporting {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("SolidCraft is making the files…").color(t.text_dim));
                });
            } else if newest.files.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("None for revision {} yet.", newest.revision)).color(t.text_dim));
                    let button = ui.button("Make them").on_hover_text("STEP, STL and 3MF of the released version, as in the settings");
                    if button.clicked() {
                        actions.push(Action::MakeReleaseFiles { path: path.to_owned(), version: newest.version, revision: newest.revision.clone() });
                    }
                });
            }
            for release in releases.iter().filter(|r| !r.files.is_empty()) {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Revision {}", release.revision)).strong());
                    ui.label(RichText::new(format!("v{}", release.version)).color(t.text_dim));
                    if release.files.len() > 1 && ui.small_button("Save all…").on_hover_text("Into a folder you choose").clicked() {
                        actions.push(Action::SaveReleaseFiles(release.files.clone()));
                    }
                });
                for file in &release.files {
                    ui.horizontal(|ui| {
                        ui.add_space(4.0);
                        icons::show(ui, Icon::for_file(&file.name), 22.0, t.text);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            ui.label(RichText::new(&file.name).font(egui::FontId::monospace(size::SMALL + 0.5)));
                            ui.label(RichText::new(human_size(file.size)).small().color(t.text_dim));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Save…").clicked() {
                                actions.push(Action::SaveReleaseFile(file.clone()));
                            }
                            let extension = file.name.rsplit_once('.').map_or(String::new(), |(_, e)| e.to_ascii_lowercase());
                            let (label, hint) = match apps.get(&extension) {
                                Some(app) => (format!("Open in {}", app.name), format!("Settings > Production files chooses the program for {} files", extension.to_uppercase())),
                                None => ("Open".to_owned(), format!("With the system's program for {} files; Settings > Production files chooses another, a slicer say", extension.to_uppercase())),
                            };
                            if ui.small_button(label).on_hover_text(hint).clicked() {
                                actions.push(Action::OpenReleaseFile(file.clone()));
                            }
                        });
                    });
                }
            }
            if !exporting && !newest.files.is_empty() {
                let again = ui.small_button("Make them again").on_hover_text(format!("For revision {}, with the formats in the settings", newest.revision));
                if again.clicked() {
                    actions.push(Action::MakeReleaseFiles { path: path.to_owned(), version: newest.version, revision: newest.revision.clone() });
                }
            }
        });
}

/// The two sides' files, compared.
fn compare_sides(backend: &Backend, path: &str, history: &[Version], from: Side, to: Side) -> Result<Comparison, String> {
    let bytes = |side: Side| -> Result<Vec<u8>, String> {
        match side {
            Side::Working => std::fs::read(backend.absolute(path)).map_err(|e| format!("working file: {e}")),
            Side::Version(n) => {
                let v = history.iter().find(|v| v.number == n).ok_or(format!("no v{n}"))?;
                backend.version_bytes(v).map_err(|e| e.to_string())
            }
        }
    };
    scformat::compare::compare(&bytes(from)?, &bytes(to)?).map_err(|e| e.to_string())
}

/// A choice of the working file or one of the versions.
fn side_picker(ui: &mut egui::Ui, id: &str, side: &mut Side, history: &[Version]) {
    let label = |s: Side| match s {
        Side::Working => "working file".to_owned(),
        Side::Version(n) => format!("v{n}"),
    };
    egui::ComboBox::from_id_salt(("compare-side", id)).selected_text(label(*side)).width(150.0).show_ui(ui, |ui| {
        ui.selectable_value(side, Side::Working, "working file");
        for v in history {
            let text = if v.comment.is_empty() { format!("v{}", v.number) } else { format!("v{} · {}", v.number, v.comment) };
            ui.selectable_value(side, Side::Version(v.number), text);
        }
    });
}

fn added() -> Color32 {
    theme::now().ok
}
fn removed() -> Color32 {
    theme::now().error
}
fn changed() -> Color32 {
    theme::now().warning
}

/// The differences, by section.
fn comparison_ui(ui: &mut egui::Ui, c: &Comparison) {
    if c.is_empty() {
        ui.label("No differences.");
        return;
    }
    let changes = |ui: &mut egui::Ui, title: &str, list: &[Change]| {
        if list.is_empty() {
            return;
        }
        egui::CollapsingHeader::new(RichText::new(format!("{title} ({})", list.len())).strong()).default_open(true).show(ui, |ui| {
            egui::Grid::new(("compare", title)).num_columns(4).spacing([12.0, 4.0]).max_col_width(280.0).striped(true).show(ui, |ui| {
                for ch in list {
                    let (mark, color) = match (&ch.before, &ch.after) {
                        (None, _) => ("+", added()),
                        (_, None) => ("-", removed()),
                        _ => ("~", changed()),
                    };
                    ui.colored_label(color, format!("{mark} {}", ch.what));
                    // Values wrap (the grid limits the column width): cut off, "boar…" says nothing.
                    ui.add(egui::Label::new(RichText::new(ch.before.as_deref().unwrap_or("")).weak()).wrap());
                    ui.label(if ch.before.is_some() && ch.after.is_some() { "»" } else { "" });
                    ui.add(egui::Label::new(ch.after.as_deref().unwrap_or("")).wrap());
                    ui.end_row();
                }
            });
        });
    };
    changes(ui, "Properties", &c.properties);
    changes(ui, "Parameters", &c.parameters);
    if !c.features.is_empty() || c.features_reordered {
        egui::CollapsingHeader::new(RichText::new(format!("Features ({})", c.features.len())).strong())
            .default_open(true)
            .show(ui, |ui| {
                for f in &c.features {
                    let (mark, color, detail) = match &f.change {
                        FeatureChangeKind::Added => ("+", added(), "added".to_owned()),
                        FeatureChangeKind::Removed => ("-", removed(), "removed".to_owned()),
                        edited => ("~", changed(), describe_edit(edited)),
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(color, format!("{mark} {}", f.name));
                        ui.weak(format!("({})", f.kind));
                        ui.label(detail);
                    });
                }
                if c.features_reordered {
                    ui.weak("The order of the features changed.");
                }
            });
    }
    changes(ui, "Components", &c.components);
    changes(ui, "Links", &c.links);
    if !c.other.is_empty() {
        ui.label(format!("Also changed: {}", c.other.join(", ")));
    }
}

/// "renamed from X; suppressed; changed: radius, extent"
fn describe_edit(change: &FeatureChangeKind) -> String {
    let FeatureChangeKind::Edited { renamed_from, suppressed, settings } = change else { return String::new() };
    let mut what = Vec::new();
    if let Some(old) = renamed_from {
        what.push(format!("renamed from {old}"));
    }
    match suppressed {
        Some(true) => what.push("suppressed".to_owned()),
        Some(false) => what.push("unsuppressed".to_owned()),
        None => {}
    }
    if !settings.is_empty() {
        what.push(format!("changed: {}", settings.join(", ")));
    }
    what.join("; ")
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

/// In a workspace: get a reference image from the server, or upload it.
fn image_server_buttons(ui: &mut egui::Ui, status: Option<&remote::RemoteStatus>, path: &str, actions: &mut Vec<Action>) {
    use remote::RemoteStatus as R;
    match status {
        Some(R::NotDownloaded) | Some(R::NewerOnServer) => {
            if ui.button("Get from the server").clicked() {
                actions.push(Action::Get(path.to_owned()));
                ui.close();
            }
        }
        _ => {}
    }
    match status {
        Some(R::LocalOnly) | Some(R::NewerOnServer) => {
            let label = if status == Some(&R::LocalOnly) { "Upload to the server" } else { "Upload this version" };
            if ui.button(label).clicked() {
                actions.push(Action::UploadImage(path.to_owned()));
                ui.close();
            }
        }
        _ => {}
    }
}

/// The Status column for a workspace design.
fn remote_style(status: &remote::RemoteStatus) -> (String, Color32) {
    use remote::RemoteStatus as R;
    let t = theme::now();
    match status {
        R::UpToDate => ("Up to date".into(), t.ok),
        R::NewerOnServer => ("Newer on server".into(), t.accent),
        R::NotDownloaded => ("On the server".into(), t.text_dim),
        R::CheckedOut { modified: false } => ("Checked out".into(), t.accent),
        R::CheckedOut { modified: true } => ("Checked out · changed".into(), t.warning),
        R::CheckedOutByOther(user) => (format!("Checked out by {user}"), t.other),
        R::ChangedWithoutCheckout => ("Changed, not checked out".into(), t.warning),
        R::Changed => ("Changed".into(), t.warning),
        R::Conflict => ("Conflict".into(), t.error),
        R::LocalOnly => ("Not on the server".into(), t.accent),
        R::RemovedOnServer => ("Removed on the server".into(), t.error),
    }
}

pub(crate) fn access_label(access: protocol::Access) -> &'static str {
    match access {
        protocol::Access::Read => "read only",
        protocol::Access::Write => "read and change",
        protocol::Access::Admin => "administrator",
    }
}

/// `~/PDM/<vault>`, where a new workspace goes by default.
fn default_workspace(vault: &str) -> String {
    dirs_home().map(|h| h.join("PDM").join(vault).display().to_string()).unwrap_or_default()
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

fn status_style(status: Status) -> (&'static str, Color32) {
    match status {
        Status::Current => ("Up to date", theme::now().ok),
        Status::Modified => ("Modified", theme::now().warning),
        Status::Untracked => ("Untracked", theme::now().accent),
        Status::Missing => ("Missing", theme::now().error),
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

/// An imported theme's id (kept apart from the built-in ones' ids).
fn imported_id(id: &str) -> String {
    format!("file:{id}")
}

/// The built-in themes, then the imported ones (with their ids).
fn all_themes(settings: &Settings) -> Vec<(crate::themes::ThemeFile, bool)> {
    let builtin = crate::themes::builtin().iter().map(|t| (t.clone(), false));
    let imported = settings
        .imported_themes
        .iter()
        .filter_map(|text| crate::themes::parse(text).ok())
        .map(|mut t| {
            t.id = imported_id(&t.id);
            (t, true)
        });
    builtin.chain(imported).collect()
}

/// The colour theme chosen in the settings, if any.
fn chosen_theme(settings: &Settings) -> Option<crate::themes::ThemeFile> {
    if settings.theme.is_empty() {
        return None;
    }
    all_themes(settings).into_iter().map(|(t, _)| t).find(|t| t.id == settings.theme)
}

/// `text` in the strong text colour, with the chars at `matches` highlighted.
fn highlighted(ui: &egui::Ui, text: &str, matches: &[usize], font: egui::FontId) -> egui::text::LayoutJob {
    let t = theme::tokens(ui);
    let normal = egui::TextFormat::simple(font.clone(), ui.visuals().strong_text_color());
    let hit = egui::TextFormat { background: t.accent_soft, ..egui::TextFormat::simple(font, t.accent) };
    let mut job = egui::text::LayoutJob::default();
    for (i, c) in text.chars().enumerate() {
        let format = if matches.binary_search(&i).is_ok() { hit.clone() } else { normal.clone() };
        job.append(c.encode_utf8(&mut [0; 4]), 0.0, format);
    }
    job
}

/// A card on the start page: an icon tile, a title and a line; the whole card is the button.
fn start_card(ui: &mut egui::Ui, icon: Icon, title: &str, text: &str) -> egui::Response {
    let t = theme::tokens(ui);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 84.0), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, title));
    let hovered = response.hovered();
    let painter = ui.painter();
    let fill = if hovered { t.selected_bg } else { t.card };
    let stroke = if hovered { t.accent } else { t.card_border };
    painter.rect(rect, egui::CornerRadius::same(theme::CARD_RADIUS), fill, egui::Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
    let tile = egui::Rect::from_min_size(rect.left_top() + egui::vec2(16.0, (rect.height() - 44.0) / 2.0), egui::vec2(44.0, 44.0));
    painter.rect_filled(tile, egui::CornerRadius::same(theme::RADIUS), t.panel_header);
    icons::paint(ui, icon, tile.shrink(8.0), t.icon);
    let x = tile.right() + 14.0;
    let room = rect.right() - x - 12.0;
    let title_galley = painter.layout(title.to_owned(), theme::semibold(size::SUBTITLE), if hovered { t.selected_ink } else { t.text }, room);
    let text_galley = painter.layout(text.to_owned(), egui::FontId::proportional(size::BODY), t.text_dim, room);
    let block = title_galley.size().y + 4.0 + text_galley.size().y;
    let top = rect.center().y - block / 2.0;
    painter.galley(egui::pos2(x, top), title_galley.clone(), t.text);
    painter.galley(egui::pos2(x, top + title_galley.size().y + 4.0), text_galley, t.text_dim);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A recent vault: its icon, name and folder.
fn recent_row(ui: &mut egui::Ui, path: &std::path::Path) -> egui::Response {
    let t = theme::tokens(ui);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 46.0), egui::Sense::click());
    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, path.display().to_string()));
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, egui::CornerRadius::same(theme::RADIUS), t.hover);
    }
    let workspace = remote::Workspace::is_workspace(path);
    let icon = if workspace { Icon::Server } else { Icon::Folder };
    icons::paint(ui, icon, egui::Rect::from_center_size(egui::pos2(rect.left() + 24.0, rect.center().y), egui::vec2(18.0, 18.0)), t.icon);
    let name_galley = painter.layout_no_wrap(name, theme::medium(size::HEADING), t.text);
    painter.galley(egui::pos2(rect.left() + 48.0, rect.center().y - name_galley.size().y / 2.0), name_galley, t.text);
    let where_ = path.display().to_string();
    let path_galley = painter.layout_no_wrap(where_, egui::FontId::monospace(size::SMALL + 0.5), t.text_dim);
    painter.galley(egui::pos2(rect.left() + 260.0, rect.center().y - path_galley.size().y / 2.0), path_galley, t.text_dim);
    let kind = if workspace { "Server vault" } else { "Vault" };
    let kind_galley = painter.layout_no_wrap(kind.to_owned(), egui::FontId::proportional(size::SMALL + 0.5), t.text_dim);
    painter.galley(egui::pos2(rect.right() - 16.0 - kind_galley.size().x, rect.center().y - kind_galley.size().y / 2.0), kind_galley, t.text_dim);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A thin vertical divider on the app bar.
fn app_bar_divider(ui: &mut egui::Ui, t: &theme::Tokens) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(9.0, 18.0), egui::Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range(), egui::Stroke::new(1.0, t.app_bar_dim.gamma_multiply(0.5)));
}

fn status_divider(ui: &mut egui::Ui, t: &theme::Tokens) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, 12.0), egui::Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range(), egui::Stroke::new(1.0, t.border));
}

/// A key or shortcut, as a small key cap.
fn keycap(ui: &mut egui::Ui, keys: &str) {
    let t = theme::tokens(ui);
    let galley = ui.painter().layout_no_wrap(keys.to_owned(), egui::FontId::monospace(size::SMALL - 1.0), t.text_dim);
    let (rect, _) = ui.allocate_exact_size(galley.size() + egui::vec2(10.0, 4.0), egui::Sense::hover());
    ui.painter().rect(rect, egui::CornerRadius::same(4), t.footer, egui::Stroke::new(1.0, t.rule), egui::StrokeKind::Inside);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, t.text_dim);
}

/// "server.lan" from "http://server.lan:7380/".
fn host_of(url: &str) -> String {
    url.split("://").nth(1).unwrap_or(url).trim_end_matches('/').to_owned()
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Release notes (the changelog's Markdown): headings bold, the rest as written.
fn release_notes(ui: &mut egui::Ui, notes: &str) {
    for line in notes.lines() {
        let heading = line.trim_start_matches('#');
        if heading.len() < line.len() {
            ui.add_space(4.0);
            ui.label(RichText::new(heading.trim()).strong());
        } else if !line.trim().is_empty() {
            ui.add(egui::Label::new(line.replace('*', "").replace('`', "")).wrap());
        }
    }
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
