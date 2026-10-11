//! Throughput benchmark for the dartq parser over a synthesized Dart corpus.
//!
//! Parses a deterministic, synthetic compilation unit through the public
//! [`dartq_parser::parse_unit_with`] entry (scanner token table in, listener
//! events out) with min-of-K timing: every source is parsed K times and the
//! fastest round determines the reported number, mirroring the scanner
//! throughput example (`crates/dartq-scanner/examples/throughput.rs`). The
//! first of the K rounds doubles as the warmup; only the minimum counts.
//!
//! The corpus is synthesized in memory — no RNG, no filesystem — and split
//! into per-construct sections (directives, statement-bodied declarations,
//! expression-bodied members, generics, patterns) so the per-section lines
//! show where parse time goes, the analogue of the scanner bench's per-file
//! breakdown. Each source is pre-scanned once (untimed) so the timer covers
//! parsing only. The sections concatenated in order (directives first, as
//! directive ordering requires) form the aggregate unit behind the
//! `aggregate:` line.
//!
//! Run (from the repository root):
//!
//! ```text
//! cargo run -p dartq-parser --example throughput --release
//! cargo run -p dartq-parser --example throughput --release -- 8
//! ```
//!
//! The optional positional argument is the total synthesized size in MB
//! (default 1). Throughput uses the same 10^6-byte convention as the
//! scanner bench.

use std::fmt::Write as _;
use std::iter::once;
use std::path::Path;
use std::process::exit;
use std::time::{Duration, Instant};

use dartq_language::FeatureSet;
use dartq_parser::{Message, ParserFeatures, ParserListener, TokenSpan, parse_unit_with};
use dartq_scanner::{ScanConfig, TokenTable, scan};
use dartq_source::FileTable;

/// Number of timed rounds; the fastest round determines throughput.
const ROUNDS: usize = 5;

/// Per-construct sections of the synthesized corpus, in combined-unit order.
const SECTION_NAMES: [&str; 5] = [
    "directives",
    "statements",
    "expressions",
    "generics",
    "patterns",
];

/// One section synthesizer, keyed by index with [`SECTION_NAMES`].
const SECTION_UNITS: [fn(u64, &mut String); 5] = [
    synth_directives,
    synth_statements,
    synth_expressions,
    synth_generics,
    synth_patterns,
];

/// One benchmark source, prescanned once so timed rounds do no scanning.
struct Source {
    name: &'static str,
    source: String,
    tokens: TokenTable,
    bytes: u64,
    /// Shortest parse time seen across all rounds.
    min_time: Duration,
    /// Recoverable-error events observed across all rounds.
    errors: usize,
    /// Parser-inserted tokens (closer splits + recovery synthetics) returned
    /// across all rounds.
    inserted: usize,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let target_mb = match (args.next(), args.next()) {
        (None, _) => 1u64,
        (Some(arg), None) => match arg.parse::<u64>() {
            Ok(mb @ 1..) => mb,
            _ => {
                eprintln!("error: expected a positive integer (target MB)");
                usage();
                exit(2);
            }
        },
        (Some(_), Some(_)) => {
            eprintln!("error: expected at most one positional argument (target MB)");
            usage();
            exit(2);
        }
    };

    // Latest-language defaults; the benchmark measures the parser, not the
    // feature gates.
    let config = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let features = ParserFeatures::from_feature_set(FeatureSet::latest());
    let mut table = FileTable::new();
    let file_id = table.intern(Path::new("bench/synthetic.dart"));

    let mut sections: Vec<Source> = SECTION_NAMES
        .iter()
        .zip(SECTION_UNITS)
        .map(|(&name, synth)| {
            let mut source = String::new();
            synth(target_mb * 1_000_000 / 5, &mut source);
            build_source(name, source, config, file_id)
        })
        .collect();
    let corpus_bytes: u64 = sections.iter().map(|s| s.bytes).sum();
    let token_total: usize = sections.iter().map(|s| s.tokens.len()).sum();
    let combined_bytes = corpus_bytes + (sections.len() - 1) as u64;

    // One big unit: all sections concatenated (directives first), the
    // realistic whole-file shape behind the aggregate number.
    let mut combined = build_source(
        "aggregate",
        sections
            .iter()
            .map(|s| s.source.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        config,
        file_id,
    );

    println!("dartq-parser throughput benchmark (min-of-K, K = {ROUNDS})");
    println!(
        "corpus:      synthesized ({} construct sections, {corpus_bytes} bytes, target {target_mb} MB)",
        sections.len()
    );
    println!("convention:  1 MB/s = 10^6 bytes/s");
    println!("per-section (fastest of {ROUNDS} rounds):");

    for _ in 0..ROUNDS {
        for s in sections.iter_mut().chain(once(&mut combined)) {
            let start = Instant::now();
            let (listener, inserted) =
                parse_unit_with(&s.source, &s.tokens, features, Counting::default());
            let elapsed = start.elapsed();
            s.errors += listener.errors;
            s.inserted += inserted.len();
            if elapsed < s.min_time {
                s.min_time = elapsed;
            }
        }
    }

    for s in &sections {
        let secs = s.min_time.as_secs_f64();
        println!(
            "  {:<12} {:>9} B  {:>7.1} MB/s  ({:.3} ms)",
            s.name,
            s.bytes,
            mb_per_s(s.bytes, secs),
            secs * 1e3
        );
    }

    let secs = combined.min_time.as_secs_f64();
    println!(
        "aggregate:   {:.1} MB/s  ({} bytes, one unit, K = {ROUNDS}, fastest round {:.3} ms)",
        mb_per_s(combined.bytes, secs),
        combined.bytes,
        secs * 1e3,
    );
    println!(
        "tokens:      {token_total} prescanned once (untimed; combined unit adds {combined_bytes} B)"
    );
    println!(
        "listener:    {} recoverable errors, {} parser-inserted tokens across {ROUNDS} rounds",
        sections.iter().map(|s| s.errors).sum::<usize>() + combined.errors,
        sections.iter().map(|s| s.inserted).sum::<usize>() + combined.inserted,
    );
}

/// Counts recoverable errors. The synthesized corpus should report zero,
/// which doubles as a sanity check on the input and keeps the parse result
/// observable. All other listener events keep their defaults, so the parser
/// still runs on the zero-cost listener path.
struct Counting {
    errors: usize,
}

impl Default for Counting {
    fn default() -> Self {
        Self { errors: 0 }
    }
}

impl ParserListener for Counting {
    fn handle_recoverable_error(&mut self, _message: Message, _start: TokenSpan, _end: TokenSpan) {
        self.errors += 1;
    }
}

fn usage() {
    eprintln!("usage: cargo run -p dartq-parser --example throughput --release -- [TARGET_MB]");
}

/// Throughput in MB/s, using the 10^6-byte convention.
#[allow(clippy::cast_precision_loss)]
fn mb_per_s(bytes: u64, secs: f64) -> f64 {
    bytes as f64 / 1_000_000.0 / secs
}

/// Repeats one construct's unit until the section reaches its byte target.
fn synth_section(unit: fn(usize, &mut String), target_bytes: u64, out: &mut String) {
    let mut i = 0usize;
    while (out.len() as u64) < target_bytes {
        unit(i, out);
        i += 1;
    }
}

/// Prescans one synthesized source; scanning happens once, outside timing.
fn build_source(
    name: &'static str,
    source: String,
    config: ScanConfig,
    file_id: dartq_source::FileId,
) -> Source {
    let tokens = scan(&source, config, file_id).tokens;
    Source {
        name,
        bytes: source.len() as u64,
        source,
        tokens,
        min_time: Duration::MAX,
        errors: 0,
        inserted: 0,
    }
}

/// Directives with combinators, `deferred as`, and directive metadata, plus
/// a trailing `part` block. The directive-ordering state machine admits
/// `part` only after the last import/export, and the section is parsed as
/// one unit, so the two families stay in canonical order across the repeated
/// units: import/export units fill 90% of the target, then `part` lines
/// close the section.
fn synth_directives(target: u64, out: &mut String) {
    let import_target = target - target / 10;
    let mut i = 0usize;
    while (out.len() as u64) < import_target {
        let _ = write!(
            out,
            "import 'src/lib{i}.dart' show Alpha{i}, Beta{i} hide Gamma{i};\n"
        );
        let _ = write!(out, "import 'src/ext{i}.dart' deferred as ext{i};\n");
        let _ = write!(out, "@Deprecated('superseded by lib{i}')\n");
        let _ = write!(out, "export 'src/api{i}.dart' show Delta{i};\n");
        i += 1;
    }
    let mut j = 0usize;
    while (out.len() as u64) < target {
        let _ = writeln!(out, "part 'src/part{j}.dart';");
        j += 1;
    }
}

/// Classes and top-level functions with full `{ ... }` statement bodies.
fn synth_statements(target: u64, out: &mut String) {
    synth_section(statements_unit, target, out);
}

/// One statements-section unit: two classes (field + constructor + methods,
/// one abstract with an empty-body method) and a top-level function with a
/// `try/on` body.
fn statements_unit(i: usize, out: &mut String) {
    let _ = write!(
        out,
        "\
\n/// Domain entity {i}.
class C{i} implements I{i} {{
  static const int k{i} = {r0};
  final int f{i};

  C{i}(this.f{i});

  int m{i}(int a, int b) {{
    var s = k{i} + f{i};
    for (var j = 0; j < b; j++) {{
      s += a * j - {r1};
    }}
    if (s > f{i}) {{
      return s - f{i};
    }} else if (s < 0) {{
      return -s;
    }}
    return s;
  }}

  void n{i}(List<int> xs) {{
    for (final x in xs) {{
      print('C{i}: $x');
    }}
  }}
}}

abstract class I{i} {{
  int m{i}(int a, int b);
}}

int t{i}(int a) {{
  var r = a;
  while (r > {r2}) {{
    r -= {r3};
  }}
  try {{
    r = r ~/ (a + {r4});
  }} on Exception {{
    r = -1;
  }}
  return r;
}}
",
        r0 = i % 97,
        r1 = i % 13,
        r2 = i % 17,
        r3 = i % 5 + 1,
        r4 = i % 7,
    );
}

/// Members with `=>` bodies: conditionals, spread of literals, cascades,
/// string interpolation, collection `for`.
fn synth_expressions(target: u64, out: &mut String) {
    synth_section(expressions_unit, target, out);
}

/// One expressions-section unit: one class of arrow-bodied members.
fn expressions_unit(i: usize, out: &mut String) {
    let _ = write!(
        out,
        "\
\nclass E{i} {{
  final List<int> seed{i};

  E{i}(this.seed{i});

  int f{i}(int a) => a < {r0} ? a * {r1} : a + seed{i}.length;

  List<int> g{i}() => [for (var k = 0; k < {r2}; k++) k * {r3}, {r4}, -{r5}];

  Map<String, int> h{i}() => <String, int>{{'a{i}': {r0}, 'b{i}': {r1}}};

  String s{i}(Object o) => 'E{i}(${{o.toString()}})={r2}';

  Set<int> w{i}() => <int>{{{r3}, {r4}}}..add({r5})..remove({r0});
}}
",
        r0 = i % 31,
        r1 = i % 13 + 1,
        r2 = i % 11 + 1,
        r3 = i % 9,
        r4 = i % 7,
        r5 = i % 5,
    );
}

/// Generic declarations: bounded type variables, nested type arguments
/// (`>>`/`>>>` splitting), function types, record type aliases.
fn synth_generics(target: u64, out: &mut String) {
    synth_section(generics_unit, target, out);
}

/// One generics-section unit: a bounded generic class, two new-style
/// typedefs, and a generic top-level function.
fn generics_unit(i: usize, out: &mut String) {
    let _ = write!(
        out,
        "\
\nclass G{i}<T extends Comparable<T>, U extends Object>
    implements Comparable<G{i}<T, U>> {{
  final Map<String, List<Map<int, T>>> table{i};

  G{i}(this.table{i});

  @override
  int compareTo(G{i}<T, U> other) => table{i}.length.compareTo(other.table{i}.length);

  V fold{i}<V>(V init, V Function(V, T) combine) {{
    var acc = init;
    for (final values in table{i}.values) {{
      for (final v in values) {{
        acc = combine(acc, v);
      }}
    }}
    return acc;
  }}
}}

typedef Predicate{i}<T> = bool Function(T value, [int flag{i}]);
typedef Pair{i} = (int, String, {{bool ok}});

void top{i}<T extends num>(List<List<T>> grid, Map<String, Set<T>> index) {{
  for (final row in grid) {{
    for (final v in row) {{
      index.putIfAbsent('$v', Set<T>.new).add(v);
    }}
  }}
}}
",
    );
}

/// Dart 3 patterns: switch statements (list/map/record/object patterns with
/// guards), switch expressions, and `if-case`.
fn synth_patterns(target: u64, out: &mut String) {
    synth_section(patterns_unit, target, out);
}

/// One patterns-section unit: a switch statement, a switch expression, and
/// an `if-case` guard.
fn patterns_unit(i: usize, out: &mut String) {
    let _ = write!(
        out,
        "\
\nString p{i}(Object o) {{
  switch (o) {{
    case int n when n > {r0}:
      return 'int $n';
    case String s when s.length == {r1}:
      return 'str $s';
    case [var head, ...var tail] when tail.length > {r2}:
      return 'list $head + ${{tail.length}}';
    case {{'name': String name, 'v': int v}}:
      return 'map $name=$v';
    case (int a, String b):
      return 'record $a$b';
    case C0(f0: var f):
      return 'obj $f';
    default:
      return 'other';
  }}
}}

int e{i}(Object o) => switch (o) {{
      int n when n > {r3} => n * 2,
      String s => s.length + {r4},
      [int a, int b] => a + b,
      (int x, int y) => x + y * {r5},
      _ => {r6},
    }};

bool guard{i}(Object o) {{
  if (o case int n when n % {r7} == 0) {{
    return true;
  }}
  return false;
}}
",
        r0 = i % 41,
        r1 = i % 8 + 1,
        r2 = i % 3,
        r3 = i % 29,
        r4 = i % 3,
        r5 = i % 7 + 1,
        r6 = i % 11,
        r7 = i % 6 + 2,
    );
}
