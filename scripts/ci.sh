#!/usr/bin/env bash
# Release-readiness gate: per-crate rustfmt + clippy over the workspace.
#
# Walks the workspace members declared in the root Cargo.toml (skipping
# fixture/test-only members), one crate at a time, serially:
#
#   cargo fmt   -p <crate> -- --check
#   cargo clippy -p <crate> --offline --all-targets -- -D warnings
#
# `--check-only` runs a fast `cargo check -p <crate>` instead. The script
# exits nonzero at the first failure and names the crate. Compile-job
# concurrency comes from the workspace config (.cargo/config.toml pins
# jobs = 2); no -j is passed anywhere.
#
# Usage: scripts/ci.sh [--check-only]

set -u -o pipefail

cd "$(dirname "$0")/.." || exit 2

mode="gate"
case "${1:-}" in
"") ;;
--check-only) mode="check" ;;
*)
    echo "usage: scripts/ci.sh [--check-only]" >&2
    exit 2
    ;;
esac

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

# Heavyweight phases (test suites, coverage, differential gates) are NOT run
# here: they execute real processes over the whole workspace and are reserved
# for the serial verification phase (AGENTS.md resource discipline):
#   scripts/coverage-gate.sh — llvm-cov coverage gate (serial phase only)
#   scripts/m1-gate.sh       — M1 differential gate (serial phase only)
#   scripts/repro-gate.sh    — §19.5 reproducibility gate (serial phase only)
echo "ci: gate passed"
