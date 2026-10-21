# Changelog

The notes of each version become its GitLab release notes: the release job takes the section
whose heading starts with `## <version>`.

## Unreleased

### Team server

- `scpdm-server`: shared vaults for a small team, with users, logins and read / write / admin
  access per vault. See [docs/server.md](docs/server.md).
- Workspaces: a local folder per server vault, where SolidCraft opens and saves the designs.
- Get, check out (one person at a time), check in, undo check-out; a change made without
  checking out, or on an old version, is caught and never overwritten.
- Moves update links on the server; other workspaces follow. Releases, revisions, obsolete,
  properties, folders and reference images are shared.
- The app: *Vault > Connect to a server…*, server statuses and who has a design checked out.
- The command line: `scpdm remote …`.

## 0.1.0 — 2026-10-21

The first release: a PDM for SolidCraft designs, for one person, native on Linux.

### Vault and versions

- A vault is any folder of SolidCraft designs: versions live in a hidden `.pdm` folder, your
  files stay where they are and open in SolidCraft as before.
- Check in, history with a thumbnail per version, restore (never loses unsaved work), save a
  copy of any version, open an old version read-only.
- Compare two versions: properties, parameters, features, components, links.
- Lifecycle: in progress, released, obsolete. Releasing writes the revision (A, B, C … or 01,
  02 …) into the design and records which version it is.
- Move and rename keep the history and update the links between designs.
- Edit design properties (part number, description, …) right in the file.

### Desktop app

- Folder tree with drag and drop, new folders and new designs.
- Big thumbnails, fuzzy search through the whole vault, choosable columns.
- Reference images (PNG, JPEG, WebP) shown next to the designs in their folder.
- Opens designs in SolidCraft as installed (also as an AppImage), interface size up to 250 %.
- *Add SolidCraft PDM to the application menu* for the AppImage, with its icon.

### Command line

- `scpdm`: `init`, `status`, `ci`, `log`, `diff`, `restore`, `export`, `mv`, `release`,
  `revise`, `obsolete`, `props`, `new`, and more (`scpdm --help`); in the AppImage:
  `SolidCraft-PDM-0.1.0-x86_64.AppImage cli <command>`.

### Downloads

- Linux: `SolidCraft-PDM-0.1.0-x86_64.AppImage` (x86_64, glibc 2.36 or newer). Make it
  executable and start it.
- Windows: not built for this release (the Windows build is started by hand).
