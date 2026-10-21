# solidcraft-pdm

Product data management for [SolidCraft](https://github.com/tangency-labs/solidcraft) designs:
browse `.solidcraft` files in folders, see their properties and thumbnails, and keep versions.

```sh
cargo run --release -p scpdm-gui -- ~/CAD     # desktop app (opens or offers to create a vault)
cargo run --release -p scpdm -- --help        # command line
```

## Download

CI on gitlab.com builds these (pipeline → job → *Browse* / *Download* artifacts):

- **Linux:** `SolidCraft-PDM-<version>-x86_64.AppImage` (job `linux:appimage`; glibc 2.36 or
  newer). Make it executable and run it; `SolidCraft-PDM-….AppImage cli status` runs the command
  line. Build one locally with `packaging/linux/build-appimage.sh`.
- **Windows:** `solidcraft-pdm.exe` and `scpdm.exe` (job `windows:build`, started by hand); see
  `docs/windows.md`.

## Concepts

- A **vault** is a folder of designs with a hidden `.pdm/` folder. Your files stay ordinary files
  that SolidCraft opens and saves as before.
- **Check in** records a file's current content as its next version (v1, v2, …) with a comment.
- Each design is **Untracked**, **Up to date**, **Modified** (changed since its last version)
  or **Missing** (tracked, but the file is gone).
- **Restore** puts an earlier version back. It never loses work: changes not checked in yet are
  checked in first, and the restore itself is recorded as a new version.
- **Move / rename** inside the PDM keeps the history. A file moved in the file manager is
  recognised by its content and can be relinked to its history.

## Desktop app

- **Left:** status filters and the folder tree (also empty folders). *+ New folder* creates one in
  the selected folder; right-click a folder for a subfolder, *Show in file manager* or *Delete*
  (empty folders only). Drag designs onto a folder, or use *Move to* in a design's right-click
  menu, to move them with their history.
- **Middle:** designs, and in folder views also the folder's reference images (PNG, JPEG,
  WebP): shown with a thumbnail, opened in your image viewer on double-click, moved like
  designs, but not versioned. Right-click a row, or below the list, for *New design here…*.
  The search bar (Ctrl+F, Esc clears) searches the whole vault, whatever folder is
  selected: fuzzy and ranked, each word matched within the file name, design name, part number,
  description, project or folder, with the matched letters highlighted. The slider sets the
  thumbnail size.
- **Right:** the selected design's properties, links (*Uses* / *Used in*), check-in and history
  with each version's thumbnail.

- **Top:** *+ New design* creates an empty design (name, folder, optional part number and
  description), checks it in as v1 and opens it in SolidCraft. *View* chooses the list columns
  (also by right-clicking the list header) and the interface size (also Ctrl + / Ctrl - /
  Ctrl 0; more presets in Vault > Settings). The interface starts at 150 %.
- **Double-click** a design, its preview or the working version in the history to open it in
  SolidCraft; an older version opens as a read-only copy (to work on it, restore it first).

The vault is rescanned every few seconds while the window has focus, and when it regains focus,
so saving in SolidCraft shows up as *Modified* by itself.

New designs are written as plain JSON in format 1, which every SolidCraft release opens and
upgrades (checked with SolidCraft 0.18.3: it saves such a file as a `solidcraft/29` container
and keeps its properties).

**Opening designs.** Double-click opens a design in SolidCraft as installed: the app finds
SolidCraft's desktop entry (the one the AppImage installs on its first run, or one from Gear
Lever, AppImageLauncher, appimaged, Flatpak or a package) and runs its `Exec` line, else
`solidcraft` on `PATH`. It does not use `xdg-open`: the AppImage registers no MIME type for
`*.solidcraft`, so `xdg-open` guesses from the contents (ZIP or JSON) and may pick an unrelated
program. Vault > Settings shows what was found and takes another program or AppImage.

## Command line

```text
scpdm init [dir]                 make a folder a vault
scpdm new <file> [--part-number P] [--description D]   create an empty design as v1
scpdm status                     untracked / modified / missing designs
scpdm ci <files…> -m "comment"   check in
scpdm log <file>                 versions, newest first
scpdm restore <file> <n>         put version n back
scpdm export <file> <n> <out>    save a copy of version n
scpdm mv <from> <to>             move or rename, keeping history
scpdm relink <missing> <to>      attach history to a file moved outside the PDM
scpdm ls [dir] / info <file> / thumb <file> <out.png>
```

## Layout

| Path | What |
|---|---|
| `crates/scformat` | Reads `.solidcraft` files (ZIP container or plain JSON): properties, components, links, thumbnail |
| `crates/vault` | The vault: SQLite index (`.pdm/vault.db`) and content-addressed store (`.pdm/objects/`) |
| `apps/scpdm` | Command-line tool |
| `apps/scpdm-gui` | Desktop app (egui), binary `solidcraft-pdm` |
| `docs/windows.md` | Building for Windows 10 / 11, and what differs there |
| `packaging/linux/` | AppImage: build script, `AppRun`, desktop entry (icon in `assets/`) |
| `.gitlab-ci.yml` | Tests on Linux; AppImage and Windows builds |

Tests: `cargo test --workspace`. The GUI test drives the real window over a demo vault;
`SCPDM_SCREENSHOTS=<dir> cargo test -p scpdm-gui --test ui` also saves screenshots there.

## Design decisions

- **Own reader, no dependency on SolidCraft's crates.** They need a newer Rust, pull in the
  geometry kernel, and would tie the PDM to one SolidCraft release. The parts a PDM needs are
  documented and stable (`docs/design-container.md`, `docs/book/src/guide/files.md` upstream):
  - container: ZIP with `container.json`, `document.json`, `blobs/<sha256>.<ext>`, `cache/thumbnail.png`
  - `metadata`: `project`, `part_number`, `revision`, `author`, `organization`, `description`, `license`
  - `links`: component id → `{path, feature}`, the path relative to the design's folder
- **Read-only towards design files.** SolidCraft lets other tools store data under their own
  reverse-DNS `metadata` key and promises to keep it: the hook for writing PDM data into files
  later, as a deliberate step.
- **Versions are whole-file copies, stored once per distinct content** (SHA-256, like SolidCraft's
  own blobs) and verified on read. Objects are read-only on disk.
- **Single user.** No check-out or locking yet; SQLite runs in WAL mode so the app and the CLI
  can be used side by side.
- Fixtures in `crates/scformat/tests/fixtures` come from the SolidCraft repository
  (MIT/Apache-2.0), commit `728f753`.

## Roadmap

1. ~~Read design files~~
2. ~~Vault, versions, restore, move/relink~~
3. ~~Desktop GUI~~
4. Update links in other designs when a linked file is moved or renamed
5. Compare two versions (properties, parameters, features)
6. Lifecycle states (WIP → released) and a revision scheme; edit design properties
7. Upgrade egui (0.33 → current), packaging (`.desktop` file, AppImage; Windows: first build, icon, installer, CI — see `docs/windows.md`)
8. Multi-user: check-out / locking, a shared vault
