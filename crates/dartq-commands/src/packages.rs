//! The `dartq packages` face: the work-point inventory of a tree — one
//! implementation shared by the CLI shell and later faces.
//!
//! The scan lives in [`dartq_pub::universe`] (the pubspec-declared
//! boundary, pub's workspace semantics verbatim); this module adds the
//! config edge (`dartq.yaml`'s `packages:` section, found on the ladder
//! from the invocation directory to the universe root) and the
//! navigation layer: every degraded state answers *what to run next*,
//! never a bare failure — an unresolved package names `dart pub get`
//! and the SDK constraint it needs, a missing universe names the
//! nearest pubspec or checkout marker above the invocation directory.

use std::path::{Path, PathBuf};

use dartq_pub::PubError;
use dartq_pub::universe::{ScanOptions, ScanReport, Universe, enclosing_universe, scan};

use crate::config::{ConfigError, DartqConfig, load as load_dartq_config};

/// Why the inventory could not be assembled: the usage tier's text body
/// (navigation wording — no universe, or a config glob that cannot
/// parse), or the internal I/O tier.
#[derive(Debug, thiserror::Error)]
pub enum PackagesError {
    /// Usage-tier navigation text (no package universe around the
    /// directory, or an unusable config value).
    #[error("{0}")]
    Usage(String),
    /// The `dartq.yaml` on the ladder exists but cannot be read.
    #[error(transparent)]
    Config(#[from] ConfigError),
}

/// The assembled inventory: the universe ladder's answer, the scan, and
/// the config that shaped it.
pub struct PackageInventory {
    /// The enclosing universe (root + layout when a pubspec unit
    /// exists; the `.git`-capped floor otherwise).
    pub universe: Universe,
    /// The work points and boundaries of the scan.
    pub report: ScanReport,
    /// The `dartq.yaml` that shaped the scan, when one sits on the
    /// ladder (the nearest one from `from` up to the universe root).
    pub config: Option<DartqConfig>,
}

/// The one-line navigation hint for an unresolved package: what to run
/// and what it needs.
#[must_use]
pub fn resolve_hint(dir: &Path, sdk: Option<&str>) -> String {
    match sdk {
        Some(sdk) => format!(
            "unresolved — run `dart pub get` in {} (needs SDK {})",
            dir.display(),
            sdk
        ),
        None => format!("unresolved — run `dart pub get` in {}", dir.display()),
    }
}

/// Assembles the inventory for `from`: the enclosing universe, the
/// config on the ladder, and the scan shaped by it.
///
/// # Errors
///
/// [`PackagesError::Usage`] when neither a pubspec nor a `.git`
/// marker exists above `from` (navigation text names the directory);
/// [`PackagesError::Config`] when the ladder's `dartq.yaml` is
/// malformed.
pub fn inventory(from: &Path) -> Result<PackageInventory, PackagesError> {
    let universe = enclosing_universe(from).map_err(|error| match error {
        PubError::NoRoot(text) => PackagesError::Usage(format!(
            "no Dart package or checkout at {} — {text}",
            from.display()
        )),
        other => PackagesError::Usage(other.to_string()),
    })?;
    // The nearest config on the ladder: `from` up to (and including)
    // the universe root.
    let mut config = None;
    let mut dir = Some(from);
    while let Some(current) = dir {
        if let Some(found) = load_dartq_config(current)? {
            config = Some(found);
            break;
        }
        if current == universe.root {
            break;
        }
        dir = current.parent();
    }
    let packages = config.as_ref().and_then(|config| config.packages.as_ref());
    let options = match packages {
        Some(policy) => ScanOptions {
            include: policy
                .include
                .iter()
                .map(|pattern| dartq_pub::glob::Glob::new(pattern))
                .collect::<Result<_, _>>()
                .map_err(|error| {
                    PackagesError::Usage(format!("dartq.yaml packages.include: bad glob: {error}"))
                })?,
            exclude: policy
                .exclude
                .iter()
                .map(|pattern| dartq_pub::glob::Glob::new(pattern))
                .collect::<Result<_, _>>()
                .map_err(|error| {
                    PackagesError::Usage(format!("dartq.yaml packages.exclude: bad glob: {error}"))
                })?,
        },
        None => ScanOptions::default(),
    };
    let report = scan(&universe.root, &options);
    Ok(PackageInventory {
        universe,
        report,
        config,
    })
}

/// The default package set for bare multi-package invocations: the
/// config's `packages.default` resolved against the universe root,
/// falling back to the work points the scan found.
#[must_use]
pub fn default_set(inventory: &PackageInventory) -> Vec<PathBuf> {
    let declared = inventory
        .config
        .as_ref()
        .and_then(|config| config.packages.as_ref())
        .map(|policy| &policy.default);
    match declared {
        Some(default) if !default.is_empty() => default
            .iter()
            .map(|relative| dartq_pub::normalize(&inventory.universe.root.join(relative)))
            .collect(),
        _ => inventory
            .report
            .packages
            .iter()
            .map(|entry| entry.dir.clone())
            .collect(),
    }
}
