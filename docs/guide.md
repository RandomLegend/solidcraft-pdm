# SolidCraft PDM guide

## Concepts

- A **vault** is a folder of designs with a hidden `.pdm/` folder. Your files stay ordinary files
  that SolidCraft opens and saves as before.
- **Check in** records a file's current content as its next version (v1, v2, …) with a comment.
- Each design is **Untracked**, **Up to date**, **Modified** (changed since its last version)
  or **Missing** (tracked, but the file is gone).
- **Restore** puts an earlier version back. It never loses work: changes not checked in yet are
  checked in first, and the restore itself is recorded as a new version.
- **Lifecycle:** a tracked design is *in progress*, *released* or *obsolete*. *Release* gives it
  a revision (A, B, C … without I, O, Q, S, X, Z, or 01, 02 … per vault; a design never released
  here proposes the revision its file already has), writes it into the design's Revision
  property and records which version that revision is. Checking in a released design again
  starts work on the next revision. Released files are not made read-only (SolidCraft saves by
  renaming a new file over the old one): a released design that changed is shown as such.
- **Edit properties…** writes name, part number, revision, project, description, author,
  organization and licence into the design file (SolidCraft's *Design Properties*), and checks
  that in if you like.
- **Move / rename** inside the PDM keeps the history, and updates the links: in designs that
  link to the moved one, and the moved design's own relative links. An updated design that was
  up to date gets a new version saying why; one with changes not checked in yet keeps them and
  gets no version of its own. A file moved in the file manager is recognised by its content and
  can be relinked to its history.

## The window

- **Top:** *+ New design* creates an empty design (name, folder, optional part number and
  description), checks it in as v1 and opens it in SolidCraft. *View* chooses the list columns
  (also by right-clicking the list header) and the interface size (also Ctrl + / Ctrl - /
  Ctrl 0; more presets in Vault > Settings). The interface starts at 150 %.
- **Left:** status and lifecycle filters, and the folder tree (also empty folders).
  *+ New folder* creates one in the selected folder; right-click a folder for a subfolder,
  *Show in file manager* or *Delete* (empty folders only). Drag designs onto a folder, or use
  *Move to* in a design's right-click menu, to move them with their history.
- **Middle:** designs, and in folder views also the folder's reference images (PNG, JPEG,
  WebP): shown with a thumbnail, opened in your image viewer on double-click, moved like
  designs, but not versioned. Right-click a row, or below the list, for *New design here…*.
  The search bar (Ctrl+F, Esc clears) searches the whole vault, whatever folder is selected:
  fuzzy and ranked, each word matched within the file name, design name, part number,
  description, project or folder, with the matched letters highlighted. The slider sets the
  thumbnail size.
- **Right:** the selected design's lifecycle, properties, links (*Uses* / *Used in*), check-in
  and history with each version's thumbnail.
- **Compare…** on a version in the history shows what changed against the working file (or
  between any two versions): properties, parameters, features (added, removed, renamed,
  suppressed, which settings), components, links, and which other parts of the design differ.
- **Double-click** a design, its preview or the working version in the history to open it in
  SolidCraft; an older version opens as a read-only copy (to work on it, restore it first).

The vault is rescanned every few seconds while the window has focus, and when it regains focus,
so saving in SolidCraft shows up as *Modified* by itself.

## Opening designs

Double-click opens a design in SolidCraft as installed: the app finds SolidCraft's desktop entry
(the one the AppImage installs on its first run, or one from Gear Lever, AppImageLauncher,
appimaged, Flatpak or a package) and runs its `Exec` line, else `solidcraft` on `PATH`. It does
not use `xdg-open`: the AppImage registers no MIME type for `*.solidcraft`, so `xdg-open`
guesses from the contents (ZIP or JSON) and may pick an unrelated program. Vault > Settings shows
what was found and takes another program or AppImage. On Windows see [windows.md](windows.md).

## New designs

New designs are written as plain JSON in format 1, which every SolidCraft release opens and
upgrades (checked with SolidCraft 0.18.3: it saves such a file as a `solidcraft/29` container
and keeps its properties).

## Deleting designs

Right-click a design (or *···* in its details) > *Delete…*. In a vault of your own it goes into
*Recently deleted* (in the list on the left) with all its versions, for 30 days; *Restore*
brings it back where it was (or beside a design that took its name meanwhile). Changes not
checked in yet are checked in first, so what comes back is what you deleted. In a server vault
a design is deleted for everyone; the server keeps it for a while, and an administrator can
restore it ([server.md](server.md#deleted-designs)). The confirmation says when other designs
use the one you delete: their links to it break.

## Checking in automatically

For one person, checking in by hand is often one step too many. *Vault > Settings > Versions >
Check in automatically*: a design you saved in SolidCraft becomes the next version a few
seconds later (the comment says it was checked in automatically), new designs in the vault
too, also while the PDM is in the background. In a server vault this happens where check-out
is optional (see [server.md](server.md)), and for designs you have checked out, which stay
checked out.

## Thumbnails

SolidCraft saves a small picture in each design. The PDM draws better ones with SolidCraft
itself (`solidcraft-cli`), shaded with the model's edges outlined, as on SolidCraft's start
page, while the saved picture is shown until then. It finds SolidCraft's command line program
on `PATH`, next to SolidCraft, or in SolidCraft's AppImage (taken out once, into the cache);
`SCPDM_SOLIDCRAFT_CLI` names it otherwise. Each version is drawn once, about a second each,
and kept in `~/.cache/solidcraft-pdm/thumbnails` (Windows: `%LOCALAPPDATA%\SolidCraft PDM`). A
design that this SolidCraft cannot read (a newer format) keeps its saved picture.
*Settings > Thumbnails* switches the drawing off.

## Updates

The AppImage and the Windows installation look for a new release once a day (asking
gitlab.com, nothing else is sent). A newer one shows as *⬆ Update to …* in the menu bar, with
its notes:

- *Install and restart*: downloads it, checks it against the release's `SHA256SUMS` (a broken
  or cut-off download changes nothing), puts it in place of the running AppImage (the version
  in the file name changes with it, and the menu entry follows) and starts it. On Windows the
  installer runs and starts the app again.
- *Release page*, *Skip this version* (not pointed out again), *Later*.

*Vault > Settings > Updates* switches the daily look off, or checks now. A build from source
does not look by itself; *Check now* still tells you about new releases.

## Working with a team

A vault can also live on a server that a small team shares: see [server.md](server.md).
