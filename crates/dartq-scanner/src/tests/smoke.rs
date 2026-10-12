//! Legality golden smoke: scans the official scanner's own source (~2900 lines
//! of real Dart) and a ~1MB synthetic source (release <5ms, PRD §19.3
//! performance axis). Automatically skipped unless `DARTQ_SDK_CHECKOUT` is
//! set to a dart-lang/sdk checkout.

use std::path::PathBuf;

use super::{config, file_id};
use crate::scan;

/// The official scanner source inside a dart-lang/sdk checkout
/// (`DARTQ_SDK_CHECKOUT`), the same file the upstream scanner tests scan.
fn official_scanner_path() -> PathBuf {
    PathBuf::from(std::env::var("DARTQ_SDK_CHECKOUT").unwrap_or_default())
        .join("pkg/_fe_analyzer_shared/lib/src/scanner/abstract_scanner.dart")
}

#[test]
fn smoke_official_scanner_source() {
    let path = official_scanner_path();
    let Ok(src) = std::fs::read_to_string(&path) else {
        eprintln!(
            "skip: set DARTQ_SDK_CHECKOUT to a dart-lang/sdk checkout to scan {}",
            path.display()
        );
        return;
    };
    let r = scan(&src, config(), file_id());
    assert!(r.tokens.len() > 5_000, "token count {}", r.tokens.len());
    assert_eq!(r.tokens.last().kind, crate::TokenKind::Eof);
    // Valid Dart should produce near-zero diagnostics (upper-bound guard:
    // crash-like unexpected behavior is not allowed).
    assert!(
        r.diagnostics.len() <= 5,
        "unexpected diagnostics: {:?}",
        r.diagnostics
            .iter()
            .map(|d| (d.code.name, d.range.offset))
            .collect::<Vec<_>>()
    );
    eprintln!(
        "smoke ok: {} bytes, {} tokens, {} diagnostics, version={:?}",
        src.len(),
        r.tokens.len(),
        r.diagnostics.len(),
        r.language_version_override
    );
}

#[test]
fn perf_one_megabyte() {
    // ~1MB realistic source: the official scanner source concatenated repeatedly
    // (includes strings/comments/interpolation/generics).
    let Some(base) = std::fs::read_to_string(official_scanner_path()).ok() else {
        eprintln!("skip: official source not present");
        return;
    };
    let mut src = String::with_capacity(1 << 21);
    while src.len() < 1 << 20 {
        src.push_str(&base);
    }
    // Two warm-up passes, then take the minimum of 7 passes (removes scheduling
    // noise; PRD §19.3 metrics are steady-state figures).
    for _ in 0..2 {
        let warm = scan(&src, config(), file_id());
        assert!(warm.tokens.len() > 100_000);
    }
    let mut best = std::time::Duration::MAX;
    let mut last = None;
    for _ in 0..7 {
        let start = std::time::Instant::now();
        let r = scan(&src, config(), file_id());
        best = best.min(start.elapsed());
        last = Some(r);
    }
    let elapsed = best;
    let r = last.unwrap();
    eprintln!(
        "perf: {} bytes, {} tokens, {} diags, {:?}",
        src.len(),
        r.tokens.len(),
        r.diagnostics.len(),
        elapsed
    );
    if cfg!(debug_assertions) {
        assert!(
            elapsed.as_millis() < 200,
            "debug scan too slow: {elapsed:?}"
        );
    } else {
        assert!(
            elapsed.as_millis() < 5,
            "release scan exceeds 5ms: {elapsed:?}"
        );
    }
}

#[test]
fn smoke_realistic_dart_program() {
    let src = r"
// @dart = 3.6
import 'dart:async' show Future, Stream;

/// Doc comment: with generics and interpolation.
abstract class Repo<T extends Object> {
  Future<List<T>> query(String sql, {int? limit});

  static const int kMax = 0x7F_FF;
}

void main() async {
  final repo = Repo<String>();
  var items = await repo.query('SELECT * FROM t WHERE id = ${1 + 2}');
  for (var i = 0; i < items.length; i++) {
    print('item #$i: ${items[i].toUpperCase()}');
  }
  const triple = 1 >>> 2;
  switch (items) {
    case [var first, ...var rest] when rest.isNotEmpty:
      print('$first + ${rest.length}');
    default:
      break;
  }
}
";
    let r = scan(src, config(), file_id());
    assert!(r.tokens.len() > 150);
    assert!(
        r.diagnostics.is_empty(),
        "diagnostics: {:?}",
        r.diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        r.language_version_override,
        Some(dartq_language::LanguageVersion::new(3, 6))
    );
}
