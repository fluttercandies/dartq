//! `--emit-replay-seq <n>`: a deterministic `edits.jsonl` replay sequence
//! over the generated tree (axis 2a).
//!
//! The schema matches `tests/corpus/incremental/*\/edits.jsonl` and the
//! harness `replay_edit` parser exactly: one JSON object per line, one of
//! three forms — in-file UTF-16 splice (`file`/`line`/`column`/`length`/
//! `new_text`), `{"remove": path}`, `{"create": path, "text": full}`.
//!
//! The sequence is composed of self-inverting units — every unit returns the
//! tree to byte-identical state, so the whole sequence round-trips. Unit
//! kinds rotate deterministically on a per-seed stream: comment-only noise
//! splices, class-rename waves across the 6-link inheritance chain, arity
//! waves on the same chain, and delete/create pairs on an imported facet
//! file.
//!
//! Generation order note: this module only READS content captured during the
//! corpus pass; it adds no files to the plan and touches nothing the
//! fingerprint covers.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;

use crate::rng::SplitMix64;

/// Corpus-relative paths captured during generation for replay use. All are
/// emitted at every scale (chain links are fixed at 6; `feature_0000` and the
/// first controller/test exist at 1k and up).
pub const CAPTURE_PATHS: &[&str] = &[
    "lib/main.dart",
    "lib/src/controllers/controller_00000.dart",
    "packages/core/lib/src/interfaces/chain_link_0.dart",
    "packages/core/lib/src/interfaces/chain_link_1.dart",
    "packages/core/lib/src/interfaces/chain_link_2.dart",
    "packages/core/lib/src/interfaces/chain_link_3.dart",
    "packages/core/lib/src/interfaces/chain_link_4.dart",
    "packages/core/lib/src/interfaces/chain_link_5.dart",
    "packages/feature_0000/lib/src/facets/accel_stub.dart",
    "tests/unit_00000_test.dart",
];

#[must_use]
pub fn is_capture(path: &str) -> bool {
    CAPTURE_PATHS.contains(&path)
}

/// One `edits.jsonl` line, rendered as JSON text (stdlib manual escaping —
/// the corpus content is ASCII/UTF-8, and the only escapes needed inside a
/// JSON string are `"`, `\` and control characters).
fn push_json_string(out: &mut String, raw: &str) {
    out.push('"');
    for ch in raw.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                write!(out, "\\u{:04x}", c as u32).expect("string write");
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn step_splice(out: &mut String, file: &str, line: u32, column: u32, length: u32, new_text: &str) {
    out.push_str("{\"file\":");
    push_json_string(out, file);
    write!(
        out,
        ",\"line\":{line},\"column\":{column},\"length\":{length},\"new_text\":"
    )
    .expect("string write");
    push_json_string(out, new_text);
    out.push_str("}\n");
}

fn step_remove(out: &mut String, file: &str) {
    out.push_str("{\"remove\":");
    push_json_string(out, file);
    out.push_str("}\n");
}

fn step_create(out: &mut String, file: &str, text: &str) {
    out.push_str("{\"create\":");
    push_json_string(out, file);
    out.push_str(",\"text\":");
    push_json_string(out, text);
    out.push_str("}\n");
}

/// UTF-16 (line, column) — 1-based per the replay schema — of the scalar
/// `byte_offset` inside `text`. The caller guarantees `offset` sits on a char
/// boundary.
fn line_col_utf16(text: &str, byte_offset: usize) -> (u32, u32) {
    let mut line = 1u32;
    let mut col = 1u32;
    for (i, ch) in text.char_indices() {
        if i >= byte_offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += u32::try_from(ch.len_utf16()).expect("utf16 length fits u32");
        }
    }
    (line, col)
}

fn utf16_len(text: &str) -> u32 {
    text.chars()
        .map(|c| u32::try_from(c.len_utf16()).expect("utf16 length fits u32"))
        .sum()
}

/// Locates the `occurrence`-th (0-based) appearance of `needle` in `haystack`
/// and returns its `(line, column)` plus the needle's UTF-16 length.
fn locate(haystack: &str, needle: &str, occurrence: usize) -> Option<(u32, u32, u32)> {
    let mut offset = 0usize;
    for _ in 0..=occurrence {
        offset = haystack[offset..].find(needle)? + offset + needle.len();
    }
    let start = offset - needle.len();
    let (line, col) = line_col_utf16(haystack, start);
    Some((line, col, utf16_len(needle)))
}

/// Emits `splice(needle -> new_text)` followed by its exact inverse
/// (`new_text -> needle` at the same position), keeping the unit
/// self-inverting.
fn splice_pair(
    out: &mut String,
    text: &str,
    file: &str,
    needle: &str,
    new_text: &str,
    occurrence: usize,
) -> bool {
    let Some((line, col, len)) = locate(text, needle, occurrence) else {
        return false;
    };
    step_splice(out, file, line, col, len, new_text);
    // Inverse step: the replacement text now occupies `len'` units at the
    // same position.
    step_splice(out, file, line, col, utf16_len(new_text), needle);
    true
}

/// Writes `<out>/edits.jsonl` with `steps` replay steps, composed of complete
/// self-inverting units so the final tree state equals the initial state.
///
/// `captures` maps corpus-relative paths to their generated content; only
/// [`CAPTURE_PATHS`] are stored during the corpus pass.
///
/// # Errors
///
/// Returns `io::Error` from writing `edits.jsonl`.
///
/// # Panics
///
/// Never in practice: the only `expect` indexes `CAPTURE_PATHS` with a value
/// `below` its length, which cannot fail. The lint-visible panic is a
/// bound-check formality.
pub fn emit(
    out_dir: &Path,
    steps: u64,
    captures: &BTreeMap<String, String>,
    seed: u64,
) -> io::Result<()> {
    let mut body = String::with_capacity(64 * 1024);
    let mut rng = SplitMix64::new(seed ^ 0x5DE0_1A11_9E55_2B3C);
    let mut emitted = 0u64;
    // `CAPTURE_PATHS[2..8]` are the six `chain_link_{0..5}.dart` files.
    let chain = &CAPTURE_PATHS[2..8];

    // Round-trip units and their exact step counts. The loop picks a unit
    // that fits the remaining budget exactly; the 2-step splice pair and the
    // 1-step identity splice absorb any tail so `steps` is hit exactly.
    while emitted < steps {
        let remaining = steps - emitted;
        if remaining == 1 {
            // A zero-length insert of empty text is a schema-valid identity
            // splice on any captured file.
            let file =
                CAPTURE_PATHS[usize::try_from(rng.below(CAPTURE_PATHS.len() as u64)).expect("idx")];
            step_splice(&mut body, file, 1, 1, 0, "");
            emitted += 1;
            continue;
        }
        // Unit step counts: comment-pair = 2, triple = 4, waves = 22. The
        // pick is capped to units that fit the remaining budget so the
        // sequence lands exactly on `steps` (1-step noop handles the odd tail).
        let pick = match remaining {
            r if r < 4 => 0,
            r if r < 22 => rng.below(2),
            _ => rng.below(4),
        };
        match pick {
            // Comment-only noise splice pair on a random captured file.
            0 => {
                let file = CAPTURE_PATHS
                    [usize::try_from(rng.below(CAPTURE_PATHS.len() as u64)).expect("idx")];
                let text = &captures[file];
                // Insert at the end of line 1 (the comment marker line).
                let marker = format!(" // rw-noise-{}", rng.next_u64() % 10_000);
                let line1_len = utf16_len(text.split('\n').next().unwrap_or(""));
                step_splice(&mut body, file, 1, line1_len + 1, 0, &marker);
                step_splice(&mut body, file, 1, line1_len + 1, utf16_len(&marker), "");
                emitted += 2;
            }
            // splice + remove + create triple: mark a file, delete the
            // imported facet stub, recreate it byte-exact, unmark. Nets to
            // identity as a 4-step unit.
            1 => {
                let stub = "packages/feature_0000/lib/src/facets/accel_stub.dart";
                let main = "lib/main.dart";
                let marker = format!(" // rw-mark-{}", rng.next_u64() % 10_000);
                let main_text = &captures[main];
                let line1_len = utf16_len(main_text.split('\n').next().unwrap_or(""));
                step_splice(&mut body, main, 1, line1_len + 1, 0, &marker);
                step_remove(&mut body, stub);
                step_create(&mut body, stub, &captures[stub]);
                step_splice(&mut body, main, 1, line1_len + 1, utf16_len(&marker), "");
                emitted += 4;
            }
            // Class-rename wave across the 6-link inheritance chain:
            // `ChainNode{k}` -> `ChainNode{k}R` in the declaring file and in
            // the `extends` clause of link k+1, each edit paired with its
            // inverse.
            2 => {
                for k in 0..6usize {
                    let file = chain[k];
                    let text = &captures[file];
                    let from = format!("ChainNode{k}");
                    let to = format!("ChainNode{k}R");
                    splice_pair(&mut body, text, file, &from, &to, 0);
                    if k + 1 < 6 {
                        let next_file = chain[k + 1];
                        let next_text = &captures[next_file];
                        splice_pair(&mut body, next_text, next_file, &from, &to, 0);
                    }
                }
                emitted += 22; // 6 decl + 5 extends sites, paired
            }
            // Arity wave on `depth()` down the chain: `int depth()` gains an
            // `int v` parameter and `super.depth()` passes `0`, paired.
            _ => {
                for (k, &file) in chain.iter().enumerate() {
                    let text = &captures[file];
                    splice_pair(&mut body, text, file, "int depth()", "int depth(int v)", 0);
                    if k > 0 {
                        splice_pair(&mut body, text, file, "super.depth()", "super.depth(0)", 0);
                    }
                }
                emitted += 22; // 6 decl + 5 call sites, paired
            }
        }
    }
    fs::write(out_dir.join("edits.jsonl"), body)
}
