//! `scpdm`: command-line PDM for SolidCraft designs.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::{Local, TimeZone};
use clap::{Parser, Subcommand};
use scformat::{DesignInfo, Encoding};
use vault::{Status, Vault};

#[derive(Parser)]
#[command(name = "scpdm", about = "Manage SolidCraft designs: properties, versions, history")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show a design's properties, contents and links
    Info { file: PathBuf },
    /// List every design below a folder (no vault needed)
    Ls { dir: Option<PathBuf> },
    /// Save a design's thumbnail as PNG
    Thumb { file: PathBuf, out: PathBuf },
    /// Make a folder a vault
    Init { dir: Option<PathBuf> },
    /// Show which designs are untracked, modified or missing
    Status,
    /// Create an empty design (checked in as v1)
    New {
        file: PathBuf,
        #[arg(long)]
        part_number: Option<String>,
        #[arg(long)]
        description: Option<String>,
    },
    /// Record files' current content as new versions
    #[command(visible_alias = "ci")]
    Checkin {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// What changed
        #[arg(short, long, default_value = "")]
        message: String,
    },
    /// Show a design's versions, newest first
    Log { file: PathBuf },
    /// Release a design as its next revision (writes the revision into it and checks it in)
    Release {
        file: PathBuf,
        #[arg(short, long, default_value = "")]
        message: String,
        /// The revision (default: the one the design already has if never released, else the next)
        #[arg(long)]
        revision: Option<String>,
    },
    /// Take a released design back to work in progress
    Revise { file: PathBuf },
    /// Mark a design obsolete (--undo: back to work in progress)
    Obsolete {
        file: PathBuf,
        #[arg(long)]
        undo: bool,
    },
    /// Show a design's properties, or change them (--set part_number=P-1; empty value removes)
    Props {
        file: PathBuf,
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
        #[arg(long)]
        name: Option<String>,
        /// Check the change in as a new version
        #[arg(long)]
        check_in: bool,
    },
    /// Show or set the vault's revision scheme (letters: A, B, C…; numbers: 01, 02…)
    Scheme { scheme: Option<String> },
    /// What changed between two versions ("working" is the working file; default: compare with it)
    Diff { file: PathBuf, from: String, to: Option<String> },
    /// Put an earlier version back (recorded as a new version; unsaved changes are checked in first)
    Restore { file: PathBuf, version: u32 },
    /// Save a copy of a version elsewhere, leaving the working file alone
    Export { file: PathBuf, version: u32, out: PathBuf },
    /// Move or rename a design, keeping its history
    Mv { from: PathBuf, to: PathBuf },
    /// Attach a missing design's history to the file it was moved to
    Relink { missing: PathBuf, to: PathBuf },
}

type Result = std::result::Result<(), Box<dyn std::error::Error>>;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("scpdm: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: Command) -> Result {
    match command {
        Command::Info { file } => info(&file),
        Command::Ls { dir } => ls(dir.as_deref().unwrap_or(Path::new("."))),
        Command::Thumb { file, out } => thumb(&file, &out),
        Command::Init { dir } => {
            let vault = Vault::init(dir.as_deref().unwrap_or(Path::new(".")))?;
            println!("Vault created in {}", vault.root().display());
            Ok(())
        }
        Command::Status => status(),
        Command::New { file, part_number, description } => {
            let mut vault = Vault::discover(".")?;
            let mut path = vault.relative(&file)?;
            if !path.ends_with(".solidcraft") {
                path.push_str(".solidcraft");
            }
            let name = path.rsplit('/').next().unwrap_or(&path).trim_end_matches(".solidcraft").to_owned();
            let bytes = scformat::new_design(
                &name,
                &[
                    ("part_number", part_number.as_deref().unwrap_or("")),
                    ("description", description.as_deref().unwrap_or("")),
                ],
            );
            vault.add_new(&path, &bytes, "New design")?;
            println!("{path}: v1");
            Ok(())
        }
        Command::Checkin { files, message } => {
            let mut vault = Vault::discover(".")?;
            for file in files {
                let path = vault.relative(&file)?;
                let v = vault.check_in(&path, &message)?;
                println!("{path}: v{}", v.number);
            }
            Ok(())
        }
        Command::Log { file } => log(&file),
        Command::Release { file, message, revision } => {
            let mut vault = Vault::discover(".")?;
            let path = vault.relative(&file)?;
            let revision = match revision {
                Some(r) => r,
                None => vault.suggested_revision(&path)?,
            };
            let r = vault.release_as(&path, &revision, &message)?;
            println!("{path}: released revision {} (v{})", r.revision, r.version);
            Ok(())
        }
        Command::Revise { file } => {
            let mut vault = Vault::discover(".")?;
            let path = vault.relative(&file)?;
            vault.revise(&path)?;
            println!("{path}: work in progress on revision {}", vault.next_revision(&path)?);
            Ok(())
        }
        Command::Obsolete { file, undo } => {
            let mut vault = Vault::discover(".")?;
            let path = vault.relative(&file)?;
            vault.set_obsolete(&path, !undo)?;
            println!("{path}: {}", if undo { "work in progress" } else { "obsolete" });
            Ok(())
        }
        Command::Props { file, set, name, check_in } => props(&file, &set, name.as_deref(), check_in),
        Command::Scheme { scheme } => {
            let mut vault = Vault::discover(".")?;
            match scheme.as_deref() {
                None => {}
                Some("letters") => vault.set_revision_scheme(vault::RevisionScheme::Letters)?,
                Some("numbers") => vault.set_revision_scheme(vault::RevisionScheme::Numbers)?,
                Some(other) => return Err(format!("unknown scheme {other:?} (letters or numbers)").into()),
            }
            println!("{:?}", vault.revision_scheme()?);
            Ok(())
        }
        Command::Diff { file, from, to } => diff(&file, &from, to.as_deref().unwrap_or("working")),
        Command::Restore { file, version } => {
            let mut vault = Vault::discover(".")?;
            let path = vault.relative(&file)?;
            let v = vault.restore(&path, version)?;
            println!("{path}: restored v{version} as v{}", v.number);
            Ok(())
        }
        Command::Export { file, version, out } => {
            let vault = Vault::discover(".")?;
            let path = vault.relative(&file)?;
            let v = vault
                .history(&path)?
                .into_iter()
                .find(|v| v.number == version)
                .ok_or_else(|| vault::Error::NoSuchVersion(path.clone(), version))?;
            if out.exists() {
                return Err(format!("{} already exists", out.display()).into());
            }
            std::fs::write(&out, vault.version_bytes(&v)?)?;
            println!("{path} v{version} saved as {}", out.display());
            Ok(())
        }
        Command::Mv { from, to } => {
            let mut vault = Vault::discover(".")?;
            let (from, to) = (vault.relative(&from)?, vault.relative(&to)?);
            let report = vault.rename(&from, &to)?;
            println!("{from} → {to}");
            for u in &report.updated {
                match u.version {
                    Some(v) => println!("  links updated in {} (v{v})", u.path),
                    None => println!("  links updated in {} (joins its changes not checked in yet)", u.path),
                }
            }
            for (path, e) in &report.failed {
                eprintln!("  links NOT updated in {path}: {e}");
            }
            Ok(())
        }
        Command::Relink { missing, to } => {
            let mut vault = Vault::discover(".")?;
            let (missing, to) = (vault.relative(&missing)?, vault.relative(&to)?);
            vault.relink(&missing, &to)?;
            println!("{to} now has the history of {missing}");
            Ok(())
        }
    }
}

fn status() -> Result {
    let mut vault = Vault::discover(".")?;
    let entries = vault.scan()?;
    let mut clean = 0;
    for e in &entries {
        let label = match e.status {
            Status::Current => {
                clean += 1;
                continue;
            }
            Status::Untracked => "untracked",
            Status::Modified => "modified ",
            Status::Missing => "missing  ",
        };
        let version = e.latest.as_ref().map_or(String::new(), |v| format!(" (v{})", v.number));
        let moved = e.moved_from.as_ref().map_or(String::new(), |m| format!("  ← moved from {m}? (scpdm relink)"));
        println!("{label}  {}{version}{moved}", e.path);
    }
    println!("{} designs, {clean} up to date", entries.len());
    Ok(())
}

fn log(file: &Path) -> Result {
    let vault = Vault::discover(".")?;
    let path = vault.relative(file)?;
    let releases = vault.lifecycle(&path)?.releases;
    for v in vault.history(&path)? {
        let released = releases.iter().find(|r| r.version == v.number).map(|r| format!("[released {}]", r.revision));
        let props = [v.part_number.as_deref(), v.revision.as_deref().map(|r| r.trim())]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" rev ");
        println!(
            "v{:<4} {}  {:<10} {:<20} {}{}",
            v.number,
            date(v.created_at),
            v.author,
            props,
            released.map_or(String::new(), |r| format!("{r} ")),
            if v.comment.is_empty() { "-" } else { &v.comment }
        );
    }
    Ok(())
}

const PROPERTIES: [&str; 7] = ["part_number", "revision", "project", "description", "author", "organization", "license"];

fn props(file: &Path, set: &[String], name: Option<&str>, check_in: bool) -> Result {
    let mut vault = Vault::discover(".")?;
    let path = vault.relative(file)?;
    if !set.is_empty() || name.is_some() {
        let mut values = Vec::new();
        for item in set {
            let (key, value) = item.split_once('=').ok_or_else(|| format!("{item:?}: use KEY=VALUE"))?;
            if !PROPERTIES.contains(&key) {
                return Err(format!("unknown property {key:?} (one of {})", PROPERTIES.join(", ")).into());
            }
            values.push((key, Some(value)));
        }
        vault.edit_properties(&path, name, &values)?;
        if check_in {
            match vault.check_in(&path, "Edited properties") {
                Ok(v) => println!("{path}: v{}", v.number),
                Err(vault::Error::Unchanged(_)) => println!("{path}: unchanged"),
                Err(e) => return Err(e.into()),
            }
        }
    }
    let info = scformat::read_file(vault.absolute(&path))?;
    let m = &info.metadata;
    println!("{:<14}{}", "name", info.name.as_deref().unwrap_or("-"));
    for (key, value) in PROPERTIES.iter().zip([&m.part_number, &m.revision, &m.project, &m.description, &m.author, &m.organization, &m.license]) {
        println!("{key:<14}{}", value.as_deref().unwrap_or("-"));
    }
    if let Ok(lifecycle) = vault.lifecycle(&path) {
        let state = match lifecycle.state {
            vault::State::Wip => format!("work in progress (next release: {})", vault.next_revision(&path)?),
            vault::State::Released => format!("released ({})", lifecycle.releases.first().map_or("?", |r| r.revision.as_str())),
            vault::State::Obsolete => "obsolete".to_owned(),
        };
        println!("{:<14}{state}", "state");
    }
    Ok(())
}

/// The bytes of a side of a comparison: a version number (`3`, `v3`) or `working`.
fn side(vault: &Vault, path: &str, which: &str) -> std::result::Result<(String, Vec<u8>), Box<dyn std::error::Error>> {
    if which == "working" {
        return Ok(("working file".into(), std::fs::read(vault.absolute(path))?));
    }
    let number: u32 = which.trim_start_matches('v').parse().map_err(|_| format!("{which:?} is not a version (use 3, v3 or working)"))?;
    let version = vault
        .history(path)?
        .into_iter()
        .find(|v| v.number == number)
        .ok_or_else(|| vault::Error::NoSuchVersion(path.to_owned(), number))?;
    Ok((format!("v{number}"), vault.version_bytes(&version)?))
}

fn diff(file: &Path, from: &str, to: &str) -> Result {
    use scformat::compare::FeatureChangeKind;
    let vault = Vault::discover(".")?;
    let path = vault.relative(file)?;
    let (from_label, a) = side(&vault, &path, from)?;
    let (to_label, b) = side(&vault, &path, to)?;
    let c = scformat::compare::compare(&a, &b)?;
    println!("{path}: {from_label} → {to_label}");
    if c.is_empty() {
        println!("  no differences");
        return Ok(());
    }
    let changes = |title: &str, list: &[scformat::compare::Change]| {
        if list.is_empty() {
            return;
        }
        println!("{title}");
        for ch in list {
            match (&ch.before, &ch.after) {
                (None, Some(a)) => println!("  + {}: {a}", ch.what),
                (Some(b), None) => println!("  - {}: {b}", ch.what),
                (Some(b), Some(a)) => println!("  ~ {}: {b} → {a}", ch.what),
                (None, None) => {}
            }
        }
    };
    changes("Properties", &c.properties);
    changes("Parameters", &c.parameters);
    if !c.features.is_empty() || c.features_reordered {
        println!("Features");
        for f in &c.features {
            match &f.change {
                FeatureChangeKind::Added => println!("  + {} ({})", f.name, f.kind),
                FeatureChangeKind::Removed => println!("  - {} ({})", f.name, f.kind),
                FeatureChangeKind::Edited { .. } => println!("  ~ {} ({}): {}", f.name, f.kind, describe(&f.change)),
            }
        }
        if c.features_reordered {
            println!("  (features reordered)");
        }
    }
    changes("Components", &c.components);
    changes("Links", &c.links);
    if !c.other.is_empty() {
        println!("Also changed: {}", c.other.join(", "));
    }
    Ok(())
}

/// "renamed from X; suppressed; changed: radius, extent"
fn describe(change: &scformat::compare::FeatureChangeKind) -> String {
    let scformat::compare::FeatureChangeKind::Edited { renamed_from, suppressed, settings } = change else {
        return String::new();
    };
    let mut what = Vec::new();
    if let Some(old) = renamed_from {
        what.push(format!("renamed from {old}"));
    }
    match suppressed {
        Some(true) => what.push("suppressed".to_owned()),
        Some(false) => what.push("unsuppressed".to_owned()),
        None => {}
    }
    if !settings.is_empty() {
        what.push(format!("changed: {}", settings.join(", ")));
    }
    what.join("; ")
}

fn info(file: &Path) -> Result {
    let d = scformat::read_file(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let m = &d.metadata;
    let row = |label: &str, value: Option<&str>| {
        if let Some(v) = value {
            println!("{label:<14}{v}");
        }
    };
    println!("{}", file.display());
    row("name", d.name.as_deref());
    row("stored as", Some(encoding(&d)));
    row("format", Some(d.format.as_deref().unwrap_or("(pre-format legacy)")));
    row("units", d.units.as_deref());
    row("part number", m.part_number.as_deref());
    row("revision", m.revision.as_deref());
    row("project", m.project.as_deref());
    row("description", m.description.as_deref());
    row("author", m.author.as_deref());
    row("organization", m.organization.as_deref());
    row("license", m.license.as_deref());
    row("features", Some(&d.feature_count.to_string()));
    row("parameters", Some(&d.param_count.to_string()));
    row("components", Some(&d.components.len().to_string()));
    if d.encoding == Encoding::Container {
        row("blobs", Some(&d.blob_count.to_string()));
        row("model cache", Some(yes_no(d.has_model_cache)));
        row("thumbnail", Some(yes_no(d.thumbnail_png.is_some())));
    }
    for key in m.extensions.keys() {
        row("tool data", Some(key));
    }
    let folder = file.parent().unwrap_or(Path::new(""));
    for link in &d.links {
        let component = d.components.iter().find(|c| c.id == link.component).map_or("?", |c| c.name.as_str());
        let status = if folder.join(&link.path).exists() { "" } else { "  (missing)" };
        println!("{:<14}{component} → {}{status}", "link", link.path);
    }
    if let Ok(vault) = Vault::discover(folder.canonicalize().unwrap_or_default()) {
        if let Ok(history) = vault.relative(file).and_then(|p| vault.history(&p)) {
            row("versions", Some(&history.len().to_string()));
        }
    }
    Ok(())
}

fn ls(dir: &Path) -> Result {
    let mut files = Vec::new();
    vault::find_designs(dir, &mut files)?;
    files.sort();
    println!("{:<44} {:<12} {:<4} {:<9} {:>5}  NAME", "FILE", "PART NO", "REV", "STORED", "FMT");
    for file in &files {
        let shown = file.strip_prefix(dir).unwrap_or(file).display().to_string();
        match scformat::read_file(file) {
            Ok(d) => println!(
                "{shown:<44} {:<12} {:<4} {:<9} {:>5}  {}",
                d.metadata.part_number.as_deref().unwrap_or("-"),
                d.metadata.revision.as_deref().unwrap_or("-"),
                encoding(&d),
                d.format_version().map_or("-".into(), |v| v.to_string()),
                d.name.as_deref().unwrap_or(""),
            ),
            Err(e) => println!("{shown:<44} unreadable: {e}"),
        }
    }
    Ok(())
}

fn thumb(file: &Path, out: &Path) -> Result {
    let d = scformat::read_file(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let png = d
        .thumbnail_png
        .ok_or("this file carries no thumbnail (SolidCraft writes one on a warm save with the model cache)")?;
    std::fs::write(out, png)?;
    Ok(())
}

fn date(unix: i64) -> String {
    Local.timestamp_opt(unix, 0).single().map_or("?".into(), |t| t.format("%Y-%m-%d %H:%M").to_string())
}

fn encoding(d: &DesignInfo) -> &'static str {
    match d.encoding {
        Encoding::Container => "container",
        Encoding::Json => "json",
    }
}

fn yes_no(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}
