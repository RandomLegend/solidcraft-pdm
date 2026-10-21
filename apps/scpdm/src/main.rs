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
            vault.rename(&from, &to)?;
            println!("{from} → {to}");
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
    for v in vault.history(&path)? {
        let props = [v.part_number.as_deref(), v.revision.as_deref().map(|r| r.trim())]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" rev ");
        println!(
            "v{:<4} {}  {:<10} {:<20} {}",
            v.number,
            date(v.created_at),
            v.author,
            props,
            if v.comment.is_empty() { "-" } else { &v.comment }
        );
    }
    Ok(())
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
