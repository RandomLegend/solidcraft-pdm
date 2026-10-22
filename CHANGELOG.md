# Changelog

The notes of each version become its GitLab release notes: the release job takes the section
whose heading starts with `## <version>`.

## Unreleased

### Windows

- Every release has a Windows installer (`SolidCraft-PDM-<version>-setup.exe`), built on
  Linux; the installed app updates itself from the following release on. Not tried on a real
  Windows machine yet: see [docs/windows.md](docs/windows.md).

### App

- Designs can be deleted (right-click, or *···* in the details). In a vault of your own they
  wait in *Recently deleted* for 30 days, with all their versions, and can be restored.
- Right-clicking anywhere in an empty folder's list offers a new design or folder there.
- Vaults use schema 4 (deleted designs): once opened by this version, older versions of the app
  cannot open them.

### Server

- Deleting a design deletes it for everyone (unchanged copies in workspaces go too); the server
  keeps it 7 days (adjustable) on the administration page's new *Deleted* page, where an
  administrator restores it or deletes it for good. An hourly clean-up removes what is older.

## 0.2.3 — 2026-10-22

### App

- Thumbnails drawn by SolidCraft itself, with the edges outlined as on SolidCraft's start page
  (`solidcraft-cli`, found on `PATH`, next to SolidCraft or in its AppImage), each version once,
  kept in the cache; the picture saved in the file is the fallback. *Settings > Thumbnails*.
- *Settings > Versions > Check in automatically*: saved designs become versions by themselves.
- Server vaults without check-outs: a saved design shows as *Changed* and is checked in directly.

### Server

- Check-out can be made optional per vault (*Vaults* on the administration page, or
  `scpdm-server vault checkout NAME optional`), for a vault one person uses.
- Docker: `PUID` and `PGID` run the server as another user, such as the owner of a backup
  folder on a share. A backup folder the server may not write to now says which user needs
  access.

## 0.2.2 — 2026-10-21

### App

- A new look, the one of SolidCraft itself: its colours (dark, or light; *Settings >
  Appearance*), IBM Plex Sans and Mono, its icons. A dark app bar with the open vault and the
  server connection, a toolbar with the search, a navigation column, a cleaner list with
  status dots and release pills, the details as cards with one primary action, a start page.

### Server

- Automatic backups of users and every vault to another folder: a second disk, or an SMB or
  NFS share. Each backup is a complete data folder the server runs from as it is; unchanged
  version files are hard-linked to the previous backup where the folder allows it. A schedule
  (every hour to every week), how many to keep and for how long, on the administration page's
  new *Backups* page, with the runs, their results and the backups in the folder.
- `--backup-dir` / `SCPDM_BACKUP_DIR` fixes the folder (and switches backups on); in Docker the
  `/backups` volume. `scpdm-server backup` makes one from a script.
- See [Backups](docs/server.md#backups) in the server guide, also for restoring.

## 0.2.1 — 2026-10-21

### Server

- An administration page in the browser at the server's address: a dashboard (users online,
  vaults, designs, versions, check-outs, disk use, check-ins per day, recent activity),
  managing users and their access to vaults, adding vaults, breaking a check-out.

### App

- In a workspace, the filters and the status bar say *Not downloaded* and *Not on the server*
  instead of *Missing* and *Untracked*.

### Builds

- The AppImage is built on `main` only when a push changes the app (by hand otherwise).

## 0.2.0 — 2026-10-21

Shared vaults for a small team, with an optional server, and updates for the app.

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
- Runs with Docker: the image `registry.gitlab.com/randomlegend/solidcraft-pdm/server` and a
  `compose.yml` (with optional HTTPS through Caddy), or as a program of its own
  (`scpdm-server-0.2.0-x86_64-linux`).

### Updates

- The AppImage (and the Windows installation) looks for a new release once a day, shows what
  is new and, if you want, installs it and restarts. Downloads are checked against the
  release's `SHA256SUMS`. *Settings > Updates* switches it off or checks now.

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
