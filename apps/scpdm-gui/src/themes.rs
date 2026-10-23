//! SolidCraft's colour themes (`solidcraft-theme/1` files): the ones SolidCraft comes with
//! (`assets/themes/`, Gruvbox, Catppuccin, Nord and others) and files imported in the settings.
//! A theme file names a base palette (dark or light) and sets SolidCraft's colours by name; the
//! PDM takes the ones it has a counterpart for, and works out the rest from them, so it looks
//! like SolidCraft in the same theme.
//!
//! ```json
//! {"format": "solidcraft-theme/1", "name": "Mine", "base": "dark",
//!  "colors": {"panel": "#282828", "accent": "#fe8019"}}
//! ```

use std::collections::BTreeMap;
use std::sync::OnceLock;

use eframe::egui::Color32;
use serde_json::Value;

use crate::theme::{DARK, LIGHT, Tokens};

pub const FORMAT: &str = "solidcraft-theme/1";
/// Hostile-input caps, as SolidCraft's: file size, colours, name length.
const MAX_BYTES: usize = 256 * 1024;
const MAX_COLORS: usize = 512;
const MAX_NAME: usize = 80;
/// Imported themes kept in the settings.
pub const MAX_IMPORTED: usize = 64;

/// A theme: its id (the setting's value), name, base and colours by SolidCraft's names.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeFile {
    pub id: String,
    pub name: String,
    pub dark: bool,
    colors: BTreeMap<String, Color32>,
}

impl ThemeFile {
    /// Swatches for the settings: background, header, accent, text.
    pub fn swatches(&self) -> [Color32; 4] {
        let t = self.tokens();
        [t.panel, t.app_bar, t.accent, t.text]
    }

    /// The PDM's colours in this theme.
    pub fn tokens(&self) -> Tokens {
        let base = if self.dark { DARK } else { LIGHT };
        let get = |keys: &[&str]| keys.iter().find_map(|k| self.colors.get(*k).copied());
        let or = |keys: &[&str], fallback: Color32| get(keys).unwrap_or(fallback);
        let panel = or(&["panel"], base.panel);
        let panel_header = or(&["panel_header"], base.panel_header);
        let text = or(&["text"], base.text);
        let text_dim = or(&["text_dim"], base.text_dim);
        let accent = or(&["accent"], base.accent);
        let error = or(&["error", "form_error"], base.error);
        let warning = or(&["warning"], base.warning);
        let ok = or(&["form_success", "agent_ok"], base.ok);
        let app_bar = or(&["app_bar"], base.app_bar);
        let app_bar_text = or(&["app_bar_text"], base.app_bar_text);
        let border = or(&["border"], base.border);
        let primary = or(&["form_primary"], accent);
        Tokens {
            dark: self.dark,
            panel,
            toolbar: or(&["toolbar"], panel),
            panel_header,
            field: or(&["field"], base.field),
            app_bar,
            app_bar_text,
            app_bar_dim: mix(app_bar, app_bar_text, 0.55),
            app_bar_hover: or(&["app_bar_hover"], base.app_bar_hover),
            app_bar_tab: or(&["app_bar_tab_active"], mix(app_bar, app_bar_text, 0.12)),
            card: mix(panel, panel_header, 0.5),
            footer: mix(panel, panel_header, 0.75),
            border,
            card_border: or(&["dialog_border"], border),
            rule: or(&["dialog_rule"], mix(panel, border, 0.7)),
            control: or(&["control_fill"], panel),
            control_border: or(&["control_border"], border),
            text,
            text_dim,
            label: mix(text, text_dim, 0.5),
            accent,
            accent_soft: or(&["accent_soft"], mix(panel, accent, 0.25)),
            primary,
            on_primary: or(&["on_accent"], readable_on(primary)),
            selected_bg: or(&["selected_bg", "accent_soft"], mix(panel, accent, 0.25)),
            selected_ink: or(&["selected_ink"], text),
            hover: or(&["hover"], base.hover),
            error,
            error_bg: or(&["form_error_fill"], mix(panel, error, 0.22)),
            warning,
            warning_bg: mix(panel, warning, 0.18),
            ok,
            ok_bg: or(&["form_success_fill"], mix(panel, ok, 0.2)),
            other: base.other,
            icon: or(&["icon"], base.icon),
        }
    }
}

/// `a` towards `b` by `f` (0: `a`, 1: `b`), opaque.
fn mix(a: Color32, b: Color32, f: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * f).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// Black or white, whichever reads better on `fill`.
fn readable_on(fill: Color32) -> Color32 {
    let luma = 0.299 * fill.r() as f32 + 0.587 * fill.g() as f32 + 0.114 * fill.b() as f32;
    if luma > 150.0 { Color32::from_rgb(0x10, 0x14, 0x1a) } else { Color32::WHITE }
}

/// `#rrggbb` or `#rrggbbaa` (alpha not premultiplied).
fn parse_color(s: &str) -> Option<Color32> {
    let hex = s.trim().strip_prefix('#')?;
    if !(hex.len() == 6 || hex.len() == 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    let a = if hex.len() == 8 { byte(6)? } else { 255 };
    Some(Color32::from_rgba_unmultiplied(byte(0)?, byte(2)?, byte(4)?, a))
}

/// A theme file's text, checked: the format tag, a name, a base, colours that read as colours.
/// Lists (SolidCraft's axis colours, say) and names the PDM has no use for are passed over, so
/// a theme from a newer SolidCraft still works.
pub fn parse(text: &str) -> Result<ThemeFile, String> {
    if text.len() > MAX_BYTES {
        return Err(format!("a theme file is at most {} KiB", MAX_BYTES / 1024));
    }
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not a theme file: {e}"))?;
    if v.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err(format!("not a theme file: `format` must be \"{FORMAT}\""));
    }
    let name = v.get("name").and_then(Value::as_str).map(str::trim).unwrap_or_default();
    if name.is_empty() || name.chars().count() > MAX_NAME {
        return Err(format!("`name` must be 1 to {MAX_NAME} characters"));
    }
    let dark = match v.get("base").and_then(Value::as_str) {
        Some("dark") => true,
        Some("light") => false,
        _ => return Err("`base` must be \"dark\" or \"light\"".into()),
    };
    let mut colors = BTreeMap::new();
    match v.get("colors") {
        None => {}
        Some(Value::Object(o)) if o.len() <= MAX_COLORS => {
            for (key, value) in o {
                match value {
                    Value::String(s) => {
                        let c = parse_color(s).ok_or_else(|| format!("`{key}`: `{s}` is not a colour such as \"#282828\""))?;
                        colors.insert(key.clone(), c);
                    }
                    Value::Array(_) => {}
                    _ => return Err(format!("`{key}` must be a colour such as \"#282828\"")),
                }
            }
        }
        Some(Value::Object(_)) => return Err(format!("`colors` has more than {MAX_COLORS} entries")),
        Some(_) => return Err("`colors` must be an object of colour names and values".into()),
    }
    Ok(ThemeFile { id: slug(name), name: name.to_owned(), dark, colors })
}

/// An id from a name: lower-case letters, digits and dashes.
fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            s.push(c);
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    let s = s.trim_end_matches('-').to_owned();
    if s.is_empty() { "theme".into() } else { s }
}

macro_rules! builtin {
    ($($file:literal),* $(,)?) => {
        [$(($file, include_str!(concat!("../../../assets/themes/", $file, ".json")))),*]
    };
}

/// The themes SolidCraft 0.20 comes with, in its order.
const BUILTIN: [(&str, &str); 20] = builtin![
    "gruvbox-dark",
    "gruvbox-dark-hard",
    "gruvbox-dark-soft",
    "gruvbox-light",
    "gruvbox-light-hard",
    "gruvbox-light-soft",
    "solidcraft-95",
    "platinum-9",
    "cupertino",
    "green-phosphor",
    "amber-phosphor",
    "blueprint",
    "one-dark",
    "tokyo-night",
    "solarized-light",
    "nord",
    "high-contrast",
    "graphite",
    "nosferatu",
    "catppuccin-mocha",
];

/// The built-in themes, read once; their ids are the file names (as in SolidCraft).
pub fn builtin() -> &'static [ThemeFile] {
    static THEMES: OnceLock<Vec<ThemeFile>> = OnceLock::new();
    THEMES.get_or_init(|| {
        BUILTIN.iter().filter_map(|(id, text)| parse(text).ok().map(|t| ThemeFile { id: (*id).to_owned(), ..t })).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_theme_reads() {
        assert_eq!(builtin().len(), BUILTIN.len());
        let mocha = builtin().iter().find(|t| t.id == "catppuccin-mocha").unwrap();
        assert_eq!((mocha.name.as_str(), mocha.dark), ("Catppuccin Mocha", true));
        let t = mocha.tokens();
        assert_eq!(t.panel, Color32::from_rgb(0x1e, 0x1e, 0x2e));
        assert_eq!(t.accent, Color32::from_rgb(0xcb, 0xa6, 0xf7));
        // Light mauve buttons get dark text, as in SolidCraft.
        assert_eq!(t.on_primary, Color32::from_rgb(0x06, 0x12, 0x1b));
        assert!(builtin().iter().any(|t| !t.dark), "light ones too");
    }

    #[test]
    fn a_theme_with_a_few_colours_gets_the_rest_from_its_base() {
        let gruvbox = builtin().iter().find(|t| t.id == "gruvbox-light").unwrap().tokens();
        assert!(!gruvbox.dark);
        assert_ne!(gruvbox.selected_bg, LIGHT.selected_bg, "from its accent, not the base's");
        let mine = parse(r##"{"format": "solidcraft-theme/1", "name": "Mine!", "base": "dark", "colors": {"accent": "#fe8019", "triad": ["#fb4934", "#b8bb26", "#83a598"], "from_a_newer_solidcraft": "#123456"}}"##).unwrap();
        assert_eq!(mine.id, "mine");
        let t = mine.tokens();
        assert_eq!((t.accent, t.panel, t.on_primary), (Color32::from_rgb(0xfe, 0x80, 0x19), DARK.panel, Color32::from_rgb(0x10, 0x14, 0x1a)));
    }

    #[test]
    fn refuses_what_is_not_a_theme() {
        assert!(parse("{}").unwrap_err().contains("format"));
        assert!(parse(r#"{"format": "solidcraft-theme/1", "name": "", "base": "dark"}"#).unwrap_err().contains("name"));
        assert!(parse(r#"{"format": "solidcraft-theme/1", "name": "X", "base": "blue"}"#).unwrap_err().contains("base"));
        assert!(parse(r#"{"format": "solidcraft-theme/1", "name": "X", "base": "dark", "colors": {"panel": "red"}}"#).unwrap_err().contains("panel"));
    }
}
