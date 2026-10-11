# dartq

dartq is a semantic analysis engine for Dart and Flutter, written in Rust.
It targets CLI and CI usage as a replacement for `dart analyze`, with AI
coding agents as its first-class users: machine-readable output, incremental
re-analysis, a bounded memory footprint, and multi-SDK support. The official
Dart analyzer is the semantic oracle — dartq replicates its diagnostics and
exceeds its performance envelope, and every semantic claim is validated by
differential testing against `dart analyze`. The governing charter fixes
the priority order once: correctness against the oracle is the
non-negotiable floor, and above that floor extreme performance and a
bounded memory footprint outrank every other consideration.

## Design Pillars

1. **Performance** — end-to-end budgets: cold/warm start, incremental
   latency, cache hit rate, parallelism; controlled memory is the premise,
   not the goal.
2. **Design** — one-way layering, a strict identity model, a single source
   of truth per fact, data-driven evolution.
3. **AI-first** — every API treats AI agents as first-class users:
   structured output, impact prediction, cancellation, concurrency.
4. **Token economy** — output is a cost: tiered verbosity, stable
   references, deltas, batch queries; fewest tokens for equal information.
5. **Extensibility** — new Dart versions, experiments, platforms, and
   plugins are "add data / add rules", never "change the architecture".
6. **Analyzer replacement** — `dartq analyze` / `dartq check` take the
   place of `dart analyze` (machine-readable superset), measured against
   the official analyzer as the oracle.

## Build

```bash
cargo build -p dartq-cli          # the binary lands at target/debug/dartq
cargo build --release -p dartq-cli
cargo test --workspace
```

The toolchain is pinned by `rust-toolchain.toml` (Rust 1.96.0, edition
2024, resolver "3"). `Cargo.lock` is committed: the workspace ships
binaries (`dartq-cli`, `harness`). Concurrent cargo invocations share
`target/` under a workspace-pinned job cap (`.cargo/config.toml`,
`jobs = 2`).

## Command Surface

`dartq` has eleven commands. Every flag below is parsed by the CLI — run
`dartq <command> --help` for the full grammar.

| Command | Purpose |
| --- | --- |
| `analyze` | Full engine over a package directory: scan, parse, resolution |
| `check` | Scan + parse only — the differential face against `dart analyze` |
| `compat` | One file under several SDK roots: a per-axis compatibility matrix |
| `daemon` | LSP 3.17 server over stdin/stdout |
| `explain` | One-stop AI report for one symbol over the analyzed workspace |
| `fix` | Emit or apply anchored fix patches (`--dry-run` only prints them) |
| `assists` | Query plugin hosts' assists for one file location (report-only) |
| `index` | Build the symbol index over the analyzed workspace and report what it built |
| `impact` | Assess what a change to one file would affect |
| `query` | Query the symbol index — faces: `symbol`, `references`, `callers` |
| `version` | Print the version |

```bash
# Scan + parse a directory tree (the differential face): syntax
# diagnostics in the oracle's machine format by default.
dartq check lib/ test/
dartq check lib/ --format=json

# Full engine over a package directory (needs pubspec.yaml and
# .dart_tool/package_config.json): pub faces, discovery, scan, parse,
# resolution.
dartq analyze ~/src/my_app
dartq analyze ~/src/my_app --format=human

# Symbol index: declarations of one name plus resolved references to them.
dartq query symbol Foo
dartq query symbol Foo --format=json

# Impact: what depends on one file's API surface (a hypothetical change —
# a CLI run holds no edit baseline; the daemon passes real diffs).
dartq impact lib/models/user.dart

# Compatibility matrix: one file analyzed under several SDK roots.
dartq compat lib/api.dart --sdk-root ~/fvm/versions/3.5.0 \
                           --sdk-root ~/fvm/versions/3.13.0
# Or through dartq.yaml contexts (see below); both flags are repeatable.
dartq compat lib/api.dart --context beta

# Fixes: --dry-run only prints the patch document; without it, edits
# apply in place (drift-corrected through their anchors).
dartq fix lib/ --dry-run
dartq fix lib/

# The daemon: an LSP server over stdin/stdout (see "Daemon" below).
dartq daemon ~/src/my_app

# Version.
dartq version
dartq --version
```

Severity-sensitive flags mirror the oracle on `check`/`analyze`:
warnings are fatal by default (`--no-fatal-warnings` clears that), infos
are not (`--fatal-infos` opts in). `check`/`analyze` also accept
`--metrics-json PATH` — one JSON object at exit with file/LOC/diagnostic
counts, phase timings, library cache counters, and peak RSS.

## Exit Codes

The diagnostics faces (`check`, `analyze`) use dartdev's analyze
semantics — the oracle-parity tiers:

| Code | Meaning |
| --- | --- |
| 0 | clean, or infos present without `--fatal-infos` |
| 1 | fatal infos (`--fatal-infos`) |
| 2 | fatal warnings (the default) |
| 3 | any error |
| 4 | internal failure (stdout stays empty — a partial set never poses as a result) |
| 64 | usage error (bad flags, missing paths/package config/SDK) |

The query-family faces (`query`, `impact`, `compat`, `fix`) use the query
tiers: 0 on a successful query — zero results and incompatible compat
axes are the answer's data, not failures; stale fix anchors are safe
no-ops — plus 64 usage and 4 internal.

## Daemon

`dartq daemon <dir>` serves LSP 3.17 over stdin/stdout: hand-written
`Content-Length` framing, JSON-RPC 2.0, one client. Lifecycle is standard
(`initialize` → `initialized` → requests → `shutdown` → `exit`); whole-file
overlay edits arrive through `textDocument/didOpen`/`didChange`/`didClose`
(never touching disk), full diagnostic sets push via
`textDocument/publishDiagnostics`, and `$/cancelRequest` cancels a queued
read. Each turn batches all queued work — N edits collapse into one apply
plus one analysis.

Six `dartq/*` methods mirror the CLI commands one-to-one (announced under
`capabilities.experimental`): `dartq/analyze` (merged diagnostics, document
URIs), `dartq/check`, `dartq/query`, `dartq/impact`, `dartq/fix`, and
`dartq/compat` (canonical path labels; `dartq/compat` requires explicit
`sdk_roots` — the CLI's `$DART_SDK` fallback has no daemon equivalent).
Every answer carries `schema_version` and honors an optional `verbosity`
tier. Example frame:

```text
Content-Length: 71

{"jsonrpc":"2.0","id":1,"method":"dartq/query","params":{"name":"Foo"}}
```

## Workspace Config: dartq.yaml

An optional `dartq.yaml` at the workspace root configures the faces. It is
all-optional — an absent file is the zero-config default — unknown keys are
ignored, and an optional `version` key (currently `1`) guards the shape.
Today `contexts` feeds `dartq compat`; the other sections are parsed and
validated ahead of their consumers.

```yaml
version: 1

# Named SDK contexts — `dartq compat --context <label>` axes, in file order.
contexts:
  stable:
    sdk: system            # $DART_SDK
  beta:
    sdk: fvm:3.13.0        # an FVM-installed version
  pinned:
    sdk: path:sdks/dart-3.5  # relative to the workspace root
    target: io             # io | web | wasm
    define:                # -D variables for the context's conditional env
      MY_FLAG: "1"

# Generated-file policy.
generated:
  include: false
  editable: false
  patterns: ["*.g.dart", "*.freezed.dart"]

memory:
  limit_mib: 2048
cache:
  size_limit_gib: 8
```

## Performance

End-to-end numbers are measured over the M0 corpus (279 files, 22,656
LOC) with **debug builds** of `dartq check` — labeled as measured, not
projected:

| Metric | dartq | Comparison |
| --- | --- | --- |
| Incremental re-run, p95 | 13.97 ms | `dart analyze` ≈ 555 ms per step (≈ 40x) |
| Peak RSS over the corpus | 7.2 MiB (≈ 31.6 MiB per 100k LOC) | — |
| Machine-format output bytes | 175,114 | oracle 435,207 (see note) |

The output-volume row compares identical machine-format faces, but the
oracle currently emits a strictly larger diagnostic set (full analysis vs
dartq's syntax tier), so the gap reflects the surface difference as much
as output economy.

Component microbenchmarks (release builds, Apple M1-class arm64):

| Component | Measurement |
| --- | --- |
| Scanner | 260.5 MB/s single-thread over the `dart-sdk` package corpus |
| Type system | subtype chain 4.7M ops/s; interner hit path 29M ops/s; 64 B per interned type |
| Source layer | ~1.16 MB of persistent storage per MB of source (content-addressed) |

Design targets (budgets from the charter, not claims): ≥ 10x faster than
`dart analyze` end-to-end as the shipping floor, 100x–1000x on cold
analysis of large repositories, 1000x+ on incremental re-analysis after
small edits.

## Architecture

One-way layering, one crate per concern. 25 `dartq-*` workspace crates,
plus the differential-testing harness and the corpus generator
(`tools/corpus-gen`). Status = what ships through the CLI/daemon faces
today:

| Layer | Crate | Role | Status |
| --- | --- | --- | --- |
| Foundation | [`dartq-source`](crates/dartq-source) | Content-addressed source storage: `ContentId` (blake3), source store, `FileId`, locators | landed |
| Foundation | [`dartq-language`](crates/dartq-language) | Feature registry generated at build time from the SDK's `experimental_features.yaml`; `LanguageVersion` / `FeatureSet` resolution | landed |
| Foundation | [`dartq-diagnostics`](crates/dartq-diagnostics) | Stable diagnostic code registry and `machine` / `json` writers byte-compatible with `dart analyze --format=machine` | landed |
| Foundation | [`dartq-schema`](crates/dartq-schema) | Versioned machine-readable JSON output schemas (frozen v1) + CI validator | landed |
| Frontend | [`dartq-scanner`](crates/dartq-scanner) | Full Dart 3.15 tokenizer: single pass, no backtracking, `@dart` version-comment window, error recovery → linear token table + syntax diagnostics | landed |
| Frontend | [`dartq-parser`](crates/dartq-parser) | Fault-tolerant recursive-descent parser for the full Dart 3.15 grammar: token table in, parser-event stream out | landed |
| Frontend | [`dartq-ast`](crates/dartq-ast) | Compact arena AST: nodes reference token ranges, spans derive from the token table (byte + UTF-16) | landed |
| Environment | [`dartq-pub`](crates/dartq-pub) | Consumers for pub's resolution artifacts (`package_config.json`, `pubspec.yaml`, `pubspec.lock`), workspace discovery, freshness, resolved package graphs; no version solving | landed |
| Environment | [`dartq-package`](crates/dartq-package) | Typed package-identity model consumed by resolution, composed on `dartq-pub`'s artifacts | landed |
| Environment | [`dartq-sdk`](crates/dartq-sdk) | SDK metadata: `libraries.json` loading, identity, `dart:` library surface, target/platform scoping | landed |
| Semantics | [`dartq-types`](crates/dartq-types) | Type system M1 surface: ten-shape type skeleton, nullability dimension, canonical interning, subtype / assignability, least upper bound | landed |
| Semantics | [`dartq-resolution`](crates/dartq-resolution) | URI, import/export/part, and conditional-import resolution with namespace computation | landed |
| Semantics | [`dartq-semantic`](crates/dartq-semantic) | Element, reference, and symbol arenas forming the semantic graph | in development |
| Semantics | [`dartq-flow`](crates/dartq-flow) | Event-driven flow analysis, decoupled from the AST | in development |
| Services | [`dartq-index`](crates/dartq-index) | Reference, inheritance, and test graphs | landed |
| Services | [`dartq-incremental`](crates/dartq-incremental) | Invalidation engine on signature hashes and dependency fingerprints | namespace placeholder — implementation lives in dartq-engine |
| Services | [`dartq-cache`](crates/dartq-cache) | Layered cache and eviction policies | namespace placeholder — implementation lives in dartq-engine (L1) |
| Services | [`dartq-snapshot`](crates/dartq-snapshot) | Persistence for analysis contexts and snapshots | partial — flush machinery only |
| Services | [`dartq-engine`](crates/dartq-engine) | Runtime, scheduler, cancellation, overlay management | landed |
| Services | [`dartq-lints`](crates/dartq-lints) | Built-in lint rule set (64 official core/recommended rules) | rules implemented; CLI wiring pending |
| Services | [`dartq-plugin`](crates/dartq-plugin) | Plugin protocol v1 IPC types: frames, codec, handshake, manifest | protocol landed; core spawn pending |
| Interface | [`dartq-ai`](crates/dartq-ai) | Agent-facing queries: impact, explain, compatibility, fix | landed |
| Interface | [`dartq-commands`](crates/dartq-commands) | Per-command cores shared by the CLI and daemon faces: plain parameters in, typed results out — no parsing, printing, or exit codes | landed |
| Interface | [`dartq-cli`](crates/dartq-cli) | Command-line entry point | landed |
| Interface | [`dartq-daemon`](crates/dartq-daemon) | LSP 3.17 server over stdin/stdout: turn batching, overlay edits, plugin host, `dartq/*` methods, publish pushes | landed |
| Testing | [`harness`](tests/harness) | Differential-testing driver: oracle vs dartq over shared corpora, `(code, file)` Jaccard comparison | landed |

`crates/dartq-plugin-host/` is the host-side Dart package of the plugin
protocol and is not part of the Cargo workspace.

## Differential Testing

`tests/harness` runs the official analyzer (`dart analyze --format=machine`)
and dartq over the same corpora and compares results as `(code, file)`
entry sets (Jaccard). Corpora live in `tests/corpus/`: `m0` (assembled
analyzer-suite cases), `m0-syntax` (syntax-broken files), and the syntax
gate's code list `syntax-codes.txt` (255 codes, the scanner/parser subset
of `crates/dartq-diagnostics/data/syntax-codes.tsv`).

Current measured baseline (oracle: `dart analyze` 3.13.1, `--e2e-oracle`
self-check, both sides the analyzer — Jaccard 1.000 across all runs):

| Run | Corpus | Caliber | Files with diags | Instances | Jaccard |
| --- | --- | --- | ---: | ---: | ---: |
| full | `m0` | all | 166 | 1844 | 1.000 |
| syntax | `m0-syntax` | codes ∩ ERROR | 272 | 2144 | 1.000 |
| full | `m0` + `m0-syntax` | all | 439 | 6873 | 1.000 |

The M0 gate is the syntax-caliber run against the real dartq side,
passing at ERROR-tier Jaccard ≥ 0.99. Honest state of the recorded
evidence: the latest real-side gate run on disk
(`docs/audits/gate-m0/gate-caliber-r5`) failed at Jaccard 0.620 over a
corpus shape that has since been reorganized, so no current-tree gate
evidence exists yet — the ≥ 0.99 passing baseline is to be
re-established by the upcoming acceptance runs:

```bash
cargo test -p harness          # harness unit tests
cargo run -p harness -- corpus # assemble corpora and run pub get

# Comparator self-check: both sides dart analyze, Jaccard must be 1.000.
cargo run -p harness -- run --e2e-oracle \
  --corpus-dir tests/corpus/m0 --corpus-dir tests/corpus/m0-syntax \
  --severity-filter ERROR --codes-file tests/corpus/syntax-codes.txt

# Real differential run: swap --e2e-oracle for the dartq driver.
cargo run -p harness -- run \
  --corpus-dir tests/corpus/m0 --corpus-dir tests/corpus/m0-syntax \
  --severity-filter ERROR --codes-file tests/corpus/syntax-codes.txt \
  --dartq-cmd "cargo run -q -p dartq-cli -- check --format=machine"
```

Exit codes: `0` = gate met, `1` = below threshold, `2` = harness error.
Preflight requires every corpus package to carry a
`.dart_tool/package_config.json` with `languageVersion >= 3.13`.
`--oracle-dart`, `--type-filter`, and `--threshold` are also available; see
`cargo run -p harness -- --help`.

The M1 gate is the frozen-code semantic run over `tests/corpus/m1-semantic`
(`tests/harness/m1_codes.txt`, ERROR tier, bar 0.95 — deliberately below
the M0 0.99), driven by `scripts/m1-gate.sh`. Honest state of the recorded
evidence: the latest run on disk (`docs/audits/gate-m1/r2`) failed at
Jaccard 0.8235 over a 34-entry corpus — five dartq misses in the
`wrong_number_of_type_arguments` family plus one dartq-only attribution —
so the ≥ 0.95 passing baseline is likewise to be re-established by the
next gate run.

## Oracle Baseline

`tests/bench/oracle_baseline.sh <target-dir> [rounds]` measures the oracle
itself: wall-clock time and peak RSS of `dart analyze --format=machine`
over N sequential rounds (default 5; round 1 is "cold", rounds 2..N are
"hot"), plus corpus scale (files / LOC / bytes). One JSON object per line,
one per round plus a summary record. Run it before and after performance
work to keep comparisons honest — dartq budgets are stated relative to
these numbers.

## Versioning

- The CLI binary follows semver; the workspace version is `0.1.0`.
- Machine-readable outputs carry `schema_version`. The frozen v1 contract
  and its evolution rules (breaking change ⇒ major bump; additive fields
  stay in-major and force the schema document to move first) are specified
  in `dartq-schema`'s crate docs (`crates/dartq-schema`). The diagnostics
  envelope deliberately keys its contract `version`, not `schema_version`,
  for byte parity with `dart analyze --format=json`.
- `dartq.yaml`'s optional `version` key is currently `1`; any other value
  is rejected with a usage error.
- The on-disk snapshot format is internal to the engine — not a published
  contract.

## Limitations

Honest state of the current tree:

- `dartq analyze` covers scan, parse, and resolution; the type-system and
  semantic-diagnostic seam is still landing, so semantic errors beyond
  syntax/resolution are not yet reported.
- `query` serves three faces: `symbol`, `references`, and `callers`. More
  query kinds are unplanned future work.
- The 64 built-in lint rules are implemented and tested but not yet wired
  into the CLI/daemon diagnostic output.
- `dartq fix` plumbing (anchoring, drift-correcting applier, patch
  document) is complete, but no fix sources are wired yet — current runs
  apply zero edits.
- Plugins: the wire protocol (v1) and the host-side Dart package exist;
  the core does not yet spawn or discover plugins.
- The daemon's debounced snapshot flush machinery is in place; the
  generation payload awaits engine faces, so nothing is written yet.
- `analyze --format=human` is a convenience view, not a differential
  face — its layout may change freely.
- The benchmark numbers above are debug builds over a 22.7k-LOC corpus.

## Publication

dartq is not yet published to crates.io — build from source. Publication
is the deliberate destination, and the tree is prepared for it: one
workspace-level version/license declaration, frozen v1 output schemas
behind `schema_version`, and a committed `Cargo.lock` for reproducible
builds.

## Development Gates

```bash
scripts/ci.sh              # serial per-crate gate: cargo fmt --check,
                           # then cargo clippy --all-targets -- -D warnings
scripts/ci.sh --check-only # fast mode: cargo check per crate only
```

The loop walks the workspace members (fixture/test-only members skipped),
one crate at a time, and stops at the first failure naming the crate.
Concurrency stays under the workspace job cap — `.cargo/config.toml`
pins `jobs = 2`; the script passes no `-j`.

## Third-Party Notices

Reference sources vendored under `.third/` (the Dart SDK, pub, and
rust-analyzer repositories) are local development references used for
semantic grounding and calibration only. They are not part of the source
distribution, are never modified, and are not required to build or use
dartq.

## License

Licensed under either of Apache-2.0 or MIT, at your option:
[LICENSE-APACHE](LICENSE-APACHE) · [LICENSE-MIT](LICENSE-MIT).
