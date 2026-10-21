# Building for Windows 10 / 11

Status: the code is prepared for Windows, but it has **not been compiled or run on Windows
yet**. The development machine has no Windows toolchain. The first build on Windows is the test.

## Build (on Windows)

1. Install Rust with `rustup-init.exe` from <https://rustup.rs> (the default
   `x86_64-pc-windows-msvc` toolchain).
2. Install the *Visual Studio Build Tools* with the "Desktop development with C++" workload.
   It provides the linker, and the C compiler for the bundled SQLite (`rusqlite`'s `bundled`
   feature).
3. In the project folder:

   ```powershell
   cargo build --release -p scpdm-gui -p scpdm
   cargo test --workspace
   ```

   The app is `target\release\solidcraft-pdm.exe`, the command line `target\release\scpdm.exe`.

The tests that use a shell script as a stand-in for SolidCraft are Unix-only and are skipped.
The screenshot mode of the UI tests (`SCPDM_SCREENSHOTS`) needs a GPU adapter.

## CI

`.gitlab-ci.yml` builds and tests on gitlab.com's Windows runners (`saas-windows-medium-amd64`).
It runs automatically on the default branch and on tags; on other branches you start it by hand
in the pipeline view. The job installs Rust 1.93.1 with rustup. Its artifact
`solidcraft-pdm-windows-<commit>` holds `solidcraft-pdm.exe` and `scpdm.exe` (kept 30 days).
The Linux job runs the tests on every push.

## What is Windows-specific

| Area | Windows behaviour | Where |
|---|---|---|
| Opening designs | `solidcraft.exe` from `Program Files\SolidCraft` (or `%LOCALAPPDATA%\Programs\SolidCraft`), where SolidCraft's installer puts it; otherwise the `.solidcraft` file association, which the installer also registers | `apps/scpdm-gui/src/launcher.rs` |
| Opening images | the file association (`explorer.exe <file>`) | `launcher::open_with_system` |
| Show in folder | `explorer.exe /select,"<file>"` | `launcher::show_in_folder` |
| Console window | none (`windows_subsystem = "windows"`) | `apps/scpdm-gui/src/main.rs` |
| Paths | resolved with `dunce`, so they are `C:\…` and not `\\?\C:\…` | `crates/vault/src/lib.rs` |
| Typed paths | `folder\name` is accepted and stored as `folder/name` | `typed_path` in `app.rs` |
| Settings | eframe's storage in `%APPDATA%` | eframe |

Inside the vault, paths are always `/`-separated (`.pdm/vault.db` stays the same on both
systems). So a vault folder can be shared between Linux and Windows, as long as only one
machine uses it at a time.

## Known differences and open points

- **Read-only copies of old versions**: on Linux their folder is made read-only, so SolidCraft
  cannot save over them. Windows ignores the read-only flag on folders, so a copy could be saved
  there. It is still a temporary copy outside the vault: the vault and the working file are
  never changed.
- **Files open in SolidCraft**: Windows can refuse to replace a file another program holds open.
  A restore then fails with an error message (nothing is lost). Close the design first.
- **Letter case**: Windows paths ignore case, the vault does not. Renaming `Bracket` to
  `bracket` through the PDM works; doing it in Explorer makes the vault see a missing file and an
  untracked one, and *Relink* puts the history back.
- **Graphics**: the app draws with OpenGL (`glow`). Very old drivers or a VM without 3D support
  may fail to open the window. eframe's `wgpu` backend (Direct3D 12) is the alternative if
  that happens.
- **Still to do**: an `.exe` icon and version info (`winres`), an installer (MSI with
  `cargo-wix`, or Inno Setup), and signing the `.exe` (Windows SmartScreen warns about unsigned downloads).
