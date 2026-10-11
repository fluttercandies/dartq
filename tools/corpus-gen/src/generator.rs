//! Orchestrates one generation run: stream plan items to disk while
//! accumulating counters, coverage, and the fingerprint.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;

use crate::coverage::Coverage;
use crate::emit::{RenderCtx, render};
use crate::manifest::{Fnv64, Manifest};
use crate::plan::{Plan, Scale};
use crate::rng::SplitMix64;

/// Parameters of one generation run.
pub struct Config {
    pub out: PathBuf,
    pub scale: Scale,
    pub seed: u64,
}

/// Result of one generation run.
pub struct Stats {
    pub manifest: Manifest,
    pub coverage: Coverage,
}

/// Generates the corpus under `config.out` and writes `manifest.json` next to
/// it. Existing files with colliding names are overwritten; stale files from
/// earlier runs are not removed (callers should target a fresh directory).
///
/// # Errors
///
/// Returns any `io::Error` from creating directories or writing corpus and
/// manifest files, propagated verbatim with no partial-run retry.
pub fn generate(config: &Config) -> io::Result<Stats> {
    fs::create_dir_all(&config.out)?;
    let plan = Plan::build(config.scale);
    let mut rng = SplitMix64::new(config.seed);
    let mut coverage = Coverage::new();
    let mut fingerprint = Fnv64::new();
    let mut per_dir: BTreeMap<String, usize> = BTreeMap::new();
    let mut files = 0usize;
    let mut lines = 0usize;
    let mut rotation_ordinal = 0usize;
    for item in &plan {
        let mut ctx = RenderCtx {
            plan: &plan,
            rng: &mut rng,
            coverage: &mut coverage,
            rotation_ordinal,
        };
        let content = render(&mut ctx, &item);
        rotation_ordinal = ctx.rotation_ordinal;

        let path = config.out.join(&item.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, content.as_bytes())?;

        let size = content.len();
        lines += content.matches('\n').count();
        files += 1;
        let top = item.path.split('/').next().unwrap_or("root");
        *per_dir.entry(top.to_string()).or_insert(0) += 1;

        fingerprint.update(item.path.as_bytes());
        fingerprint.update(&[0]);
        let size_text = size.to_string();
        fingerprint.update(size_text.as_bytes());
        fingerprint.update(&[0]);
    }
    let manifest = Manifest {
        seed: config.seed,
        scale: config.scale.label(),
        files,
        lines,
        fingerprint: fingerprint.finish(),
        files_per_top_level_dir: per_dir,
    };
    fs::write(config.out.join("manifest.json"), manifest.to_json())?;
    Ok(Stats { manifest, coverage })
}
