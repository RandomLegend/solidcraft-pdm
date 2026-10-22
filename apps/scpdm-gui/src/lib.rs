//! SolidCraft PDM desktop app (the window is [`app::App`]).

pub mod app;
mod backend;
mod bom;
mod export;
pub mod icons;
mod launcher;
mod render;
#[cfg(not(windows))]
mod menu_entry;
mod scan;
mod search;
pub mod theme;
mod update;
