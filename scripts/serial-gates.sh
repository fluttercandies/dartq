#!/usr/bin/env bash
# serial-gates.sh — the serial verification window orchestrator (goal #927).
#
# Runs the whole gate marathon as one turnkey, strictly serialized chain:
# never two product processes at a time, every step under an RSS watchdog,
# every step logged, first red step stops the chain. It orchestrates the
# existing gate scripts (m1/m2/m3/lint/sem2/incremental/torture/mega/
# acceptance/repro/coverage + the schema-freeze sweep) and
# composes the two trend differential runs and the complex-corpus
# differential in the m0-gate.sh idiom (no separate trend script exists).
# It NEVER executes product code outside the
# watchdogged steps below. This script does not commit; repro-gate's
# clean-tracked-tree rule must be satisfied before the `repro` step (commit
# the tracked waves first — serial-phase-runbook Phase 4).
#
# The chain, in fixed order:
#   release-rebuild  cargo build --release -p dartq-cli -p harness
#                    (binary `dartq` per crates/dartq-cli [[bin]] ->
#./target/release/dartq; the trend + acceptance +
#                    lints-new + complex steps drive the release face,
#                    hence -p harness too)
#   debug-rebuild    cargo build -p dartq-cli -p harness
#                    (m1/m2/m3/lint/sem2/torture gates + mega-run drive
#./target/debug/{dartq,harness})
#   bench-compile-guard
#                    cargo check --all-targets -p dartq-engine — bench
#                    targets are invisible to `cargo build -p` and to
#                    `cargo check --tests`, so a bench-initializer drift
#                    (the #979 class: a config-struct field batch left the
#                    engine bench harness uncompilable) surfaces only here.
#                    Compile only: check never executes a bench (resource
#                    discipline). dartq-engine owns the workspace's only
#                    benches/ dir; extend the -p list when a new
#                    bench-owning crate lands.
#   harness-pins     cargo test -p harness -- --test-threads 2 — the
#                    frozen-universe pins (the sem2 derivation, the
#                    fixture-coverage universe equality, the syntax-TSV
#                    derivation) run at phase start, so a universe drift
#                    trips here instead of at the tail coverage step
#   trend-w5-A       run A: syntax universe (tests/corpus/syntax-codes.txt,
#                    255 codes, ERROR, bar 1.0) over the check face
#                    (`dartq check --format=json`) on the trend corpora
#                    tests/corpus/m0 + tests/corpus/m0-syntax — the exact
#                    m0-gate.sh run-A shape, label trend-w5
#   trend-w5-B       run B: union universe (255 + 36 - 1 overlap = 290,
#                    composed and pinned exactly like m0-gate.sh) over the
#                    analyze face (`dartq analyze --format=json --sdk
#                    <fvm default dart-sdk>`), label trend-w5
#   m1-r5            scripts/m1-gate.sh --label m1-r5 (ERROR, bar 1.0)
#   acceptance       scripts/acceptance-gate.sh --label serial
#                    (zero residual rows expected; a known-open defect may
#                    leave a single row — this script never suppresses rows,
#                    a red acceptance step stops the chain for adjudication)
#   lint-gate        scripts/lint-gate.sh --label lints-r1 (fresh label; the
#                    lint code-table freeze cross-checks run inside it).
#                    The N1 re-freeze has run (2026-10-14, post-W6-close:
#                    registry pinned at 276, lint_codes.txt re-frozen at
#                    239 codes) — this step is green when the committed
#                    freeze matches the registry; a red here is a real
#                    freeze/registry drift, never hand-silenced.
#   lints-new-gate   N5 (goal #1160): the 19-name promotion-wave
#                    differential face over tests/corpus/lints-new. dartq
#                    analyze --format=json (release face), envelope rows
#                    filtered to the 19 rule codes and the corpus's own
#                    lib/ + packages/ trees, then a code|file|line
#                    multiset compare against
#                    the corpus's expectations.json — the oracle-derived
#                    ground truth (verified against the upstream tests'
#                    diagnostic markers; the structurally-silent families
#                    ride as empty dartq-leg entries, see the corpus
#                    README). No oracle process at chain time: the table
#                    is the oracle leg.
#   incremental-gate scripts/incremental-gate.sh --label incr-r1 —
#                    edit-replay over tests/corpus/incremental: per-step
#                    incremental-vs-full-recompute byte equality (serves
#                    the frozen #147 verification order)
#   m2-r1            scripts/m2-gate.sh --label m2-r1 (WARNING, bar 1.0)
#   sem2-gate        scripts/sem2-gate.sh --label sem2-r1 (ERROR sem-wave-2,
#                    bar 1.0; its own corpus and 65-code count pin — the
#                    m1/m2 continuation tier, gated after them)
#   m3-r1            M3_LABEL=... scripts/m3-gate.sh (env-only label).
#                    Verification only this window: m3_unused_codes.txt is
#                    already re-frozen (11 rows) and the semantic-suite pin
#                    enforces it. Only if that pin reds: add the targeted
#                    fixture it demands first, then run `cargo test -p
#                    dartq-semantic --test m3_codes_pin
#                    regen_m3_unused_codes -- --ignored` and reconcile the
#                    header prose — never regen blind
#   complex-gate     the #1399 census admission: the hand-materialized
#                    tests/corpus/complex root (corpus_complex + the
#                    packages/stock_policy path dependency, both entries
#                    languageVersion 3.13 — the run-face floor) previously
#                    rode no differential leg. One harness run in the N2
#                    leg shape — oracle dart analyze vs the release
#                    analyze face — at the full-parity bar (ERROR,
#                    threshold 1.0), evidence under target/evidence/
#                    gate-complex/<label>. The corpus's row posture is
#                    unproven until its first live run: a red is triaged
#                    from the recorded report against the broken_*
#                    fixtures' per-line expect: comments (corpus README),
#                    never silenced. No --persist-regressions — unknown
#                    posture must not grow the frozen regression set
#                    mid-chain — and no replay-join mapping until a
#                    known-diffs entry cites the corpus. NEVER pub-get the
#                    root: its.dart_tool is hand-materialized.
#   app-gate         scripts/app-gate.sh --label "$LBL_APP" (#2063): the
#                    generated --flutter-app corpus face (tests/corpus/app/
#                    1k + 10k, git-ignored, regenerated at gate time with
#                    seed 0 — the in-corpus flutter stub makes the widget
#                    shapes resolve identically on both sides). Complex-gate
#                    differential idiom per scale (release harness + release
#                    analyze face, ERROR, threshold 1.0, the oracle timeout
#                    raised to 600 s) plus the dartq-side rss@10k bench row,
#                    plus the skip-if-absent golden hash check over
#                    goldens/app-x/slice. HEAVY step: the operator may drop
#                    it from a tight window with --skip app-gate (clean
#                    logged skip in the summary); first-round triage is from
#                    the recorded report, never silenced.
#   torture-gate     scripts/torture-gate.sh --label torture-r1<TAG>
#                    (ticket #1290): the generated ~2k-file complexity-
#                    axis corpus (tests/corpus/torture/corpus,
#                    git-ignored — regenerate with `python3
#                    tests/corpus/torture/tools/gen_torture.py --force`)
#                    at the full-parity bar against the fvm-default
#                    oracle of record (Dart 3.13.1 banner checked, FATAL
#                    on mismatch). Debug face (the recorded m1-gate
#                    DARTQ_CMD shape); the script raises the harness
#                    oracle timeout to 600 s for the big tree, persists
#                    post-filter divergences as regression cases, and
#                    refuses to overwrite recorded evidence. The
#                    heaviest differential before mega — budget
#                    wall-clock accordingly.
#   mega-r1          perf + parity: scripts/mega-run.sh --scale 1k
#                    --oracle-leg --strict (parity leg, 1k-only by contract;
#                    its A7 cold pair pins a release-face DARTQ_CMD via an
#                    env prefix on that one invocation, never a global
#                    export)
#                    then --scale ${SERIAL_MEGA_SCALE:-10k} --strict (perf
#                    face; "all" runs the full ladder incl. 100k)
#   mega-x-r1        the adversarial-axis gate (#1811):
#                    scripts/mega-x-run.sh --strict over tests/corpus/mega-x
#                    — X1 rss-100k on the regenerated 100k (the zadv/
#                    overlay's memory-shape assert), X2 the 1k valid-subset
#                    differential (errstorm + diagnostic-bearing unicode
#                    variants excluded), X3 the storm inverse leg (every
#                    zadv/errstorm_* package must produce oracle entries),
#                    X4 harness replay over the 1k tree's generated
#                    edits.jsonl. Groups with the RSS legs: it regenerates
#                    the 100k tree, so it sits immediately after mega-r1.
#   scanner-suite    cargo test -p dartq-scanner  -- --test-threads 2
#   flow-suite       cargo test -p dartq-flow     -- --test-threads 2
#   parser-suite     cargo test -p dartq-parser   -- --test-threads 2
#   foundation-suites
#                    cargo test over the 12 foundation crates that have no
#                    dedicated suite step — dartq-{source, language, ast,
#                    sdk, package, pub, resolution, diagnostics, types,
#                    index, snapshot, ai} — one chained invocation,
#                    --test-threads 2. Without this face they execute only
#                    inside the instrumented coverage pass, where a red
#                    costs a full instrumented re-run and reports as a
#                    stability mismatch, not a test failure; the log names
#                    the failing crate
#   semantic-suite   cargo test -p dartq-semantic -- --test-threads 2
#   lints-suite      cargo test -p dartq-lints    -- --test-threads 2
#   engine-suite     the #1700 per-binary run-order: heavy tier
#                    (incremental_oracle+incremental @ threads 2, then
#                    incremental_warm @ 1), memory_accounting solo in its
#                    own watchdog leg, medium tier @ 2, remaining binaries
#                    @ 2 (explicit --test lists under a 23-binary count pin)
#   daemon-suite     cargo test -p dartq-daemon   -- --test-threads 2
#                    (the crate's six serial-phase #[ignore]d tests — the
#                    rpc_commands live-dispatch test and the five lib units —
#                    belong to daemon-live below)
#   plugin-suite     cargo test -p dartq-plugin -- --test-threads 2 (lib +
#                    every integration target, doctests included). The six
#                    dart-spawning integration files opt out of this face
#                    via #[ignore = ...] and are owned by plugin-live below
#                    — exclusion is annotation-driven: a new tests/*.rs is
#                    in this face by default and must carry the #[ignore] if
#                    it spawns dart.
#   commands-suite   cargo test -p dartq-commands -- --test-threads 2
#   schema-suite     cargo test -p dartq-schema   -- --test-threads 2
#   schema-freeze    scripts/schema-freeze-sweep.sh --no-build (the schema
#                    v1 freeze gate, #474: 9 faces over the CLI machine/
#                    json shapes and a live daemon RPC session; drives the
#                    debug dartq built by debug-rebuild)
#   daemon-live      live gate: every dartq-daemon #[ignore] wakes here,
#                    exactly once, two legs —
#                      (a) cargo test -p dartq-daemon --test rpc_commands --
#                      --ignored: the live plugin dispatch end to end
#                      (assists + fix over a real fixer sidecar);
#                      (b) cargo test -p dartq-daemon --lib -- --ignored:
#                      the idle-flush and deferred-publish real-time server
#                      windows (no dart needed) plus the three live
#                      plugin-runtime units in src/plugins.rs (echo-fixture
#                      launch/cycle/shutdown, session-disable penalty,
#                      idle eviction) — those spawn real dart processes.
#                    Environment: both legs prepend the chain's fvm default
#                    SDK bin ($SDK_ROOT, preflight-validated) to PATH —
#                    the live tests resolve plain `dart` from PATH (they
#                    assert `dart --version` themselves and fail loudly
#                    without it); there is no DARTQ_SDK variable on this
#                    face. The plugin units drive the dartq-plugin-host
#                    fixtures (bin/echo_plugin.dart, bin/fix_plugin.dart),
#                    which need the host package pub-getted once (the
#                    host-live pub-get leg materializes it, or
#                    `dart pub get` in crates/dartq-plugin-host).
#                    Runs by default this window (DARTQ_M4_LIVE defaults
#                    to 1 — the 2026-10-13 live failure predates the #1137
#                    discovery/spawn rewrite); an explicit 0 leaves a clean
#                    logged skip
#   plugin-live      live gate: cargo test -p dartq-plugin --
#                    --ignored --test-threads 2 (wakes every #[ignore]d
#                    test in the crate — the five dart-spawning integration
#                    files plus the #644 compat_live matrix, the crate's
#                    only ignores); same default-1 flag, explicit 0 skips
#   host-live        live gate: dart pub get + dart test in the
#                    dartq-plugin-host Dart package; same default-1 flag,
#                    explicit 0 skips
#   repro            scripts/repro-gate.sh (clean tracked tree required)
#   coverage         scripts/coverage-gate.sh --stability — the reproducibility
#                    two-run protocol (identical run-to-run totals); LAST,
#                    never parallel with anything
#
# Window phases around the chain (the chain above is Phase 1 of the serial
# window; these bracket it and are executed by the operator, wrapped in the
# same watchdog discipline):
#
#   N1 — BEFORE the chain, output committed first (lint-gate consumes
#        tests/harness/lint_codes.txt and the repro step refuses a dirty
#        tracked tree, so the regen can never run inside the chain):
#          cargo test -p dartq-lints --test registry_pin \
#            regen_lint_codes_freeze -- --ignored
#            (rewrites tests/harness/lint_codes.txt, curated header
#            preserved; then reconcile the header prose with the new row
#            count — the STRUCTURALLY_SILENT pin demands exact set equality)
#          cargo test -p dartq-lints --test registry_pin -- --test-threads 2
#            (every pin green, including the count pin)
#        The historical precondition (the 244->263 promotion, #1153, then the
#        W6 closeout to 276) has landed and N1's output is committed: the
#        count pin expects the current registry and lint-gate opens green.
#        A red after that is real freeze/registry drift — never
#        hand-silenced; adjudicate the drift instead.
#
#   N2 — AFTER the chain: the 212-case known-diffs re-adjudication.
#        Soundness (goal #1264): the former instrument read "divergence
#        fixed" off `harness replay` exit codes — unsound, because replay
#        pits one live dartq daemon session against a fresh dartq
#        recompute (BOTH ARMS DARTQ), so exit 0 is fully compatible with
#        the divergence still red against the oracle; a replay verdict
#        only ever measures incremental consistency.
#
#        AUTHORITY — the differential face. The phase gates on
#        target/evidence/regressions-replay/replay-results.json (schema
#        known-diffs-replay-v1: per table entry one verdict
#        already-fixed | still-red | oracle-ambiguous, joined offline
#        from the fresh lowercase (code, corpus-relative file) diff
#        sets). The gate: the artifact must exist and be fresh — mtime
#        newer than./target/release/dartq and its inputs sha pin equal
#        to sha256 of tests/corpus/regressions/known-diffs.json — or the
#        phase aborts: "replay-results.json missing or stale — produce
#        it with the five differential legs below, then rerun N2; no
#        retirement without this window's differential evidence". The
#        producing legs run strictly serially, one oracle process at a
#        time, each wrapped in this script's watchdog_run discipline (or
#        the bench runbook's guarded_run out-of-chain), NEVER with
#        --persist-regressions pointing into
#        tests/corpus/regressions/cases (persisting mid-adjudication
#        would mutate the frozen evidence):
#./target/release/harness run \
#            --corpus-dir tests/corpus/m0 --corpus-dir tests/corpus/m0-syntax \
#            --corpus-dir tests/corpus/pub-null-sections \
#            --corpus-dir tests/corpus/pub-universe-shadow \
#            --oracle-dart "$DART_BIN" \
#            --dartq-cmd "./target/release/dartq analyze --format=json --sdk $SDK_ROOT" \
#            --out target/evidence/regressions-replay/m0-merged
#          (reused corpora are never re-anchored: each corpus keeps its
#          assembled.dart_tool, so a new DARTQ_SDK_CHECKOUT is not re-diffed
#          — run `harness corpus --rebuild` first when the checkout changes)
#          (the #1347 pub-null-sections root rides leg 1: its hand-written
#          config's self entry is languageVersion 3.13, the run-face floor,
#          so the per-corpus preflight admits it; it carries no pre-pinned
#          expectations — its rows only grow the leg's denominator and any
#          divergence lands in the leg diff for N2 triage)
#          (the #1377 pub-universe-shadow root rides leg 1 the same way:
#          every entry in both hand-written configs is languageVersion
#          3.13; it exercises the #1344 per-context package universes —
#          the name `sibling` maps to pkgs/sibling_a in the root config
#          and pkgs/sibling_b in pkgs/tool's config, the crossing imports
#          (package:tool from the root, package:universe_shadow from tool)
#          are uri_does_not_exist, and the wrong-context members
#          (siblingBOnly in the root, siblingAOnly in tool) are undefined;
#          a flattened universe resolving either crossing differently is
#          the divergence this root exists to catch — same denominator
#          rule, N2 triage pins the exact row set)
#          (leg 2: the same shape over --corpus-dir tests/corpus/m1-semantic
#          with --out target/evidence/regressions-replay/m1)
#          (leg 3: a bare dump pair, on purpose — the sii corpus has no
#.dart_tool and the run-face preflight rejects it; never mutate
#          the corpus to force it through:
#            "$DART_BIN" analyze --format=machine tests/corpus/sii-probes \
#              > target/evidence/regressions-replay/sii.oracle.machine
#./target/release/dartq analyze --format=json --sdk "$SDK_ROOT" \
#              tests/corpus/sii-probes \
#              > target/evidence/regressions-replay/sii.dartq.json)
#          (legs 4 and 5: the remaining #1347 pub roots, bare dump pairs in
#          the leg-3 idiom — never `harness run`, which would die whole:
#          pub-nested-example's configs sit at languageVersion 3.5/3.2,
#          below the run-face floor (the nested-versions precedent), and
#          pub-corrupt-config's package_config.json is truncated mid-entry,
#          so the preflight's JSON parse fails. Set-compare each pair
#          offline on the same lowercase (code, file) caliber:
#            "$DART_BIN" analyze --format=machine tests/corpus/pub-nested-example \
#              > target/evidence/regressions-replay/pub-nested-example.oracle.machine
#./target/release/dartq analyze --format=json --sdk "$SDK_ROOT" \
#              tests/corpus/pub-nested-example \
#              > target/evidence/regressions-replay/pub-nested-example.dartq.json
#            "$DART_BIN" analyze --format=machine tests/corpus/pub-corrupt-config \
#              > target/evidence/regressions-replay/pub-corrupt-config.oracle.machine
#./target/release/dartq analyze --format=json --sdk "$SDK_ROOT" \
#              tests/corpus/pub-corrupt-config \
#              > target/evidence/regressions-replay/pub-corrupt-config.dartq.json
#          On pub-corrupt-config the oracle may emit its own error-tier row
#          for the corrupt resolution while dartq answers with the bounded
#          DARTQ_PACKAGE_CONFIG_INVALID containment face — that oracle-side
#          noise is the pin's point, expected and adjudicated at N2
#          triage, never a gate verdict. replay-join consumes none of
#          these dumps: no known-diffs entry cites the corpora yet, and
#          the joiner aborts on an unmapped corpus, so any future entry
#          forces the leg mapping consciously.) The complex-gate corpus
#          stays unmapped the same way: its chain step is the leg and its
#          evidence lands under target/evidence/gate-complex/, which
#          replay-join never reads.
#        Retirement decisions come ONLY from replay-results.json:
#        already-fixed retires the case (its json + edits + its
#        known-diffs rows; the 18 re-persisted sii fixtures swept only
#        after all 16 cases are decided); still-red keeps with a fresh repro
#        date in the reason; oracle-ambiguous adjudicates nothing — fix
#        the environment, rerun the leg. Retire-on-green only.
#
#        COMPANION — the per-case incremental replay, demoted from
#        verdict instrument: mandatory for the 16 sii-probes cases (25
#        known-diffs rows; 18 fixtures on disk), optional otherwise. One
#        case at a time — one daemon + one analyze subprocess pair,
#        never two product processes:
#          target/debug/harness replay \
#            --corpus-dir <dir from the case json's corpus field> \
#            --edits tests/corpus/regressions/cases/<case>.edits.jsonl \
#            --dartq-cmd "./target/release/dartq analyze --format=json" \
#            --daemon-cmd "./target/release/dartq daemon {dir}"
#        Its exit codes are incremental-consistency verdicts, never
#        retirement verdicts: exit 0 non-vacuous = incrementally
#        consistent; exit 1 = incremental divergence (fresh repro date
#        in the reason); exit 2 or vacuous-all-equal = NOT a verdict —
#        fix the environment, rerun.
#        Tail, after the batch: `cargo test -p
#        harness sem2_codes_file_matches_derivation` (a moved derived set
#        is consciously re-frozen, never auto-rewritten); `./target/debug/
#        harness run --list-known` (every surviving entry ACTIVE; prune
#        STALE only after a re-check); `scripts/untracked-ref-gate.sh`
#        clean; then commit the retirement batch.
#
#   N3 — AFTER the chain, before bench: nested multi-version dual-leg.
#        tests/corpus/nested-versions cannot run through `harness run` (its
#        preflight rejects any package under the analysis floor, by design,
#        no bypass), so this face is a hand-composed dual-leg in the
#        m0-gate idiom, zero code changes — oracle leg
#        `"$DART_BIN" analyze --format=machine
#        tests/corpus/nested-versions` vs dartq leg
#        `./target/release/dartq analyze --format=json --sdk $SDK_ROOT
#        tests/corpus/nested-versions`, then a jq set-compare of lowercase
#        (code,file) pairs per file against the README expectation
#        inventory (tests/corpus/nested-versions/README.md, 17 files).
#        Pass: identical oracle/dartq verdict sets on every file. The two
#        options-driven lint expectations (always_declare_return_types,
#        prefer_final_locals) are admitted on both sides symmetrically or
#        dropped on both — never one-sided.
#
# Watchdog (vendored from the /tmp/rss_cmd.sh pattern — /tmp is ephemeral,
# so this script is self-contained): each step runs in its OWN session —
# launched via `perl -MPOSIX -e 'setsid; exec @ARGV'` (perl ships with
# macOS; bash 3.2 has no setsid) — so its process group is exactly its
# descendant tree, with the orchestrator and every other step outside it.
# Each step's stdout+stderr goes to /tmp/serial_<step>.log with a trailing
# `exit=<code>` line; a 5 s poll sums the step's process-group RSS and, on
# breach, kills the step's whole group (exact — no descendant walk; one
# re-kill ~200 ms later catches a descendant forked inside the kill window)
# and appends `WATCHDOG_KILL rss=<KB> limit=<KB>`. An EXIT/INT/TERM trap
# reaps the current step's group if the orchestrator itself dies mid-step.
# Unlike the /tmp original, a watchdog kill propagates: the step is judged
# KILLED and the chain stops. Caps: 8 GiB everywhere (RSS_LIMIT_KB
# overrides), 12 GiB for mega-r1 (runbook cap above the bench's internal
# 8 GiB).
#
# Labels: every evidence-producing step gets a fresh default label; pass
# SERIAL_TAG=<suffix> to namespace a rerun (e.g. SERIAL_TAG=r2 ->
# trend-w5-r2, m1-r5-r2, mega-r1-r2). SERIAL_TAG is MANDATORY, non-optional:
# the 2026-10-13 window burned the default labels (recorded evidence at
# target/evidence/gate-m1/m1-r5/ refuses rerun with exit 2 — a tagless run
# would die mid-chain at the m1-r5 step), so the preflight dies without it.
# A gate whose evidence directory
# already exists refuses to run (exit 2) — recorded evidence is never
# overwritten; bump SERIAL_TAG or clean up consciously. A FAIL/KILLED step's
# /tmp log is additionally copied to target/evidence/serial/<step><TAG>/
# (untracked) so an untagged rerun — which reuses the same /tmp path —
# cannot orphan the failing run's log; the summary row names the archive.
#
# Environment: SERIAL_TAG (label suffix — MANDATORY this window, see
# Labels), SERIAL_MEGA_SCALE (1k|10k|100k|all,
# default 10k), SERIAL_MEGA_ORACLE_LEG (1 default, 0 drops the 1k parity
# leg), SERIAL_COVERAGE_HTML (1 adds an html pass after the coverage
# stability pair), DARTQ_M4_LIVE (1 default this window — the three -live
# steps run: they spawn real dart plugin/host processes and this script is
# the sanctioned serial window; set 0 to skip them via a clean logged
# skip), RSS_LIMIT_KB (default 8388608), FVM_HOME, DARTQ_CMD /
# DARTQ_ANALYZE_CMD (trend faces; defaults are the m0-gate shapes — the
# mega oracle leg ignores both, pinning its own release-face DARTQ_CMD
# for the A7 cold pair). The
# wrapped gate scripts honor their own env overrides unchanged.
#
# Usage:
#   scripts/serial-gates.sh [--from <step>] [--only <step>[,<step>...]]
#                           [--skip <step>...] [--help]
#
#   --from STEP     start the chain at STEP (everything before is skipped).
#   --only LIST     run exactly the named steps, in chain order (a red step
#                   can be re-run alone after a fix). Mutually exclusive
#                   with --from.
#   --skip LIST     drop the named steps from the selection (combinable with
#                   --from; with --only it filters the only-list).
#   Fail-fast: first failing step aborts the chain; the end summary always
#   prints step | status | wall | peak-RSS | log. Exit codes: 0 all selected
#   steps green; 1 a step failed or was watchdog-killed; 2 usage or
#   preflight error (missing jq/oracle/corpus .dart_tool, unknown or empty
#   step selection, dirty tracked tree with the repro step selected,
#   missing cargo-llvm-cov, existing evidence directory). Mid-chain guard
#   failures (missing rebuild artifact, union drift, bad SERIAL_MEGA_SCALE)
#   mark their own step FAIL and still print the summary — exit 1.

set -Eeuo pipefail
cd "$(dirname "$0")/.." || exit 2

RSS_DEFAULT_KB="${RSS_LIMIT_KB:-8388608}"
MEGA_CAP_KB=12582912
M0_OUT_ROOT="target/evidence/gate-m0"
UNION_CODES="target/gate/union-codes.txt"
EXPECTED_UNION_COUNT=290
FVM_HOME="${FVM_HOME:-$HOME/fvm}"
SDK_ROOT="$FVM_HOME/default/bin/cache/dart-sdk"
DART_BIN="$SDK_ROOT/bin/dart"
# dartq falls back to $DART_SDK when no --sdk is passed (captured replay
# templates and the companion replay arm carry none); without the export
# both replay arms die on a usage-tier "DART_SDK unset" error.
export DART_SDK="$SDK_ROOT"
CHECK_CMD="${DARTQ_CMD:-./target/release/dartq check --format=json}"
ANALYZE_CMD="${DARTQ_ANALYZE_CMD:-./target/release/dartq analyze --format=json --sdk $SDK_ROOT}"
# The live faces run by default in the serial window (the 2026-10-13 live
# failure predates the #1137 discovery/spawn rewrite); an explicit 0 opts
# out via a clean logged skip. Only the literal "1" runs them.
DARTQ_M4_LIVE="${DARTQ_M4_LIVE:-1}"
# The orchestrator's own process group: kill_tree refuses to signal it (a
# step probed inside its pre-setsid launch window would otherwise group-kill
# the orchestrator itself).
ORCH_PGID="$(ps -o pgid= -p $$ | tr -d ' ')"

TAG="${SERIAL_TAG:+-${SERIAL_TAG}}"
LBL_TREND="trend-w5${TAG}"
LBL_M1="m1-r5${TAG}"
LBL_ACC="serial${TAG}"
LBL_LINT="lints-r1${TAG}"
LBL_INCR="incr-r1${TAG}"
LBL_M2="m2-r1${TAG}"
LBL_SEM2="sem2-r1${TAG}"
LBL_M3="m3-r1${TAG}"
LBL_COMPLEX="complex-r1${TAG}"
LBL_APP="app-r1${TAG}"
LBL_TORTURE="torture-r1${TAG}"
LBL_MEGA="mega-r1${TAG}"
LBL_MEGAX="mega-x-r1${TAG}"
LBL_SCHEMA="schema-freeze-r1${TAG}"
LBL_REPRO="serial${TAG}"

# The authoritative chain — 36 steps, fixed order. This array defines the
# count and order (older runbook tables compressed suite rows and drifted;
# trust this array).
STEPS=(
  release-rebuild debug-rebuild bench-compile-guard harness-pins trend-w5-A
  trend-w5-B m1-r5 acceptance lint-gate lints-new-gate incremental-gate
  m2-r1 sem2-gate m3-r1 complex-gate app-gate torture-gate mega-r1 mega-x-r1
  scanner-suite flow-suite parser-suite foundation-suites
  semantic-suite lints-suite engine-suite daemon-suite plugin-suite
  commands-suite schema-suite schema-freeze daemon-live plugin-live
  host-live repro coverage
)

usage() {
  sed -n '2,/^set -Eeuo pipefail$/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

die() { # <message...>  usage/preflight error
  echo "serial-gates: $*" >&2
  exit 2
}

step_fail() { # <message...> — mid-chain guard failure: reported like any red
  # step (the chain loop records FAIL(rc=2), aborts the rest and still
  # prints the summary) instead of a bare exit that bypasses the summary.
  echo "serial-gates: $*" >&2
  WATCH_LOG=""
  WATCH_PEAK_KB=0
  return 2
}

is_step() { # <name>
  local s
  for s in "${STEPS[@]}"; do [ "$s" = "$1" ] && return 0; done
  return 1
}

in_list() { # <comma-list> <name>
  local item IFS=','
  for item in $1; do [ "$item" = "$2" ] && return 0; done
  return 1
}

index_of() { # <step> -> prints its chain index
  local i
  for i in "${!STEPS[@]}"; do
    [ "${STEPS[$i]}" = "$1" ] && {
      echo "$i"
      return 0
    }
  done
  return 1
}

cap_for_step() {
  case "$1" in
  mega-r1 | mega-x-r1) echo "$MEGA_CAP_KB" ;;
  *) echo "$RSS_DEFAULT_KB" ;;
  esac
}

# --- argument parsing ---------------------------------------------------------

FROM_STEP=""
ONLY_LIST=""
SKIP_LIST=""
while [ $# -gt 0 ]; do
  case "$1" in
  --from)
    [ $# -ge 2 ] || die "--from needs a step name"
    FROM_STEP="$2"
    shift 2
    ;;
  --only)
    [ $# -ge 2 ] || die "--only needs a comma-separated step list"
    ONLY_LIST="$2"
    shift 2
    ;;
  --skip)
    [ $# -ge 2 ] || die "--skip needs a comma-separated step list"
    SKIP_LIST="$2"
    shift 2
    ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    die "unknown argument '$1' (see scripts/serial-gates.sh --help)"
    ;;
  esac
done

[ -z "$FROM_STEP" ] || [ -z "$ONLY_LIST" ] || die "--from and --only are mutually exclusive"
[ -z "$FROM_STEP" ] || is_step "$FROM_STEP" || die "unknown --from step '$FROM_STEP'"
if [ -n "$ONLY_LIST" ]; then
  item="$(printf '%s' "$ONLY_LIST" | tr ',' ' ')"
  for item in $item; do is_step "$item" || die "unknown --only step '$item'"; done
fi
if [ -n "$SKIP_LIST" ]; then
  item="$(printf '%s' "$SKIP_LIST" | tr ',' ' ')"
  for item in $item; do is_step "$item" || die "unknown --skip step '$item'"; done
fi

selected() { # <step> -> 0 if the step runs in this invocation
  if [ -n "$ONLY_LIST" ]; then
    in_list "$ONLY_LIST" "$1" || return 1
  elif [ -n "$FROM_STEP" ]; then
    local seen=0 s
    for s in "${STEPS[@]}"; do
      [ "$s" = "$FROM_STEP" ] && seen=1
      if [ "$s" = "$1" ] && [ "$seen" = 1 ]; then
        # --skip applies in --from mode too (this branch used to return
        # before consulting SKIP_LIST, silently running skipped steps).
        ! in_list "$SKIP_LIST" "$1"
        return
      fi
    done
    return 1
  fi
  ! in_list "$SKIP_LIST" "$1"
}

# An empty selection (--from X --skip X, or a --skip that empties --only)
# must die here, not fall through to a triumphant "CHAIN PASSED".
pending_steps=0
for s in "${STEPS[@]}"; do
  if selected "$s"; then pending_steps=$((pending_steps + 1)); fi
done
[ "$pending_steps" -gt 0 ] || die "empty selection: --from/--only/--skip combine to zero steps — nothing to run"

# --- watchdog (vendored; self-contained, no /tmp dependency) ------------------

WATCH_PEAK_KB=0
WATCH_LOG=""
CURRENT_STEP_PID="" # pid of the running step (own session); "" when idle

kill_tree() { # <pid> — SIGKILL the step's whole tree. Every step runs in its
  # own session (setsid launch below), so its process group is exactly its
  # descendant tree and a group kill is exact — the orchestrator and every
  # other step sit outside it. One re-kill ~200 ms later catches a
  # descendant forked between the first kill's group scan and delivery: it
  # inherits the group after the sweep and would otherwise outlive its
  # killed parent.
  local pid="$1" pgid
  pgid="$(ps -o pgid= -p "$pid" 2>/dev/null | tr -d ' ')"
  [ -n "$pgid" ] || return 0
  if [ "$pgid" = "$ORCH_PGID" ]; then
    # Pre-setsid launch window: never signal our own group.
    kill -9 "$pid" 2>/dev/null || true
    return 0
  fi
  kill -9 "-$pgid" 2>/dev/null || true
  sleep 0.2
  kill -9 "-$pgid" 2>/dev/null || true
}

reap_current_step() { # EXIT/INT/TERM safety net: a step sits in its own
  # session, so an orchestrator death mid-step would otherwise leave it
  # running orphaned with nobody watching its RSS.
  if [ -n "${CURRENT_STEP_PID:-}" ] && kill -0 "$CURRENT_STEP_PID" 2>/dev/null; then
    kill_tree "$CURRENT_STEP_PID"
  fi
}
trap reap_current_step EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

watchdog_run() { # <name> <rss_limit_kb> <cmd...> — sets WATCH_PEAK_KB, WATCH_LOG
  local name="$1" limit="$2" pgid rss max=0 rc=0
  shift 2
  WATCH_LOG="/tmp/serial_${name}.log"
  WATCH_PEAK_KB=0
  : >"$WATCH_LOG"
  # Own session per step: perl ships with macOS and bash 3.2 has no setsid.
  # setsid makes the child a session leader (group id == its own pid), so
  # the RSS poll below sums this step alone — never the orchestrator, never
  # a previous step's orphans. exec keeps the pid, so the code waited on is
  # the command's own. The brief pre-setsid window can only sum the tiny
  # orchestrator group — far below any cap, never a false kill.
  perl -MPOSIX -e 'setsid; exec @ARGV' -- "$@" >>"$WATCH_LOG" 2>&1 &
  CURRENT_STEP_PID=$!
  while kill -0 "$CURRENT_STEP_PID" 2>/dev/null; do
    pgid="$(ps -o pgid= -p "$CURRENT_STEP_PID" 2>/dev/null | tr -d ' ')"
    rss=0
    if [ -n "$pgid" ]; then
      rss="$(ps -axo rss=,pgid= 2>/dev/null | awk -v g="$pgid" '$2==g {s+=$1} END {print s+0}')"
    fi
    if [ "$rss" -gt "$max" ]; then max="$rss"; fi
    if [ "$rss" -gt "$limit" ]; then
      kill_tree "$CURRENT_STEP_PID"
      printf 'WATCHDOG_KILL rss=%sKB limit=%sKB\n' "$rss" "$limit" >>"$WATCH_LOG"
      break
    fi
    sleep 5
  done
  wait "$CURRENT_STEP_PID" 2>/dev/null || rc=$?
  CURRENT_STEP_PID=""
  printf 'exit=%s\n' "$rc" >>"$WATCH_LOG"
  WATCH_PEAK_KB="$max"
  # Kill-exit propagation (the /tmp original never propagated a breach): a
  # watchdog kill is a failure regardless of the reaped status.
  if grep -q '^WATCHDOG_KILL' "$WATCH_LOG"; then
    return 1
  fi
  return "$rc"
}

# --- preflight ----------------------------------------------------------------

check_corpus() { # <corpus-dir>
  [ -f "$1/pubspec.yaml" ] || die "$1 is not a corpus package — assemble it first (cargo run -p harness -- corpus)"
  [ -d "$1/.dart_tool" ] || die "$1 has no .dart_tool — pub get it first (cd $1 && dart pub get; NEVER pub-get m2-warning or lints)"
}

preflight() {
  command -v jq >/dev/null 2>&1 || die "jq is required by the differential gates (brew install jq)"
  # The N5 face and the N3 nested dual-leg consume git-tracked pub
  # resolution artifacts (the.gitignore negation entries keep them
  # tracked): their absence means a truncated checkout, not a missing
  # pub-get — die before the chain burns hours on an analysis floor that
  # cannot resolve.
  local tracked_artifact
  for tracked_artifact in \
    tests/corpus/lints-new/.dart_tool/package_config.json \
    tests/corpus/nested-versions/.dart_tool/package_config.json \
    tests/corpus/nested-versions/.dart_tool/pub/workspace_ref.json; do
    [ -f "$tracked_artifact" ] || die "$tracked_artifact not found — this file is git-tracked ('git ls-files $tracked_artifact' lists it), so its absence means a truncated checkout, not a missing pub-get; re-checkout the repository and verify with 'git status'"
  done
  # SERIAL_TAG is mandatory: the default labels are burned (recorded
  # evidence at target/evidence/gate-m1/m1-r5/ refuses rerun with exit 2) —
  # a tagless run would die mid-chain at the m1-r5 step, so fail here.
  [ -n "${SERIAL_TAG:-}" ] ||
    die "SERIAL_TAG is mandatory: the default labels are burned (recorded evidence at target/evidence/gate-m1/m1-r5/ refuses rerun with exit 2) — rerun with SERIAL_TAG=<fresh suffix>"
  local need_dart=0 need_release=0 need_debug=0 count s
  for s in "${STEPS[@]}"; do
    selected "$s" || continue
    case "$s" in
    daemon-live | plugin-live | host-live)
      # The live faces are a clean skip only on an explicit non-1 flag —
      # demand the SDK whenever they will actually run (the default).
      [ "$DARTQ_M4_LIVE" = 1 ] && need_dart=1
      ;;
    schema-freeze)
      need_dart=1
      command -v python3 >/dev/null 2>&1 ||
        die "python3 is required by the schema-freeze sweep (brew install python3)"
;;
    trend-* | m1-r5 | m2-r1 | sem2-gate | m3-r1 | lint-gate | incremental-gate | complex-gate | app-gate | torture-gate | mega-r1 | mega-x-r1 | repro) need_dart=1;;
    lints-new-gate) need_dart=1;;
    esac
    case "$s" in
    trend-* | acceptance) need_release=1;;
    lints-new-gate | complex-gate | app-gate) need_release=1;;
    m1-r5 | m2-r1 | sem2-gate | m3-r1 | lint-gate | torture-gate | mega-r1 | mega-x-r1) need_debug=1;;
    esac
    case "$s" in
    trend-w5-A | trend-w5-B)
      check_corpus tests/corpus/m0
      check_corpus tests/corpus/m0-syntax
      [ -f tests/corpus/syntax-codes.txt ] || die "tests/corpus/syntax-codes.txt not found (the 255-code syntax universe)"
      [ -f tests/harness/m1_codes.txt ] || die "tests/harness/m1_codes.txt not found (the frozen M1 universe)"
;;
    lints-new-gate)
      [ -f tests/corpus/lints-new/pubspec.yaml ] ||
        die "tests/corpus/lints-new is not a corpus package — the N5 face is missing"
      [ -d tests/corpus/lints-new/.dart_tool ] ||
        die "tests/corpus/lints-new has no.dart_tool — it is hand-materialized (see the corpus README; NEVER pub-get this corpus)"
      [ -f tests/corpus/lints-new/expectations.json ] ||
        die "tests/corpus/lints-new/expectations.json not found (the N5 expectation table)"
;;
    complex-gate)
      [ -f tests/corpus/complex/pubspec.yaml ] ||
        die "tests/corpus/complex is not a corpus package — the complex differential face is missing"
      [ -d tests/corpus/complex/.dart_tool ] ||
        die "tests/corpus/complex has no.dart_tool — it is hand-materialized (see the corpus README; NEVER pub-get this corpus)"
;;
    torture-gate)
      [ -f tests/corpus/torture/corpus/pubspec.yaml ] ||
        die "tests/corpus/torture/corpus is not generated — run: python3 tests/corpus/torture/tools/gen_torture.py --force (the corpus is git-ignored, same policy as mega)"
;;
    m1-r5) check_corpus tests/corpus/m1-semantic;;
    m3-r1) check_corpus tests/corpus/m3-unused;;
    incremental-gate)
      [ -d tests/corpus/incremental ] ||
        die "tests/corpus/incremental not found (the curated edit-sequence corpus)"
      ls tests/corpus/incremental/*/edits.jsonl >/dev/null 2>&1 ||
        die "no edit sequences under tests/corpus/incremental (each sequence dir needs edits.jsonl)"
;;
    trend-w5-B)
      # The union universe is pinned (m0-gate contract) — drift surfaces
      # here, not at the trend-w5-B step.
      count="$(compose_union | wc -l | tr -d ' ')" ||
        die "union composition failed (are the corpus code lists readable?)"
      [ "$count" -eq "$EXPECTED_UNION_COUNT" ] ||
        die "union code universe drifted: expected $EXPECTED_UNION_COUNT = 255 + 36 - 1 overlap, found $count (m0-gate pins this exact universe)"
      ;;
    esac
  done
  if [ "$need_dart" = 1 ] && [ ! -x "$DART_BIN" ]; then
    die "fvm default dart not found at $DART_BIN (install the fvm default pin or set FVM_HOME)"
  fi
  # The repro gate refuses a dirty tracked tree — surface that BEFORE the
  # hours of work, not at the repro step (the plan's worktree note stays).
  if selected repro && [ -n "$(git status --porcelain --untracked-files=no)" ]; then
    die "the repro step is selected but the tracked tree is dirty — commit the tracked waves first (repro-gate would refuse at the repro step, stranding the chain's evidence)"
  fi
  # Fail-closed tool probe, the coverage-gate.sh preflight contract.
  if selected coverage && ! cargo llvm-cov --version >/dev/null 2>&1; then
    die "cargo-llvm-cov is not installed; the coverage step cannot run (install once: cargo install cargo-llvm-cov --locked; also required: rustup component add llvm-tools-preview)"
  fi
  if [ "$need_release" = 1 ] && [ ! -x "./target/release/harness" ]; then
    echo "serial-gates: NOTE ./target/release/harness missing — release-rebuild provides it" >&2
  fi
  if [ "$need_debug" = 1 ] && [ ! -x "./target/debug/harness" ]; then
    echo "serial-gates: NOTE ./target/debug/harness missing — debug-rebuild provides it" >&2
  fi
}

# --- trend runs (composed in the m0-gate.sh idiom; no trend script exists) ----

compose_union() {
  sed -e 's/#.*//' tests/corpus/syntax-codes.txt tests/harness/m1_codes.txt |
    awk '{print $1}' |
    sed 's/[[:space:]]//g' |
    grep -v '^$' |
    sort -f -u
}

trend_guard() { # <outdir> — refuse to overwrite recorded evidence
  [ -e "$1" ] && die "$1 already exists — refusing to overwrite recorded gate evidence (bump SERIAL_TAG)"
  return 0
}

trend_run() { # <syntax|union>
  local face="$1" outdir harness_args
  if [ "$face" = "syntax" ]; then
    outdir="$M0_OUT_ROOT/$LBL_TREND-syntax"
  else
    outdir="$M0_OUT_ROOT/$LBL_TREND-union"
  fi
  trend_guard "$outdir"
  [ -x ./target/release/harness ] || step_fail "./target/release/harness not found — release-rebuild first (cargo build --release -p dartq-cli -p harness)"
  [ -x "$DART_BIN" ] || step_fail "fvm default dart not found at $DART_BIN"
  case "$CHECK_CMD $ANALYZE_CMD" in *"./target/release/dartq"*) [ -x ./target/release/dartq ] || step_fail "./target/release/dartq not found — release-rebuild first" ;; esac

  harness_args=(
    run
    --corpus-dir tests/corpus/m0
    --corpus-dir tests/corpus/m0-syntax
    --oracle-dart "$DART_BIN"
    --severity-filter ERROR
    --threshold 1.0
    --persist-regressions tests/corpus/regressions/cases
    --out "$outdir"
  )
  if [ "$face" = "syntax" ]; then
    watchdog_run "trend-w5-A${TAG}" "$(cap_for_step trend-w5-A)" \
      ./target/release/harness "${harness_args[@]}" \
      --dartq-cmd "$CHECK_CMD" \
      --codes-file tests/corpus/syntax-codes.txt
  else
    mkdir -p "$(dirname "$UNION_CODES")"
    compose_union >"$UNION_CODES"
    local count
    count="$(wc -l <"$UNION_CODES" | tr -d ' ')"
    [ "$count" -eq "$EXPECTED_UNION_COUNT" ] ||
      step_fail "union code universe drifted: expected $EXPECTED_UNION_COUNT = 255 + 36 - 1 overlap, found $count (m0-gate pins this exact universe)"
    watchdog_run "trend-w5-B${TAG}" "$(cap_for_step trend-w5-B)" \
      ./target/release/harness "${harness_args[@]}" \
      --dartq-cmd "$ANALYZE_CMD" \
      --codes-file "$UNION_CODES"
  fi
}

mega_run() { # perf + parity: 1k oracle leg (parity) + scale run (perf)
  local scale="${SERIAL_MEGA_SCALE:-10k}"
  case "$scale" in 1k | 10k | 100k | all) ;; *) step_fail "SERIAL_MEGA_SCALE must be 1k|10k|100k|all, got '$scale'" ;; esac
  [ -x ./target/debug/harness ] || step_fail "./target/debug/harness not found — debug-rebuild first"
  if [ "${SERIAL_MEGA_ORACLE_LEG:-1}" = 1 ]; then
    # The A7 dartq cold leg must meet the release-quality oracle on the same
    # face (a debug dartq understates the oracle/dartq ratio). Scoped env
    # prefix — never a global assignment: CHECK_CMD/ANALYZE_CMD above read
    # DARTQ_CMD for the trend faces, and the perf-scale invocation below
    # keeps its default (debug) face. The flags are mega-run.sh's own
    # DARTQ_CMD default shape; only the binary upgrades to release.
    if [ ! -x ./target/release/dartq ]; then
      step_fail "./target/release/dartq not found — the A7 cold leg drives the release face; run the release-rebuild step first (cargo build --release -p dartq-cli -p harness)"
      return 2
    fi
    watchdog_run "mega-r1-oracleleg${TAG}" "$MEGA_CAP_KB" \
      env DARTQ_CMD="./target/release/dartq analyze --format=json --sdk $SDK_ROOT" \
      scripts/mega-run.sh --scale 1k --oracle-leg --strict --label "${LBL_MEGA}-oracleleg" || return 1
    leg_peak="$WATCH_PEAK_KB"
  fi
  local rc=0
  if [ "$scale" = "all" ]; then
    watchdog_run "mega-r1${TAG}" "$MEGA_CAP_KB" \
      scripts/mega-run.sh --all --strict --label "$LBL_MEGA" || rc=1
  else
    watchdog_run "mega-r1${TAG}" "$MEGA_CAP_KB" \
      scripts/mega-run.sh --scale "$scale" --strict --label "$LBL_MEGA" || rc=1
  fi
  # Peak of the step = heaviest phase (the leg's peak survives the perf run).
  if [ -n "${leg_peak:-}" ] && [ "${leg_peak:-0}" -gt "${WATCH_PEAK_KB:-0}" ]; then
    WATCH_PEAK_KB="$leg_peak"
  fi
  # The final phase's rc is returned explicitly: step_runner runs in a
  # guarded context (set -e suspended), so a bare fall-through after the
  # peak-max if would mask a red perf run as PASS.
  return "$rc"
}

m4_live_skip() { # <step> — the DARTQ_M4_LIVE=1 opt-in gate for the live
  # faces. The flag defaults to 1 (this script IS the sanctioned serial
  # window); only an explicit non-1 value reaches the skip. Returns 1 (run
  # the step) when the flag is 1; otherwise writes the skip note as the
  # step's log (watchdog_run log shape, trailing exit= line) and returns
  # 0 — a clean skip the summary reports as PASS.
  if [ "$DARTQ_M4_LIVE" = 1 ]; then
    return 1
  fi
  WATCH_LOG="/tmp/serial_$1.log"
  WATCH_PEAK_KB=0
  {
    echo "SKIP: DARTQ_M4_LIVE='$DARTQ_M4_LIVE' (not 1) — the live dart"
    echo "plugin/host process faces run by default in the serial window;"
    echo "they were skipped by the explicit override."
    echo "exit=0"
  } >"$WATCH_LOG"
  echo "-- skipped: DARTQ_M4_LIVE='$DARTQ_M4_LIVE' (live dart-process faces need the default flag)"
  return 0
}

# --- per-step dispatch --------------------------------------------------------

step_runner() { # <step> — runs every watchdogged phase of the step
  case "$1" in
  release-rebuild)
    watchdog_run "$1" "$(cap_for_step "$1")" cargo build --release -p dartq-cli -p harness
    ;;
  debug-rebuild)
    watchdog_run "$1" "$(cap_for_step "$1")" cargo build -p dartq-cli -p harness
;;
  bench-compile-guard)
    # Bench targets are invisible to `cargo build -p` and to
    # `cargo check --tests` — only --all-targets compiles them. Catching
    # the drift here costs a check; catching it at bench time costs a
    # debug session (the #979 class: a config-struct field batch left the
    # engine bench harness uncompilable, unseen by every test face).
    # Compile-only by construction: cargo check never executes a bench.
    watchdog_run "$1" "$(cap_for_step "$1")" \
      cargo check --all-targets -p dartq-engine
;;
  harness-pins)
    # The frozen-universe pins are file-derived: running them immediately
    # after the rebuilds means a universe drift fails the chain at phase
    # start, not at the tail coverage step.
    watchdog_run "$1" "$(cap_for_step "$1")" \
      cargo test -p harness -- --test-threads 2
;;
  trend-w5-A) trend_run syntax;;
  trend-w5-B) trend_run union;;
  m1-r5)
    watchdog_run "$1" "$(cap_for_step "$1")" scripts/m1-gate.sh --label "$LBL_M1"
    ;;
  acceptance)
    watchdog_run "$1" "$(cap_for_step "$1")" scripts/acceptance-gate.sh --label "$LBL_ACC"
    ;;
  lint-gate)
    # Red here almost always means the pre-window lint-freeze regen (N1,
    # see the header) has not run after the rule promotion — fix the
    # precondition, never silence the step.
    watchdog_run "$1" "$(cap_for_step "$1")" scripts/lint-gate.sh --label "$LBL_LINT"
;;
  lints-new-gate)
    # N5 (goal #1160): the dartq leg over the 19-name promotion-wave
    # corpus, multiset-compared (code|file|line) against the corpus's
    # expectation table. The table IS the oracle-derived ground truth —
    # verified upstream-marker-by-marker; the structurally-silent families
    # (unnecessary_ignore, remove_deprecations_in_breaking_versions) are
    # encoded as empty dartq-leg entries, so a dartq row there reds the
    # gate instead of slipping through (corpus README, adjudications).
    [ -x ./target/release/dartq ] ||
      step_fail "./target/release/dartq not found — the N5 face drives the release face; run the release-rebuild step first" || return 2
    # Exit codes 1-3 are `dart analyze`'s issue tiers (info/warning/error
    # found) — this corpus carries compile errors by design, so the oracle
    # itself exits 3; only a crash code or a watchdog kill fails the step.
    local analyze_rc=0
    watchdog_run "$1" "$(cap_for_step "$1")" \
./target/release/dartq analyze --format=json --sdk "$SDK_ROOT" \
      tests/corpus/lints-new || analyze_rc=$?
    if [ "$analyze_rc" -gt 3 ] || grep -q '^WATCHDOG_KILL' "$WATCH_LOG"; then
      step_fail "dartq analyze over tests/corpus/lints-new exited $analyze_rc (crash or watchdog kill — issue tiers 1-3 are expected)"
      return 1
    fi
    # The json envelope is one stdout line inside the watchdog log (the
    # log also carries stderr and the trailing exit= marker); a missing
    # envelope is a parse failure of this step, never an empty set.
    local json_line=""
    json_line="$(grep -m1 '^{"version":1,"diagnostics":' "$WATCH_LOG" || true)"
    if [ -z "$json_line" ]; then
      step_fail "N5 face: no json envelope in ${WATCH_LOG} (the analyze leg wrote no parsable output)"
      return 1
    fi
    local expected_set actual_set
    expected_set="$(jq -r '
.expected | to_entries[] as $f |
      $f.value | to_entries[] | $f.key as $file |
.key as $code |.value[] | "\($code)|\($file)|\(.)"' \
      tests/corpus/lints-new/expectations.json | LC_ALL=C sort)"
    # json envelope shape: {version, diagnostics:[{code, severity, type,
    # location{file, range{start{line, ...}}}, ...}]}. Filter to the 19
    # rule codes (stock diagnostics are out of face on both legs; json
    # codes are already lowercase rule names), normalize the file path to
    # corpus-root-relative (dartq prints the canonical absolute path; the
    # as-given and corpus-qualified relative shapes are kept for
    # harness-replay logs), keep the corpus's own lib/ and packages/ trees
    # only (vendored dependency rows are out of face).
    actual_set="$(jq -r --slurpfile exp tests/corpus/lints-new/expectations.json '
      .diagnostics[]
      | (.location.file
         | sub("^.*tests/corpus/lints-new/"; "")
         | sub("^\\./"; "")) as $file
      | select($file | test("^(lib|packages)/"))
      | select(.code as $c | $exp[0].rules | index($c))
      | "\(.code)|\($file)|\(.location.range.start.line)"
    ' <<<"$json_line" | LC_ALL=C sort)"
    if [ "$actual_set" = "$expected_set" ]; then
      echo "-- [$1] N5 face green: $(printf '%s\n' "$expected_set" | grep -c.) expected rows matched exactly"
      return 0
    fi
    {
      echo "N5 lints-new drift (diff: expectations.json vs dartq json envelope rows):"
      diff <(printf '%s\n' "$expected_set") <(printf '%s\n' "$actual_set") |
        sed 's/^/  | /' | head -40
    } >&2
    step_fail "N5 lints-new face drifted from expectations.json (see the diff above; full rows in ${WATCH_LOG})"
    return 1
;;
  incremental-gate)
    watchdog_run "$1" "$(cap_for_step "$1")" scripts/incremental-gate.sh --label "$LBL_INCR"
;;
  m2-r1)
    watchdog_run "$1" "$(cap_for_step "$1")" scripts/m2-gate.sh --label "$LBL_M2"
;;
  sem2-gate)
    watchdog_run "$1" "$(cap_for_step "$1")" scripts/sem2-gate.sh --label "$LBL_SEM2"
;;
  m3-r1)
    watchdog_run "$1" "$(cap_for_step "$1")" env M3_LABEL="$LBL_M3" scripts/m3-gate.sh
;;
  complex-gate)
    # N2-leg shape over the hand-materialized complex corpus (the #1399
    # census admission): oracle dart analyze vs the release analyze face
    # at the full-parity bar. No --persist-regressions — the corpus's row
    # posture is unproven until the first live run, so divergences land in
    # the recorded report for triage instead of growing the frozen
    # regression set mid-chain; see the header prose.
    [ -x ./target/release/harness ] ||
      step_fail "./target/release/harness not found — release-rebuild first (cargo build --release -p dartq-cli -p harness)" || return 2
    local outdir="target/evidence/gate-complex/$LBL_COMPLEX"
    trend_guard "$outdir"
    watchdog_run "$1" "$(cap_for_step "$1")" \
./target/release/harness run \
      --corpus-dir tests/corpus/complex \
      --oracle-dart "$DART_BIN" \
      --severity-filter ERROR \
      --threshold 1.0 \
      --dartq-cmd "$ANALYZE_CMD" \
      --out "$outdir"
;;
  app-gate)
    # #2063: the generated --flutter-app corpus face — the widget-shape
    # differential + rss@10k leg + skip-if-absent golden hash check. Release
    # face like complex-gate; the script self-guards the oracle timeout at
    # 600 s and refuses to overwrite recorded evidence. HEAVY: dropping it
    # from a tight window with --skip app-gate is a sanctioned clean skip.
    watchdog_run "$1" "$(cap_for_step "$1")" \
      scripts/app-gate.sh --label "$LBL_APP"
;;
  torture-gate)
    # Ticket #1290: the generated ~2k-file torture corpus at the
    # full-parity bar against the fvm-default oracle of record. Debug
    # face; the script self-guards the oracle version banner, raises the
    # oracle timeout to 600 s, persists divergences as regression cases,
    # and refuses to overwrite recorded evidence. SERIAL PHASE ONLY by
    # its own contract — this watchdogged chain is that phase.
    watchdog_run "$1" "$(cap_for_step "$1")" \
      scripts/torture-gate.sh --label "$LBL_TORTURE"
;;
  mega-r1) mega_run;;
  mega-x-r1)
    # The adversarial-axis gate (#1811): same corpus-gen trees under
    # tests/corpus/mega-x/ plus the X-legs. Debug face like mega-r1; the
    # script regenerates missing trees (100k included), so it rides the
    # 12 GiB mega cap. Heavy legs live inside mega-x-run.sh — serial window
    # only by construction.
    [ -x ./target/debug/harness ] ||
      step_fail "./target/debug/harness not found — debug-rebuild first"
    watchdog_run "$1" "$(cap_for_step "$1")" \
      scripts/mega-x-run.sh --strict --label "$LBL_MEGAX"
    ;;
  scanner-suite | flow-suite | parser-suite | semantic-suite | lints-suite)
    watchdog_run "$1" "$(cap_for_step "$1")" \
      cargo test -p "dartq-${1%-suite}" -- --test-threads 2
    ;;
  engine-suite)
    # Per-binary run-order (#1700; #147 ledger 2026-10-16 18:35): the heavy
    # incremental tier first at threads 2, the single warm-start test at
    # threads 1, then memory_accounting alone in its own watchdog leg (its
    # RSS numbers mean nothing with a second test process in the group),
    # then the medium tier and the remaining binaries at threads 2. Explicit
    # per-leg --test lists cannot pick up a future test binary, so the count
    # pin turns that silent skip into a loud FAIL forcing a conscious
    # refold of the legs.
    [ "$(ls crates/dartq-engine/tests/*.rs 2>/dev/null | wc -l | tr -d ' ')" = "24" ] ||
      step_fail "dartq-engine has a different test-binary count than 24 — fold new binaries into the #1700 run-order legs consciously (heavy: incremental_oracle+incremental@2; warm: incremental_warm@1; solo: memory_accounting; medium: analysis_options+modifier_restrictions+pipeline+semantic_gaps+context; rest@2 incl. sdk_faces)"
    local leg_peak rc=0
    watchdog_run "engine-heavy${TAG}" "$(cap_for_step "$1")" \
      cargo test -p dartq-engine --test incremental_oracle --test incremental -- --test-threads 2 || rc=1
    leg_peak="$WATCH_PEAK_KB"
    watchdog_run "engine-warm${TAG}" "$(cap_for_step "$1")" \
      cargo test -p dartq-engine --test incremental_warm -- --test-threads 1 || rc=1
    watchdog_run "engine-memsolo${TAG}" "$(cap_for_step "$1")" \
      cargo test -p dartq-engine --test memory_accounting -- --test-threads 1 || rc=1
    watchdog_run "engine-medium${TAG}" "$(cap_for_step "$1")" \
      cargo test -p dartq-engine --test analysis_options --test modifier_restrictions \
        --test pipeline --test semantic_gaps --test context -- --test-threads 2 || rc=1
    watchdog_run "engine-rest${TAG}" "$(cap_for_step "$1")" \
      cargo test -p dartq-engine --test apply_tidy --test budget --test discovery \
        --test discovery_excluded --test discovery_root_files --test memory_file_elements \
        --test memory_raw_sites --test merge_memory --test path_universe --test pub_universe \
        --test query_index --test reproducibility --test sdk_faces --test store_gc --test trace -- --test-threads 2 || rc=1
    [ "$leg_peak" -gt "${WATCH_PEAK_KB:-0}" ] && WATCH_PEAK_KB="$leg_peak"
    return "$rc"
    ;;
  foundation-suites)
    # The 12 foundation crates have no dedicated suite step; without this
    # face they execute only inside the instrumented coverage step, where a
    # red costs a full instrumented re-run and reports as a stability
    # mismatch instead of a test failure. One chained invocation — cargo
    # names the failing crate in the log. Threads 2: resource discipline,
    # matching every suite step.
    watchdog_run "$1" "$(cap_for_step "$1")" \
      cargo test -p dartq-source -p dartq-language -p dartq-ast \
        -p dartq-sdk -p dartq-package -p dartq-pub -p dartq-resolution \
        -p dartq-diagnostics -p dartq-types -p dartq-index \
        -p dartq-snapshot -p dartq-ai -- --test-threads 2
;;
  daemon-suite | commands-suite | schema-suite)
    watchdog_run "$1" "$(cap_for_step "$1")" \
      cargo test -p "dartq-${1%-suite}" -- --test-threads 2
    ;;
  plugin-suite)
    # dartq-plugin's integration tests split at the dart-process boundary
    # via #[ignore] annotations: the six dart-spawning files opt out of this
    # default face and are owned by the flag-gated plugin-live step. A new
    # tests/*.rs file is in this face automatically — it must carry
    # #[ignore = "spawns real dart processes; ..."] if it spawns dart.
    watchdog_run "$1" "$(cap_for_step "$1")" \
      cargo test -p dartq-plugin -- --test-threads 2
    ;;
  schema-freeze)
    watchdog_run "$1" "$(cap_for_step "$1")" \
      env DARTQ_SWEEP_SDK="$SDK_ROOT" \
      scripts/schema-freeze-sweep.sh --no-build --label "$LBL_SCHEMA"
    ;;
  daemon-live)
    # Every dartq-daemon #[ignore] wakes here, exactly once (the header's
    # daemon-live entry names them): leg one the rpc_commands live-dispatch
    # test, leg two the five lib ignores. Leg two runs --test-threads 1 on
    # purpose: the idle-flush unit asserts over a PROCESS-GLOBAL flush tally
    # (see the probe module note in src/lib.rs — the crate's own contract is
    # a serial-phase window with no concurrent tests), and the two real-time
    # windows are wall-clock bounded, so nothing may run alongside them.
    # PATH gets the preflight-validated fvm SDK bin first so the live tests'
    # plain `dart` resolves to the same pinned SDK the rest of the window
    # uses. The plugin units need the dartq-plugin-host package pub-getted
    # (its bin/ fixtures are the spawned entrypoints).
    m4_live_skip "$1" && return 0
    local leg_peak rc=0
    watchdog_run "daemon-live-rpc${TAG}" "$(cap_for_step "$1")" \
      env PATH="$SDK_ROOT/bin:$PATH" \
      cargo test -p dartq-daemon --test rpc_commands -- --ignored --test-threads 2 || rc=1
    leg_peak="$WATCH_PEAK_KB"
    watchdog_run "daemon-live-lib${TAG}" "$(cap_for_step "$1")" \
      env PATH="$SDK_ROOT/bin:$PATH" \
      cargo test -p dartq-daemon --lib -- --ignored --test-threads 1 || rc=1
    [ "$leg_peak" -gt "${WATCH_PEAK_KB:-0}" ] && WATCH_PEAK_KB="$leg_peak"
    return "$rc"
    ;;
  plugin-live)
    m4_live_skip "$1" && return 0
    # --ignored wakes exactly the crate's #[ignore]d tests: the five
    # dart-spawning integration files plus compat_live (the #644 compat
    # matrix) — there are no other ignores in dartq-plugin.
    watchdog_run "plugin-live${TAG}" "$(cap_for_step "$1")" \
      cargo test -p dartq-plugin -- --ignored --test-threads 2
    ;;
  host-live)
    m4_live_skip "$1" && return 0
    # bash -c (not --directory): the pub/test cwd form works on every dart
    # 3.x; the SDK comes from the chain's FVM_HOME resolution.
    watchdog_run "host-live-pubget${TAG}" "$(cap_for_step "$1")" \
      bash -c "cd crates/dartq-plugin-host && exec '$DART_BIN' pub get" || return 1
    local leg_peak="$WATCH_PEAK_KB" rc=0
    watchdog_run "host-live${TAG}" "$(cap_for_step "$1")" \
      bash -c "cd crates/dartq-plugin-host && exec '$DART_BIN' test -j 2" || rc=1
    [ "$leg_peak" -gt "${WATCH_PEAK_KB:-0}" ] && WATCH_PEAK_KB="$leg_peak"
    return "$rc"
    ;;
  repro)
    watchdog_run "$1" "$(cap_for_step "$1")" scripts/repro-gate.sh --label "$LBL_REPRO"
    ;;
  coverage)
    local cov_args=(--stability)
    [ "${SERIAL_COVERAGE_HTML:-0}" = 1 ] && cov_args+=(--html)
    watchdog_run "$1" "$(cap_for_step "$1")" scripts/coverage-gate.sh "${cov_args[@]}"
    ;;
  esac
}

# --- run the chain ------------------------------------------------------------

fmt_wall() { # <seconds>
  printf '%dm%02ds' "$(($1 / 60))" "$(($1 % 60))"
}

fmt_mb() { # <kb>
  echo "$(($1 / 1024))MB"
}

# Run identity (runbook Phase 0.3): head + tracked-dirty note. The repro
# gate refuses a dirty tracked tree — surface that BEFORE the hours of work,
# not at the repro step.
echo "== serial-gates plan"
printf '  %-14s %s\n' "head" "$(git rev-parse HEAD)"
tracked_dirty="$(git status --porcelain | grep -cv '^??' || true)"
printf '  %-14s %s tracked file(s) modified%s\n' "worktree" "$tracked_dirty" \
  "$([ "${tracked_dirty:-0}" -gt 0 ] && echo ' — commit tracked waves before the repro step (repro-gate refuses a dirty tree)')"
printf '  %-14s %s KiB (mega-r1: %s KiB)\n' "rss cap" "$RSS_DEFAULT_KB" "$MEGA_CAP_KB"
[ -n "$ONLY_LIST" ] && printf '  %-14s %s\n' "only" "$ONLY_LIST"
[ -n "$FROM_STEP" ] && printf '  %-14s %s\n' "from" "$FROM_STEP"
[ -n "$SKIP_LIST" ] && printf '  %-14s %s\n' "skip" "$SKIP_LIST"
[ -n "$TAG" ] && printf '  %-14s %s\n' "tag" "${TAG#-}"
printf '  %-14s %s\n' "mega" "scale=${SERIAL_MEGA_SCALE:-10k} oracle_leg=${SERIAL_MEGA_ORACLE_LEG:-1}"

preflight

# Per-step records: parallel arrays (bash 3.2 — no associative arrays).
STEP_STATUS=()
STEP_WALL=()
STEP_PEAK=()
STEP_LOG=()
for s in "${STEPS[@]}"; do
  if selected "$s"; then
    STEP_STATUS+=("PENDING")
  else
    STEP_STATUS+=("SKIP")
  fi
  STEP_WALL+=("-")
  STEP_PEAK+=("-")
  STEP_LOG+=("-")
done

failed_step=""
for s in "${STEPS[@]}"; do
  i="$(index_of "$s")"
  [ "${STEP_STATUS[$i]}" = "PENDING" ] || continue
  echo
  echo "== [$s] (cap $(fmt_mb "$(cap_for_step "$s")"))"
  t0=$(date +%s)
  rc=0
  step_runner "$s" || rc=$?
  wall=$(($(date +%s) - t0))
  STEP_WALL[$i]="$(fmt_wall "$wall")"
  STEP_PEAK[$i]="$(fmt_mb "${WATCH_PEAK_KB:-0}")"
  if [ -n "$WATCH_LOG" ]; then STEP_LOG[$i]="$WATCH_LOG"; else STEP_LOG[$i]="-"; fi
  if [ "$rc" -eq 0 ]; then
    STEP_STATUS[$i]="PASS"
    echo "-- [$s] PASS (wall $(fmt_wall "$wall"), peak $(fmt_mb "${WATCH_PEAK_KB:-0}"), log ${STEP_LOG[$i]})"
  else
    # "-" log = a step_fail guard fired before this step started a watchdog
    # run; the log greps must be skipped (a bare "-" file argument would
    # read stdin and hang).
    if [ "${STEP_LOG[$i]}" != "-" ] && grep -q '^WATCHDOG_KILL' "${STEP_LOG[$i]}" 2>/dev/null; then
      STEP_STATUS[$i]="KILLED"
      echo "-- [$s] KILLED by the RSS watchdog (log ${STEP_LOG[$i]})" >&2
    else
      STEP_STATUS[$i]="FAIL(rc=$rc)"
      echo "-- [$s] FAILED (exit $rc, log ${STEP_LOG[$i]})" >&2
      if [ "${STEP_LOG[$i]}" != "-" ]; then
        tail -20 "${STEP_LOG[$i]}" 2>/dev/null | sed 's/^/  | /' >&2 || true
      fi
    fi
    # Preserve the failing run's log: /tmp is ephemeral and the next rerun
    # overwrites the same path, so the failure evidence lands next to the
    # recorded gate evidence (target/evidence is gitignored).
    if [ -f "${STEP_LOG[$i]}" ]; then
      archive_dir="target/evidence/serial/$s${TAG}"
      if mkdir -p "$archive_dir" 2>/dev/null && cp "${STEP_LOG[$i]}" "$archive_dir/" 2>/dev/null; then
        echo "-- [$s] log archived to $archive_dir/$(basename "${STEP_LOG[$i]}")" >&2
        STEP_LOG[$i]="${STEP_LOG[$i]} -> $archive_dir/"
      fi
    fi
    failed_step="$s"
    break
  fi
done

# Steps after an abort never ran.
if [ -n "$failed_step" ]; then
  aborted=0
  for idx in "${!STEPS[@]}"; do
    if [ "${STEPS[$idx]}" = "$failed_step" ]; then
      aborted=1
      continue
    fi
    if [ "$aborted" = 1 ] && [ "${STEP_STATUS[$idx]}" = "PENDING" ]; then
      STEP_STATUS[$idx]="ABORTED"
    fi
  done
fi

echo
echo "== serial-gates summary"
printf '%-19s %-11s %-8s %-9s %s\n' "step" "status" "wall" "peak-RSS" "log"
for idx in "${!STEPS[@]}"; do
  case "${STEP_STATUS[$idx]}" in
  PASS | FAIL* | KILLED) log="${STEP_LOG[$idx]}" ;;
  *) log="-" ;;
  esac
  printf '%-19s %-11s %-8s %-9s %s\n' \
    "${STEPS[$idx]}" "${STEP_STATUS[$idx]}" "${STEP_WALL[$idx]}" "${STEP_PEAK[$idx]}" "$log"
done

if [ -n "$failed_step" ]; then
  echo "serial-gates: CHAIN FAILED at $failed_step (fix, then resume with --from / --only)"
  exit 1
fi
echo "serial-gates: CHAIN PASSED (every selected step green)"
