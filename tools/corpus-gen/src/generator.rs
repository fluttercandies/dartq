//! Orchestrates one generation run: stream plan items to disk while
//! accumulating counters, coverage, and the fingerprint.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;

use crate::coverage::Coverage;
use crate::emit::{RenderCtx, render};
use crate::manifest::{Fnv64, Manifest};
use crate::plan::{Options, Plan, Scale};
use crate::rng::SplitMix64;

/// Parameters of one generation run.
pub struct Config {
    pub out: PathBuf,
    pub scale: Scale,
    pub seed: u64,
    /// Layout + version-ladder faces (1d/1e). `Options::default()` is the
    /// flat single-version tree — the byte-stable pre-#1812 shape.
    pub options: Options,
    /// When set, additionally writes `<out>/edits.jsonl`: a `steps`-long
    /// deterministic replay sequence (axis 2a) that round-trips to the
    /// initial tree state. Not part of the plan or the fingerprint.
    pub emit_replay_seq: Option<u64>,
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
    let plan = Plan::build_with(config.scale, config.options);
    let mut rng = SplitMix64::new(config.seed);
    let mut coverage = Coverage::new();
    let mut fingerprint = Fnv64::new();
    let mut per_dir: BTreeMap<String, usize> = BTreeMap::new();
    let mut files = 0usize;
    let mut lines = 0usize;
    let mut rotation_ordinal = 0usize;
    // Captured file contents for the optional replay sequence; bounded by
    // replay::CAPTURE_PATHS (10 small files).
    let mut captures: BTreeMap<String, String> = BTreeMap::new();
    for item in &plan {
        let mut ctx = RenderCtx {
            plan: &plan,
            rng: &mut rng,
            coverage: &mut coverage,
            rotation_ordinal,
        };
        let content = render(&mut ctx, &item);
        rotation_ordinal = ctx.rotation_ordinal;

        if config.emit_replay_seq.is_some() && crate::replay::is_capture(&item.path) {
            captures.insert(item.path.clone(), content.clone());
        }

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
    if let Some(steps) = config.emit_replay_seq {
        crate::replay::emit(&config.out, steps, &captures, config.seed)?;
    }
    Ok(Stats { manifest, coverage })
}
