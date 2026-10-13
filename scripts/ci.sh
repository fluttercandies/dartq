#!/usr/bin/env bash
# Release-readiness gate: calibration pin tests + per-crate rustfmt + clippy
# over the workspace.
#
# Walks the workspace members declared in the root Cargo.toml (skipping
# fixture/test-only members), one crate at a time, serially:
#
#   cargo fmt   -p <crate> -- --check
#   cargo clippy -p <crate> --offline --all-targets -- -D warnings
#
# then runs the oracle-free calibration pins (#937): `cargo test -p harness`
# and `cargo test -p dartq-semantic --test m3_codes_pin` — seconds-fast,
# catch frozen-universe drift before any differential work. The m3 pin is
# RED by design until the serial window runs the m3 re-freeze regen (the
# freeze file is at 10 rows, the registry family at 11); ci.sh going red
# there today is the calibration protection working, not a bug.
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
# (the 80GB/90GB memory-incident discipline). Advisory runs and the §19.5
# two-run stability protocol remain direct calls (scripts/coverage-gate.sh,
# scripts/coverage-run.sh). Mutually exclusive with --check-only.
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
if [ "$mode" = "check" ] && [ "$coverage" = 1 ]; then
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
fi

# Coverage gate (--coverage): the llvm-cov line-coverage floor gate over
# the full instrumented suite. Serial-phase-only — see the header.
if [ "$coverage" = 1 ]; then
    echo "== coverage gate: scripts/coverage-gate.sh --enforce"
    if ! bash scripts/coverage-gate.sh --enforce; then
        echo "ci: coverage gate FAILED" >&2
        exit 1
    fi
fi

# Remaining heavyweight phases (differential gates, test suites beyond the
# calibration pins above) are NOT run
# here: they execute real processes over the whole workspace and are reserved
# for the serial verification phase (the resource discipline after the 80GB/90GB memory incidents):
#   scripts/m1-gate.sh         — M1 differential gate (serial phase only)
#   scripts/acceptance-gate.sh — valid-syntax acceptance gate (serial phase only)
#   scripts/repro-gate.sh      — §19.5 reproducibility gate (serial phase only)
#   scripts/serial-gates.sh    — the full serial chain, coverage included
# The coverage face is the one exception: --coverage opts this invocation in
# as the serial window's coverage step (the gate above).

# Untracked-reference gate: no tracked file may reference never-tracked local
# workspace material (vendored checkouts, evidence/ledger paths, home paths).
bash "$(dirname "$0")/untracked-ref-gate.sh"
echo "ci: gate passed"
