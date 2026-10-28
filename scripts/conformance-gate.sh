#!/bin/sh
# The conformance release gate: registries match the pin, the extracted
# corpus matches the pin, dartq matches the oracle expectations on the
# whole corpus, and every locked entry stays green with zero crashes.
#
# Environment:
#   DARTQ_SDK_SRC    (required) the pinned Dart SDK source tree
#   DART_LINTS_SRC   (optional) the pinned package:lints tree, needed by
#                    linter-meta-gen
#   DARTQ_BIN        (optional) the dartq binary, default target/release/dartq
#   DARTQ_ORACLE_SDK (optional) a released Dart SDK root passed to dartq as
#                    its --sdk-path; without it dartq resolves $DART_SDK
#   DARTQ_PERF       (optional) set to 1 to append the performance
#                    threshold face (scripts/perf-gate.sh over
#                    conformance/perf-thresholds.toml). Off by default:
#                    perf numbers need a quiet machine, and a loaded host
#                    trips the wall tripwires on scheduling noise. When
#                    opted in, the face's verdict is honored — a perf
#                    failure fails the run — except exit 77 (nothing
#                    measured, inputs absent), which is reported and
#                    tolerated: a skip is explicit, never a silent pass.
#   DARTQ_REALWORLD  (optional) set to 1 to append the real-world face
#                    (gate 4 over the pub-cache packages in
#                    conformance/realworld.toml, comparing dartq against
#                    `dart analyze` on the packages themselves). Requires
#                    DARTQ_ORACLE_SDK (the released SDK providing the
#                    oracle dart and dartq's --sdk-path). Packages absent
#                    from the local pub cache are reported SKIP
#                    explicitly; a baseline regression fails the run and
#                    exit 77 (nothing measured) is reported and tolerated
#                    like the perf face.

set -eu

if [ -z "${DARTQ_SDK_SRC:-}" ]; then
    echo "error: set DARTQ_SDK_SRC to the pinned Dart SDK source tree" >&2
    exit 2
fi
DARTQ_BIN="${DARTQ_BIN:-target/release/dartq}"

echo "== gate 1/4: generated registries match the pin"
cargo run -q -p upstream-gen --bin analyzer-codes-gen -- --check
cargo run -q -p upstream-gen --bin linter-meta-gen -- --check

echo "== gate 2/4: case corpus matches the pin"
cargo run -q -p conformance -- extract --check --sdk-src "$DARTQ_SDK_SRC"

echo "== gate 3/4: dartq vs oracle over the corpus"
if [ -n "${DARTQ_ORACLE_SDK:-}" ]; then
    cargo run -q -p conformance -- run --dartq "$DARTQ_BIN" --sdk "$DARTQ_ORACLE_SDK"
else
    cargo run -q -p conformance -- run --dartq "$DARTQ_BIN"
fi

echo "== gate 4/4: locked entries green, no crashes"
cargo run -q -p conformance -- gate

# Optional face (opt-in via DARTQ_PERF=1): the performance threshold gates.
# Never part of the default run — see the DARTQ_PERF note in the header.
if [ "${DARTQ_PERF:-}" = "1" ]; then
    echo "== optional face: performance threshold gates (DARTQ_PERF=1)"
    perf_rc=0
    bash scripts/perf-gate.sh || perf_rc=$?
    if [ "$perf_rc" -eq 77 ]; then
        echo "perf face: nothing measured (inputs absent) — explicit skip, see the perf-gate output above"
    elif [ "$perf_rc" -ne 0 ]; then
        echo "conformance gate: perf face FAILED (rc=$perf_rc)" >&2
        exit "$perf_rc"
    fi
fi

# Optional face (opt-in via DARTQ_REALWORLD=1): the real-world pub-cache
# packages against the oracle. Never part of the default run — see the
# DARTQ_REALWORLD note in the header.
if [ "${DARTQ_REALWORLD:-}" = "1" ]; then
    echo "== optional face: real-world pub-cache packages vs the oracle (DARTQ_REALWORLD=1)"
    if [ -z "${DARTQ_ORACLE_SDK:-}" ]; then
        echo "error: DARTQ_REALWORLD=1 needs DARTQ_ORACLE_SDK (the released SDK that provides the oracle dart and dartq's --sdk-path)" >&2
        exit 2
    fi
    rw_rc=0
    cargo run -q -p conformance -- realworld --dartq "$DARTQ_BIN" --oracle-sdk "$DARTQ_ORACLE_SDK" || rw_rc=$?
    if [ "$rw_rc" -eq 77 ]; then
        echo "realworld face: nothing measured (pub-cache packages absent) — explicit skip, see the output above"
    elif [ "$rw_rc" -ne 0 ]; then
        echo "conformance gate: realworld face FAILED (rc=$rw_rc)" >&2
        exit "$rw_rc"
    fi
fi

echo "conformance gate: PASS"
