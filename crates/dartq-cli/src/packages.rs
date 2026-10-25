//! The `dartq packages` shell: argument parsing, the three renderers,
//! and the exit tiers. The inventory itself lives in
//! `dartq-commands`' packages module (one implementation the later
//! faces share).

use std::io::{self, BufWriter, Write};
use std::path::Path;

use clap::Args;

use dartq_commands::packages::{PackagesError, inventory};

use crate::exit;

/// `dartq packages` — the work-point inventory of the enclosing tree.
#[derive(Args)]
pub struct PackagesArgs {
    /// The directory to inventory; defaults to the current directory.
    /// The scan runs over its enclosing universe (the workspace root or
    /// checkout the ladder finds).
    #[arg(default_value = ".")]
    pub dir: std::path::PathBuf,
    /// Output format; absent falls to the terminal's shape — the human
    /// table on a TTY, machine rows piped.
    #[arg(long, value_enum)]
    pub format: Option<PackagesFormat>,
}

impl PackagesArgs {
    fn effective_format(&self) -> PackagesFormat {
        self.format.unwrap_or(if crate::non_tty_stdout() {
            PackagesFormat::Machine
        } else {
            PackagesFormat::Human
        })
    }
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum PackagesFormat {
    /// Aligned table with a navigation summary (the terminal face).
    Human,
    /// `PKG|path|name|kind|sdk|resolved` and
    /// `BOUNDARY|path|reason[|detail]` rows.
    Machine,
    /// One JSON object per line.
    Json,
}

/// The entry: assemble, render, and tier. A completed listing exits 0 —
/// unresolved packages are rows with a navigation hint, not failures;
/// only a missing universe (usage, with navigation text) or a malformed
/// config fails.
pub fn run_entry(args: &PackagesArgs) -> u8 {
    let assembled = inventory(&args.dir);
    let listing = match assembled {
        Ok(listing) => listing,
        Err(PackagesError::Usage(message)) => {
            eprintln!("error: {message}");
            return exit::USAGE;
        }
        Err(PackagesError::Config(dartq_commands::config::ConfigError::Invalid(message))) => {
            eprintln!("error: {message}");
            return exit::USAGE;
        }
        Err(PackagesError::Config(dartq_commands::config::ConfigError::Io(error))) => {
            eprintln!("dartq: internal error: {error}");
            return exit::INTERNAL_ERROR;
        }
    };
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    let written = match args.effective_format() {
        PackagesFormat::Human => write_human(&mut out, &listing),
        PackagesFormat::Machine => write_machine(&mut out, &listing),
        PackagesFormat::Json => write_json(&mut out, &listing),
    };
    match written.and_then(|()| out.flush()) {
        Ok(()) => exit::CLEAN,
        Err(error) => {
            eprintln!("dartq: internal error: writing packages failed: {error}");
            exit::INTERNAL_ERROR
        }
    }
}

/// The display spelling of a directory: relative to the universe root
/// when inside it (the readable form), absolute otherwise.
fn display_dir(root: &Path, dir: &Path) -> String {
    match dir.strip_prefix(root) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.display().to_string(),
        _ => dir.display().to_string(),
    }
}

fn write_human(out: &mut impl Write, listing: &dartq_commands::packages::PackageInventory) -> io::Result<()> {
    let root = &listing.universe.root;
    writeln!(out, "universe: {}", root.display())?;
    if listing.universe.layout.is_none() {
        writeln!(
            out,
            "  no pubspec inside this checkout — files here resolve against nothing"
        )?;
    }
    for entry in &listing.report.packages {
        let dir = display_dir(root, &entry.dir);
        let resolved = if entry.resolved { "resolved" } else { "unresolved" };
        let note = entry.note.as_deref().map(|note| format!("  [{note}]")).unwrap_or_default();
        writeln!(
            out,
            "{:<18} {:<24} {:<10} {:<14} {}{}",
            entry.kind.as_str(),
            entry.name.as_deref().unwrap_or("?"),
            entry.sdk.as_deref().unwrap_or("-"),
            resolved,
            dir,
            note
        )?;
    }
    for boundary in &listing.report.boundaries {
        writeln!(
            out,
            "{:<18} {}{}",
            boundary.reason.as_str(),
            display_dir(root, &boundary.dir),
            boundary.detail.as_deref().map(|d| format!(" ({d})")).unwrap_or_default()
        )?;
    }
    let unresolved: Vec<_> = listing
        .report
        .packages
        .iter()
        .filter(|entry| !entry.resolved && entry.note.is_none())
        .collect();
    if !unresolved.is_empty() {
        writeln!(out)?;
        for entry in &unresolved {
            writeln!(
                out,
                "{}",
                dartq_commands::packages::resolve_hint(&entry.dir, entry.sdk.as_deref())
            )?;
        }
    }
    Ok(())
}

fn write_machine(out: &mut impl Write, listing: &dartq_commands::packages::PackageInventory) -> io::Result<()> {
    for entry in &listing.report.packages {
        writeln!(
            out,
            "PKG|{}|{}|{}|{}|{}",
            entry.dir.display(),
            entry.name.as_deref().unwrap_or("-"),
            entry.kind.as_str(),
            entry.sdk.as_deref().unwrap_or("-"),
            if entry.resolved { "yes" } else { "no" }
        )?;
        if let Some(note) = &entry.note {
            writeln!(out, "NOTE|{}|{}", entry.dir.display(), note.replace('\n', " "))?;
        }
    }
    for boundary in &listing.report.boundaries {
        match &boundary.detail {
            Some(detail) => writeln!(
                out,
                "BOUNDARY|{}|{}|{}",
                boundary.dir.display(),
                boundary.reason.as_str(),
                detail.replace('\n', " ")
            )?,
            None => writeln!(out, "BOUNDARY|{}|{}", boundary.dir.display(), boundary.reason.as_str())?,
        }
    }
    Ok(())
}

fn write_json(out: &mut impl Write, listing: &dartq_commands::packages::PackageInventory) -> io::Result<()> {
    let line = serde_json::json!({
        "schema_version": dartq_commands::SCHEMA_VERSION,
        "type": "universe",
        "root": listing.universe.root.display().to_string(),
        "has_pubspec": listing.universe.layout.is_some(),
    });
    writeln!(out, "{line}")?;
    for entry in &listing.report.packages {
        let line = serde_json::json!({
            "type": "pkg",
            "dir": entry.dir.display().to_string(),
            "name": entry.name,
            "kind": entry.kind.as_str(),
            "sdk": entry.sdk,
            "resolved": entry.resolved,
            "note": entry.note,
        });
        writeln!(out, "{line}")?;
    }
    for boundary in &listing.report.boundaries {
        let line = serde_json::json!({
            "type": "boundary",
            "dir": boundary.dir.display().to_string(),
            "reason": boundary.reason.as_str(),
            "detail": boundary.detail,
        });
        writeln!(out, "{line}")?;
    }
    Ok(())
}
