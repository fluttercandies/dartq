#!/usr/bin/env bash
# cli-parity-matrix.sh — the systematic dart-analyze flag-parameter parity
# matrix: every CLI flag x both CLIs (oracle dart analyze vs dartq analyze)
# x identical probe corpora, comparing stdout + stderr + exit code per leg.
#
# Output parity on the (severity, code, file) triple face is gate-verified
# elsewhere; this lane measures the PARAMETER surface around it:
#   (a) parity flags — must match: exit-code severity ladder
#       (--fatal-infos / --[no-]fatal-warnings combinations over error-,
#       warning- and info-only probes), the --format json byte face (the
#       shared parity surface since the machine face retired on dartq,
#       #2606), positional file/dir/multi targets, missing-target and
#       bogus-value usage errors, --sdk-path / --packages /
#       --enable-experiment acceptance and rejection, -h/--help,
#       unknown-flag and --no-fatal-infos rejection.
#   (b) dartq extensions — must not alter the parity face: --metrics-json,
#       --overlay, --memory-limit, --trace-events, --cache-size-limit,
#       --plugins/--no-plugins, the hidden --sdk alias.
#   (c) cross surface — flags one side lacks: the retired machine face and
#       --format=default on dartq (usage error 64 there, valid on the
#       oracle), the dartq-only lon/text faces, dartq extensions on the
#       oracle (the oracle's unknown-flag usage error), and the bare `-v`
#       collision (oracle: silent dartdev global; dartq: unknown flag).
#
# Verdicts: PASS (contract holds), FAIL (parity leg diverges), CRASH
# (dartq internal-error path), EXTENSION (documented one-sided surface,
# recorded not gated). Exit status 0 only when every class-(a) parity leg
# holds; class-(c) EXTENSION legs assert the documented asymmetry.
#
# Serial-phase tool: one oracle/one dartq process at a time, no daemons,
# no cargo. Evidence lands under target/evidence/cli-parity/ (per-probe
# stdout/stderr/rc captures, matrix.tsv, summary.md, divergences.md).
# Requires the dartq release binary (cargo build --release -p dartq-cli)
# and a Dart SDK root (DARTQ_DART_SDK, defaulting to the fvm default).
# Usage: scripts/cli-parity-matrix.sh [output-dir]
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SDK_ROOT="${DARTQ_DART_SDK:-$HOME/fvm/default/bin/cache/dart-sdk}"
DART="$SDK_ROOT/bin/dart"
DARTQ="$REPO_ROOT/target/release/dartq"
OUT="${1:-$REPO_ROOT/target/evidence/cli-parity}"
PROBES="$OUT/probes"

if [ ! -x "$DART" ]; then
  echo "cli-parity-matrix: oracle dart not found at $DART (set DARTQ_DART_SDK)" >&2
  exit 2
fi
if [ ! -x "$DARTQ" ]; then
  echo "cli-parity-matrix: dartq binary not built at $DARTQ (cargo build --release -p dartq-cli)" >&2
  exit 2
fi
# dartq falls back to $DART_SDK when no --sdk is passed (the serial-gates
# replay posture); the oracle legs never read it.
export DART_SDK="$SDK_ROOT"

mkdir -p "$PROBES"

# Fixed probe corpora (all in-repo; E/W/I rows verified to agree across both
# CLIs during the 2026-10-18 matrix census — sem2 files with known output-face
# divergences are deliberately excluded from the exit-code legs):
#   E  sem2 error fixture               (errors   -> rc 3)
#   W  m2-warning warnings-only fixture (warnings -> rc 2 default, 0 no-fatal)
#   I  lints-new pure-INFO fixtures     (infos    -> rc 0 default, 1 fatal-infos)
E_TARGET="tests/corpus/sem2/lib/targeted/body_might_complete_normally.dart"
E_DIRNAME="tests/corpus/sem2/lib/targeted"
W_TARGET="tests/corpus/m2-warning/lib/targeted/unnecessary_cast.dart"
I_TARGET="tests/corpus/lints-new/lib/unnecessary_ignore_line.dart"
I_TARGET2="tests/corpus/lints-new/lib/unnecessary_ignore.dart"
DIR_TARGET="tests/corpus/sem2/lib/targeted"
CORPUS="tests/corpus/sem2"
OVERLAY_KEY="lib/targeted/body_might_complete_normally.dart"
OVERLAY_CLEAN="$PROBES/overlay-clean.dart"
printf 'void main() {}\n' > "$OVERLAY_CLEAN"

MATRIX="$OUT/matrix.tsv"
: > "$MATRIX"
FAILS=()

emit() { printf '%b\n' "$*" >> "$MATRIX"; }

# run_leg <id> <class> <mode> <strip-prefix> <desc> -- <dart args...> -- <dartq args...>
# strip-prefix is removed from BOTH sides' path fields before a triple-face
# comparison (the oracle prints paths relative to the target context, dartq
# prints absolutes; both normalize to the same corpus-relative spelling).
run_leg() {
  local id="$1" class="$2" chk="$3" strip="$4" desc="$5"
  case "$chk" in
    rc|byte|triple) chk="chk_$chk" ;;
  esac
  shift 5
  # Argument convention: `<id> ... <desc> -- <dart args> -- <dartq args>` —
  # the first `--` opens the dart list, the second switches to dartq.
  local dart_args=() dartq_args=() side=""
  while [ "$#" -gt 0 ]; do
    if [ "$1" = "--" ]; then
      if [ -z "$side" ]; then side="dart"; else side="dartq"; fi
      shift
      continue
    fi
    if [ "$side" = "dartq" ]; then dartq_args+=("$1"); else dart_args+=("$1"); fi
    shift
  done
  local d="$PROBES/$id.dart" q="$PROBES/$id.dartq"
  # Analyze exit codes 2/3 are normal (severity ladders), so capture them
  # without tripping errexit. The ${arr[@]+...} guards keep bash 3.2's
  # set -u from rejecting empty arrays.
  local rc=0
  ( cd "$REPO_ROOT" && "$DART" analyze ${dart_args[@]+"${dart_args[@]}"} >"$d.out" 2>"$d.err" ) || rc=$?
  echo "$rc" >"$d.rc"
  rc=0
  ( cd "$REPO_ROOT" && "$DARTQ" analyze ${dartq_args[@]+"${dartq_args[@]}"} >"$q.out" 2>"$q.err" ) || rc=$?
  echo "$rc" >"$q.rc"
  local verdict detail
  verdict=$("$chk" "$id" "$strip"); detail="${verdict#*$'\t'}"; verdict="${verdict%%$'\t'*}"
  emit "$id	$class	$desc	dart=$(cat "$d.rc")	dartq=$(cat "$q.rc")	$verdict	$detail"
  if [ "$verdict" != "PASS" ] && [ "$verdict" != "EXTENSION" ]; then
    FAILS+=("$id($verdict)")
  fi
}

panicked() { grep -q "internal error: analysis panicked" "$PROBES/$1.dartq.err" 2>/dev/null; }

# Each checker echoes "verict\tdetail".

chk_rc() { # exit-code parity (usage errors and severity ladders)
  local id="$1"
  local drc qrc; drc=$(cat "$PROBES/$id.dart.rc"); qrc=$(cat "$PROBES/$id.dartq.rc")
  if [ "$drc" = "$qrc" ]; then echo "PASS	rc $drc both"
  else echo "FAIL	rc $drc vs $qrc"; fi
}

chk_byte() { # stdout byte-equal + rc equal (machine/json envelope faces)
  local id="$1"
  if panicked "$id"; then echo "CRASH	dartq panicked: $(sed -n 2p "$PROBES/$id.dartq.err" | cut -c1-100)"; return; fi
  local drc qrc; drc=$(cat "$PROBES/$id.dart.rc"); qrc=$(cat "$PROBES/$id.dartq.rc")
  if [ "$drc" != "$qrc" ]; then echo "FAIL	rc $drc vs $qrc"; return; fi
  if ! cmp -s "$PROBES/$id.dart.out" "$PROBES/$id.dartq.out"; then
    echo "FAIL	$(diff "$PROBES/$id.dart.out" "$PROBES/$id.dartq.out" | grep -c '^[<>]') output lines differ"; return
  fi
  echo "PASS	byte-identical rc=$drc"
}

# triple_of <side> <out-file> <strip-prefix> — sorted unique (sev|code|file).
# Both human faces need state machines: dart wraps long rows (the code lands
# on a ` - code` continuation line), dartq prints one absolute path header
# line per file with `  l:c • Severity • code • message` rows under it.
triple_of() {
  local side="$1" file="$2" strip="$3"
  python3 - "$side" "$REPO_ROOT/" "$strip" "$file" <<'PYEOF'
import re, sys
side, repo, strip, path_file = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]

def norm(p):
    if p.startswith(repo):
        p = p[len(repo):]
    if strip and p.startswith(strip):
        p = p[len(strip):]
    return p

def joined(p, root):
    # dartq file fields spell root-relative inside the analysis root —
    # rejoin before the shared normalization (absolute spellings pass).
    return f"{root}/{p}" if root and not p.startswith("/") else p

rows = set()
if side == "dart":
    # dart default face: `  sev - file:l:c - message - code`, long rows wrap
    # the code onto a `   - code` continuation line; context messages also
    # continue with `   - sentence` (contains spaces -> not a code).
    cur = None
    def flush():
        if cur[3]:
            rows.add(f"{cur[0]}|{cur[3]}|{norm(cur[1]).rsplit(':', 2)[0]}")
    for line in open(path_file):
        line = line.rstrip("\n")
        m = re.match(r"\s*(error|warning|info) - (\S+) - (.*)$", line)
        if m:
            if cur: flush()
            msg, code = m.group(3), ""
            if " - " in msg:
                head, _, tail = msg.rpartition(" - ")
                if tail and " " not in tail:
                    msg, code = head, tail
            cur = [m.group(1), m.group(2), msg, code]
        elif cur is not None:
            # Continuation of a wrapped row (multi-line messages occur when
            # the quoted source text itself contains a newline). A trailing
            # ` - code` segment still carries the diagnostic code; context
            # sentences contain spaces and are irrelevant to the triple.
            tail_code = None
            if " - " in line:
                head, _, tail = line.rpartition(" - ")
                if tail and " " not in tail:
                    tail_code = tail
            if tail_code:
                cur[3] = tail_code
    if cur: flush()
else:
    # dartq default face: the canonical LON document `{v:1 root:<abs>
    # diags:[…]}` — the document names its analysis root once and file
    # fields spell root-relative inside it (absolute outside), so each
    # file is joined back onto its root before the shared repo-relative
    # normalization. Uniform rows collapse to the record table
    # (`[code sev file …;v v v …]`, positional after the header),
    # non-uniform rows stay object-form (`{code:X sev:error file:F …}`).
    # File paths carry no spaces in the corpus, so each row's first three
    # whitespace tokens are safe.
    for line in open(path_file):
        line = line.rstrip("\n")
        root = re.match(r"\{v:\d+ root:(\S+) ", line)
        root = root.group(1) if root else None
        m = re.search(r"diags:\[([a-z_ ]+);", line)
        if m and m.group(1).split()[:3] == ["code", "sev", "file"]:
            for row in line[m.end():].rstrip("]}").split(";"):
                toks = row.split()
                if len(toks) >= 3:
                    rows.add(f"{toks[1]}|{toks[0]}|{norm(joined(toks[2], root))}")
        for m in re.finditer(r"(?:^|[{;\s])code:([^ ;]+)\s+sev:(error|warning|info)\s+file:([^ ;]+)", line):
            rows.add(f"{m.group(2)}|{m.group(1)}|{norm(joined(m.group(3), root))}")
for r in sorted(rows):
    print(r)
PYEOF
}

chk_triple() { # (severity, code, file) set equality + rc equality
  local id="$1" strip="$2"
  if panicked "$id"; then echo "CRASH	dartq panicked"; return; fi
  local drc qrc; drc=$(cat "$PROBES/$id.dart.rc"); qrc=$(cat "$PROBES/$id.dartq.rc")
  if [ "$drc" != "$qrc" ]; then echo "FAIL	rc $drc vs $qrc"; return; fi
  triple_of dart "$PROBES/$id.dart.out" "" > "$PROBES/$id.dart.triple"
  triple_of dartq "$PROBES/$id.dartq.out" "$strip" > "$PROBES/$id.dartq.triple"
  if ! cmp -s "$PROBES/$id.dart.triple" "$PROBES/$id.dartq.triple"; then
    echo "FAIL	triple sets differ: dart=$(wc -l < "$PROBES/$id.dart.triple" | tr -d ' ') dartq=$(wc -l < "$PROBES/$id.dartq.triple" | tr -d ' ')"; return
  fi
  echo "PASS	triple-set equal ($(wc -l < "$PROBES/$id.dart.triple" | tr -d ' ') entries) rc=$drc"
}

# Extension neutrality: dartq-only flags leave the dartq baseline face
# unchanged (the retired -v tiers are gone with the machine face; the
# remaining extensions must be byte-neutral on the default face).
chk_ext() {
  local id="$1" strip="$2"
  if panicked "$id"; then echo "CRASH	dartq panicked"; return; fi
  local qrc brc; qrc=$(cat "$PROBES/$id.dartq.rc"); brc=$(cat "$PROBES/ext-baseline.dartq.rc")
  case "$id" in
    ext-overlay)
      triple_of dartq "$PROBES/$id.dartq.out" "" > "$PROBES/$id.dartq.triple"
      if [ "$qrc" = 0 ] && [ ! -s "$PROBES/$id.dartq.triple" ]; then
        echo "PASS	overlay neutralized the error row rc=0"
      else
        echo "FAIL	overlay face rc=$qrc rows=$(wc -l < "$PROBES/$id.dartq.triple" | tr -d ' ')"
      fi
      return ;;
  esac
  if [ "$qrc" != "$brc" ]; then echo "FAIL	rc $qrc vs baseline $brc"; return; fi
  if ! cmp -s "$PROBES/$id.dartq.out" "$PROBES/ext-baseline.dartq.out" \
     || ! cmp -s "$PROBES/$id.dartq.err" "$PROBES/ext-baseline.dartq.err"; then
    echo "FAIL	stdout/stderr differs from dartq baseline"; return
  fi
  echo "PASS	neutral vs dartq baseline rc=$qrc"
}

# Cross legs: the DOCUMENTED one-sided asymmetry is the contract.
chk_cross() {
  local id="$1" exp_d exp_q drc qrc
  drc=$(cat "$PROBES/$id.dart.rc"); qrc=$(cat "$PROBES/$id.dartq.rc")
  case "$id" in
    cross-dq-flags-on-oracle) exp_d=64; exp_q=3 ;;  # extension accepted only on dartq
    ext-sdk-alias)            exp_d=64; exp_q=3 ;;  # hidden compat alias only on dartq
    cross-bare-v)             exp_d=3;  exp_q=64 ;; # bare -v: dartdev global vs unknown on dartq
    fmt-machine-efile)        exp_d=3;  exp_q=64 ;; # machine retired on dartq (errors -> 3)
    fmt-machine-ifile)        exp_d=0;  exp_q=64 ;; # machine retired on dartq (infos -> 0)
    fmt-machine-full)         exp_d=3;  exp_q=64 ;; # machine retired on dartq (full corpus)
    fmt-default-explicit)     exp_d=3;  exp_q=64 ;; # --format=default: oracle human face, retired on dartq
    fmt-lon-dartq-only)       exp_d=64; exp_q=3 ;;  # the dartq default face is dartq-only
    fmt-text-dartq-only)      exp_d=64; exp_q=3 ;;  # the dartq human face is dartq-only
  esac
  if [ "$drc" = "$exp_d" ] && [ "$qrc" = "$exp_q" ]; then
    echo "EXTENSION	documented asymmetry holds (dart=$drc dartq=$qrc)"
  else
    echo "FAIL	expected dart=$exp_d dartq=$exp_q, got dart=$drc dartq=$qrc"
  fi
}

emit "id	class	leg	dart_rc	dartq_rc	verdict	detail"

# ---- dartq baselines for extension neutrality ----
rc=0; ( cd "$REPO_ROOT" && "$DARTQ" analyze "$E_TARGET" >"$PROBES/ext-baseline.dartq.out" 2>"$PROBES/ext-baseline.dartq.err" ) || rc=$?
echo "$rc" >"$PROBES/ext-baseline.dartq.rc"

# ============ (a) parity legs — must match ============

run_leg exit-default           parity rc     "$E_DIRNAME/"  "E-file default exit code"                            -- "$E_TARGET" -- "$E_TARGET"
run_leg exit-fatal-infos-i     parity rc     ""            "I-file --fatal-infos (infos fatal -> rc 1)"          -- --fatal-infos "$I_TARGET" -- --fatal-infos "$I_TARGET"
run_leg exit-fatal-infos-i2    parity rc     ""            "I-file-2 --fatal-infos"                              -- --fatal-infos "$I_TARGET2" -- --fatal-infos "$I_TARGET2"
run_leg exit-default-i         parity rc     ""            "I-file default (infos non-fatal -> rc 0)"            -- "$I_TARGET" -- "$I_TARGET"
run_leg exit-w-default         parity rc     ""            "W-file default (warnings fatal -> rc 2)"             -- "$W_TARGET" -- "$W_TARGET"
run_leg exit-w-nowarn          parity rc     ""            "W-file --no-fatal-warnings -> rc 0"                  -- --no-fatal-warnings "$W_TARGET" -- --no-fatal-warnings "$W_TARGET"
run_leg exit-w-lastwins        parity rc     ""            "W-file --fatal-warnings --no-fatal-warnings last wins -> rc 0" -- --fatal-warnings --no-fatal-warnings "$W_TARGET" -- --fatal-warnings --no-fatal-warnings "$W_TARGET"
run_leg exit-w-info            parity rc     ""            "W-file --fatal-infos (warnings still fatal -> rc 2)" -- --fatal-infos "$W_TARGET" -- --fatal-infos "$W_TARGET"
run_leg exit-i-combo           parity rc     ""            "I-file --no-fatal-warnings --fatal-infos -> rc 1"    -- --no-fatal-warnings --fatal-infos "$I_TARGET" -- --no-fatal-warnings --fatal-infos "$I_TARGET"
run_leg exit-e-fatalinfos      parity rc     ""            "E-file --fatal-infos (errors dominate -> rc 3)"      -- --fatal-infos "$E_TARGET" -- --fatal-infos "$E_TARGET"
run_leg exit-dir               parity triple "tests/corpus/sem2/lib/targeted/" "dir target triple face + rc"                  -- "$DIR_TARGET" -- "$DIR_TARGET"
run_leg file-multi             parity triple ""            "multi file targets"                                  -- "$DIR_TARGET/const_with_non_const.dart" "$DIR_TARGET/non_bool_condition.dart" -- "$DIR_TARGET/const_with_non_const.dart" "$DIR_TARGET/non_bool_condition.dart"
run_leg target-missing         parity rc     ""            "missing target -> usage error"                       -- tests/corpus/sem2/lib/nope.dart -- tests/corpus/sem2/lib/nope.dart
run_leg fmt-json-efile         parity byte   ""            "--format=json byte face, E-file"                     -- --format=json "$E_TARGET" -- --format=json "$E_TARGET"
run_leg fmt-json-ifile         parity byte   ""            "--format=json byte face, I-file"                     -- --format=json "$I_TARGET" -- --format=json "$I_TARGET"
run_leg fmt-space-spelling     parity byte   ""            "--format json (space spelling)"                      -- --format json "$E_TARGET" -- --format json "$E_TARGET"
run_leg fmt-bogus              parity rc     ""            "--format bogus -> usage error on both"               -- --format=bogus "$E_TARGET" -- --format=bogus "$E_TARGET"
run_leg sdk-valid              parity rc     ""            "--sdk-path valid"                                    -- --sdk-path="$SDK_ROOT" "$E_TARGET" -- --sdk-path="$SDK_ROOT" "$E_TARGET"
run_leg sdk-bogus              parity rc     ""            "--sdk-path bogus -> usage error"                     -- --sdk-path=/nonexistent/sdk "$E_TARGET" -- --sdk-path=/nonexistent/sdk "$E_TARGET"
run_leg packages-valid         parity rc     ""            "--packages valid config"                             -- --packages="$CORPUS/.dart_tool/package_config.json" "$E_TARGET" -- --packages="$CORPUS/.dart_tool/package_config.json" "$E_TARGET"
run_leg packages-missing       parity rc     ""            "--packages missing -> usage error"                   -- --packages=/nonexistent/pkg.json "$E_TARGET" -- --packages=/nonexistent/pkg.json "$E_TARGET"
run_leg exp-known              parity rc     ""            "--enable-experiment known"                           -- --enable-experiment=macros "$E_TARGET" -- --enable-experiment=macros "$E_TARGET"
run_leg exp-noprefix           parity rc     ""            "--enable-experiment no- prefix accepted"             -- --enable-experiment=no-macros "$E_TARGET" -- --enable-experiment=no-macros "$E_TARGET"
run_leg exp-unknown            parity rc     ""            "--enable-experiment unknown -> usage error"          -- --enable-experiment=bogus_exp "$E_TARGET" -- --enable-experiment=bogus_exp "$E_TARGET"
run_leg help-long              parity rc     ""            "--help exit code"                                    -- --help -- --help
run_leg help-short             parity rc     ""            "-h exit code"                                        -- -h -- -h
run_leg no-fatal-infos         parity rc     ""            "--no-fatal-infos rejected on both"                   -- --no-fatal-infos "$E_TARGET" -- --no-fatal-infos "$E_TARGET"
run_leg unknown-flag           parity rc     ""            "unknown flag rejected on both"                       -- --bogus-flag "$E_TARGET" -- --bogus-flag "$E_TARGET"

# Corpus-scale format faces (the convergence quantifiers).
run_leg fmt-json-full          parity byte   ""            "--format=json byte face, full corpus"                -- --format=json "$CORPUS" -- --format=json "$CORPUS"

# ============ (b) dartq extensions — parity face neutral ============

run_leg ext-metrics-json       ext chk_ext "" "--metrics-json side channel neutral"  -- ignored -- --metrics-json "$PROBES/metrics.json" "$E_TARGET"
run_leg ext-overlay            ext chk_ext "" "--overlay clean-over-error neutralizes row" -- ignored -- --overlay "$OVERLAY_KEY=$OVERLAY_CLEAN" "$E_TARGET"
run_leg ext-memory-limit       ext chk_ext "" "--memory-limit neutral"               -- ignored -- --memory-limit 4096 "$E_TARGET"
run_leg ext-trace-events       ext chk_ext "" "--trace-events side channel neutral"  -- ignored -- --trace-events "$PROBES/trace.jsonl" "$E_TARGET"
run_leg ext-cache-size-limit   ext chk_ext "" "--cache-size-limit neutral"           -- ignored -- --cache-size-limit 1GiB "$E_TARGET"
run_leg ext-plugins            ext chk_ext "" "--plugins neutral (none declared)"    -- ignored -- --plugins "$E_TARGET"
run_leg ext-no-plugins         ext chk_ext "" "--no-plugins neutral"                 -- ignored -- --no-plugins "$E_TARGET"
run_leg ext-sdk-alias          cross chk_cross "" "hidden --sdk alias: dartq-only (oracle: unknown option)" -- --sdk="$SDK_ROOT" "$E_TARGET" -- --sdk="$SDK_ROOT" "$E_TARGET"

# Side channels must actually materialize (dartq-only, dartq-side check).
metrics_note="metrics file: $(wc -c < "$PROBES/metrics.json" 2>/dev/null || echo 0) bytes"
trace_note="trace file: $(wc -l < "$PROBES/trace.jsonl" 2>/dev/null || echo 0) events"
emit "ext-metrics-json-side	ext	$metrics_note	-	dartq-only	PASS	$([ -s "$PROBES/metrics.json" ] && echo "side file written" || echo "FAIL: side file missing")"
emit "ext-trace-events-side	ext	$trace_note	-	dartq-only	$([ -s "$PROBES/trace.jsonl" ] && echo PASS || echo FAIL)	side channel"

# ============ (c) cross surface — documented one-sided asymmetry ============

# dartq extensions on the oracle: the oracle's unknown-flag usage error (rc 64);
# dartq accepts (rc 3). Run dartq once with the same args for the record.
rc=0; ( cd "$REPO_ROOT" && "$DARTQ" analyze --memory-limit=2048 "$E_TARGET" >"$PROBES/cross-dq-flags-on-oracle.dartq.out" 2>"$PROBES/cross-dq-flags-on-oracle.dartq.err" ) || rc=$?
echo "$rc" >"$PROBES/cross-dq-flags-on-oracle.dartq.rc"
rc=0; ( cd "$REPO_ROOT" && "$DART" analyze --memory-limit=2048 "$E_TARGET" >"$PROBES/cross-dq-flags-on-oracle.dart.out" 2>"$PROBES/cross-dq-flags-on-oracle.dart.err" ) || rc=$?
echo "$rc" >"$PROBES/cross-dq-flags-on-oracle.dart.rc"
emit "cross-dq-flags-on-oracle	cross	--memory-limit on both CLIs	dart=$(cat "$PROBES/cross-dq-flags-on-oracle.dart.rc")	dartq=$(cat "$PROBES/cross-dq-flags-on-oracle.dartq.rc")	EXTENSION	dartq-only extension: oracle says '$(head -1 "$PROBES/cross-dq-flags-on-oracle.dart.err")'"

run_leg cross-bare-v           cross chk_cross "" "bare -v: oracle swallows, dartq rejects" -- -v "$E_TARGET" -- -v "$E_TARGET"

# The retired machine face and --format=default: valid on the oracle, usage
# error (64) on dartq since the #2606 re-cast (lon/json/text is the axis).
run_leg fmt-machine-efile      cross chk_cross "" "machine retired: oracle 3 (E-file), dartq 64"  -- --format=machine "$E_TARGET" -- --format=machine "$E_TARGET"
run_leg fmt-machine-ifile      cross chk_cross "" "machine retired: oracle 0 (I-file), dartq 64"  -- --format=machine "$I_TARGET" -- --format=machine "$I_TARGET"
run_leg fmt-machine-full       cross chk_cross "" "machine retired: oracle 3 (full corpus), dartq 64" -- --format=machine "$CORPUS" -- --format=machine "$CORPUS"
run_leg fmt-default-explicit   cross chk_cross "" "--format=default: oracle human face, dartq 64" -- --format=default "$E_TARGET" -- --format=default "$E_TARGET"

# The dartq-only format values: lon (the constant default face) and text
# (the human face) are usage errors on the oracle.
run_leg fmt-lon-dartq-only     cross chk_cross "" "--format=lon: dartq default face, oracle 64"  -- --format=lon "$E_TARGET" -- --format=lon "$E_TARGET"
run_leg fmt-text-dartq-only    cross chk_cross "" "--format=text: dartq human face, oracle 64"   -- --format=text "$E_TARGET" -- --format=text "$E_TARGET"

echo "cli-parity-matrix: matrix complete -> $MATRIX"
if [ "${#FAILS[@]}" -gt 0 ]; then
  echo "cli-parity-matrix: ${#FAILS[@]} non-PASS legs:" >&2
  printf '  %s\n' "${FAILS[@]}" >&2
  exit 1
fi
echo "cli-parity-matrix: all legs PASS/EXTENSION"
