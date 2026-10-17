#!/usr/bin/env bash
# release.sh — the one-shot release harness.
#
# Orchestrates the release gate set (the adjudicated full-parity bars and
# release rulings):
#   M0/M1 differential at full parity (bar 1.0 per the 2026-10-12 ADR,
#   the full-parity bars ADR, which superseded the earlier
#   0.99/0.95 numbers), calibration pins green, lints/m2/m3 green,
#   ci.sh green,
#   repro-gate sealed, bench five-axis evidence persisted, the release
#   version's CHANGELOG section present, legacy-name gate clean, clean
#   tree -> annotated tag ->
#   push. Tag + binary distribution; crates.io publish deferred (D1).
#
# Phases (each a function; run one with --phase NAME or a bare NAME; the
# default with no arguments runs preflight only; --all runs every phase in
# order and never reaches the tag phase if any earlier phase failed):
#
#   preflight  clean tracked tree (refuses otherwise), HEAD on main,
#              legacy-name gate (tracked content must not mention the old
#              product name), Cargo.lock consistency probe, required tools
#              (toolchain pin match, sha256 tool, jq, gh). Every
#              miss exits 2 with a remediation hint. Read-only: --dry-run
#              is accepted for symmetry and changes nothing.
#   build      release build of the cli + harness binaries, then a plain
#              debug-tree refresh (cargo build --workspace) so the gate
#              chain runs against warm, current binaries (the stale
#              debug-binary problem). The workspace pins no rustc-wrapper;
#              a locally enabled sccache (RUSTC_WRAPPER) stays transparent.
#              --dry-run prints the exact
#              commands and executes nothing.
#   verify     the gate chain in order, one subprocess at a time, each with
#              its own RSS watchdog slot (and a timeout backstop when
#              timeout(1) exists) and full exit-code propagation:
#                calibration pins (harness unit pins + m3_codes_pin)
#                -> serial-window evidence assertion (SERIAL_TAG; no
#                   process — asserts the scripts/serial-gates.sh window's
#                   evidence directories exist and postdate HEAD's commit)
#                -> m0-gate -> m1-gate -> lint-gate -> m2-gate -> m3-gate
#                -> M4 suites (daemon/plugin/commands/schema) + the
#                schema-freeze sweep (+ the DARTQ_M4_LIVE live faces, opt-in)
#                -> ci.sh (fmt + clippy + calibration pins + rustdoc doc
#                   gate + untracked-ref gate) -> repro-gate
#              One PASS/FAIL line per gate; on FAIL the harness stops and
#              names the gate and its evidence directory. Before repro-gate
#              the tracked tree is re-checked: m0/m1 persist regression
#              cases into tests/corpus/regressions/cases, and repro-gate
#              refuses a dirty tracked tree — commit those outputs first,
#              then re-run verify. --dry-run forwards --dry-run to every
#              gate (all support it; ci.sh has no dry mode and is reported
#              as SKIP).
#   bench      five-axis evidence plan per the bench-readiness ledger.
#              Default: print the plan only. --run-bench executes the two
#              named runs (mega-r1 oracle leg incl. the A7 cold pair — the
#              release-face DARTQ_CMD + --strict shape of serial-gates.sh
#              (#1145), and the resident incremental_p95 engine bench)
#              under the watchdog; the m0 harness-bench invocation stays a
#              printed step (its --out label is a release-owner call per
#              the readiness doc). The G2 tokenizer caveat is printed in
#              both modes.
#   bundle     dist/dartq-v<version>-<target-triple>/ with the release
#              binaries (dartq, harness), README.md, CHANGELOG.md,
#              LICENSE-APACHE, LICENSE-MIT and a SHA256SUMS file. CHANGELOG
#              must already contain a "<version>" section — checked, never
#              created. Refuses to overwrite an existing bundle dir.
#              --dry-run prints the layout without creating anything.
#   tag        default: print the exact git tag + push commands (dry-run).
#              --execute performs them, but only after a full preflight
#              re-check (clean tracked tree, on main, tag absent, remote
#              present, bundle dir exists, CHANGELOG section present). In
#              --all mode the tag phase only runs when every prior phase
#              exited 0; standalone --execute additionally relies on the
#              operator having a green verify phase — the script says so.
#
# Resource discipline: build/verify/executing-bench phases print the
# RSS-watchdog reminder and run their children strictly one at a time. When
# /tmp/rss_cmd.sh exists it wraps every child (caps: gates 8 GiB, harness
# bench 4 GiB, mega 12 GiB, builds 8 GiB) and the exit code is judged from
# the log's exit= line (the wrapper does not propagate kills); without it
# the harness degrades gracefully with a warning.
#
# Labels: the gate scripts refuse to overwrite recorded evidence and a FAIL
# still burns a label. m0 gets --label "${M0_LABEL:-m0-r6}"; m1/lints/m2 use
# their own defaults (m1-r4 / lints-r1 / m2-r1) and env overrides; m3 is run
# with M3_LABEL="${M3_LABEL:-m3-r1}"; repro-gate uses its default label.
# Override any of them via the gate's own env for reruns (-r2 convention).
# SERIAL_TAG (verify, required): the tag the serial window ran under — the
# serial-window evidence assertion composes the window's label directories
# from it (serial-gates.sh's "<default>-$SERIAL_TAG" rule); unset is a
# usage error (exit 2) outside --dry-run.
#
# Usage:
#   scripts/release.sh                            # preflight only
#   scripts/release.sh [--phase NAME|NAME] [--all]
#                      [--dry-run] [--run-bench] [--execute] [--help]
#   SERIAL_TAG=<tag> scripts/release.sh --phase verify   # verify needs the
#                      serial window's tag (see Labels above)
#
#   --phase NAME   one of: preflight build verify bench bundle tag
#   --all          full chain: preflight -> build -> verify -> bench ->
#                  bundle -> tag (stop at the first failing phase)
#   --dry-run      per-phase: print the exact mutations, execute nothing
#                  (preflight is read-only either way)
#   --run-bench    bench phase: actually run (otherwise plan only)
#   --execute      tag phase: perform the tag + push instead of printing
#   Exit codes: 0 requested phases passed; 1 a gate/phase failed its bar;
#   2 usage or environment error (with a remediation hint).
#
# SERIAL PHASE ONLY for build/verify/run-bench: they spawn real cargo, dart
# and dartq processes. Never run them alongside development lanes.
set -u -o pipefail
export LC_ALL=C

cd "$(dirname "$0")/.." || exit 2

RSS_WRAPPER="/tmp/rss_cmd.sh"
TIMEOUT_BIN="$(command -v timeout 2>/dev/null || true)"
# The fvm-pinned SDK, resolved the serial-gates.sh way (same names): the
# bench mega oracle leg pins a release-face DARTQ_CMD against it.
FVM_HOME="${FVM_HOME:-$HOME/fvm}"
SDK_ROOT="$FVM_HOME/default/bin/cache/dart-sdk"

# Assembled from pieces on purpose: this file is tracked content and the
# preflight naming gate greps tracked content for the old product name —
# writing that name literally here would trip the gate on itself.
OLD_NAME='d''artiq'

die() { echo "release: $*" >&2; exit 2; }
warn() { echo "release: WARN $*" >&2; }

sha_cmd() {
    if command -v sha256sum >/dev/null 2>&1; then
        echo "sha256sum"
    elif command -v shasum >/dev/null 2>&1; then
        echo "shasum -a 256"
    else
        echo ""
    fi
}

workspace_version() {
    sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1
}

# run_guarded NAME CAP_KIB TIMEOUT_SECS CMD... — run one child, one at a
# time, under the RSS watchdog when present; TIMEOUT_SECS empty disables
# the timeout backstop. Returns the child's exit code (judged from the
# watchdog log — the wrapper does not propagate kills).
run_guarded() {
    name=$1; cap=$2; tmo=$3; shift 3
    if [ -x "$RSS_WRAPPER" ]; then
        # rm -f first: the exit code below is judged from the log's exit=
        # line, and a stale log from a previous run (the wrapper dying before
        # writing) would yield a false PASS (#931 P2). Here, so no caller can
        # forget it.
        rm -f "/tmp/serial_${name}.log"
        if [ -n "$tmo" ] && [ -n "$TIMEOUT_BIN" ]; then
            "$RSS_WRAPPER" "$name" "$cap" "$TIMEOUT_BIN" "$tmo" "$@"
        else
            "$RSS_WRAPPER" "$name" "$cap" "$@"
        fi
        log="/tmp/serial_${name}.log"
        if grep -q "WATCHDOG_KILL" "$log" 2>/dev/null; then
            echo "release: watchdog killed '$name' (RSS cap ${cap} KiB) — log: $log" >&2
            return 137
        fi
        ec="$(sed -n 's/^exit=//p' "$log" 2>/dev/null | tail -n 1)"
        [ -n "$ec" ] || { echo "release: no exit= line in $log for '$name'" >&2; return 2; }
        return "$ec"
    fi
    warn "$RSS_WRAPPER missing — '$name' runs unwatched (restore it from the serial-phase runbook)"
    if [ -n "$tmo" ] && [ -n "$TIMEOUT_BIN" ]; then
        "$TIMEOUT_BIN" "$tmo" "$@"
    else
        "$@"
    fi
}

# run_gate NAME TIMEOUT_SECS CAP_KIB EVIDENCE_DESC CMD... — run one gate,
# print the one-line verdict, stop the chain on FAIL.
run_gate() {
    name=$1; tmo=$2; cap=$3; evidence=$4; shift 4
    echo "verify: running $name ..."
    if run_guarded "$name" "$cap" "$tmo" "$@"; then
        echo "verify: PASS $name"
        return 0
    else
        # Captured inside else: after fi, $? is the if compound's status, not the condition's.
        ec=$?
        echo "verify: FAIL $name (exit $ec) — evidence: $evidence" >&2
        log="/tmp/serial_${name}.log"
        if [ -f "$log" ]; then
            echo "verify: last log lines ($log):" >&2
            tail -n 15 "$log" >&2
        fi
        return 1
    fi
}

# --- preflight checks --------------------------------------------------------

check_clean_tree() {
    dirt="$(git status --porcelain --untracked-files=no)"
    if [ -n "$dirt" ]; then
        echo "release: tracked working tree is dirty — a release tag must name committed content only (repro-gate enforces the same guard). Commit or stash the in-flight work:" >&2
        printf '%s\n' "$dirt" | head -n 12 | sed 's/^/  /' >&2
        total="$(printf '%s\n' "$dirt" | wc -l | tr -d ' ')"
        [ "$total" -gt 12 ] && echo "  ... (+$((total - 12)) more)" >&2
        exit 2
    fi
    # Untracked files outside the policy-exempt set also count as in-flight
    # work: a fresh checkout of the tag would not contain them. The exempt
    # set is the workspace-policy list that must never enter Git (the
# vendored checkouts, local evidence, the local ledger, the charter files) — it can never be "cleaned",
    # so refusing on it would make every release impossible.
    leftovers="$(git status --porcelain | grep '^??' | cut -c4- | grep -v -E '^([.](third|goal)/|[d]ocs/|AGENTS[.]md|[P]RD[.]md|REF[.]md|GOALS(_ARCHIVE)?[.]md)/?$' || true)"
    if [ -n "$leftovers" ]; then
        echo "release: untracked files outside the release-exempt set (vendored checkouts, local evidence, the local ledger, the local charter files) — commit them or add them to .gitignore before releasing:" >&2
        printf '%s\n' "$leftovers" | sed 's/^/  /' >&2
        exit 2
    fi
}

check_branch() {
    branch="$(git branch --show-current)"
    [ "$branch" = "main" ] || die "HEAD is on '$branch', not main — the release tag is cut from main (D3/D5). Switch to main and re-run."
}

check_naming() {
    matches="$(git grep -I -i -n -e "$OLD_NAME" -- . 2>/dev/null || true)"
    if [ -n "$matches" ]; then
        echo "release: naming gate failed — the old product name appears in tracked content (the product name is dartq; sweep it to zero):" >&2
        printf '%s\n' "$matches" | head -n 10 | sed 's/^/  /' >&2
        exit 2
    fi
}

check_lock() {
    command -v cargo >/dev/null 2>&1 || die "cargo not found — install Rust via rustup (https://rustup.rs)"
    if ! cargo metadata --locked --offline --format-version 1 >/dev/null 2>&1; then
        die "Cargo.lock is inconsistent with the workspace manifests (cargo metadata --locked failed) — refresh and commit it (any cargo command regenerates it; e.g. cargo metadata) before releasing."
    fi
}

check_tools() {
    pin="$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml | head -n 1)"
    [ -n "$pin" ] || die "cannot read the toolchain channel from rust-toolchain.toml — the pin is load-bearing for release binaries."
    rustc_ver="$(rustc -vV 2>/dev/null | sed -n 's/^release: //p')"
    [ "$rustc_ver" = "$pin" ] || die "rustc $rustc_ver != pinned $pin (rust-toolchain.toml). With rustup installed the pin resolves automatically (rustup show); otherwise install $pin."
    command -v rustup >/dev/null 2>&1 || warn "rustup not found — the pin was verified by direct rustc version match instead."
    [ -n "$(sha_cmd)" ] || die "no sha256 tool found — need sha256sum or shasum (macOS ships shasum at /usr/bin/shasum)."
    command -v jq >/dev/null 2>&1 || die "jq is required by every differential gate (brew install jq)"
    command -v gh >/dev/null 2>&1 || die "gh is required for the tag-push probe (brew install gh, then gh auth login)"
    [ -n "$TIMEOUT_BIN" ] || warn "timeout(1) not found — gate timeout backstops are disabled (the RSS watchdog still applies when present)"
    [ -x "$RSS_WRAPPER" ] || warn "$RSS_WRAPPER missing — heavy phases will run unwatched (restore it from the serial-phase runbook before the serial phase)"
}

phase_preflight() {
    echo "release: [preflight] clean tracked tree"
    check_clean_tree
    echo "release: [preflight] HEAD on main"
    check_branch
    echo "release: [preflight] legacy-name gate"
    check_naming
    echo "release: [preflight] Cargo.lock consistency"
    check_lock
    echo "release: [preflight] required tools"
    check_tools
    echo "release: PASS preflight"
}

# --- build -------------------------------------------------------------------

harness_pkg() {
    pkg="$(sed -n 's/^name = "\(.*\)"/\1/p' tests/harness/Cargo.toml | head -n 1)"
    [ -n "$pkg" ] || die "cannot read the harness package name from tests/harness/Cargo.toml"
    echo "$pkg"
}

phase_build() {
    hp="$(harness_pkg)"
    rel_cmd=(cargo build --release -p dartq-cli -p "$hp")
    dbg_cmd=(cargo build --workspace)
    echo "release: [build] RSS-watchdog reminder: serial phase — one cargo lane at a time (jobs=2 pinned via .cargo/config.toml; sccache stays on locally)"
    if [ "$DRY_RUN" = 1 ]; then
        echo "release: [build] would run: ${rel_cmd[*]}"
        echo "release: [build] would run: ${dbg_cmd[*]}   # debug-tree refresh for the gate chain"
        echo "release: SKIP build (dry-run)"
        return 0
    fi
    echo "release: [build] release profile: dartq + $hp"
    run_guarded build-release 8388608 "" "${rel_cmd[@]}" || {
        echo "release: FAIL build (release profile) — see /tmp/serial_build-release.log" >&2
        return 1
    }
    echo "release: [build] debug-tree refresh (the stale debug-binary fix: the gates run ./target/debug/*)"
    run_guarded build-debug 8388608 "" "${dbg_cmd[@]}" || {
        echo "release: FAIL build (debug refresh) — see /tmp/serial_build-debug.log" >&2
        return 1
    }
    for bin in "target/release/dartq" "target/release/$hp" "target/debug/dartq" "target/debug/$hp"; do
        [ -x "$bin" ] || die "build finished but $bin is missing — investigate the build logs before continuing."
    done
    echo "release: PASS build"
}

# --- verify ------------------------------------------------------------------

# Serial-window evidence assertion (#1182, the #1179 P2-3 ruling). The
# verify chain below is the #622 subset; the full 32-step universe
# (acceptance, sem2, incremental, trend, lints-new, mega perf, the crate
# suites, coverage) is scripts/serial-gates.sh (#147), run separately under
# SERIAL_TAG. This step never re-runs the window: it asserts that every
# evidence-producing window step left its label directory on disk AND that
# each is newer than HEAD's commit time — i.e. the window ran against the
# content being released (a commit after the window stales all of it; run
# the window last). Steps without a target/evidence directory (rebuilds,
# pins, the crate suites, lints-new-gate, schema-freeze, the live faces,
# coverage under target/coverage) cannot be asserted this way and stay out
# of this face. The path table mirrors serial-gates.sh's label block
# (default label + "-$SERIAL_TAG") and each gate script's OUT_ROOT.
serial_evidence_paths() {
    tag="-$1"
    cat <<EOF
target/evidence/gate-m0/trend-w5${tag}-syntax
target/evidence/gate-m0/trend-w5${tag}-union
target/evidence/gate-m1/m1-r5${tag}
target/evidence/gate-acceptance/serial${tag}
target/evidence/gate-lints/lints-r1${tag}
target/evidence/incremental/incr-r1${tag}
target/evidence/gate-m2/m2-r1${tag}
target/evidence/gate-sem2/sem2-r1${tag}
target/evidence/gate-m3/m3-r1${tag}
target/evidence/mega/mega-r1${tag}-oracleleg
target/evidence/mega/mega-r1${tag}
target/evidence/repro/serial${tag}
EOF
}

serial_evidence_assert() {
    # SERIAL_TAG presence is a usage check at dispatch time (before any phase
    # runs), so an unset tag never reaches this point outside --dry-run.
    # Reference file stamped with HEAD's commit time; format-local and
    # touch -t both use the local zone, and find -newer is a POSIX mtime
    # compare — no date parsing, no stat(1) dialect.
    ref="$(mktemp)" || die "mktemp failed"
    touch -t "$(git log -1 --date=format-local:%Y%m%d%H%M.%S --format=%cd)" "$ref" || die "cannot stamp the HEAD commit time onto $ref"
    head_desc="HEAD $(git log -1 --format='%h committed %cI')"
    bad=0
    for p in $(serial_evidence_paths "$SERIAL_TAG"); do
        if [ ! -e "$p" ]; then
            echo "verify: serial-evidence MISSING $p" >&2
            bad=1
        elif [ -z "$(find "$p" -prune -newer "$ref")" ]; then
            echo "verify: serial-evidence STALE $p (older than $head_desc)" >&2
            bad=1
        fi
    done
    rm -f "$ref"
    [ "$bad" = 0 ]
}

# run_gate NAME TIMEOUT CAP EVIDENCE CMD...
phase_verify() {
    echo "release: [verify] RSS-watchdog reminder: the gates spawn real dart/dartq processes — freeze every other lane for the duration"
    hp="$(harness_pkg)"
    m0_label="${M0_LABEL:-m0-r6}"
    m1_label="${M1_LABEL:-m1-r4}"
    lints_label="${LINTS_LABEL:-lints-r1}"
    m2_label="${M2_LABEL:-m2-r1}"
    m3_label="${M3_LABEL:-m3-r1}"
    gate_dry=""; [ "$DRY_RUN" = 1 ] && gate_dry="--dry-run"

    # Calibration pins first (#937): the oracle-free pin tests are
    # seconds-fast and catch frozen-universe drift before hours of oracle
    # gate work. Required step, not best-effort. The m3 pin asserts set
    # equality between tests/harness/m3_unused_codes.txt (already
    # re-frozen: 11 rows) and the 11-variant USAGE_FAMILY in
    # m3_codes_pin.rs — verification only, green on the committed freeze.
    # A red here is a real drift, never a designed one: add the targeted
    # fixture it demands first, then `cargo test -p dartq-semantic --test
    # m3_codes_pin regen_m3_unused_codes -- --ignored` and reconcile the
    # header prose — never regen blind. Nothing below works around a red.
    if [ "$DRY_RUN" = 1 ]; then
        echo "release: [verify] would run: cargo test -p harness   # calibration pins"
        echo "release: [verify] would run: cargo test -p dartq-semantic --test m3_codes_pin   # m3 unused-family freeze pin (set equality, 11 rows)"
        echo "verify: SKIP pins (dry-run)"
    else
        run_gate pins-harness 600 8388608 "terminal output (harness calibration pins)" \
            cargo test -p harness || return 1
        run_gate pins-m3 600 8388608 "terminal output (m3 unused-family freeze pin)" \
            cargo test -p dartq-semantic --test m3_codes_pin || return 1
    fi

    # Serial-window evidence next (#1182): a filesystem check, instant — a
    # missing or stale window fails here, before the hours of gate work
    # below (the same seconds-first logic as the pins). See
    # serial_evidence_assert above for what it proves and what it cannot.
    if [ "$DRY_RUN" = 1 ]; then
        echo "release: [verify] would assert serial-window evidence (SERIAL_TAG=${SERIAL_TAG:-<unset>}) present and newer than HEAD's commit time:"
        serial_evidence_paths "${SERIAL_TAG:-<tag>}" | sed 's/^/    /'
        echo "verify: SKIP serial-evidence (dry-run)"
    else
        echo "verify: running serial-evidence..."
        if serial_evidence_assert; then
            echo "verify: PASS serial-evidence"
        else
            echo "verify: FAIL serial-evidence — the serial window has not run against HEAD: run SERIAL_TAG=<fresh tag> scripts/serial-gates.sh (#147), then re-run verify with the same SERIAL_TAG" >&2
            return 1
        fi
    fi

    # Gates are invoked via `bash scripts/...` (the CI convention; every gate
    # script is committed 100755, so the prefix is convention, not a crutch).
    run_gate m0 2400 8388608 "target/evidence/gate-m0/${m0_label}-syntax, target/evidence/gate-m0/${m0_label}-union" \
        bash scripts/m0-gate.sh --label "$m0_label" $gate_dry || return 1
    run_gate m1 900 8388608 "target/evidence/gate-m1/$m1_label" \
        bash scripts/m1-gate.sh $gate_dry || return 1
    run_gate lints 900 8388608 "target/evidence/gate-lints/$lints_label" \
        bash scripts/lint-gate.sh $gate_dry || return 1
    run_gate m2 900 8388608 "target/evidence/gate-m2/$m2_label" \
        bash scripts/m2-gate.sh $gate_dry || return 1
    run_gate m3 600 8388608 "target/evidence/gate-m3/$m3_label" \
        env M3_LABEL="$m3_label" bash scripts/m3-gate.sh $gate_dry || return 1

    # M4 surfaces (#945): the daemon/plugin/commands/schema suites, then the
    # schema v1 freeze sweep (cross-face: CLI machine/json shapes + a live
    # daemon RPC session; drives the debug dartq built by the build phase).
    # plugin-suite runs the whole dartq-plugin default face — the six
    # dart-spawning integration files opt out via #[ignore] and belong to
    # the DARTQ_M4_LIVE plugin-live face below (which wakes every
    # #[ignore]d test in the crate); no target list to keep in sync.
    if [ "$DRY_RUN" = 1 ]; then
        echo "release: [verify] would run: cargo test -p dartq-daemon -- --test-threads 2   # daemon-suite"
        echo "release: [verify] would run: cargo test -p dartq-plugin -- --test-threads 2   # plugin-suite"
        echo "release: [verify] would run: cargo test -p dartq-commands -- --test-threads 2   # commands-suite"
        echo "release: [verify] would run: cargo test -p dartq-schema -- --test-threads 2   # schema-suite"
        echo "release: [verify] would run: scripts/schema-freeze-sweep.sh --no-build   # schema-freeze"
        echo "verify: SKIP m4 suites (dry-run)"
    else
        run_gate daemon-suite 1800 8388608 "terminal output (dartq-daemon suite)" \
            cargo test -p dartq-daemon -- --test-threads 2 || return 1
        run_gate plugin-suite 1800 8388608 "terminal output (dartq-plugin default face; dart-spawning suites #[ignore]d out)" \
            cargo test -p dartq-plugin -- --test-threads 2 || return 1
        run_gate commands-suite 900 8388608 "terminal output (dartq-commands suite)" \
            cargo test -p dartq-commands -- --test-threads 2 || return 1
        run_gate schema-suite 900 8388608 "terminal output (dartq-schema suite)" \
            cargo test -p dartq-schema -- --test-threads 2 || return 1
        run_gate schema-freeze 1200 8388608 "terminal output (per-face PASS/FAIL verdicts)" \
            bash scripts/schema-freeze-sweep.sh --no-build --label "${SCHEMA_FREEZE_LABEL:-schema-freeze-r1}" || return 1
    fi
    # Live host-SDK faces: real dart plugin/host processes — opt-in via
    # DARTQ_M4_LIVE=1 (resource discipline; the same flag gates these faces
    # in serial-gates.sh). A dry-run prints the plan either way.
    if [ "${DARTQ_M4_LIVE:-0}" = 1 ]; then
        if [ "$DRY_RUN" = 1 ]; then
            echo "release: [verify] would run: cargo test -p dartq-daemon --test rpc_commands -- --ignored --test-threads 2   # daemon-live"
            echo "release: [verify] would run: cargo test -p dartq-plugin -- --ignored --test-threads 2   # plugin-live (all #[ignore]d suites incl. the #644 compat matrix)"
            echo "release: [verify] would run: (cd crates/dartq-plugin-host && dart pub get && dart test -j 2)   # host-live"
        else
            run_gate daemon-live 900 8388608 "terminal output (rpc_commands serial-phase test)" \
                cargo test -p dartq-daemon --test rpc_commands -- --ignored --test-threads 2 || return 1
            run_gate plugin-live 1800 8388608 "terminal output (all #[ignore]d dart-spawning plugin suites incl. the #644 compat matrix)" \
                cargo test -p dartq-plugin -- --ignored --test-threads 2 || return 1
            run_gate host-live 1800 8388608 "terminal output (dartq_plugin_host dart test)" \
                bash -c "cd crates/dartq-plugin-host && dart pub get && dart test -j 2" || return 1
        fi
    elif [ "$DRY_RUN" != 1 ]; then
        echo "release: [verify] SKIP m4-live faces (DARTQ_M4_LIVE not set — live dart-process suites stay opt-in)"
    fi

    if [ "$DRY_RUN" = 1 ]; then
        echo "release: [verify] would run: scripts/ci.sh   # no dry mode: per-crate serial fmt + clippy, calibration pins, rustdoc doc gate (#1169), untracked-ref gate (15-40 min)"
        echo "verify: SKIP ci (dry-run)"
    else
        run_gate ci 3600 8388608 "terminal output (per-crate fmt + clippy, calibration pins, rustdoc doc gate, untracked-ref gate; first-fail)" \
            bash scripts/ci.sh || return 1
    fi
    if [ "$DRY_RUN" != 1 ]; then
        # m0/m1 persist divergence cases into tests/corpus/regressions/cases;
        # repro-gate refuses a dirty tracked tree, so the gate outputs must
        # be committed before the seal can run.
        if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
            echo "release: FAIL verify at repro-gate precondition — the tracked tree went dirty during the chain (gate-persisted regression cases under tests/corpus/regressions/cases, most likely)." >&2
            echo "release: commit the gate outputs first (a rerun of a failed gate needs a fresh label: M0_LABEL=<...>-r2 etc.), then re-run --phase verify." >&2
            return 1
        fi
        run_gate repro 1800 8388608 "target/evidence/repro/default" \
            bash scripts/repro-gate.sh || return 1
    else
        echo "release: [verify] would run: scripts/repro-gate.sh   # refuses a dirty tracked tree at run time"
        echo "verify: SKIP repro (dry-run)"
    fi
    echo "release: PASS verify"
}

# --- bench -------------------------------------------------------------------

phase_bench() {
    mega_label="${MEGA_LABEL:-mega-r1}"
    cat <<EOF
release: [bench] five-axis evidence plan (the bench-readiness ledger) — serial phase, RSS-watchdog caps below
  axis 1 rss-100k + axis 2 incremental-p95 + axis 3 token-cost + axis 4 cold (m0 smoke scale):
    rss_cmd.sh bench 4294967296 ./target/release/harness bench \\
      --corpus-dir tests/corpus/m0 \\
      --manifest target/evidence/bench/incremental-manifest.json \\
      --metric rss-100k --metric incremental-p95 --metric token-cost --metric cold \\
      --dartq-cmd "./target/release/dartq check --format=machine" \\
      --oracle-dart "\$HOME/fvm/default/bin/dart" \\
      --with-oracle --out target/evidence/bench-<fresh-label>
  axis 4 cold (milestone provenance, mega-r1 oracle leg incl. the A7 cold pair; release face + --strict, the serial-gates #1145 shape):
    rss_cmd.sh mega1 12582912 env DARTQ_CMD="./target/release/dartq analyze --format=machine --sdk $SDK_ROOT" \\
      scripts/mega-run.sh --scale 1k --oracle-leg --strict --label $mega_label
  axis 2 resident arm:
    rss_cmd.sh cargobench 4294967296 cargo bench -p dartq-engine --bench incremental_p95
  axis 5 reproducibility tuple: covered by the verify phase (repro-gate seal).
  caveat G2: token-cost uses a chars-per-4 heuristic tokenizer — absolute token
  numbers are uncalibrated (savings ratio unaffected); verbosity tiers are
  fixed on the driven faces (no -v sweep yet).
EOF
    if [ "$RUN_BENCH" != 1 ]; then
        echo "release: SKIP bench (plan only; pass --run-bench to execute)"
        return 0
    fi
    echo "release: [bench] RSS-watchdog reminder: serial phase — one dart/cargo process at a time from here on"
    # The A7 dartq cold leg must meet the release-quality oracle on the same
    # face (a debug dartq understates the oracle/dartq ratio), and without
    # --strict mega-run asserts nothing — the serial-gates.sh #1145 shape.
    # Scoped env prefix, never a global export: mega-run's own default
    # (debug) face stays untouched for any other invocation.
    [ -x ./target/release/dartq ] || die "./target/release/dartq not found — the A7 cold leg drives the release face; run --phase build first."
    run_guarded bench-mega 12582912 3600 \
        env DARTQ_CMD="./target/release/dartq analyze --format=machine --sdk $SDK_ROOT" \
        scripts/mega-run.sh --scale 1k --oracle-leg --strict --label "$mega_label" || {
        echo "release: FAIL bench (mega-run $mega_label) — see target/evidence/mega/$mega_label and /tmp/serial_bench-mega.log" >&2
        return 1
    }
    run_guarded bench-engine-p95 4294967296 1800 \
        cargo bench -p dartq-engine --bench incremental_p95 || {
        echo "release: FAIL bench (engine incremental_p95) — see /tmp/serial_bench-engine-p95.log" >&2
        return 1
    }
    echo "release: caveat G2: the persisted token-cost numbers ride the chars-per-4 heuristic — absolute counts uncalibrated, savings ratio unaffected."
    echo "release: PASS bench"
}

# --- bundle ------------------------------------------------------------------

phase_bundle() {
    version="$(workspace_version)"
    [ -n "$version" ] || die "cannot read the workspace version from Cargo.toml ([workspace.package] version)"
    triple="$(rustc -vV | sed -n 's/^host: //p')"
    [ -n "$triple" ] || die "cannot read the host target triple from rustc -vV"
    hp="$(harness_pkg)"
    bundle="dist/dartq-v$version-$triple"
    grep -q "^## \[$version\]" CHANGELOG.md || die "CHANGELOG.md has no '## [$version]' section — the Keep-a-Changelog contract requires it before a $version tag. The harness checks, it never creates."
    for f in README.md CHANGELOG.md LICENSE-APACHE LICENSE-MIT "target/release/dartq" "target/release/$hp"; do
        [ -e "$f" ] || die "bundle input missing: $f (run --phase build for the binaries; README, CHANGELOG and licenses are tracked files)"
    done
    files=(dartq "$hp" README.md CHANGELOG.md LICENSE-APACHE LICENSE-MIT)
    if [ "$DRY_RUN" = 1 ]; then
        echo "release: [bundle] would create $bundle/ with:"
        printf '  %s\n' "${files[@]}" | sed 's/^/  /'
        echo "release: [bundle] would write $bundle/SHA256SUMS via: $(sha_cmd)"
        echo "release: SKIP bundle (dry-run)"
        return 0
    fi
    [ -e "$bundle" ] && die "$bundle already exists — a rerun never overwrites recorded evidence; remove it or bump the label."
    echo "release: [bundle] creating $bundle/"
    mkdir -p "$bundle" || die "cannot mkdir $bundle"
    cp "target/release/dartq" "target/release/$hp" README.md CHANGELOG.md LICENSE-APACHE LICENSE-MIT "$bundle/" || die "copy into $bundle failed"
    (cd "$bundle" && $(sha_cmd) "${files[@]}" > SHA256SUMS) || die "checksum generation failed"
    echo "release: [bundle] SHA256SUMS:"
    sed 's/^/  /' "$bundle/SHA256SUMS"
    echo "release: PASS bundle ($bundle)"
}

# --- tag ---------------------------------------------------------------------

phase_tag() {
    version="$(workspace_version)"
    [ -n "$version" ] || die "cannot read the workspace version from Cargo.toml"
    tag="v$version"
    triple="$(rustc -vV | sed -n 's/^host: //p')"
    bundle="dist/dartq-$tag-$triple"
    cat <<EOF
release: [tag] the exact release commands:
  git tag -a $tag -m "dartq $version"
  git push origin main
  git push origin $tag
EOF
    if [ "$EXECUTE" != 1 ]; then
        echo "release: SKIP tag (dry-run; pass --execute to perform — in --all mode only after every earlier phase passed)"
        return 0
    fi
    echo "release: [tag] --execute: re-running the mechanical preconditions"
    check_clean_tree
    check_branch
    check_naming
    command -v gh >/dev/null 2>&1 || die "gh is required for the tag-push probe (brew install gh, then gh auth login)"
    git remote get-url origin >/dev/null 2>&1 || die "no 'origin' remote — D3 requires the repository pushed before any tag (https://github.com/fluttercandies/dartq)."
    git rev-parse -q --verify "refs/tags/$tag" >/dev/null 2>&1 && die "tag $tag already exists — refusing to move a release tag."
    [ -d "$bundle" ] || die "bundle directory $bundle missing — run --phase bundle first; never tag unattached to its artifacts."
    grep -q "^## \[$version\]" CHANGELOG.md || die "CHANGELOG.md has no '## [$version]' section."
    if [ "$ALL" != 1 ]; then
        echo "release: [tag] NOTE standalone execute: the gate chain itself is the operator's responsibility here — prefer --all --execute so the tag only follows a green chain." >&2
    fi
    git tag -a "$tag" -m "dartq $version" || die "git tag failed — resolve and re-run."
    echo "release: [tag] created $tag at $(git rev-parse --short HEAD)"
    git push origin main || die "push of main failed — the tag was NOT pushed; resolve and re-run (the tag exists locally)."
    git push origin "$tag" || die "push of $tag failed — push it manually once resolved."
    echo "release: PASS tag ($tag pushed; main pushed before the tag per D3)"
}

# --- usage + dispatch --------------------------------------------------------

usage() {
    # The whole header through the Usage block, anchored on the `set` line
    # (the serial-gates.sh idiom) — a hardcoded line range truncated
    # --help mid-sentence as the header grew.
    sed -n '2,/^set -u -o pipefail$/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

ALL=0; DRY_RUN=0; RUN_BENCH=0; EXECUTE=0
phase=""; got_phase=0
while [ $# -gt 0 ]; do
    case "$1" in
    --all) ALL=1 ;;
    --dry-run) DRY_RUN=1 ;;
    --run-bench) RUN_BENCH=1 ;;
    --execute) EXECUTE=1 ;;
    --phase)
        [ $# -ge 2 ] || die "--phase needs a name (preflight|build|verify|bench|bundle|tag) — try --help"
        phase="$2"; got_phase=1; shift
        ;;
    --phase=*)
        phase="${1#--phase=}"; got_phase=1
        [ -n "$phase" ] || die "--phase= needs a name (preflight|build|verify|bench|bundle|tag)"
        ;;
    --help | -h) usage; exit 0 ;;
    -*) die "unknown argument '$1' (--help for usage)" ;;
    *)
        # Bare phase name, e.g. `scripts/release.sh preflight --dry-run`.
        if [ "$got_phase" = 0 ] && [ -z "$phase" ]; then
            phase="$1"; got_phase=1
        else
            die "unexpected argument '$1' (--help for usage)"
        fi
        ;;
    esac
    shift
done

if [ "$ALL" = 1 ] && [ "$got_phase" = 1 ]; then
    die "--all and --phase are mutually exclusive (--all runs the full chain in order)"
fi
if [ "$EXECUTE" = 1 ] && [ "$ALL" = 0 ] && [ "$phase" != "tag" ]; then
    die "--execute only applies to the tag phase (--phase tag --execute, or --all --execute)"
fi
if [ "$RUN_BENCH" = 1 ] && [ -n "$phase" ] && [ "$phase" != "bench" ]; then
    die "--run-bench only applies to the bench phase"
fi
# verify's serial-window evidence assertion composes the window's label
# directories from SERIAL_TAG — refuse up front, not after preflight + build.
if [ "$DRY_RUN" = 0 ] && [ -z "${SERIAL_TAG:-}" ] && { [ "$ALL" = 1 ] || [ "$phase" = "verify" ]; }; then
    die "SERIAL_TAG is required by verify's serial-window evidence assertion: pass the tag the window ran under (SERIAL_TAG=<tag> scripts/serial-gates.sh, #147), e.g. SERIAL_TAG=r7 scripts/release.sh --phase verify"
fi

run_phase() {
    case "$1" in
    preflight) phase_preflight ;;
    build) phase_build ;;
    verify) phase_verify ;;
    bench) phase_bench ;;
    bundle) phase_bundle ;;
    tag) phase_tag ;;
    *) die "unknown phase '$1' — valid: preflight build verify bench bundle tag (--help for usage)" ;;
    esac
}

if [ "$ALL" = 1 ]; then
    for p in preflight build verify bench bundle tag; do
        run_phase "$p" || {
            ec=$?
            echo "release: chain aborted at '$p' (exit $ec) — the tag phase never runs on a red chain" >&2
            exit "$ec"
        }
    done
    echo "release: full chain green."
else
    run_phase "${phase:-preflight}"
fi
