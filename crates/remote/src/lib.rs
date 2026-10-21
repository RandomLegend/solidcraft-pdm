//! SolidCraft PDM on a server: the API client ([`Client`]) and workspaces ([`Workspace`]), local
//! folders mirroring a server vault where designs are opened, changed and checked in.

pub mod client;
pub mod workspace;

pub use client::{Client, Error, Result};
pub use workspace::{Base, RemoteEntry, RemoteImage, RemoteStatus, Status, WORKSPACE_DIR, Workspace};
