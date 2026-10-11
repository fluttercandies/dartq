//! Output manifest and the reproducibility fingerprint.
//!
//! The fingerprint is FNV-1a 64 over, for every generated file in
//! lexicographic (== generation) order: the relative path bytes, a `0x00`
//! separator, the decimal byte size, and another `0x00` separator. Std-only
//! by design; see the crate README for the full contract.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// FNV-1a 64-bit hash.
pub struct Fnv64 {
    state: u64,
}

impl Fnv64 {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: 0xcbf2_9ce4_8422_2325,
        }
    }

    pub fn update(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.state ^= u64::from(byte);
            self.state = self.state.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }

    #[must_use]
    pub fn finish(&self) -> u64 {
        self.state
    }
}

impl Default for Fnv64 {
    fn default() -> Self {
        Self::new()
    }
}

/// Summary of one generation run, serialized to `<out>/manifest.json`.
pub struct Manifest {
    pub seed: u64,
    pub scale: &'static str,
    pub files: usize,
    pub lines: usize,
    pub fingerprint: u64,
    /// File counts bucketed by the first path component; root-level files are
    /// bucketed as `root`. `BTreeMap` keeps the JSON key order deterministic.
    pub files_per_top_level_dir: BTreeMap<String, usize>,
}

impl Manifest {
    /// Renders the manifest as deterministic JSON (sorted keys, trailing
    /// newline).
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut out = String::with_capacity(512);
        out.push_str("{\n");
        writeln!(out, "  \"files\": {},", self.files).expect("string write");
        out.push_str("  \"files_per_top_level_dir\": {");
        for (index, (dir, count)) in self.files_per_top_level_dir.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            write!(out, "\n    \"{dir}\": {count}").expect("string write");
        }
        out.push_str("\n  },\n");
        writeln!(
            out,
            "  \"fingerprint\": \"fnv1a64:{:016x}\",",
            self.fingerprint
        )
        .expect("string write");
        out.push_str("  \"generator\": \"corpus-gen\",\n");
        writeln!(out, "  \"lines\": {},", self.lines).expect("string write");
        writeln!(out, "  \"scale\": \"{}\",", self.scale).expect("string write");
        writeln!(out, "  \"seed\": {}", self.seed).expect("string write");
        out.push_str("}\n");
        out
    }
}
