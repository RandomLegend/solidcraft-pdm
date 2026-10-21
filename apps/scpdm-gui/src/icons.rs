//! Icons: SolidCraft's own (`assets/icons/solidcraft/`, MIT OR Apache-2.0) and a few in its
//! style for what SolidCraft has none of (`assets/icons/pdm/`). SVGs, recoloured for the
//! theme as SolidCraft does (the `currentColor` line role, and the face roles of its
//! `palette.toml` for the shaded model icons), rendered by egui_extras' SVG loader.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    Folder,
    FolderOpen,
    Search,
    Settings,
    Plus,
    Close,
    Chevron,
    Upload,
    Download,
    Home,
    Help,
    Eye,
    Refresh,
    Lock,
    Server,
    User,
    Image,
    Check,
    Alert,
    /// Shaded model icons.
    NewDesign,
    OpenDesign,
    Compare,
    Restore,
    Library,
    Delete,
    Rename,
    Edit,
    Copy,
    Filter,
    List,
    Checkpoint,
}

impl Icon {
    fn source(self) -> (&'static str, &'static str) {
        macro_rules! sc {
            ($name:literal) => {
                ($name, include_str!(concat!("../../../assets/icons/solidcraft/", $name, ".svg")))
            };
        }
        macro_rules! own {
            ($name:literal) => {
                ($name, include_str!(concat!("../../../assets/icons/pdm/", $name, ".svg")))
            };
        }
        match self {
            Icon::Folder => sc!("flat-folder"),
            Icon::FolderOpen => sc!("model-folder"),
            Icon::Search => sc!("flat-zoom"),
            Icon::Settings => sc!("flat-settings"),
            Icon::Plus => sc!("flat-plus"),
            Icon::Close => sc!("flat-close"),
            Icon::Chevron => sc!("flat-chevron"),
            Icon::Upload => sc!("flat-export_tray"),
            Icon::Download => sc!("flat-import_tray"),
            Icon::Home => sc!("flat-home"),
            Icon::Help => sc!("flat-help"),
            Icon::Eye => sc!("flat-eye"),
            Icon::Refresh => own!("refresh"),
            Icon::Lock => own!("lock"),
            Icon::Server => own!("server"),
            Icon::User => own!("user"),
            Icon::Image => own!("image"),
            Icon::Check => own!("check"),
            Icon::Alert => own!("alert"),
            Icon::NewDesign => sc!("model-new"),
            Icon::OpenDesign => sc!("model-open"),
            Icon::Compare => sc!("model-compare"),
            Icon::Restore => sc!("model-restore"),
            Icon::Library => sc!("model-library"),
            Icon::Delete => sc!("model-delete"),
            Icon::Rename => sc!("model-rename"),
            Icon::Edit => sc!("model-edit"),
            Icon::Copy => sc!("model-copy"),
            Icon::Filter => sc!("model-filter"),
            Icon::List => sc!("model-list"),
            Icon::Checkpoint => sc!("model-checkpoint"),
        }
    }
}

/// SolidCraft's colour roles (source colour in the SVG → dark theme colour).
const DARK_ROLES: [(&str, &str); 9] = [
    ("#34465b", "#1f2021"),
    ("#e2e8ef", "#8da3c3"),
    ("#b8c3cf", "#6d81a2"),
    ("#8e9faf", "#546782"),
    ("#6f879c", "#8093a4"),
    ("#b9ddf8", "#4ea1dc"),
    ("#328bd3", "#3287d1"),
    ("#236ead", "#2976b8"),
    ("#ffffff", "#1e2530"),
];

type Key = (Icon, Color32, bool);

fn cache() -> &'static Mutex<HashMap<Key, (String, Arc<[u8]>)>> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<Key, (String, Arc<[u8]>)>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The icon's SVG for `ink` (line art) in the given theme, and a URI naming that variant.
fn variant(icon: Icon, ink: Color32, dark: bool) -> (String, Arc<[u8]>) {
    let mut cache = cache().lock().unwrap_or_else(|e| e.into_inner());
    cache
        .entry((icon, ink, dark))
        .or_insert_with(|| {
            let (name, svg) = icon.source();
            let [r, g, b, _] = ink.to_array();
            let mut svg = svg.replace("currentColor", &format!("#{r:02x}{g:02x}{b:02x}"));
            if dark {
                for (from, to) in DARK_ROLES {
                    svg = svg.replace(from, to);
                }
            }
            (format!("bytes://icon/{name}-{r:02x}{g:02x}{b:02x}-{}.svg", if dark { "d" } else { "l" }), Arc::from(svg.into_bytes()))
        })
        .clone()
}

/// The icon as an image, its line art in `ink`.
pub fn image(ui: &egui::Ui, icon: Icon, ink: Color32) -> egui::Image<'static> {
    let (uri, bytes) = variant(icon, ink, ui.visuals().dark_mode);
    egui::Image::from_bytes(uri, egui::load::Bytes::Shared(bytes))
}

/// Paints the icon into `rect`.
pub fn paint(ui: &egui::Ui, icon: Icon, rect: egui::Rect, ink: Color32) {
    image(ui, icon, ink).paint_at(ui, rect);
}

/// The icon at `size` points, as a widget.
pub fn show(ui: &mut egui::Ui, icon: Icon, size: f32, ink: Color32) -> egui::Response {
    ui.add(image(ui, icon, ink).fit_to_exact_size(egui::vec2(size, size)))
}
