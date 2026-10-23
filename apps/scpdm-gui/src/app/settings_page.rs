//! Settings: a page of its own in place of the vault (sections on the left, the chosen one on
//! the right), so it has room as settings are added. Changes apply at once.

use eframe::egui::{self, RichText};

use super::{Action, App, Appearance, ZOOM_PRESETS};
use crate::backend::Backend;
use crate::icons::{self, Icon};
use crate::launcher::{self, FileApp};
use crate::theme::{self, size};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Opening,
    ProductionFiles,
    Versions,
    Thumbnails,
    Appearance,
    Updates,
}

impl Section {
    const ALL: [Section; 6] =
        [Section::Opening, Section::ProductionFiles, Section::Versions, Section::Thumbnails, Section::Appearance, Section::Updates];

    fn title(self) -> &'static str {
        match self {
            Section::Opening => "Opening designs",
            Section::ProductionFiles => "Production files",
            Section::Versions => "Versions",
            Section::Thumbnails => "Thumbnails",
            Section::Appearance => "Appearance",
            Section::Updates => "Updates",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Section::Opening => Icon::OpenDesign,
            Section::ProductionFiles => Icon::FileStep,
            Section::Versions => Icon::Checkpoint,
            Section::Thumbnails => Icon::Image,
            Section::Appearance => Icon::Eye,
            Section::Updates => Icon::Download,
        }
    }
}

/// The page while it is open.
pub struct SettingsPage {
    pub section: Section,
    /// The SolidCraft program being typed (applied when the field is left).
    open_command: String,
    /// Installed programs for each production file type, looked for when the page opened.
    apps: Vec<(String, Vec<FileApp>)>,
}

impl App {
    /// Opens the settings at `section`.
    pub(super) fn open_settings(&mut self, section: Section) {
        self.detected = launcher::detect();
        let apps = crate::export::FORMATS.iter().map(|(ext, _)| ((*ext).to_owned(), launcher::apps_for(ext))).collect();
        self.settings_page = Some(SettingsPage { section, open_command: self.settings.open_command.clone(), apps });
    }

    /// Closes the settings, keeping what was typed.
    pub(super) fn close_settings(&mut self, actions: &mut Vec<Action>) {
        if let Some(page) = self.settings_page.take() {
            if page.open_command.trim() != self.settings.open_command {
                actions.push(Action::SetOpenCommand(page.open_command));
            }
        }
    }

    pub(super) fn settings_ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(mut page) = self.settings_page.take() else { return };
        let t = theme::tokens(ui);
        let mut close = false;
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(RichText::new("Settings").font(theme::semibold(size::TITLE)).color(t.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                close = ui.add(theme::primary("Done", t)).on_hover_text("Esc").clicked();
            });
        });
        ui.add_space(10.0);
        ui.horizontal_top(|ui| {
            // The sections.
            ui.vertical(|ui| {
                ui.set_width(210.0);
                for section in Section::ALL {
                    let selected = page.section == section;
                    let ink = if selected { t.accent } else { t.text_dim };
                    let button = egui::Button::image_and_text(
                        icons::image(ui, section.icon(), ink).fit_to_exact_size(egui::vec2(18.0, 18.0)),
                        RichText::new(section.title()).font(theme::medium(size::BODY)).color(if selected { t.text } else { t.text_dim }),
                    )
                    .selected(selected)
                    .frame_when_inactive(false)
                    .min_size(egui::vec2(210.0, 34.0));
                    if ui.add(button).clicked() {
                        page.section = section;
                    }
                }
            });
            ui.add_space(16.0);
            // The section's settings.
            ui.vertical(|ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    ui.set_max_width(760.0);
                    ui.spacing_mut().item_spacing.y = 7.0;
                    ui.label(RichText::new(page.section.title()).font(theme::semibold(size::TITLE - 2.0)).color(t.text));
                    ui.add_space(4.0);
                    match page.section {
                        Section::Opening => self.opening(ui, &mut page, actions),
                        Section::ProductionFiles => self.production_files(ui, &page, actions),
                        Section::Versions => self.versions(ui, actions),
                        Section::Thumbnails => self.thumbnails(ui, actions),
                        Section::Appearance => self.appearance(ui, actions),
                        Section::Updates => self.updates(ui, actions),
                    }
                    ui.add_space(24.0);
                });
            });
        });
        self.settings_page = Some(page);
        if close {
            self.close_settings(actions);
        }
    }

    fn opening(&mut self, ui: &mut egui::Ui, page: &mut SettingsPage, actions: &mut Vec<Action>) {
        card(ui, "SolidCraft", |ui| {
            match &self.detected {
                Some(found) => {
                    ui.label(format!("Found: {}", found.command[0]));
                    note(ui, &format!("from {}", found.source));
                }
                None => {
                    ui.label("SolidCraft was not found (no desktop entry, not on PATH). Choose its AppImage below.");
                }
            }
            ui.add_space(4.0);
            ui.label("Use this program instead (optional):");
            ui.horizontal(|ui| {
                let field = ui.add(
                    egui::TextEdit::singleline(&mut page.open_command)
                        .hint_text("e.g. ~/Applications/SolidCraft.AppImage")
                        .desired_width(ui.available_width() - 90.0),
                );
                if field.lost_focus() && page.open_command.trim() != self.settings.open_command {
                    actions.push(Action::SetOpenCommand(page.open_command.clone()));
                }
                if ui.button("Browse…").clicked() {
                    if let Some(file) = rfd::FileDialog::new().set_title("SolidCraft program or AppImage").pick_file() {
                        page.open_command = file.display().to_string();
                        actions.push(Action::SetOpenCommand(page.open_command.clone()));
                    }
                }
            });
            note(ui, "Leave empty to use SolidCraft as found. %f stands for the design; without it, the design is added at the end.");
        });
        #[cfg(not(windows))]
        card(ui, "Application menu", |ui| super::menu_entry_settings(ui, actions));
    }

    fn production_files(&mut self, ui: &mut egui::Ui, page: &SettingsPage, actions: &mut Vec<Action>) {
        card(ui, "Made when a design is released", |ui| {
            for (extension, hint) in crate::export::FORMATS {
                let mut on = self.settings.release_files.iter().any(|e| e == extension);
                ui.horizontal(|ui| {
                    icons::show(ui, Icon::for_file(&format!("x.{extension}")), 20.0, theme::tokens(ui).text);
                    if ui.checkbox(&mut on, hint).changed() {
                        actions.push(Action::SetReleaseFormat { extension: extension.to_owned(), on });
                    }
                });
            }
            note(ui, "SolidCraft writes them from the released version; they are kept with the release, in the vault or on the server.");
        });
        card(ui, "Open with", |ui| {
            egui::Grid::new("open-with").num_columns(2).spacing([14.0, 10.0]).show(ui, |ui| {
                for (extension, found) in &page.apps {
                    ui.horizontal(|ui| {
                        icons::show(ui, Icon::for_file(&format!("x.{extension}")), 20.0, theme::tokens(ui).text);
                        ui.label(format!("{} files", extension.to_uppercase()));
                    });
                    let chosen = self.settings.file_apps.get(extension).cloned();
                    let shown = chosen.as_ref().map_or("The system's program".to_owned(), |a| a.name.clone());
                    egui::ComboBox::from_id_salt(("open-with", extension.as_str())).selected_text(shown).width(300.0).show_ui(ui, |ui| {
                        if ui.selectable_label(chosen.is_none(), "The system's program").clicked() {
                            actions.push(Action::SetFileApp { extension: extension.clone(), app: None });
                        }
                        for app in found {
                            if ui.selectable_label(chosen.as_ref() == Some(app), &app.name).clicked() {
                                actions.push(Action::SetFileApp { extension: extension.clone(), app: Some(app.clone()) });
                            }
                        }
                        if let Some(own) = chosen.as_ref().filter(|c| !found.contains(c)) {
                            let _ = ui.selectable_label(true, &own.name);
                        }
                        ui.separator();
                        if ui.selectable_label(false, "Other program…").clicked() {
                            if let Some(file) = rfd::FileDialog::new().set_title(format!("Program for {} files", extension.to_uppercase())).pick_file() {
                                actions.push(Action::SetFileApp { extension: extension.clone(), app: Some(FileApp::from_program(&file)) });
                            }
                        }
                    });
                    ui.end_row();
                }
            });
            note(ui, "Open in the details' production files starts this program with the file, a slicer for STL and 3MF, say. Installed programs that open the type are listed.");
        });
    }

    fn versions(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        card(ui, "Checking in", |ui| {
            let mut auto = self.settings.auto_checkin;
            if ui.checkbox(&mut auto, "Check in automatically when a design is saved").changed() {
                actions.push(Action::SetAutoCheckin(auto));
            }
            note(ui, "For one person: a saved design becomes the next version a few seconds later, new designs too. In a server vault where check-out is required, only designs you have checked out.");
        });
        if let Some(Backend::Local(vault)) = &self.backend {
            card(ui, "Revisions in this vault", |ui| {
                let current = vault.revision_scheme().unwrap_or(vault::RevisionScheme::Letters);
                for (scheme, label) in [
                    (vault::RevisionScheme::Letters, "Letters: A, B, C … (no I, O, Q, S, X, Z)"),
                    (vault::RevisionScheme::Numbers, "Numbers: 01, 02, 03 …"),
                ] {
                    if ui.radio(current == scheme, label).clicked() && current != scheme {
                        actions.push(Action::SetScheme(scheme));
                    }
                }
                note(ui, "Applies to the next release of each design.");
            });
        }
    }

    fn thumbnails(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        card(ui, "Drawn by SolidCraft", |ui| {
            let mut draw = self.settings.draw_thumbnails;
            if ui.checkbox(&mut draw, "Draw thumbnails with SolidCraft, with the edges outlined").changed() {
                actions.push(Action::SetDrawThumbnails(draw));
            }
            let text = match self.renderer.as_ref().map(|r| r.found()) {
                Some(crate::render::Found::Program(p)) => format!("Drawn by {}. Kept in the cache, each version once.", p.display()),
                Some(crate::render::Found::Missing) => {
                    "SolidCraft's command line program (solidcraft-cli) was not found: the pictures saved in the files are shown. Set SCPDM_SOLIDCRAFT_CLI to it, or choose SolidCraft under Opening designs.".to_owned()
                }
                Some(crate::render::Found::NotYet) => "Looked for when the first picture is needed.".to_owned(),
                None => "Off: the pictures saved in the files are shown.".to_owned(),
            };
            note(ui, &text);
            note(ui, "The same program makes the production files and the large pictures for comparing versions.");
        });
    }

    fn appearance(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let themed = !self.settings.theme.is_empty();
        card(ui, "Theme", |ui| {
            ui.horizontal(|ui| {
                for (choice, label) in [(Appearance::System, "As the system"), (Appearance::Dark, "Dark"), (Appearance::Light, "Light")] {
                    if ui.selectable_label(!themed && self.settings.appearance == choice, label).clicked() {
                        actions.push(Action::SetAppearance(choice));
                    }
                }
            });
            note(ui, "SolidCraft's own colours, dark and light.");
        });
        card(ui, "SolidCraft's colour themes", |ui| {
            let themes = super::all_themes(&self.settings);
            let width = ((ui.available_width() - 16.0) / 3.0).max(150.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                for (theme, imported) in &themes {
                    let selected = self.settings.theme == theme.id;
                    let response = theme_tile(ui, theme, selected, width);
                    if response.clicked() {
                        actions.push(Action::SetTheme(theme.id.clone()));
                    }
                    if *imported {
                        response.context_menu(|ui| {
                            if ui.button("Remove this theme").clicked() {
                                actions.push(Action::RemoveTheme(theme.id.clone()));
                            }
                        });
                    }
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Import a theme file…").clicked() {
                    actions.push(Action::ImportTheme);
                }
                note(ui, "A solidcraft-theme/1 file, as SolidCraft exports one. Right-click an imported theme to remove it.");
            });
            note(ui, "The same themes as in SolidCraft 0.20 (Gruvbox, Catppuccin, Nord and more): the PDM looks like SolidCraft in the same theme.");
        });
        card(ui, "Interface size", |ui| {
            ui.horizontal_wrapped(|ui| {
                let current = ui.ctx().zoom_factor();
                for zoom in ZOOM_PRESETS {
                    let selected = (current - zoom).abs() < 0.01;
                    if ui.selectable_label(selected, format!("{:.0} %", zoom * 100.0)).clicked() {
                        ui.ctx().set_zoom_factor(zoom);
                    }
                }
            });
            note(ui, "Also Ctrl + and Ctrl - anywhere, Ctrl 0 to reset.");
        });
    }

    fn updates(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        card(ui, "New versions", |ui| {
            ui.horizontal_wrapped(|ui| {
                let mut on = self.settings.check_updates;
                let automatic = self.install_kind.checks();
                let checkbox = ui.add_enabled(automatic, egui::Checkbox::new(&mut on, "Look for a new version once a day"));
                if checkbox.on_disabled_hover_text("Only the AppImage and the Windows installation look by themselves").changed() {
                    actions.push(Action::SetUpdateChecks(on));
                }
                let checking = self.update_check.is_some();
                if ui.add_enabled(!checking, egui::Button::new("Check now")).clicked() {
                    actions.push(Action::CheckForUpdates);
                }
                if checking {
                    ui.spinner();
                }
            });
            note(ui, &format!("This is version {}. Releases come from gitlab.com/randomlegend/solidcraft-pdm.", env!("CARGO_PKG_VERSION")));
        });
    }
}

/// A theme to pick: its name and four swatches; drawn selected when it is the one in use.
fn theme_tile(ui: &mut egui::Ui, theme: &crate::themes::ThemeFile, selected: bool, width: f32) -> egui::Response {
    let t = theme::tokens(ui);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 44.0), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &theme.name));
    let [panel, bar, accent, text] = theme.swatches();
    let painter = ui.painter();
    let stroke = if selected { egui::Stroke::new(2.0, t.accent) } else if response.hovered() { egui::Stroke::new(1.0, t.accent) } else { egui::Stroke::new(1.0, t.border) };
    painter.rect(rect, egui::CornerRadius::same(theme::RADIUS), panel, stroke, egui::StrokeKind::Inside);
    // The header strip and the accent, as the theme draws them.
    let strip = egui::Rect::from_min_size(rect.min + egui::vec2(2.0, 2.0), egui::vec2(rect.width() - 4.0, 10.0));
    painter.rect_filled(strip, egui::CornerRadius { nw: 4, ne: 4, sw: 0, se: 0 }, bar);
    painter.circle_filled(egui::pos2(rect.right() - 16.0, rect.center().y + 6.0), 6.0, accent);
    painter.text(egui::pos2(rect.left() + 10.0, rect.center().y + 6.0), egui::Align2::LEFT_CENTER, &theme.name, theme::medium(size::BODY), text);
    response.on_hover_text(if theme.dark { "Dark" } else { "Light" })
}

/// A group of settings with its title.
fn card(ui: &mut egui::Ui, title: &str, content: impl FnOnce(&mut egui::Ui)) {
    let t = theme::tokens(ui);
    theme::card(t).inner_margin(egui::Margin::same(16)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(title).font(theme::semibold(size::HEADING)).color(t.text));
        ui.add_space(2.0);
        content(ui);
    });
    ui.add_space(8.0);
}

fn note(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).small().weak());
}
