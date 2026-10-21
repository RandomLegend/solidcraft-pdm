# solidcraft-pdm

Product data management for [SolidCraft](https://github.com/tangency-labs/solidcraft) designs:
browse `.solidcraft` files in folders, see their properties and thumbnails, and keep versions.

```sh
cargo run --release -p scpdm-gui -- ~/CAD     # desktop app (opens or offers to create a vault)
cargo run --release -p scpdm -- --help        # command line
```

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

Folders and status filters on the left, designs with thumbnails in the middle, and for the
selected design: properties, links (*Uses* / *Used in*), check-in and history with each
version's thumbnail. The vault is rescanned every few seconds and when the window regains focus,
so saving in SolidCraft shows up as *Modified* by itself. Double-click opens a design in
SolidCraft (`solidcraft` on `PATH`, else `xdg-open`; set your own under Vault > Settings, e.g.
an AppImage path).

## Command line

```text
scpdm init [dir]                 make a folder a vault
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
7. Upgrade egui (0.33 → current), packaging (`.desktop` file, AppImage)
8. Multi-user: check-out / locking, a shared vault
