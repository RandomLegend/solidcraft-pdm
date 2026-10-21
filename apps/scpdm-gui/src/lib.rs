//! SolidCraft PDM desktop app (the window is [`app::App`]).

pub mod app;
mod backend;
pub mod icons;
mod launcher;
#[cfg(not(windows))]
mod menu_entry;
mod scan;
mod search;
pub mod theme;
mod update;
