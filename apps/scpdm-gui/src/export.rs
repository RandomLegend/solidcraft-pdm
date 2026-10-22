//! Production files from a release: STEP, STL and 3MF, written by SolidCraft's command line
//! program (`solidcraft-cli run --in design --out part.step --out part.stl …`) and stored with
//! the release, so what went to the workshop stays with the revision it was made from.
//!
//! The released version is written next to the design as a hidden file (the scans skip those),
//! so an assembly's relative links lead to the same parts as from the design itself; the files
//! are written into a temporary folder and read back.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use eframe::egui;

/// The formats SolidCraft writes, by file extension, with what they are for.
pub const FORMATS: [(&str, &str); 3] = [("step", "STEP: for CAD and CNC"), ("stl", "STL: for 3D printing"), ("3mf", "3MF: for 3D printing, with units")];

/// An assembly with many parts takes a while; longer than this is given up.
const TIMEOUT: Duration = Duration::from_secs(300);

/// A release to make production files of.
pub struct Request {
    pub path: String,
    pub version: u32,
    pub revision: String,
    /// The released version's content.
    pub bytes: Vec<u8>,
    /// Where the design is (its links are relative to that folder).
    pub file: PathBuf,
    /// Extensions from [`FORMATS`].
    pub formats: Vec<String>,
}

pub struct Finished {
    pub path: String,
    pub version: u32,
    pub revision: String,
    /// The files, by name, or why they could not be made.
    pub result: Result<Vec<(String, Vec<u8>)>, String>,
}

/// Makes the files on a worker thread; `cli`: SolidCraft's program, looked for there when
/// `None` (see [`crate::render::find_cli`]).
pub fn start(ctx: &egui::Context, request: Request, cli: Option<PathBuf>, solidcraft: Option<PathBuf>) -> Receiver<Finished> {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let cli = cli.or_else(|| crate::render::cache_root().and_then(|root| crate::render::find_cli(solidcraft.as_deref(), &root)));
        let result = match cli {
            Some(cli) => make(&cli, &request),
            None => Err("SolidCraft's command line program (solidcraft-cli) was not found. Set SCPDM_SOLIDCRAFT_CLI to it, or choose SolidCraft in the settings.".to_owned()),
        };
        let _ = tx.send(Finished { path: request.path, version: request.version, revision: request.revision, result });
        ctx.request_repaint();
    });
    rx
}

/// `pi4-case-A.step`: the design's name, the revision, the format.
pub fn file_name(path: &str, revision: &str, extension: &str) -> String {
    let name = path.rsplit_once('/').map_or(path, |(_, n)| n);
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    let revision: String = revision.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    if revision.is_empty() { format!("{stem}.{extension}") } else { format!("{stem}-{revision}.{extension}") }
}

fn make(cli: &Path, request: &Request) -> Result<Vec<(String, Vec<u8>)>, String> {
    if request.formats.is_empty() {
        return Ok(Vec::new());
    }
    let folder = request.file.parent().ok_or("the design has no folder")?;
    // Unique, also for the same content made twice at once (two windows, say).
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let id = format!("{}-{}-{nanos}", std::process::id(), COUNT.fetch_add(1, Ordering::Relaxed));
    let design = folder.join(format!(".scpdm-export-{id}.solidcraft"));
    let out = std::env::temp_dir().join(format!("scpdm-export-{id}"));
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let result = (|| {
        std::fs::write(&design, &request.bytes).map_err(|e| format!("{}: {e}", design.display()))?;
        let names: Vec<String> = request.formats.iter().map(|ext| file_name(&request.path, &request.revision, ext)).collect();
        let mut command = Command::new(cli);
        command.arg("run").arg("--in").arg(&design);
        for name in &names {
            command.arg("--out").arg(out.join(name));
        }
        command.arg("--quiet");
        run(&mut command, &out.join("errors.txt"))?;
        names
            .into_iter()
            .map(|name| match std::fs::read(out.join(&name)) {
                Ok(bytes) if !bytes.is_empty() => Ok((name, bytes)),
                _ => Err(format!("SolidCraft did not write {name}")),
            })
            .collect()
    })();
    let _ = std::fs::remove_file(&design);
    let _ = std::fs::remove_dir_all(&out);
    result
}

/// Runs the program to its end (at most [`TIMEOUT`]); on failure, what it said.
fn run(command: &mut Command, errors: &Path) -> Result<(), String> {
    let log = std::fs::File::create(errors).map_err(|e| e.to_string())?;
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(log);
    // A program written a moment ago can be "busy" while another thread starts a process.
    let mut tries = 0;
    let mut child = loop {
        match command.spawn() {
            Ok(child) => break child,
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 20 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("could not start SolidCraft's command line program: {e}")),
        }
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => {
                let said = std::fs::read_to_string(errors).unwrap_or_default();
                let last: Vec<&str> = said.lines().filter(|l| !l.trim().is_empty()).collect();
                let tail = last[last.len().saturating_sub(3)..].join(" ");
                return Err(if tail.is_empty() { "SolidCraft could not export the design".to_owned() } else { format!("SolidCraft: {tail}") });
            }
            Ok(None) if start.elapsed() < TIMEOUT => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("SolidCraft took longer than {} minutes", TIMEOUT.as_secs() / 60));
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A stand-in for solidcraft-cli: writes "<extension> of <design>" to each --out, unless the
    /// design says "bad".
    fn fake_cli(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            r#"#!/bin/sh
[ "$1" = run ] || exit 2
shift
while [ $# -gt 0 ]; do case "$1" in --in) shift; in="$1";; --out) shift; outs="$outs $1";; esac; shift; done
grep -q bad "$in" && { echo "error: feature 3 failed" >&2; exit 1; }
for o in $outs; do printf '%s of %s' "${o##*.}" "$(cat "$in")" > "$o"; done
"#,
        )
        .unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn request(dir: &Path, bytes: &[u8]) -> Request {
        Request {
            path: "parts/lid.solidcraft".into(),
            version: 3,
            revision: "B".into(),
            bytes: bytes.to_vec(),
            file: dir.join("parts/lid.solidcraft"),
            formats: vec!["step".into(), "stl".into()],
        }
    }

    /// With SolidCraft's own program, when `SCPDM_SOLIDCRAFT_CLI` names it.
    #[test]
    fn exports_with_solidcraft() {
        let Some(cli) = std::env::var_os("SCPDM_SOLIDCRAFT_CLI").map(PathBuf::from) else { return };
        let dir = tempfile::tempdir().unwrap();
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/../../crates/scformat/tests/fixtures/json-pi-case.solidcraft");
        let mut request = request(dir.path(), &std::fs::read(fixture).unwrap());
        std::fs::create_dir_all(dir.path().join("parts")).unwrap();
        request.formats = FORMATS.iter().map(|(ext, _)| (*ext).to_owned()).collect();
        let files = make(&cli, &request).unwrap();
        assert_eq!(files.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["lid-B.step", "lid-B.stl", "lid-B.3mf"]);
        assert!(files[0].1.starts_with(b"ISO-10303-21;"));
    }

    #[test]
    fn names_the_files_after_the_design_and_revision() {
        assert_eq!(file_name("parts/lid.solidcraft", "B", "step"), "lid-B.step");
        assert_eq!(file_name("lid.solidcraft", "a/b", "3mf"), "lid-ab.3mf");
        assert_eq!(file_name("lid.solidcraft", "", "stl"), "lid.stl");
    }

    #[test]
    fn exports_the_released_content_next_to_the_design() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("parts")).unwrap();
        let cli = dir.path().join("bin/solidcraft-cli");
        fake_cli(&cli);
        let files = make(&cli, &request(dir.path(), b"released lid")).unwrap();
        assert_eq!(files, [("lid-B.step".to_owned(), b"step of released lid".to_vec()), ("lid-B.stl".to_owned(), b"stl of released lid".to_vec())]);
        assert_eq!(std::fs::read_dir(dir.path().join("parts")).unwrap().count(), 0, "the hidden copy is gone");

        let error = make(&cli, &request(dir.path(), b"a bad lid")).unwrap_err();
        assert_eq!(error, "SolidCraft: error: feature 3 failed");
    }
}
