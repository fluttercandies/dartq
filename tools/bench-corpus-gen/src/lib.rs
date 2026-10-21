//! Deterministic LOC-targeted synthetic Dart corpus generator (library face).
//!
//! `render(seed, loc_target)` is a pure function: the same seed and target
//! produce the same file list with byte-identical bodies on every run and
//! platform. Disk writing lives in the binary so tests can exercise the
//! render without touching the filesystem.
//!
//! The output is a single self-contained Dart package (`corpus100k`): no
//! external dependencies, `dart:` core libraries only, with a hand-written
//! `.dart_tool/package_config.json` so both dartq and the pinned oracle
//! analyze it without a pub get.

pub mod barrel_family;
pub mod general_family;
pub mod rng;
pub mod switch_family;

/// One emitted corpus file: package-relative path and full body
/// (LF-terminated).
pub struct File {
    pub path: String,
    pub body: String,
}

/// Per-family emission counters.
#[derive(Clone, Copy, Default)]
pub struct FamilyStats {
    pub files: usize,
    pub lines: usize,
}

/// Full result of one render run.
pub struct Rendered {
    /// All files in emission order: pubspec, package config, family A, B, C.
    pub files: Vec<File>,
    pub switch_heavy: FamilyStats,
    pub barrel_chains: FamilyStats,
    pub general_mix: FamilyStats,
    pub loc_target: usize,
    pub seed: u64,
    /// FNV-1a 64 over, per file in emission order, the path bytes, `0x00`,
    /// the decimal body size, `0x00`.
    pub fingerprint: u64,
}

/// Physical line count of an LF-terminated body.
pub fn count_lines(body: &str) -> usize {
    body.as_bytes().iter().filter(|&&b| b == b'\n').count()
}

/// Sequential token substitution for the family templates. Tokens use the
/// `::name` form (never valid Dart), so plain Dart braces need no escaping.
pub(crate) fn sub(template: &str, binds: &[(&str, String)]) -> String {
    let mut out = template.to_string();
    for (token, value) in binds {
        out = out.replace(token, value);
    }
    out
}

/// Fixed package manifest: zero dependencies, SDK constraint matching the
/// pinned Dart 3.13.1 oracle.
const PUBSPEC: &str = "\
name: corpus100k
description: Deterministic synthetic bench corpus emitted by the bench-corpus-gen tool.
environment:
  sdk: ^3.13.0
";

/// Fixed package config so the tree analyzes without a pub get. `rootUri` is
/// relative to the `.dart_tool` directory holding this file.
const PACKAGE_CONFIG: &str = "\
{
  \"configVersion\": 2,
  \"packages\": [
    {
      \"name\": \"corpus100k\",
      \"rootUri\": \"../\",
      \"packageUri\": \"lib/\",
      \"languageVersion\": \"3.13\"
    }
  ]
}
";

struct Fnv1a(u64);

impl Fnv1a {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

impl Default for Fnv1a {
    fn default() -> Self {
        Self::new()
    }
}

/// Renders the whole corpus as an ordered file list (nothing written to
/// disk). Family shares: ~35% switch-heavy, ~25% barrel chains, ~40%
/// general mix; the remainder after the integer splits goes to the general
/// family.
#[must_use]
pub fn render(seed: u64, loc_target: usize) -> Rendered {
    let mut rng = rng::SplitMix64::new(seed);
    let switch_budget = loc_target * 35 / 100;
    let barrel_budget = loc_target * 25 / 100;
    let general_budget = loc_target - switch_budget - barrel_budget;

    let (switch_files, switch_lines) = switch_family::render_switch_family(switch_budget, &mut rng);
    let (barrel_files, barrel_lines) =
        barrel_family::render_barrel_family(barrel_budget, &mut rng);
    let (general_files, general_lines) =
        general_family::render_general_family(general_budget, &mut rng);

    let switch_files_count = switch_files.len();
    let barrel_files_count = barrel_files.len();
    let general_files_count = general_files.len();
    let mut files = Vec::with_capacity(
        2 + switch_files_count + barrel_files_count + general_files_count,
    );
    files.push(File {
        path: "pubspec.yaml".to_string(),
        body: PUBSPEC.to_string(),
    });
    files.push(File {
        path: ".dart_tool/package_config.json".to_string(),
        body: PACKAGE_CONFIG.to_string(),
    });
    files.extend(switch_files);
    files.extend(barrel_files);
    files.extend(general_files);

    let mut fingerprint = Fnv1a::new();
    for file in &files {
        fingerprint.write(file.path.as_bytes());
        fingerprint.write(&[0x00]);
        fingerprint.write(file.body.len().to_string().as_bytes());
        fingerprint.write(&[0x00]);
    }

    Rendered {
        files,
        switch_heavy: FamilyStats {
            files: switch_files_count,
            lines: switch_lines,
        },
        barrel_chains: FamilyStats {
            files: barrel_files_count,
            lines: barrel_lines,
        },
        general_mix: FamilyStats {
            files: general_files_count,
            lines: general_lines,
        },
        loc_target,
        seed,
        fingerprint: fingerprint.0,
    }
}

/// Deterministic `manifest.json` text. Written next to the corpus; not part
/// of the fingerprint (mirrors the corpus-gen contract).
#[must_use]
pub fn manifest_text(stats: &Rendered) -> String {
    let total_lines = stats.switch_heavy.lines + stats.barrel_chains.lines + stats.general_mix.lines;
    let dart_files = stats.files.len() - 2;
    format!(
        "{{\n  \"generator\": \"bench-corpus-gen\",\n  \"seed\": {},\n  \"loc_target\": {},\n  \"loc_switch_heavy\": {},\n  \"loc_barrel_chains\": {},\n  \"loc_general_mix\": {},\n  \"loc_total\": {},\n  \"dart_files\": {},\n  \"fingerprint\": \"fnv1a64:{:016x}\"\n}}\n",
        stats.seed,
        stats.loc_target,
        stats.switch_heavy.lines,
        stats.barrel_chains.lines,
        stats.general_mix.lines,
        total_lines,
        dart_files,
        stats.fingerprint
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dart_lines(stats: &Rendered) -> usize {
        stats.switch_heavy.lines + stats.barrel_chains.lines + stats.general_mix.lines
    }

    #[test]
    fn same_seed_same_loc_is_byte_identical() {
        let a = render(7, 6000);
        let b = render(7, 6000);
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_eq!(a.files.len(), b.files.len());
        for (x, y) in a.files.iter().zip(b.files.iter()) {
            assert_eq!(x.path, y.path);
            assert_eq!(x.body, y.body);
        }
    }

    #[test]
    fn seed_changes_output() {
        let a = render(0, 6000);
        let b = render(1, 6000);
        assert_ne!(a.fingerprint, b.fingerprint);
    }

    #[test]
    fn loc_budget_is_met_with_bounded_overshoot() {
        let target = 12000;
        let stats = render(0, target);
        let lines = dart_lines(&stats);
        assert!(lines >= target, "emitted {lines} < target {target}");
        assert!(
            lines <= target + 5000,
            "emitted {lines} overshoots target {target}"
        );
    }

    #[test]
    fn every_family_emits_files() {
        let stats = render(0, 12000);
        assert!(stats.switch_heavy.lines >= 3_500);
        assert!(stats.barrel_chains.lines >= 2_000);
        assert!(stats.general_mix.lines >= 3_500);
        let family_files = stats.switch_heavy.files
            + stats.barrel_chains.files
            + stats.general_mix.files;
        assert_eq!(family_files + 2, stats.files.len());
    }

    #[test]
    fn package_files_are_fixed() {
        let stats = render(3, 4000);
        let pubspec = stats
            .files
            .iter()
            .find(|f| f.path == "pubspec.yaml")
            .expect("pubspec emitted");
        assert_eq!(pubspec.body, PUBSPEC);
        let config = stats
            .files
            .iter()
            .find(|f| f.path == ".dart_tool/package_config.json")
            .expect("package config emitted");
        assert_eq!(config.body, PACKAGE_CONFIG);
    }

    #[test]
    fn manifest_is_deterministic() {
        let a = manifest_text(&render(0, 8000));
        let b = manifest_text(&render(0, 8000));
        assert_eq!(a, b);
        assert!(a.contains("\"loc_total\": "));
        assert!(a.contains("fnv1a64:"));
    }
}
