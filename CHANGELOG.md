# Changelog

The notes of each version become its GitLab release notes: the release job takes the section
whose heading starts with `## <version>`.

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
