#!/usr/bin/env bash
# Release-readiness gate: calibration pin tests + per-crate rustfmt + clippy
# + rustdoc over the workspace.
#
# Walks the workspace members declared in the root Cargo.toml (skipping
# fixture/test-only members), one crate at a time, serially:
#
#   cargo fmt   -p <crate> -- --check
#   cargo clippy -p <crate> --offline --all-targets -- -D warnings
#
# then runs the oracle-free calibration pins (#937): `cargo test -p harness`
# and `cargo test -p dartq-semantic --test m3_codes_pin` — seconds-fast,
# catch frozen-universe drift before any differential work. The m3 pin
# asserts set equality between tests/harness/m3_unused_codes.txt (already
# re-frozen: 11 rows) and the 11-variant USAGE_FAMILY in m3_codes_pin.rs —
# verification only, green on the committed freeze. A red there is a real
# drift, never a designed one: add the targeted fixture it demands first,
# then `cargo test -p dartq-semantic --test m3_codes_pin
# regen_m3_unused_codes -- --ignored` and reconcile the header prose —
# never regen blind. The harness pins include the freeze --check family
# (#2681): tests/harness/src/freeze.rs re-derives the m1/m2_warning/m3
# codes freezes byte-for-byte from repo-internal sources
# (analyzer-codes.tsv, the m3 targeted fixtures), and the fixture-coverage
# manifest check below its tier table asserts byte equality against the
# same canonical builder its regen writes — a stale freeze fails the gate
# naming its regen command.
#
# `--check-only` runs a fast `cargo check -p <crate>` instead (no pins: the
# mode compiles only). The script
# exits nonzero at the first failure and names the crate. Compile-job
# concurrency comes from the workspace config (.cargo/config.toml pins
# jobs = 2); no -j is passed anywhere.
#
# `--coverage` appends the llvm-cov line-coverage gate after the
# calibration pins: scripts/coverage-gate.sh --enforce, scored against the
# committed per-crate floors (scripts/coverage-floors.tsv). SERIAL PHASE
# ONLY: the flag rebuilds the whole workspace with coverage instrumentation
# and runs the full test suite, so a --coverage invocation IS the sanctioned
# serial window's coverage face — never alongside development-phase lanes
# (the 80GB/90GB memory-incident discipline). Advisory runs and the reproducibility
# two-run stability protocol remain direct calls (scripts/coverage-gate.sh,
# scripts/coverage-run.sh). Mutually exclusive with --check-only.
#
# The env var COVERAGE is the flag-free opt-in (the flag wins when both are
# given): COVERAGE=1|true|full selects the serial-window face above;
# COVERAGE=smoke runs the cheap advisory subset pre-flight
# (scripts/coverage-gate.sh --smoke: the three fastest pure-logic crates
# only, distinct target/coverage outputs, never a floors baseline). Any
# other value is a usage error, never a silent skip. Unset: no coverage
# face.
# Doc gate (#1169): after the faces above, `cargo doc -p <crate> --no-deps`
# runs one crate at a time (same member skip rule) with
# RUSTDOCFLAGS="-D warnings": every rustdoc warning — broken intra-doc
# links included, a class check/clippy cannot see (the #1166 sweep found
# stale links that escaped every existing face) — fails the gate naming
# the crate. Rustdoc honors the workspace jobs = 2 pin via
#.cargo/config.toml; no -j is passed. From a warm target dir the face
# costs a few seconds per crate (~1-2 min over the 24 members). Gate mode
# only: --check-only stays the compile-only probe.
#
# Usage: scripts/ci.sh [--check-only] [--coverage]

set -u -o pipefail

cd "$(dirname "$0")/.." || exit 2

mode="gate"
coverage=0
for arg in "$@"; do
    case "$arg" in
    --check-only) mode="check" ;;
    --coverage) coverage=1 ;;
    *)
        echo "usage: scripts/ci.sh [--check-only] [--coverage]" >&2
        exit 2
        ;;
    esac
done
# COVERAGE env opt-in (see the header): flag --coverage already set wins;
# 1/true/full = the full serial-window face, smoke = the advisory subset
# pre-flight, anything else = hard usage error. Applied BEFORE the
# mutual-exclusion guard so --check-only rejects every coverage face.
if [ "$coverage" = 0 ] && [ -n "${COVERAGE:-}" ]; then
    case "$COVERAGE" in
    1 | true | full) coverage=1 ;;
    smoke) coverage=smoke ;;
    *)
        echo "ci: unsupported COVERAGE value '$COVERAGE' (use 1|true|full|smoke)" >&2
        exit 2
        ;;
    esac
fi
if [ "$mode" = "check" ] && [ "$coverage" != 0 ]; then
    echo "ci: --check-only and --coverage are mutually exclusive (compile-only probe vs the serial-phase instrumented suite)" >&2
    exit 2
fi

# Member paths from the explicit member list (crates/* and tests/harness).
members=$(sed -n '/^members = \[/,/^\]/p' Cargo.toml | grep -o '"[^"]*"' | tr -d '"')
if [ -z "$members" ]; then
    echo "ci: no workspace members parsed from Cargo.toml" >&2
    exit 2
fi

for member in $members; do
    case "$member" in tests/*) continue ;; esac # fixture/test-only members
    crate=${member##*/}

    if [ "$mode" = "check" ]; then
        echo "== cargo check: $crate"
        if ! cargo check -q -p "$crate"; then
            echo "ci: cargo check FAILED: $crate" >&2
            exit 1
        fi
    else
        echo "== cargo fmt: $crate"
        if ! cargo fmt -p "$crate" -- --check; then
            echo "ci: cargo fmt --check FAILED: $crate" >&2
            exit 1
        fi
        echo "== cargo clippy: $crate"
        if ! cargo clippy -q -p "$crate" --offline --all-targets -- -D warnings; then
            echo "ci: cargo clippy FAILED: $crate" >&2
            exit 1
        fi
    fi
done

# Calibration pins (#937): oracle-free, seconds-fast, required — they fail
# the gate naming the drifted freeze before any differential work starts.
# Gate mode only: --check-only is the fast compile-only probe.
if [ "$mode" = "gate" ]; then
    echo "== cargo test pins: harness"
    if ! cargo test -q -p harness; then
        echo "ci: cargo test pins FAILED: harness" >&2
        exit 1
    fi
    echo "== cargo test pins: dartq-semantic m3_codes_pin"
    if ! cargo test -q -p dartq-semantic --test m3_codes_pin; then
        echo "ci: cargo test pins FAILED: dartq-semantic (m3_codes_pin)" >&2
        exit 1
    fi
    # Registry drift pin (#2677): the tracked analyzer-codes.tsv must be
    # exactly what tools/upstream-gen derives from the SDK tree of record.
    # Env-gated like the repo's other SDK-tree steps — the checkout is a
    # local read-only tree tracked code never references.
    if [ -n "${DART_SDK_SRC:-}" ]; then
        echo "== analyzer-codes-gen --check (DART_SDK_SRC set)"
        if ! cargo run -q --offline -p upstream-gen --bin analyzer-codes-gen -- --check --sdk-src "$DART_SDK_SRC"; then
            echo "ci: analyzer-codes-gen --check FAILED (stale data/analyzer-codes.tsv)" >&2
            exit 1
        fi
    else
        echo "== analyzer-codes-gen --check: skipped (DART_SDK_SRC not set)"
    fi
    # Lint metadata drift pin (#2676): the tracked rule_meta.rs and
    # lint_docs_table.rs must be exactly what tools/upstream-gen derives
    # from the SDK tree plus the pinned package:lints snapshot (both local
    # read-only trees). Gated on both env vars — either missing skips.
    if [ -n "${DART_SDK_SRC:-}" ] && [ -n "${DART_LINTS_SRC:-}" ]; then
        echo "== linter-meta-gen --check (DART_SDK_SRC and DART_LINTS_SRC set)"
        if ! cargo run -q --offline -p upstream-gen --bin linter-meta-gen -- --check --sdk-src "$DART_SDK_SRC" --lints-src "$DART_LINTS_SRC"; then
            echo "ci: linter-meta-gen --check FAILED (stale generated rule metadata)" >&2
            exit 1
        fi
    else
        echo "== linter-meta-gen --check: skipped (DART_SDK_SRC or DART_LINTS_SRC not set)"
    fi
    # Harness freeze --check (#2681): the gate-universe codes freezes
    # must equal their repo-internal derivations byte-for-byte (the
    # analyzer-codes.tsv mirror for m1/m2_warning, the targeted fixtures
    # for m3) — no env gating. These also run inside the harness pins
    # above; this named step pins the drift face next to the generator
    # checks. The fixture-coverage manifest byte check rides the harness
    # pins step (fixture_coverage_manifest_matches_corpus_derivation).
    echo "== harness freeze --check (m1/m2_warning/m3 codes freezes)"
    if ! cargo test -q -p harness freeze_check; then
        echo "ci: harness freeze --check FAILED (stale gate-universe freeze; the failing test prints its regen command)" >&2
        exit 1
    fi
fi

# e2e goldens (#2631, gate mode): the routed-vs-cold parity leg over the
# release binary — analyze routed (default) == `--cold` byte for byte,
# target filters included, a killed daemon between calls included. The
# leg list stays selectable (tests/e2e/run.sh [legs...]); CI runs the
# golden leg alone — the full e2e suite remains a serial-phase face.
if [ "$mode" = "gate" ]; then
    echo "== e2e: routed-parity leg (release build)"
    if ! cargo build -q --release -p dartq-cli; then
        echo "ci: release build FAILED (e2e leg)" >&2
        exit 1
    fi
    if ! bash tests/e2e/run.sh 11; then
        echo "ci: e2e routed-parity leg FAILED" >&2
        exit 1
    fi
fi

# Coverage gate (--coverage / COVERAGE): the llvm-cov line-coverage floor
# gate. Serial-phase-only — see the header.
if [ "$coverage" != 0 ]; then
    if [ "$coverage" = "smoke" ]; then
        echo "== coverage gate: scripts/coverage-gate.sh --smoke (advisory subset pre-flight)"
        if ! bash scripts/coverage-gate.sh --smoke; then
            echo "ci: coverage gate FAILED" >&2
            exit 1
        fi
    else
        echo "== coverage gate: scripts/coverage-gate.sh --enforce"
        if ! bash scripts/coverage-gate.sh --enforce; then
            echo "ci: coverage gate FAILED" >&2
            exit 1
        fi
    fi
fi

# Remaining heavyweight phases (differential gates, test suites beyond the
# calibration pins above) are NOT run
# here: they execute real processes over the whole workspace and are reserved
# for the serial verification phase (the resource discipline after the 80GB/90GB memory incidents):
#   scripts/m1-gate.sh         — M1 differential gate (serial phase only)
#   scripts/acceptance-gate.sh — valid-syntax acceptance gate (serial phase only)
#   scripts/repro-gate.sh      — reproducibility gate (serial phase only)
#   scripts/serial-gates.sh    — the full serial chain, coverage included
# The coverage face is the one exception: --coverage (or COVERAGE=1) opts
# this invocation in as the serial window's coverage step (the gate above);
# COVERAGE=smoke opts in only the advisory subset pre-flight.

# Doc gate (#1169): rustdoc per member with --no-deps, one crate at a time
# (same skip rule as the faces above — fixture/test members excluded).
# RUSTDOCFLAGS="-D warnings" promotes every rustdoc warning to a hard
# error, structurally catching the broken-intra-doc-link rot class that
# check/clippy resolution cannot see (#1166). Gate mode only: --check-only
# stays the compile-only probe.
if [ "$mode" = "gate" ]; then
    for member in $members; do
        case "$member" in tests/*) continue;; esac
        crate=${member##*/}
        echo "== cargo doc: $crate"
        if ! RUSTDOCFLAGS="-D warnings" cargo doc -q -p "$crate" --no-deps --offline; then
            echo "ci: cargo doc FAILED: $crate" >&2
            exit 1
        fi
    done
fi

# Untracked-reference gate: no tracked file — and no untracked candidate on
# its way into the repo — may reference never-tracked local workspace
# material (vendored checkouts, evidence/ledger paths, home paths).
bash "$(dirname "$0")/untracked-ref-gate.sh"
echo "ci: gate passed"
