# The team server

SolidCraft PDM works on its own, for one person. For a small team, a **server** can hold
shared vaults: everyone works in a folder of their own (a *workspace*), and a design is
changed by one person at a time, who has it *checked out*.

Status: new in 0.2 (not yet released). The server runs on Linux; a Docker image and a Windows
build come later. It is meant for a trusted team on one network or a VPN.

## How it works

- The server keeps each vault as an ordinary vault (the same history, thumbnails, releases and
  links as on your own machine). Its files are always the latest versions.
- Your **workspace** is a normal folder with the vault's folder structure, say `~/PDM/workshop`.
  SolidCraft opens and saves the designs there, so links between assemblies and parts work as
  usual. A hidden `.pdm-workspace` folder records which version of each file you fetched, and
  your login.
- **Get** fetches a design (or everything new and newer). Designs you have not checked out are
  read-only, as a reminder.
- **Check out** locks a design on the server: nobody else can check it in, release it or change
  its properties until you check it in or undo the check-out. CAD files cannot be merged, so
  this is the only way two people do not overwrite each other's work.
- **Check in** uploads your saved file as the next version, and gives the check-out back (or
  keeps it, *Check in, keep checked out*). It is refused if the server has a newer version
  than the one you started from.
- **Nothing local is overwritten.** A design you changed without checking it out shows as
  *Changed, not checked out*. If someone else checks in meanwhile, it shows as *Conflict*. Get
  refuses to replace it; check it out, or keep a copy and get the latest.
- **Moves and renames** run on the server, which updates the links in the designs that point to
  the moved file. That needs those designs to be checked in. Other workspaces follow the move
  on their next refresh.
- **Releases, revisions, obsolete, properties, folders and reference images** are changed on
  the server, so everyone sees the same state.

Statuses in the app: *On the server* (not fetched), *Up to date*, *Newer on server*, *Checked
out*, *Checked out · changed*, *Checked out by anna*, *Changed, not checked out*, *Conflict*,
*Not on the server* (a new file of yours: *Add to the server*), *Removed on the server*.

## Running the server

Build it (Rust 1.93 or newer):

```sh
cargo build --release -p scpdm-server     # target/release/scpdm-server
```

Set it up with a data folder (`--data DIR`, or `$SCPDM_SERVER_DATA`; default
`./pdm-server-data`):

```sh
export SCPDM_SERVER_DATA=/srv/pdm
scpdm-server user add anna              # asks for the password
scpdm-server user add ben
scpdm-server user add root --admin      # every vault; may break others' check-outs
scpdm-server vault add workshop
scpdm-server grant anna workshop write  # read | write | admin
scpdm-server grant ben workshop write
scpdm-server serve --listen 0.0.0.0:7380 --name "Workshop server"
```

`user password NAME`, `user remove NAME`, `user list`, `vault list` and `revoke USER VAULT`
manage them later; `--password-stdin` reads a password from standard input, for scripts. The
settings of `serve` can also come from `$SCPDM_SERVER_LISTEN` and `$SCPDM_SERVER_NAME`.

The data folder holds `server.db` (users, sessions, access) and `vaults/<name>/`, one vault
each. Back it up as a whole while the server is stopped (or with a file-system snapshot).
Logins last 30 days; passwords are stored as Argon2 hashes, sessions as hashes of their
tokens.

**HTTPS:** the server speaks plain HTTP. On your own network or a VPN that is fine; otherwise
put a reverse proxy with TLS in front of it (for example Caddy:
`caddy reverse-proxy --from pdm.example.com --to localhost:7380`). The app warns when a
password would travel over `http://` to another machine.

## In the app

*Vault > Connect to a server…*: the address (`http://server:7380`), your user name and
password. Pick a vault and the folder for its workspace (`~/PDM/<vault>` by default), then
*Open the vault*. Next time, open the workspace folder like any vault; the status bar shows
the server and your user. If the login has expired, the app asks for your password again;
without a connection the status bar says *Offline* and why.

The details panel has *Get*, *Check out*, *Check in* (with a comment), *Undo check-out* and
*Undo and discard changes*. The toolbar's *Get all new and newer* fetches everything that
changed.

## On the command line

```text
scpdm remote vaults <url> <user>                 the vaults you may use
scpdm remote join <url> <user> <vault> <dir>     make a folder a workspace
scpdm remote status                              designs and how they relate to the server
scpdm remote get <files…> | --all                fetch the latest versions
scpdm remote checkout <file>                     check out
scpdm remote ci <file> -m "comment" [--keep]     check in (--keep: stay checked out)
scpdm remote undo <file> [--discard]             give the check-out back
scpdm remote add <file> -m "comment"             put a new design onto the server
scpdm remote mv <from> <to>                      move or rename (links are updated)
scpdm remote release <file> --revision R [-m note]   release
scpdm remote log <file>                          versions on the server
scpdm remote login                               log in again
```

They work inside a workspace. The password is asked for, or read from `$SCPDM_PASSWORD`.
