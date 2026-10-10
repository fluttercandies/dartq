//! Build-time generator for the official diagnostic code table.
//!
//! Parses `data/syntax-codes.tsv` — the authoritative code data captured in
//! `docs/facts/syntax-codes.md` §9, locked to the dart 3.13.1 oracle — and
//! emits `syntax_codes_generated.rs` into `OUT_DIR`, which `src/code.rs`
//! includes. Any malformed row fails the build with a `file:line` message;
//! the emitted file is a pure function of the TSV, so builds are
//! reproducible byte-for-byte.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

/// One validated TSV row (the 6 columns of `data/syntax-codes.tsv`).
struct Row {
    line: usize,
    name: String,
    machine: String,
    sev: String,
    source: String,
    message: String,
    has_published_docs: bool,
}

fn main() {
    let manifest_dir_var = env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let manifest_dir = Path::new(&manifest_dir_var);
    let tsv_path = manifest_dir.join("data/syntax-codes.tsv");
    println!("cargo:rerun-if-changed=data/syntax-codes.tsv");

    let raw = fs::read_to_string(&tsv_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", tsv_path.display()));

    let mut rows = Vec::new();
    for (no, line) in raw.lines().enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        assert!(
            fields.len() == 6,
            "data/syntax-codes.tsv:{}: expected 6 tab-separated columns, got {} \
             (the message column must not contain tabs)",
            no + 1,
            fields.len()
        );
        let has_published_docs = match fields[5] {
            "true" => true,
            "false" => false,
            other => panic!(
                "data/syntax-codes.tsv:{}: hasPublishedDocs must be true or false, got {other:?}",
                no + 1
            ),
        };
        rows.push(Row {
            line: no + 1,
            name: fields[0].to_owned(),
            machine: fields[1].to_owned(),
            sev: fields[2].to_owned(),
            source: fields[3].to_owned(),
            message: fields[4].to_owned(),
            has_published_docs,
        });
    }
    validate(&rows);
    emit(&rows);
}

/// Structural validation over the whole table; panics with the offending
/// data line on the first violation.
fn validate(rows: &[Row]) {
    let mut names: Vec<&str> = Vec::with_capacity(rows.len());
    for row in rows {
        let Row {
            line,
            name,
            machine,
            sev,
            source,
            message,
            ..
        } = row;
        for (col, value) in [("name", name), ("MACHINE", machine), ("message", message)] {
            assert!(
                !value.is_empty(),
                "data/syntax-codes.tsv:{line}: empty {col}"
            );
        }
        // Stray whitespace in any column is a byte-diff class error (a
        // trailing space in `message` would flow into `message_template` and
        // then into `problemMessage` output); reject it at build time.
        for (col, value) in [
            ("name", name),
            ("MACHINE", machine),
            ("SEV", sev),
            ("SOURCE", source),
            ("message", message),
        ] {
            assert!(
                value == value.trim(),
                "data/syntax-codes.tsv:{line}: {col} {value:?} has leading or trailing whitespace"
            );
        }
        assert!(
            sev == "ERROR" || sev == "WARNING",
            "data/syntax-codes.tsv:{line}: SEV must be ERROR or WARNING, got {sev:?}"
        );
        // `source` = oracle registry class + same-name const count; the first
        // char picks the machine TYPE in `dtype_of`. An unknown class must be
        // a build error, not a silent SYNTACTIC_ERROR fallback.
        assert!(
            matches!(source.as_bytes().first(), Some(b'P' | b'S' | b'W' | b'C')),
            "data/syntax-codes.tsv:{line}: source {source:?} must start with P, S, W or C"
        );
        assert!(
            machine == &name.to_uppercase(),
            "data/syntax-codes.tsv:{line}: MACHINE {machine:?} is not the uppercase of {name:?}"
        );
        for (index, _) in placeholders(message) {
            assert!(
                index <= 1,
                "data/syntax-codes.tsv:{line}: placeholder index {index} exceeds the \
                 measured upper bound (syntax templates take at most 2 arguments)"
            );
        }
        names.push(name);
    }
    let mut sorted = names.clone();
    sorted.sort_unstable();
    let dupes: Vec<&str> = sorted
        .windows(2)
        .filter(|w| w[0] == w[1])
        .map(|w| w[0])
        .collect();
    assert!(
        dupes.is_empty(),
        "data/syntax-codes.tsv: duplicate code names {dupes:?}"
    );
}

/// Indices of `{N}` placeholders in a template, as `(index, byte offset)`.
///
/// A placeholder is `{`, a non-empty ASCII digit run, then `}` — scanned in
/// a single pass, so multi-digit indices (`{10}`) are validated too (a
/// 3-byte window is blind to them). Literal braces (`"{'"`, `"{}"`) never
/// match because a placeholder requires digits inside.
fn placeholders(message: &str) -> Vec<(u32, usize)> {
    let bytes = message.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let digits = bytes[i + 1..]
                .iter()
                .take_while(|&&b| b.is_ascii_digit())
                .count();
            if digits > 0 && bytes.get(i + 1 + digits) == Some(&b'}') {
                let index: u32 = message[i + 1..i + 1 + digits].parse().unwrap_or_else(|e| {
                    panic!("placeholder index at byte {i} does not fit u32: {e}")
                });
                out.push((index, i));
                i += digits + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// The [`crate::model::DiagnosticType`] variant name for a row, from its
/// oracle registry class (TSV `source` column).
///
/// - `C*` sources: `CompileTimeErrorCode` consts (the ERROR-level
///   language-version rows) — class type `COMPILE_TIME_ERROR`
///   (analyzer-8.4.1 codes.g.dart: `illegalLanguageVersionOverride`
///   `CompileTimeErrorTemplate`, `inconsistentLanguageVersionOverride`
///   `CompileTimeErrorWithoutArguments`).
/// - `W*` sources: `WarningCode` consts (`invalid_language_version_override`)
///   — class type `STATIC_WARNING`.
/// - Everything else: parser/scanner class type `SYNTACTIC_ERROR`.
///
/// Per-row severity overrides the class severity (facts §2/§3); the emitted
/// SEVERITY comes from the diagnostics, the TYPE from this mapping.
/// `COMPILE_TIME_ERROR` routing of pseudoSharedCode parser errors (facts §2)
/// is an emit-site decision per parse route and is not a property of the
/// table.
fn dtype_of(source: &str) -> &'static str {
    if source.starts_with('C') {
        "CompileTimeError"
    } else if source.starts_with('W') {
        "StaticWarning"
    } else {
        "SyntacticError"
    }
}

fn emit(rows: &[Row]) {
    let out_dir_var = env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let out_dir = Path::new(&out_dir_var);
    let mut out = String::new();
    out.push_str("// @generated by build.rs from data/syntax-codes.tsv; do not edit.\n");
    out.push_str("// Source of truth: docs/facts/syntax-codes.md section 9\n");
    out.push_str("// (dart 3.13.1 oracle lock, captured 2026-10-08).\n");
    out.push_str("// Regenerate: cargo build -p dartq-diagnostics.\n");
    out.push_str("use crate::code::CodeInfo;\n");
    out.push_str("use crate::model::DiagnosticType;\n\n");

    // One constant per code; the MACHINE column is already the SCREAMING_SNAKE
    // Rust identifier (validated above).
    for row in rows {
        let doc = format!("`{}` — {}", row.name, row.message);
        writeln!(out, "#[doc = {}]", rust_str(&doc)).unwrap();
        writeln!(
            out,
            "pub const {}: CodeInfo = CodeInfo::new(\n    {},\n    {},\n    DiagnosticType::{},\n    {},\n);\n",
            row.machine,
            rust_str(&row.name),
            rust_str(&row.machine),
            dtype_of(&row.source),
            row.has_published_docs,
        )
        .unwrap();
    }

    writeln!(
        out,
        "/// All {} official codes (255 parser/scanner syntax codes plus the\n/// 3-code language-version family), in table order.",
        rows.len()
    )
    .unwrap();
    out.push_str("pub static OFFICIAL_CODES: &[CodeInfo] = &[\n");
    for row in rows {
        writeln!(
            out,
            "    CodeInfo::new({}, {}, DiagnosticType::{}, {}),",
            rust_str(&row.name),
            rust_str(&row.machine),
            dtype_of(&row.source),
            row.has_published_docs,
        )
        .unwrap();
    }
    out.push_str("];\n\n");

    out.push_str(
        "/// `(name, oracle SEV, message template, hasPublishedDocs)` rows in table order;\n",
    );
    out.push_str(
        "/// backs the consistency tests and [`crate::code::message_template`]. Test-only\n",
    );
    out.push_str("/// payload: emit sites carry their message texts, so the static is compiled\n");
    out.push_str("/// only into test builds.\n");
    out.push_str("#[cfg(test)]\n");
    out.push_str("pub static OFFICIAL_CODE_DATA: &[(&str, &str, &str, bool)] = &[\n");
    for row in rows {
        writeln!(
            out,
            "    ({}, {}, {}, {}),",
            rust_str(&row.name),
            rust_str(&row.sev),
            rust_str(&row.message),
            row.has_published_docs,
        )
        .unwrap();
    }
    out.push_str("];\n");

    fs::write(out_dir.join("syntax_codes_generated.rs"), out).expect("write generated code table");
}

/// Renders `s` as a Rust string literal (escapes `\` and `"`).
fn rust_str(s: &str) -> String {
    let mut quoted = String::with_capacity(s.len() + 2);
    quoted.push('"');
    for c in s.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}
