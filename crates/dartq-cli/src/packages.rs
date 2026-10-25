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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use dartq_commands::packages::PackageInventory;
    use dartq_pub::universe::{
        BoundaryEntry, BoundaryReason, PackageEntry, PackageKind, ScanReport, Universe,
    };

    /// `derive(Args)` brings no `try_parse_from`; parse through the
    /// augmented command the parent `Cli` would splice in.
    fn parse(args: &[&str]) -> Result<PackagesArgs, clap::Error> {
        let command = <PackagesArgs as clap::Args>::augment_args(clap::Command::new("packages"));
        let matches = command
            .try_get_matches_from(std::iter::once("packages").chain(args.iter().copied()))?;
        <PackagesArgs as clap::FromArgMatches>::from_arg_matches(&matches)
    }

    /// The grammar and the format resolution: absent `--format` falls to
    /// the terminal's shape (machine piped — the test harness captures
    /// stdout), and an explicit `--format` always wins.
    #[test]
    fn packages_args_parse_and_resolve_the_format() {
        let args = parse(&[]).expect("parse");
        assert_eq!(args.dir, std::path::PathBuf::from("."));
        assert!(args.format.is_none());
        assert!(
            matches!(args.effective_format(), PackagesFormat::Machine),
            "a captured (non-TTY) stdout defaults to machine rows"
        );
        for (spelling, expected) in [
            ("human", PackagesFormat::Human),
            ("machine", PackagesFormat::Machine),
            ("json", PackagesFormat::Json),
        ] {
            let args = parse(&["--format", spelling, "/w"]).expect("parse");
            assert_eq!(args.dir, std::path::PathBuf::from("/w"));
            assert_eq!(
                std::mem::discriminant(&args.effective_format()),
                std::mem::discriminant(&expected),
                "{spelling} wins over the terminal shape"
            );
        }
        assert!(parse(&["--format", "yaml"]).is_err());
    }

    /// One inventory rendered through all three faces: the human table
    /// spells directories relative to the universe root (absolute
    /// outside it), the machine rows stay absolute with `NOTE`/`BOUNDARY`
    /// companions (multi-line notes flattened), and the JSON lines parse.
    #[test]
    fn renderers_shape_every_face() {
        let root = Path::new("/w/uni");
        let listing = PackageInventory {
            universe: Universe { root: root.to_path_buf(), layout: None },
            report: ScanReport {
                root: root.to_path_buf(),
                packages: vec![
                    PackageEntry {
                        dir: root.join("pkgs").join("a"),
                        name: Some("a".to_owned()),
                        kind: PackageKind::WorkspaceMember { root: root.to_path_buf() },
                        sdk: Some("^3.5.0".to_owned()),
                        resolved: true,
                        note: None,
                    },
                    PackageEntry {
                        dir: root.join("vendor").join("x"),
                        name: None,
                        kind: PackageKind::Independent,
                        sdk: None,
                        resolved: false,
                        note: Some("pubspec.yaml: line 1\nbad".to_owned()),
                    },
                    // Unresolved without a note: the navigation hint's shape.
                    PackageEntry {
                        dir: root.join("solo"),
                        name: Some("solo".to_owned()),
                        kind: PackageKind::Independent,
                        sdk: Some("^3.6.0".to_owned()),
                        resolved: false,
                        note: None,
                    },
                ],
                boundaries: vec![
                    BoundaryEntry {
                        dir: root.join(".stash"),
                        reason: BoundaryReason::HiddenTree,
                        detail: None,
                    },
                    BoundaryEntry {
                        dir: root.join("build"),
                        reason: BoundaryReason::BuildTree,
                        detail: Some("contains build/pkg/pubspec.yaml".to_owned()),
                    },
                ],
            },
            config: None,
        };

        let mut human = Vec::new();
        write_human(&mut human, &listing).expect("human");
        let human = String::from_utf8(human).expect("utf-8");
        assert!(human.contains("universe: /w/uni"), "{human}");
        assert!(human.contains("no pubspec inside this checkout"), "{human}");
        assert!(human.contains("pkgs/a") && !human.contains("/w/uni/pkgs/a"), "inside dirs are relative: {human}");
        let member_row = human.lines().find(|line| line.contains("pkgs/a")).expect("member row");
        assert!(
            member_row.contains("workspace-member") && member_row.contains("a") && member_row.contains("resolved"),
            "{member_row}"
        );
        let vendor_row = human.lines().find(|line| line.contains("vendor/x")).expect("vendor row");
        assert!(vendor_row.contains("independent") && vendor_row.contains("unresolved"), "{vendor_row}");
        let hidden = human.lines().find(|line| line.contains(".stash")).expect("hidden boundary");
        assert!(hidden.contains("hidden-tree"), "{hidden}");
        let built = human.lines().find(|line| line.contains("build-tree")).expect("build boundary");
        assert!(built.contains("build (contains build/pkg/pubspec.yaml)"), "{built}");
        assert!(
            human.contains("run `dart pub get` in /w/uni/solo (needs SDK ^3.6.0)"),
            "the unresolved hint: {human}"
        );

        let mut machine = Vec::new();
        write_machine(&mut machine, &listing).expect("machine");
        let machine = String::from_utf8(machine).expect("utf-8");
        assert!(machine.contains("PKG|/w/uni/pkgs/a|a|workspace-member|^3.5.0|yes"), "{machine}");
        assert!(machine.contains("PKG|/w/uni/vendor/x|-|independent|-|no"), "{machine}");
        assert!(machine.contains("NOTE|/w/uni/vendor/x|pubspec.yaml: line 1 bad"), "one flattened line: {machine}");
        assert!(machine.contains("BOUNDARY|/w/uni/.stash|hidden-tree"), "{machine}");
        assert!(machine.contains("BOUNDARY|/w/uni/build|build-tree|contains build/pkg/pubspec.yaml"), "{machine}");

        let mut json = Vec::new();
        write_json(&mut json, &listing).expect("json");
        let lines: Vec<serde_json::Value> = String::from_utf8(json)
            .expect("utf-8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("one object per line"))
            .collect();
        assert_eq!(lines[0]["type"], "universe");
        assert_eq!(lines[0]["root"], "/w/uni");
        assert_eq!(lines[0]["has_pubspec"], false);
        assert_eq!(lines[1]["type"], "pkg");
        assert_eq!(lines[1]["dir"], "/w/uni/pkgs/a");
        assert_eq!(lines[1]["resolved"], true);
        assert_eq!(lines[2]["note"], "pubspec.yaml: line 1\nbad");
        assert_eq!(lines[3]["type"], "pkg");
        assert_eq!(lines[3]["resolved"], false);
        assert_eq!(lines[4]["type"], "boundary");
        assert_eq!(lines[4]["reason"], "hidden-tree");
        assert_eq!(lines[5]["detail"], "contains build/pkg/pubspec.yaml");
    }

    /// End to end: a bare tree with one package inventories clean (the
    /// captured stdout picks the machine face); a tree with no universe
    /// above it is the usage tier with navigation text.
    #[test]
    fn run_entry_tiers_over_real_trees() {
        let root = std::env::temp_dir().join(format!("dqs-pkgs-ok-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("lib")).expect("dir");
        std::fs::write(root.join("pubspec.yaml"), "name: fx\nenvironment:\n  sdk: ^3.5.0\n").expect("pubspec");
        let ok = root.display().to_string();
        assert_eq!(run_entry(&parse(&[&ok]).expect("parse")), exit::CLEAN);

        let bare = std::env::temp_dir().join(format!("dqs-pkgs-bare-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&bare);
        std::fs::create_dir_all(&bare).expect("dir");
        let bare_arg = bare.display().to_string();
        assert_eq!(
            run_entry(&parse(&[&bare_arg]).expect("parse")),
            exit::USAGE,
            "no pubspec or checkout marker above: navigation error"
        );
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&bare).ok();
    }
}
