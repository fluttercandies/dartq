//! Deterministic synthetic Dart corpus generator for dartq stress testing.
//!
//! `corpus-gen` renders a self-contained multi-package Dart project whose
//! size is controlled by `--scale` (`1k`, `10k`, `100k` files). The output is
//! a pure function of `--seed`: the same seed produces byte-identical files
//! and the same `manifest.json` fingerprint on every platform.
//!
//! Generation is streamed to disk file by file; memory stays constant beyond
//! the file currently being rendered. The plan enumerates files in
//! lexicographic path order, which is also the fingerprint order, so neither
//! requires buffering the corpus.

pub mod coverage;
pub mod emit;
pub mod generator;
pub mod manifest;
pub mod plan;
pub mod rng;

#[cfg(test)]
mod tests;
