# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Generated SDK facade tables: `tools/sdk-faces-gen` extracts two checked-in
  tables from the oracle SDK in one pinned run — the flat 50-entry `dart:`
  library existence table (`crates/dartq-sdk`) and full top-level export
  faces for the 25 public `dart:` libraries, 1551 names
  (`crates/dartq-engine`'s `SDK_LIBRARY_FACES`) — replacing the hand-grown
  samples; SDK-rooted `uri_does_not_exist` and `undefined_annotation` false
  positives are gone (#2005).
- `corpus-gen --flutter-app`: a widget-skeleton emitter over a checked-in
  stub Flutter package (the same bytes feed the oracle and dartq), plus the
  `scripts/app-gate.sh` gate leg — the serial verification chain grows
  35 → 36 steps (#2063).
- Vendored real-package corpora: riverpod 3.4.3 (57 files) and drift 2.34.0
  (211 files) under `tests/corpus/real/`, each with a `PROVENANCE.md`
  pinning the exact upstream version (#2064).
- `dartq rename` — rename one symbol across the analyzed workspace (the dry
  run prints the machine-readable edit set; `--write` applies it in place,
  refusing files outside the workspace).
- Diagnostic context messages: diagnostics the oracle annotates with
  `contextMessages` now carry the related info, wired per code for every
  code whose context the engine already computes; census rows without
  engine-computed context stay unwired rather than fabricating rows or
  anchors (#1798).
- Library-URI face: the resolution library graph builds a file→library
  reverse map when the graph finishes, and the analysis face answers
  `library_uri`/`uri_of_file` queries over it (`None` for files outside
  the indexed universe) (#1900).
- Daemon session memory posture: a session-wide ceiling of 262,144 live
  paths gates path admission — admissions beyond it are rejected with a
  structured error plus one residency-degrade line, and legitimate
  corpora never reach the ceiling — and the memory-budget reporting now
  includes structural residency (file-table and string-table bytes)
  (#1887).
- Daemon completion faces: `dartq/completion` and `dartq/resolveAt` answer
  over the analyzed view — fifteen served `dartq/*` request methods
  (#1569).
- The built-in fix table grew to 232 anchored fixes (130 lint, 42 semantic,
  60 parser sources).
- Five previously fact-bound lint fixes gained rewriters —
  `avoid_function_literals_in_foreach`,
  `prefer_for_elements_to_map_fromIterable`, `sort_child_properties_last`,
  `unnecessary_underscores`, and `use_key_in_widget_constructors`; the fix
  table's 232-entry count is unchanged (#1883).
- Resolved type operations for flow analysis: per-library `LibraryTypes`
  snapshots feed a `TypeOps` back-end through the flow layer's mount seam —
  sealed-scrutinee exhaustiveness answers across imports, the six base ops
  resolve every recorder-observed name, and `is!`-promotion factors generic
  receivers through their written argument trees (#2204).
- Clause trees for generic subtyping: per-clause captures (type
  parameters, supertype spellings) travel through the library snapshot
  into `is_subtype_typed`, so subtype and promotion verdicts penetrate
  written type arguments per clause instead of comparing bare heads;
  the flow model's four base sites consume the typed seam and
  `try_promote` keeps the strict `is_subtype(t, f) && !is_subtype(f, t)`
  gate (#2204).

### Changed

- LSP wire types migrated into the daemon crate (#1680).
- The daemon's `uri` module moved into `dartq-commands`; the daemon re-exports
  it, so `dartq_daemon::uri` keeps resolving (#1680).
- The resident RPC face: diagnostics pushes arrive as `dartq/diagnostics`
  notifications (no `version` field — a push is a pure diff against the
  baseline); client edits report through the `dartq/sync` notification
  (`{paths: [...]}`, disk truth only); request cancellation is the
  `dartq/cancel` notification (`{id}`). The `dartq/*` request methods and
  the `dartq/overlay.*` faces are unchanged (#1680).
- The lint registry grew 276 → 279 rules (the analyzer pubspec-validator
  diagnostic mirrors).
- `analysis_options.yaml` semantics: include chains, `analyzer.errors`
  severity remaps, `exclude:` globs, and `INCOMPATIBLE_LINT` diagnostics
  for incompatible rule keys.
- Ten Flutter-family lint rules over framework-creation and widget chains
  switched to negative-identity refutation: framework identity is refuted
  through the parsed resolution queries — constructor resolution,
  declaration-binding lookups, and supertype-head checks in the oracle's
  decision order — instead of assumed from name shape, so identically-named
  application classes no longer false-fire; targets that stay unresolvable
  keep the `semantics: none` fallback (#1899).
- Lint rows that fire after the engine's tail window are recorded and
  re-joined by the shell instead of dropped (`unnecessary_ignore` and its
  late-join family), and the family's rules register unconditionally in
  the official posture (#1904).
- `comment_references` gains its resolution face: doc-comment references
  resolve against scope, members, inheritance, and extensions in upstream's
  decision order — confident misses report, undecidable references stay
  silent (#1903).
- Function-type substrate: `Ty::Function` shapes are interned and persisted
  (facts-shape v12, signature v7); `avoid_types_on_closure_parameters`
  resolves assignment/callee-formal targets and `prefer_initializing_formals`
  decides through resolved types over the assignability lattice, keeping
  spelling equality as the unresolvable fallback (#1926).
- `--[no-]fatal-warnings`: when spelled repeatedly, the last spelling now
  wins, matching `dart analyze`'s argument handling (#1910).
- `dartq compatibility` is the subcommand's primary spelling; `compat`
  remains as a hidden alias (#1776).
- Flutter framework lint rules: the structural refutation gates hoist ahead
  of framework-spelling resolution and share a per-run refutation memo,
  cutting repeated resolution work per widget-creation row (#1914).
- The semantic diagnoser, the lint rules, and the fix engine now live as
  module trees (`diagnose/`, `rules/**`, `fix/`) instead of three monolith
  files — a pure layout split proven by exact test-count no-op runs; the
  thin module roots stay in place until the final verified checkpoint.

### Removed

- The daemon's LSP provider shell (11 provider modules plus their test
  files): the daemon now serves the resident RPC face only — AI-first CLI
  focus (user directive, 2026-10-16) (#1680).
- The daemon's LSP lifecycle and document-sync frames: `initialize`,
  `initialized`, `shutdown`, `exit`, `textDocument/didOpen`,
  `textDocument/didChange`, `textDocument/didClose`, and `$/cancelRequest`
  are gone; the server is ready at startup and a client close (EOF) is the
  session's only end signal (#1680).
- The `analyze --format=lsp` CLI spelling (#1682).

### Fixed

- Parser recovery anchors: eight same-anchor dual `expected_token` faces now
  anchor at the next token — operator keep-list parity (`??`, `&&`, `||`,
  `++`, `--`, `..`, `??=` are no longer swallowed at recovery), spill
  relocation to the next non-zero-length token, and the second-separator
  anchor rule — and a zero-width synthetic `got` in the missing-identifier
  face anchors at the following non-zero-length token (#2024).
- Parser modifier faces: function-typed parameters consume and record
  `var`/`final` (a seven-row recovery cascade dies structurally), and the
  final-parameter acceptance face no longer diverges on the
  `avoid_final_parameters` corpus file (#1999).
- `// @dart` overrides below the 2.12.0 language-version floor report
  `illegal_language_version_override` at the comment lexeme (the override
  still takes effect for the run, as in the oracle) (#2044).
- Scanner diagnostics: identical rows collapse at the finish pass — the
  oracle's `Set<Diagnostic>` equality on code, span, and message — and
  `illegal_character` messages come from a single registry template with
  decimal UTF-16 code-unit arguments, matching the oracle's rows (#2048).
- Identical lint rows collapse at the run's aggregation tail — rows sharing
  rule, span, and message surface once, the upstream diagnostic-set
  equality — so two rule shapes firing the same expression no longer
  duplicate (#2061).
- `avoid_final_parameters` gates on the primary-constructors language
  feature, mirroring the upstream rule's own gate (#1999).
- Part-of name-form fall-through no longer reports when the part's library
  name already equals the including library's — the equal-name case never
  reaches the upstream emit (#2068).
- `no_self_assignments` and `parameter_assignments` now resolve their
  left-hand runs under flow-parent non-block bodies (`if`/`while`/`for-in`
  arms, guard cases): the operand run restarts after the header rows
  instead of absorbing condition fragments (#2028).
- Nine more semantic fixes served on the fix table (combinator
  show/hide duplicates and undefined names, `duplicate_import`,
  `import_of_non_library`, override-annotation removal, getter-invocation
  parentheses, `required` insertion) — semantic partition 42 → 51 (#2069).
- Fix computation no longer panics on an unterminated string literal with a
  pure-quote lexeme: the operand segmentation drops the fix instead of
  slicing an empty range (#2071).
- Directive `data:` URIs: the parsed scheme no longer carries the trailing
  colon, matching the oracle's scheme form (#1906).
- Expression-face diagnostics inside string interpolations: interpolation
  child rows are kept as residue when the literal's row completes, so
  diagnostics anchored through the interpolation node — undefined
  identifiers and friends — report at their own anchors instead of
  vanishing (#1902).
- Fix-application spans: the shared bracket matcher counted closers and
  sought openers asymmetrically, drifting replacement spans out of sync
  whenever they crossed a nested brace — the counting is now symmetric;
  and the `for_in_from_for_each` rewriter's replacement now includes the
  call's closing parenthesis (#1911).
- Interpolated string literal diagnostics seal at the closing string
  segment, matching the upstream parser's `endLiteralString` contract;
  completion rows span the full interpolation extent (#1916).
- Lint semantic queries now use UTF-16 coordinates end to end — on sources
  with non-ASCII content (comments, strings), query spans previously
  overshot their rows and could silently miss or mis-position lint
  reports (#1909).
- Lint query-range fixes: `no_dynamic_casts` re-collects declaration-face
  rows from the initializer (stale flattened rows no longer suppress
  reports) and admits return targets per upstream (any non-dynamic,
  non-Object? target, async-flattened); `prefer_contains` scans the full
  variable-declaration range, so collection expressions past the old
  sealed range no longer silently miss reports (#1907).
- Non-parameterized typedefs now capture their right-hand type and fold
  through `type_of` and binding positions, so `unnecessary_type_check`
  matches `dart analyze` on alias-typed formals (#1925).
- The framework-spelling refutation memo is now cleared at each lint-run
  boundary — batch runs over same-length files no longer risk inheriting
  a previous file's refuted verdicts (silent missed/false lint reports)
  (#1939).
- Unterminated block doc comments no longer crash the run: the
  `comment_references` rule sliced the comment body at `len - 2`, which
  for the scanner-recovery lexeme of a `/**` without a closing `*/` can
  land mid-UTF-8-char — a CJK doc-comment opener lexeme (`/**` followed by CJK text) with the rule enabled
  panicked with a char-boundary error instead of reporting (#1956).
- `comment_references` doc-reference resolution now recognizes the implicit
  `Object` members (`toString`, `hashCode`, `runtimeType`, `noSuchMethod`)
  on resolved containers — bare `[toString]` and qualified `[C.toString]`
  references no longer produce dartq-only reports where `dart analyze` is
  silent; extension-name heads (`[E.foo]`) resolve through the existing
  dual-registration and are pinned (#1952).
- `comment_references` now reports two-part references whose head resolves
  to a non-container, non-TypeAlias declaration — a top-level function or
  variable, a type parameter, or an import prefix (`[f.foo]`, `[v.foo]`) —
  as unresolved, matching the upstream analyzer's two-component resolution
  behavior (#1977).
- Extension applicability is a set: a face reachable through several routes
  (own unit, unit files, import edges) enters the application pool once;
  `on`-clause arguments gate applications through the oracle's subtype
  test; chained receivers keep their element types (#2207).
- `@docImport` URI validation reports
  `uri_does_not_exist_in_doc_import` through the production pipeline, one
  row per coded directive, on the selected configuration's literal
  (#2199, #2206).
- Object-pattern heads push their type-name rows, so dot-qualified and
  bare pattern heads diagnose through the type-use face like the oracle
  (#2184).
- Guards under labeled statements scope to their own span and stop leaking
  into enclosing statements (#2221).
- Sealed-class override folding: enum targets and non-abstract concrete
  targets keep their concrete verdicts, and a sealed direct superclass
  takes the abstract branch (#2202).
- `dart:js_interop` extension faces apply off checked-in declarations
  instead of missing silently (#2220).
- `await` flattens the operand type one level through the type system's
  `Future`/`FutureOr` unwrap, preserving nullability (#2223).
- Nineteen SDK interfaces (`FormatException`, `Match`, `RegExp`, the
  `Stream` family, `SocketException`, `IOSink`, …) carry their member
  faces into the override check, and `(x as dynamic).member` receivers
  keep the operand's type instead of collapsing to `bool` (#2213).
- Pattern-arm variables scope to their arm — post-switch reads no longer
  bind arm variables, receiver-position reads (`v.m = 1`, `v.m++`) keep
  the local alive, and trailing same-line `// ignore:` comments suppress
  `unused_local_variable` (#2189).
- Primary-constructor declarations feed the declaration collector through
  the parser mux, so production `Container` constructor faces populate for
  classes, enums, and extension types alike; extension-type invocations
  are pinned to the oracle's contract (#2212, #2226).
- The cast bool-latch no longer roots at `bool?`-blind gates, and frozen
  interface member verdicts (Defined/Absent/Unknown) report per member
  instead of blanket-suppressing (#2225).
- Named-argument checks compare the formal's public name
  (correspondingPublicName), cascade targets latch their receiver across
  the chain, and type-argument depth stays guarded against regressions
  (#2214, #2194).
- Const-eval faces: duplicate const map keys compare bytes (UTF-16→byte
  conversion kills phantom equal_keys), the callee-pop guard survives
  nested argument groups, const method invocations evaluate their
  receiver split, and the site-default latch drains at finish
  (#2187, #2194).
- `ignore:` citations join the canonical diagnostic name, so
  case-variant spellings of the same code suppress the row they target
  (#2137).
- On-face member scope resolves through the constraint union: mixin `on`
  constraints and extension extended types surface their members to bare
  and `this.`-receiver accesses inside the body, with honest
  Found/ChainBroken/Missing verdicts per member (#2186).
- Override checking requires the inherited concrete implementation's
  provenance (implements-reached hits no longer count), and InvalidType
  heads at top-level granularity take the oracle's lenient posture
  (#2192).
- Async return checking follows the analyzer's futureValueType
  three-branch matrix — `Future<T>`/`FutureOr<T>` unwrap one level,
  `dynamic`/`void` pass through, everything else checks against
  `Future<Object?>` (#2224).
- Extension types resolve their member faces through the resolved-kind
  lens (class-family codes for unresolved heads), frames carry their
  extension-type context, sealed dedup no longer double-reports, and
  `this`-captures bind per the oracle (#2218, #2222).
- A bare miss inside an extension-type body names the extension type's
  head (`undefined_method` on `E`, never the context-free
  `undefined_function`), and the sealed-contract fixture declares its
  foreign spellings so the mismatch stays real after the InvalidType
  leniency (#2227, #2228).
- `this`-receiver decidable misses inside mixins and extensions report
  naming `this`'s static type — the mixin itself, or the extension's
  `on` type; unresolved on-names stay record-only like the oracle's
  InvalidType extendedType (#2230).
- Parser const destructuring-target faces: a const list/record literal
  LHS no longer rides the index-target admission and reports both
  assignment codes, and assignment-target reports anchor on the LHS's
  beginToken (`const (q,) = g()` 3:12 → 3:3, binary-tail anchors fixed
  for free) (#2157).

## [1.0.0] - 2026-10-12

Initial release: a Rust-native re-implementation of the Dart analyzer's
diagnostic pipeline — scanner, parser, flow analysis, resolution, the
semantic diagnoser, and the lint engine — validated by differential
testing against `dart analyze` (Dart 3.13.1) as the oracle.

### Added

- **CLI** (`dartq`) with oracle exit-code tiers (0 clean/info · 1 fatal infos ·
  2 fatal warnings · 3 errors · 4 internal · 64 usage) across the faces:
  - `check` — scan + parse over a directory tree, machine output
    byte-compatible with `dart analyze --format=machine` (JSON alternative),
    `@dart` version-comment override rescan, `--metrics-json` run facts.
  - `analyze` — the full engine over a real package directory: pub faces,
    discovery, scan, parse, resolution, and the semantic/flow/lint
    diagnostic tiers; machine and human formats, `--overlay PATH=FILE`
    in-memory edits (stdin via `-`), `--memory-limit`, `--trace-events`
    JSONL engine events, and `--cache-size-limit` for the persistent
    content store.
  - `query` — symbol-index queries over the analyzed workspace: `symbol`,
    `references`, `callers`, `implementations`, `dependencies`, and
    `batch` (many symbols x kinds in one workspace assembly); machine rows
    or the frozen v1 JSON envelope.
  - `explain` — the one-stop JSON report for one symbol over the analyzed
    workspace (declaration, uses, imports, tests, dependencies).
  - `impact` — what a change to one file would affect: changed API symbols,
    semantic/transitive affected files, affected tests, environments.
  - `compat` — one file analyzed under several SDK roots (`--sdk-root`,
    repeatable, or `dartq.yaml` contexts) into a per-axis compatibility
    matrix with first-failure attribution.
  - `fix` / `assists` — anchored, drift-corrected fix application over
    built-in lint fixes plus plugin proposals (`--dry-run` proposes without
    writing), and report-only plugin assist queries for one location.
  - `index` — build the symbol index over the analyzed workspace and report
    what it built.
  - `daemon` — the resident JSON-RPC server over stdin/stdout (see below).
  - `version`.
- **Daemon** — dartq's RPC face over stdio: hand-written `Content-Length`
  framing, JSON-RPC 2.0, one client; the server is ready at startup and a
  client close (EOF) is the session's only end signal. Edits report through
  the `dartq/sync` notification (`{paths: [...]}`, disk truth only), merged
  diagnostic sets push as `dartq/diagnostics` notifications (a pure diff
  against the baseline), and `dartq/cancel` cancels a queued request.
  Thirteen `dartq/*` request methods mirror the CLI (`analyze`, `check`,
  `query`, `queryBatch`, `index`, `impact`, `explain`, `fix`, `assists`,
  `compat`) plus multi-agent overlay staging
  (`overlay.set`/`overlay.commit`/`overlay.clear`), delta `since` cursors,
  a `dartq/fixProgress` notification, and an optional poll-based `--watch`
  mode.
- Daemon multi-context serving and session runtime budgets: `--context`
  (repeatable `dartq.yaml` context labels served alongside the default
  context; with no `--context`, every declared context is served),
  `--memory-limit MiB`, `--cache-size-limit SIZE`, and
  `--trace-events PATH`.
- Per-context package universes: every analysis context (each package root
  with its own `.dart_tool/package_config.json`) resolves `package:`
  imports against its own package closure, never the workspace union —
  nested roots each see only their own resolved set.
- `analyze --format json`: the analyze face answers one single-line JSON
  envelope (the frozen analyze schema, v1) — the same producer bytes the
  daemon's `dartq/analyze` serves. The envelope carries diagnostics only:
  workspace advisories (stale/missing/corrupt `package_config.json`) print
  as one line on stderr ahead of it.
- **Scanner** — full Dart 3.15 tokenizer: single pass, no backtracking, the
  `@dart` version-comment window, and error recovery to a linear token
  table plus syntax diagnostics.
- **Parser** — fault-tolerant recursive-descent parsing of the full Dart
  3.15 grammar, with recovery aligned to the official parser's decision
  order, emitting a parser-event stream over the token table.
- **Diagnostics** — stable diagnostic code registry and `machine`/`json`
  writers byte-compatible with the oracle's formats. Codes are aligned per
  face — scanner/parser, compile-time/semantic, and lint — against the
  oracle release's own generated code tables, then confirmed by live
  machine-row probes against `dart analyze`; the three faces meet in one
  registry.
- **Semantic and flow analysis** — element/reference/symbol arenas, the
  type-system surface (canonical interning, subtype/assignability, least
  upper bound), and the semantic/flow diagnostic families: undefined
  identifiers and members, wrong-number-of-arguments/type-arguments, the
  part/part-of family, duplicate definitions, class-modifier decisions, and
  dead-code/reachability with oracle-anchored spans. Flow-driven type
  promotion — pattern-promotion arms, promotion joins across assignments,
  and written-argument tracking — backs the flow families' spans.
- **Lint rules** — a 276-rule registry wired into the analyze face: all 89
  `package:lints` rules (34 core + 55 recommended), the opt-in
  diagnostic-mirror rule (an analyzer diagnostic outside `package:lints`),
  the 14 removed-registry no-ops the SDK still registers (named
  rules that resolve but never fire), and 172 further SDK rules outside the
  `package:lints` sets; enable sets resolve from `analysis_options.yaml`
  include chains.
- **pub/package layer** — consumers for pub's resolution artifacts
  (`package_config.json`, `pubspec.yaml`, `pubspec.lock`), workspace
  discovery, freshness advisories, and validation faces; no version solving.
- **Symbol index** — reference, inheritance (clause-typed supertype edges),
  and test graphs backing the query and impact faces.
- **Plugin system** — protocol v1 (frames, codec, handshake, manifest), the
  enforced runtime with discovery, spawn, isolation budgets, and idle
  eviction of quiescent hosts (an eviction releases the child's resident
  set and relaunches budget-free at the next interaction — a host memory
  action, never a crash), an `analysis_server_plugin` compatibility shim,
  and the host-side Dart package.
- **Plugin discovery** — `plugins:` declarations resolve through pub's
  package config (pub-native roots) in addition to `dartq_plugin.json`
  manifests and the naming convention; unresolved or unusable roots
  surface as structured issues instead of being skipped silently.
- **Persistence** — content-addressed per-file facts snapshots with atomic
  generation save/load (daemon debounced flush), and the persistent L3
  content store backing incremental re-analysis across sessions.
- **Workspace configuration** — optional `dartq.yaml` (named SDK contexts,
  generated-file policy, memory and cache limits), all-optional; unknown
  keys are ignored (only the `version` key and context labels are
  validated).
- **Frozen v1 output schemas** — versioned machine-readable JSON contracts
  (check/analyze/assists/query/index/impact/compat/explain domains) with CI
  validators and verbosity tiers.
- **Differential-testing harness and gate chain** — oracle vs dartq over
  shared corpora with `(code, file)` Jaccard comparison, auto-bisect,
  replay/edit-replay drivers, benchmark arms, corpus tooling, and per-gate
  scripts. The oracle of record is `dart analyze` from a pinned Dart
  3.13.1 install, and every differential gate holds the full-parity bar
  (Jaccard 1.0): the M0 syntax run (check face over the 255-code syntax
  universe), the M0 union run (analyze face over the 290-code union), the
  M1 frozen-code semantic run, the INFO-tier lint gate over the frozen
  lint-code universe (adjudicated exclusions in a rule-level divergence
  ledger), the M2
  warning-tier and M3 unused-family gates, the semantic wave-2 gate (the
  ERROR-tier continuation of the m1/m2 tiers over its frozen code
  universe), the valid-syntax acceptance gate (zero diagnostics over the
  feature-family corpus), the incremental-consistency gate (every step of
  each curated edit sequence replayed through a live daemon session must
  match a fresh full recompute byte-for-byte), and the reproducibility
  gate sealing a byte-identical output digest with a repro tuple. The
  serial verification window (`scripts/serial-gates.sh`) orchestrates the
  authoritative gate chain — differential gates, per-crate suites,
  daemon/plugin live faces, reproducibility, coverage — in one fixed
  order, and the per-crate CI gate (`scripts/ci.sh`) runs a hard doc gate
  (rustdoc per member with `--no-deps` under `RUSTDOCFLAGS=-D warnings`).
- **Packaging** — dual-license (Apache-2.0 OR MIT) publishable crates,
  crates.io metadata, CI workflow, and a committed `Cargo.lock`.

### Changed

- Performance passes across the frontend: scanner memchr skip fast paths
  and cheap recovery-option snapshots, parser/resolution hot-path hashing
  on `FxHashMap`, and batched workspace reads in the engine.
- Engine incremental recompute: the merged diagnostic output is cached
  against the contributors' change generations — a cycle where no
  contributor re-analyzed serves the standing vector instead of re-walking
  the merge, and a partial recompute splices only the changed
  contributors' runs.
- Structure-fingerprint gate: per-file blake3 digests over the structural
  face gate the O(workspace) resolution-graph rebuild — a body edit that
  leaves the file's API signature stable skips the rebuild with zero
  work.
- Parallel recompute: dirty-library re-analysis and per-file facts
  interning run on rayon's work-stealing pool (batch fan-out).
- Import-cycle handling: iterative Tarjan SCC condensation with cycle
  fingerprints; recompute follows condensation order and closure products
  are reused across cycles by fingerprint.
- Language-version handling: version-derived feature enablement clamps to
  the newest known language version (features shipped at or below it stay
  on; nothing past it switches on — the official posture, made explicit),
  and a `// @dart` override above the latest known version is diagnosed
  and dropped rather than silently accepted.
- Daemon budgets: a session-wide 256 MiB total cap joins the per-agent
  overlay caps, overlay commits write with `O_NOFOLLOW`, and panics
  render as one unified line.
- Daemon robustness: off-loop fix dispatch, per-group panic containment,
  bounded plugin teardown, and clamped untrusted plugin diagnostics.
- Hot-path panic posture: an inventory of panic primitives across the
  engine hot crates (engine, semantic, parser, scanner, and the lint
  bridge) found zero P0 sites reachable from arbitrary user input, and the
  cross-component pairing drifts it catalogued degrade structurally where
  they were converted: the lint bridge's 29-site parser-event pairing
  family falls back to in-position anchors, the scanner's diagnostic walk
  clamps to the source end, import-cycle salvage incoherence bails to a
  full recompute, static code-table drift degrades to the raw official
  name (exhaustiveness enforced by a sync test instead of a runtime
  panic), mutex poisoning is tolerated on engine read/apply paths, and
  const-eval wildcard arms return non-const verdicts. A drifted invariant
  costs a degraded span, row, or unit — never a crash on a
  user-reachable path.
- Content store: source bytes are stored deduplicated and served
  mmap-backed — cold loads copy nothing and standing memory moves to the
  page cache, so cold analysis starts faster at a lower resident
  footprint.
- Lint parity closures: additional SDK rules across the flow, const,
  Flutter, and format families join the registry, closing more of the
  oracle's firing set.
- Plugin crash rows (`dartq/plugin_crashed`) route only through
  `publishDiagnostics` pushes — one-shot, anchored, never repeated;
  `dartq/analyze` pull answers no longer carry them.

### Fixed

- Dead-code region anchoring: labeled-statement rows anchor at the first
  label token and for-header regions at the first unreachable node.
- Parser recovery alignment: extraneous-modifier faces, synthetic-formals
  reset, parameter-group recovery staying in-list, bracket-mismatch spill
  draining, and constructor-invocation recovery.
- Lint gating: `no_duplicate_case_values` keyed on the patterns version
  face, rule names resolved case-insensitively.
- pub validation: git URL/path shape checks, `tag_pattern` marker rules,
  fail-closed escapes, and the stale-config advisory face.

### Performance

Measured numbers only, faces labeled where applicable, from the recorded
round-1 evidence base; the dedicated dartq-vs-oracle wall-clock rerun and
further analyze-face numbers land with the 1.0.0 release gate chain — final
numbers re-verified at release:

- 100k-tier generated corpus (99,598 files, ~5.4M LOC), cold full
  analysis: dartq ≈6.7 s vs the oracle's single-round 632.9 s (≈95x);
  peak RSS 616 MB vs the oracle's ~4.17 GB (616,112 KiB total, 14,567 KiB
  per 100k LOC, analyze face).
- 10k-tier corpus (9,898 files), cold analysis: dartq ≈1.05 s (≈32x the
  oracle's cold run).
- Small corpus, the m0 differential corpus as measured (279 files,
  25,275 LOC): incremental edit latency over a 240-step manifest with
  resident in-memory overlays — resident p50 3.5 ms / p95 6.08 ms /
  p99 10.7 ms, vs a 57.8 ms warm-engine full-pass baseline and a 600 ms
  oracle hot median (p95 ≈ 98.7x the oracle rerun).
- Check face, clean corpus: dartq emits 0 bytes of machine output vs
  the oracle's 435,207 bytes (savings ratio 1.0, by design for the
  scan+parse-only face).
- Scanner micro-benchmark: 245 MB/s single-thread tokenization.

### Known limitations

- Semantics are pinned to the Dart 3.13.1 oracle: syntax features that
  newer SDKs ship behind experiment flags are diagnosed, never silently
  accepted.
- The daemon serves dartq's own JSON-RPC face (the `dartq/*` methods,
  `dartq/sync` edit reports, `dartq/diagnostics` pushes, `dartq/cancel`);
  it is not a language server and speaks no LSP — no `textDocument/*` or
  lifecycle frames exist. The 1.0.0 differential gates cover the `check`
  and `analyze` faces.

[1.0.0]: https://github.com/fluttercandies/dartq/releases/tag/v1.0.0
