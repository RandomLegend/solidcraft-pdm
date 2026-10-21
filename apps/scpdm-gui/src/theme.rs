//! The look: SolidCraft's own design tokens (its `assets/styles/ui/palettes.rs`), fonts
//! (IBM Plex Sans and Mono) and shapes, so the PDM reads as part of SolidCraft. Dark and
//! light, following the system unless chosen in the settings.
//!
//! Colours are taken from [`Tokens`] only, through [`tokens`]; the pieces every screen uses
//! (section titles, cards, pills, primary buttons, navigation rows) are here too.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Shadow, Stroke, Theme, Vec2};

/// SolidCraft's colour tokens (names as there; values for light and dark).
#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    pub dark: bool,
    /// Panels and windows.
    pub panel: Color32,
    /// The strip under the app bar.
    pub toolbar: Color32,
    /// Inactive widget fill; table header hover.
    pub panel_header: Color32,
    /// Text fields.
    pub field: Color32,
    /// The title bar: dark in both themes.
    pub app_bar: Color32,
    pub app_bar_text: Color32,
    pub app_bar_dim: Color32,
    pub app_bar_hover: Color32,
    /// Cards and menus, their footer band (and the navigation column).
    pub card: Color32,
    pub footer: Color32,
    pub border: Color32,
    pub card_border: Color32,
    /// Dividers.
    pub rule: Color32,
    pub control: Color32,
    pub control_border: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub label: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    /// Primary buttons and focus.
    pub primary: Color32,
    pub selected_bg: Color32,
    pub selected_ink: Color32,
    pub hover: Color32,
    pub error: Color32,
    pub error_bg: Color32,
    pub warning: Color32,
    pub warning_bg: Color32,
    pub ok: Color32,
    pub ok_bg: Color32,
    /// Someone else's (a check-out by another user).
    pub other: Color32,
    /// Icons drawn on the background (SolidCraft's `line` role).
    pub icon: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

pub const LIGHT: Tokens = Tokens {
    dark: false,
    panel: hex(0xFAFBFC),
    toolbar: hex(0xF6F7F9),
    panel_header: hex(0xECEEF2),
    field: hex(0xFFFFFF),
    app_bar: hex(0x343840),
    app_bar_text: hex(0xE6EAF2),
    app_bar_dim: hex(0x8891A0),
    app_bar_hover: Color32::from_rgba_premultiplied(28, 28, 28, 28),
    card: hex(0xFFFFFF),
    footer: hex(0xF4F6F9),
    border: hex(0xD0D4DC),
    card_border: hex(0xD5DBE3),
    rule: hex(0xE6EAF0),
    control: hex(0xFFFFFF),
    control_border: hex(0xC3CBD6),
    text: hex(0x262A32),
    text_dim: hex(0x6E7480),
    label: hex(0x3A4656),
    accent: hex(0x2678DA),
    accent_soft: hex(0xD6E6FA),
    primary: hex(0x236EAD),
    selected_bg: hex(0xE8F1FA),
    selected_ink: hex(0x1A507F),
    hover: hex(0xE2EAF6),
    error: hex(0xD63636),
    error_bg: hex(0xFCEFED),
    warning: hex(0xB87A12),
    warning_bg: hex(0xFDF4E2),
    ok: hex(0x276B2C),
    ok_bg: hex(0xE5F2E6),
    other: hex(0x6A4FB5),
    icon: hex(0x34465B),
};

pub const DARK: Tokens = Tokens {
    dark: true,
    panel: hex(0x272B34),
    toolbar: hex(0x272B34),
    panel_header: hex(0x2F343E),
    field: hex(0x1E2229),
    app_bar: hex(0x1C1F26),
    app_bar_text: hex(0xE6EAF2),
    app_bar_dim: hex(0x8891A0),
    app_bar_hover: Color32::from_rgba_premultiplied(30, 30, 30, 30),
    card: hex(0x2E333D),
    footer: hex(0x2F343E),
    border: hex(0x3F4552),
    card_border: hex(0x485363),
    rule: hex(0x353B47),
    control: hex(0x272B34),
    control_border: hex(0x4A5262),
    text: hex(0xDEE2E8),
    text_dim: hex(0x949BA8),
    label: hex(0xB4BBC8),
    accent: hex(0x3A88EE),
    accent_soft: hex(0x2A466E),
    primary: hex(0x3A88EE),
    selected_bg: hex(0x2A466E),
    selected_ink: hex(0x9CC7F5),
    hover: hex(0x363D4A),
    error: hex(0xF06460),
    error_bg: hex(0x6E3236),
    warning: hex(0xECAA46),
    warning_bg: hex(0x4A3B22),
    ok: hex(0x8FDB9A),
    ok_bg: hex(0x2C4A33),
    other: hex(0xB79BEA),
    icon: hex(0xDBE4EE),
};

/// The tokens of the theme `ui` is drawn in.
pub fn tokens(ui: &egui::Ui) -> &'static Tokens {
    if ui.visuals().dark_mode { &DARK } else { &LIGHT }
}

pub fn tokens_of(ctx: &egui::Context) -> &'static Tokens {
    if ctx.global_style().visuals.dark_mode { &DARK } else { &LIGHT }
}

static DARK_NOW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Remembers the theme of this frame, for [`now`].
pub fn begin_frame(ctx: &egui::Context) {
    DARK_NOW.store(ctx.global_style().visuals.dark_mode, std::sync::atomic::Ordering::Relaxed);
}

/// The tokens of the theme the window is drawn in (for code without a `Ui` at hand).
pub fn now() -> &'static Tokens {
    if DARK_NOW.load(std::sync::atomic::Ordering::Relaxed) { &DARK } else { &LIGHT }
}

// ---- fonts ----

const PLEX_SANS: &[u8] = include_bytes!("../../../assets/fonts/IBMPlexSans-Variable.ttf");
const PLEX_MONO: &[u8] = include_bytes!("../../../assets/fonts/IBMPlexMono-Regular.ttf");
const WEIGHTS: [(&str, f32); 3] = [("ui-medium", 500.0), ("ui-semibold", 600.0), ("ui-bold", 700.0)];

/// IBM Plex Sans (regular, and medium, semibold and bold from its weight axis) and Plex Mono,
/// with egui's own fonts behind them (symbols, emoji).
fn fonts() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    let fallbacks = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let face = |weight: f32| {
        let mut data = egui::FontData::from_static(PLEX_SANS);
        data.tweak.coords = egui::epaint::text::VariationCoords::new([(b"wght", weight)]);
        std::sync::Arc::new(data)
    };
    fonts.font_data.insert("plex-sans".into(), face(400.0));
    fonts.families.insert(FontFamily::Proportional, std::iter::once("plex-sans".to_owned()).chain(fallbacks.iter().cloned()).collect());
    for (name, weight) in WEIGHTS {
        fonts.font_data.insert(name.into(), face(weight));
        fonts.families.insert(FontFamily::Name(name.into()), std::iter::once(name.to_owned()).chain(fallbacks.iter().cloned()).collect());
    }
    fonts.font_data.insert("plex-mono".into(), std::sync::Arc::new(egui::FontData::from_static(PLEX_MONO)));
    if let Some(mono) = fonts.families.get_mut(&FontFamily::Monospace) {
        mono.insert(0, "plex-mono".into());
    }
    fonts
}

pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("ui-medium".into()))
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("ui-semibold".into()))
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("ui-bold".into()))
}

/// SolidCraft's type scale.
pub mod size {
    pub const SMALL: f32 = 11.0;
    pub const CAPTION: f32 = 11.5;
    pub const BODY: f32 = 12.5;
    pub const BUTTON: f32 = 12.5;
    pub const HEADING: f32 = 14.0;
    pub const SUBTITLE: f32 = 15.0;
    pub const ARTICLE: f32 = 17.0;
    pub const TITLE: f32 = 22.0;
    pub const DISPLAY: f32 = 26.0;
}

pub const RADIUS: u8 = 6;
pub const CARD_RADIUS: u8 = 8;
pub const MENU_RADIUS: u8 = 10;
pub const CONTROL_HEIGHT: f32 = 28.0;

fn style(t: &Tokens) -> egui::Style {
    let mut style = egui::Style::default();
    style.text_styles = [
        (egui::TextStyle::Small, FontId::proportional(size::SMALL)),
        (egui::TextStyle::Body, FontId::proportional(size::BODY)),
        (egui::TextStyle::Button, FontId::proportional(size::BUTTON)),
        (egui::TextStyle::Heading, semibold(size::ARTICLE)),
        (egui::TextStyle::Monospace, FontId::monospace(12.0)),
    ]
    .into();
    let s = &mut style.spacing;
    s.item_spacing = egui::vec2(6.0, 5.0);
    s.button_padding = egui::vec2(10.0, 4.0);
    s.interact_size = egui::vec2(28.0, 22.0);
    s.slider_rail_height = 4.0;
    s.menu_margin = Margin::same(6);
    s.window_margin = Margin::same(18);
    s.indent = 14.0;
    s.scroll.floating = true;

    let mut v = if t.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = t.panel;
    v.window_fill = t.card;
    v.window_stroke = Stroke::new(1.0, t.card_border);
    v.window_corner_radius = CornerRadius::same(MENU_RADIUS);
    v.menu_corner_radius = CornerRadius::same(MENU_RADIUS);
    let shadow = Color32::from_black_alpha(if t.dark { 90 } else { 40 });
    v.window_shadow = Shadow { offset: [0, 14], blur: 36, spread: 0, color: shadow };
    v.popup_shadow = Shadow { offset: [0, 10], blur: 28, spread: 0, color: shadow };
    v.extreme_bg_color = t.field;
    v.text_edit_bg_color = Some(t.field);
    v.faint_bg_color = t.panel_header;
    v.code_bg_color = t.panel_header;
    v.override_text_color = Some(t.text);
    v.weak_text_color = Some(t.text_dim);
    v.hyperlink_color = t.accent;
    v.warn_fg_color = t.warning;
    v.error_fg_color = t.error;
    v.selection.bg_fill = t.selected_bg;
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.striped = false;
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    let radius = CornerRadius::same(RADIUS);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = t.panel;
    w.noninteractive.weak_bg_fill = t.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.rule);
    w.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
    w.noninteractive.corner_radius = radius;
    for (state, fill, border) in [
        (&mut w.inactive, t.control, t.control_border),
        (&mut w.hovered, t.hover, t.control_border),
        (&mut w.active, t.accent_soft, t.accent),
        (&mut w.open, t.panel_header, t.control_border),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, border);
        state.fg_stroke = Stroke::new(1.0, t.text);
        state.corner_radius = radius;
        state.expansion = 0.0;
    }
    style.visuals = v;
    style
}

/// Fonts and both themes' styles; `preference` as chosen in the settings.
pub fn install(ctx: &egui::Context, preference: egui::ThemePreference) {
    ctx.set_fonts(fonts());
    ctx.set_style_of(Theme::Light, style(&LIGHT));
    ctx.set_style_of(Theme::Dark, style(&DARK));
    ctx.set_theme(preference);
}

// ---- pieces ----

/// A section title: small, uppercase, spaced, dim (as SolidCraft's menus and panels).
pub fn section(ui: &mut egui::Ui, title: &str) {
    let t = tokens(ui);
    ui.add(egui::Label::new(caps(title, t.text_dim)).selectable(false));
}

/// Uppercase small semibold text with SolidCraft's tracking.
pub fn caps(text: &str, color: Color32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &text.to_uppercase(),
        0.0,
        egui::TextFormat { font_id: semibold(size::SMALL), color, extra_letter_spacing: size::SMALL * 0.06, ..Default::default() },
    );
    job
}

/// A 1px divider in the rule colour, across the available width.
pub fn rule(ui: &mut egui::Ui) {
    let t = tokens(ui);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, t.rule));
}

/// A card: the card fill, a hairline border, rounded corners.
pub fn card(t: &Tokens) -> egui::Frame {
    egui::Frame::new()
        .fill(t.card)
        .stroke(Stroke::new(1.0, t.card_border))
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(Margin::same(12))
}

/// A primary (filled, accent) button.
pub fn primary(text: impl Into<String>, t: &Tokens) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.into()).font(medium(size::BUTTON)).color(Color32::WHITE))
        .fill(t.primary)
        .stroke(Stroke::new(1.0, t.primary))
        .min_size(egui::vec2(0.0, CONTROL_HEIGHT))
}

/// A secondary (outlined) button of the same height.
pub fn secondary(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(text.into()).min_size(egui::vec2(0.0, CONTROL_HEIGHT))
}

/// A pill: small medium-weight text on a soft fill.
pub fn pill(ui: &mut egui::Ui, text: &str, ink: Color32, fill: Color32) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), medium(size::SMALL), ink);
    let size = galley.size() + egui::vec2(16.0, 6.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(MENU_RADIUS), fill);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, ink);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
    response
}

/// A status: a coloured dot and its text.
pub fn status(ui: &mut egui::Ui, text: &str, color: Color32, font: FontId) -> egui::Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 7.0;
        let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
        ui.painter().circle_filled(dot.center(), 4.0, color);
        ui.add(egui::Label::new(RichText::new(text).font(font).color(color)).truncate().selectable(false))
    })
    .inner
}

/// A row of the navigation column: icon, label, count on the right; selected or hovered rows
/// get a rounded fill. Reads as "label  (count)" to assistive technology (and tests).
pub fn nav_row(ui: &mut egui::Ui, icon: Option<crate::icons::Icon>, label: &str, count: Option<usize>, color: Option<Color32>, selected: bool) -> egui::Response {
    let t = tokens(ui);
    let height = 30.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::click());
    let name = match count {
        Some(n) => format!("{label}  ({n})"),
        None => label.to_owned(),
    };
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &name));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, CornerRadius::same(RADIUS), t.selected_bg);
        } else if response.hovered() {
            painter.rect_filled(rect, CornerRadius::same(RADIUS), t.hover);
        }
        let ink = if selected { t.selected_ink } else { color.unwrap_or(t.text) };
        let mut x = rect.left() + 10.0;
        if let Some(icon) = icon {
            let r = egui::Rect::from_center_size(egui::pos2(x + 8.0, rect.center().y), Vec2::splat(16.0));
            crate::icons::paint(ui, icon, r, if selected { t.selected_ink } else { t.icon });
            x += 24.0;
        }
        let font = if selected { semibold(size::BODY) } else { FontId::proportional(size::BODY) };
        let count_text = count.map(|n| n.to_string()).unwrap_or_default();
        let count_galley = painter.layout_no_wrap(count_text, FontId::proportional(size::SMALL), if selected { t.selected_ink } else { t.text_dim });
        let room = (rect.right() - 10.0 - count_galley.size().x - 8.0 - x).max(10.0);
        let galley = painter.layout(label.to_owned(), font, ink, room);
        let galley = if galley.rows.len() > 1 {
            let mut job = egui::text::LayoutJob::simple_singleline(label.to_owned(), FontId::proportional(size::BODY), ink);
            job.wrap = egui::text::TextWrapping::truncate_at_width(room);
            painter.layout_job(job)
        } else {
            galley
        };
        painter.galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, ink);
        let cx = rect.right() - 10.0 - count_galley.size().x;
        painter.galley(egui::pos2(cx, rect.center().y - count_galley.size().y / 2.0), count_galley, t.text_dim);
    }
    response
}

/// A label/value grid row in the properties style.
pub fn field_row(ui: &mut egui::Ui, label: &str, value: &str) {
    let t = tokens(ui);
    ui.add(egui::Label::new(RichText::new(label).color(t.text_dim)).selectable(false));
    ui.add(egui::Label::new(RichText::new(value).color(t.text)).wrap());
    ui.end_row();
}
