#!/usr/bin/env bash
# schema-freeze-sweep.sh — the schema v1 freeze gate: cross-face verification
# of the frozen output shapes end-to-end (ticket #474; the frozen-surface
# annotation lives in crates/dartq-schema/src/lib.rs, "Schema freeze (v1)").
#
# Faces swept, and what each asserts:
#
#   cli-check-machine-parity  the machine face is retired on the CLI
#                        (`--format machine` and `--verbosity` are usage
#                        errors, exit 64 — pinned); its frozen contract rides
#                        the json face: `dartq check --format json` rows
#                        re-encoded to the oracle's 8-field machine rows
#                        (SEVERITY|TYPE|CODE|FILE|LINE|COL|LEN|MESSAGE) and
#                        byte-compared with a live
#                        `dart analyze --format=machine` leg over the same
#                        probe; the clean-run face: exit 0, the
#                        empty-diagnostics envelope.
#   cli-check-json       `dartq check --format json`: the oracle envelope
#                        {"version":1,"diagnostics":[...]} with the frozen
#                        row key set (code/severity/type/location/
#                        problemMessage/correctionMessage); the oracle's
#                        location positions are 1-based line/column (the
#                        analyzer's lspToUser) plus the absolute offset.
#   rpc-session-ready    the no-handshake session face: the daemon has no
#                        lifecycle (see crates/dartq-daemon/src/server.rs
#                        `run()` — the session answers real requests from
#                        frame one), so the lightest real request,
#                        `dartq/index`, answers the frozen index envelope
#                        {v, files, libraries, symbols, references} with no
#                        handshake at all (short-vocabulary spellings, the
#                        #2602/#2603 re-projection).
#   rpc-overlay-ack      `dartq/overlay.set` / `dartq/overlay.clear` answers:
#                        exactly {v: 1}.
#   rpc-check-tiers      `dartq/check` at minimal/compact/full: the frozen
#                        envelope {v, diags} and per-tier row key sets
#                        (minimal: code/file/line/col/len; compact/full add
#                        lowercase sev + msg), and the short-id clause —
#                        the full tier substitutes rows' file paths with sN
#                        ids backed by the `symbols` table; minimal/compact
#                        stay unshortened.
#   rpc-fix-answer       `dartq/fix` at compact/full: {v, files} of patch
#                        documents {file, v, edits[]}; every edit rides the
#                        red-line floor (off/len/text/src) plus the anchor
#                        at compact; full adds the builtin rule/msg columns
#                        (priority is plugin-row-only).
#   rpc-fix-rejects      `dartq/fix` with agentId/context/since is answered
#                        INVALID_PARAMS (-32602) naming the disk-view rule —
#                        never an answer payload.
#   rpc-assists          `dartq/assists`: the document {file, v, assists[]}
#                        whose rows carry the msg/src/edits tuple (priority
#                        optional; no plugin in the fixture, so the tier
#                        split on rows stays the unit fixtures').
#   rpc-assists-rejects  `dartq/assists` with a session field: same
#                        INVALID_PARAMS rejection.
#
#   Session teardown is EOF (stdin close) — the protocol's only end signal,
#   there is no shutdown method; the driver asserts the daemon exits
#   EXIT_CLEAN (0) after it.
#
# Expected shapes are embedded below as heredoc/jq skeletons — no external
# golden files. A shape drift (missing/extra/renamed field, wrong version
# constant, wrong rejection) fails its face; the run exits 1.
#
# SERIAL PHASE ONLY: a run builds and executes real dartq processes (the CLI
# and a live daemon session over stdio). Never run it alongside
# development-phase lanes (memory-incident discipline).
#
# Usage:
#   scripts/schema-freeze-sweep.sh [--label L] [--no-build] [--help]
#
#   --label L       run label in the summary header. Default:
#                   SCHEMA_FREEZE_LABEL env, else schema-freeze-r1.
#   --no-build      skip `cargo build -p dartq-cli`; ./target/debug/dartq
#                   must already exist.
#
# Environment overrides: DARTQ_BIN (default ./target/debug/dartq),
# DARTQ_SWEEP_SDK (default $DART_SDK, else ~/fvm/default/bin/cache/dart-sdk —
# passed to the daemon as --sdk), DARTQ_SWEEP_TIMEOUT (whole RPC conversation
# budget in seconds, default 240).
#
# Requires: cargo, jq, python3, and the SDK's dart (the machine-parity leg).
# No network.
#
# Exit codes: 0 every face passed; 1 at least one face mismatch — the frozen
# v1 surface drifted; 2 environment error — missing tool, build failure, or
# the daemon could not be driven (timeout, EOF, protocol garbage).

set -euo pipefail
export LC_ALL=C
cd "$(dirname "$0")/.." || exit 2

DEFAULT_LABEL="schema-freeze-r1"
label="${SCHEMA_FREEZE_LABEL:-$DEFAULT_LABEL}"
no_build=0
while [ $# -gt 0 ]; do
  case "$1" in
  --label)
    [ $# -ge 2 ] || {
      echo "schema-freeze-sweep: --label needs a value" >&2
      exit 2
    }
    label="$2"
    shift
    ;;
  --no-build) no_build=1 ;;
  -h | --help)
    sed -n '2,/^set -euo pipefail/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
    exit 0
    ;;
  *)
    echo "schema-freeze-sweep: unknown argument '$1' (see scripts/schema-freeze-sweep.sh --help)" >&2
    exit 2
    ;;
  esac
  shift
done

# --- preflight ---------------------------------------------------------------
for tool in jq python3 cargo; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "schema-freeze-sweep: $tool is required but not on PATH" >&2
    exit 2
  }
done

DARTQ_BIN="${DARTQ_BIN:-./target/debug/dartq}"
SDK="${DARTQ_SWEEP_SDK:-${DART_SDK:-$HOME/fvm/default/bin/cache/dart-sdk}}"
DART="$SDK/bin/dart"
TIMEOUT_SECS="${DARTQ_SWEEP_TIMEOUT:-240}"

if [ "$no_build" -eq 0 ]; then
  echo "== [$label] building ./target/debug/dartq"
  cargo build -p dartq-cli || {
    echo "schema-freeze-sweep: cargo build -p dartq-cli failed" >&2
    exit 2
  }
fi
[ -x "$DARTQ_BIN" ] || {
  echo "schema-freeze-sweep: $DARTQ_BIN not found — build it first: cargo build -p dartq-cli" >&2
  exit 2
}
[ -d "$SDK" ] || {
  echo "schema-freeze-sweep: SDK root not found at $SDK — set DARTQ_SWEEP_SDK or DART_SDK" >&2
  exit 2
}
[ -x "$DART" ] || {
  echo "schema-freeze-sweep: oracle dart not found at $DART (the machine-parity face drives it)" >&2
  exit 2
}

# --- fixtures ----------------------------------------------------------------
# One package, two probes: main.dart carries three uri_does_not_exist rows on
# one deeply nested path (the depth + repetition is what makes the daemon's
# full-tier short-id economics fire deterministically), fixme.dart carries
# unnecessary_new hits so the fix face answers a non-empty patch set — the
# lint set is options-governed (no options file = no lints enabled), so the
# options file below enables exactly the rule fixme.dart trips.
work="$(mktemp -d "${TMPDIR:-/tmp}/dartq-schema-sweep.XXXXXX")"
cleanup() {
  rm -rf "$work"
}
trap cleanup EXIT

fx="$work/fixture"
deep="lib/deeply/nested/freeze/probe"
mkdir -p "$fx/.dart_tool" "$fx/$deep"
cat >"$fx/pubspec.yaml" <<'EOF'
name: sweep
version: 0.1.0
environment:
  sdk: ^3.5.0
EOF
cat >"$fx/pubspec.lock" <<'EOF'
packages: {}
sdks:
  dart: ">=3.5.0 <4.0.0"
EOF
cat >"$fx/.dart_tool/package_config.json" <<'EOF'
{"configVersion": 2, "packages": [{"name": "sweep", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub"}
EOF
# The fix face's patch source: without an options file the lint enable set is
# empty, so enable exactly the rule the probe source trips (the in-crate fix
# tests use the same minimal options shape).
cat >"$fx/analysis_options.yaml" <<'EOF'
linter:
  rules:
    - unnecessary_new
EOF
cat >"$fx/$deep/main.dart" <<'EOF'
import 'missing.dart';
import 'missing.dart';
import 'missing.dart';

void main() {}
EOF
cat >"$fx/lib/fixme.dart" <<'EOF'
var a = new Object();

void main() {
  var b = new Object();
}
EOF
# The daemon answers paths under the workspace root the way the OS resolves
# it — canonical (macOS symlinks /var → /private/var, doubled slashes
# flattened) — so the expected path must be canonical too, not the raw
# mktemp spelling.
probe_file="$(cd "$fx/$deep" && pwd -P)/main.dart"

# --- face runner --------------------------------------------------------------
face_fail=0
any_fail=0
face() {
  face_fail=0
  printf '\n== face %s\n' "$1"
}
chk() {
  local desc="$1" out
  shift
  if out="$("$@" 2>&1)"; then
    printf '   ok   %s\n' "$desc"
  else
    printf '   FAIL %s\n' "$desc"
    [ -n "$out" ] && printf '        %s\n' "$out"
    face_fail=1
  fi
  return 0
}
end_face() {
  if [ "$face_fail" -ne 0 ]; then
    any_fail=1
    printf '   -- face FAILED\n'
  else
    printf '   -- face PASS\n'
  fi
}

# jq helpers: pass when the filter yields the expected value / `true`.
jq_eq() { # <file> <filter> <expected>
  local got
  got="$(jq -r "$2" "$1")" && [ "$got" = "$3" ] || {
    echo "jq '$2' => got '${got:-<error>}', want '$3'"
    return 1
  }
}
jq_true() { # <file> <filter> — passes when the filter yields true
  local got
  got="$(jq "$2" "$1")" && [ "$got" = "true" ] || {
    echo "jq '$2' => ${got:-<error>}"
    return 1
  }
}
# Passes when every line of the file matches the ERE (zero non-matching lines).
all_match() { # <file> <ere>
  [ "$(grep -cvE "$2" "$1")" -eq 0 ]
}

# --- face: cli-check-machine-parity ---------------------------------------------
# The CLI's machine face is retired (`--format machine` / `--verbosity` are
# usage errors, pinned below); the frozen machine contract is verified through
# the json face: dartq's envelope rows are re-encoded to the oracle's machine
# rows and byte-compared against a live `dart analyze --format=machine` leg
# over the same probe FILE. Byte parity is the bar — any drift in casing,
# escaping, locators, or message text fails the face. Both legs scope to the
# probe file (not the fixture dir): the check face is options-free while the
# oracle reads analysis_options.yaml, so the dir view differs by design (the
# options-enabled lint rows); the file view is the shared diagnostic set.
machine_out="$work/machine-built.txt"
oracle_out="$work/machine-oracle.txt"
# Re-encoder: json envelope (stdin) -> oracle machine rows (stdout). The json
# fields carry everything the machine row needs: severity/type verbatim
# (uppercase in both), the machine CODE spelling = json code uppercased
# (emitMachineFormat prints `error.code?.toUpperCase()`), 1-based
# line/column, length = end.offset - start.offset, and the FILE/MESSAGE
# escaping set (backslash, pipe, LF, CR).
cat >"$work/machine_from_json.py" <<'PYEOF'
#!/usr/bin/env python3
import json
import sys

def esc(field):
    return (field.replace("\\", "\\\\").replace("|", "\\|")
            .replace("\n", "\\n").replace("\r", "\\r"))

doc = json.load(sys.stdin)
if doc.get("version") != 1:
    sys.exit("machine_from_json: unsupported envelope version %r" % (doc.get("version"),))
for diag in doc.get("diagnostics", []):
    try:
        rng = diag["location"]["range"]
        print("|".join([
            diag["severity"],
            diag["type"],
            diag["code"].upper(),
            esc(diag["location"]["file"]),
            str(rng["start"]["line"]),
            str(rng["start"]["column"]),
            str(rng["end"]["offset"] - rng["start"]["offset"]),
            esc(diag["problemMessage"]),
        ]))
    except (KeyError, TypeError) as error:
        sys.exit("machine_from_json: malformed diagnostic (%s): %r" % (error, diag))
PYEOF
face cli-check-machine-parity
rc=0
"$DARTQ_BIN" check --format json "$probe_file" >"$work/probe.json" 2>"$work/probe.err" || rc=$?
chk "error fixture exits 3 (ERRORS)" [ "$rc" -eq 3 ]
chk "envelope carries three diagnostic rows (one per directive)" \
  jq_eq "$work/probe.json" '.diagnostics | length' 3
if python3 "$work/machine_from_json.py" <"$work/probe.json" >"$machine_out" 2>"$work/probe.build.err"; then
  chk "three machine rows reconstructed" [ "$(wc -l <"$machine_out" | tr -d ' ')" -eq 3 ]
  chk "rows are ERROR|COMPILE_TIME_ERROR|URI_DOES_NOT_EXIST" \
    all_match "$machine_out" '^ERROR\|COMPILE_TIME_ERROR\|URI_DOES_NOT_EXIST\|'
  chk "first row pins locator + oracle message (the URI literal: 1|8, length 14)" \
    grep -Eq "^ERROR\|COMPILE_TIME_ERROR\|URI_DOES_NOT_EXIST\|.*main\.dart\|1\|8\|14\|Target of URI doesn't exist: 'missing.dart'\." "$machine_out"
else
  printf '   FAIL machine-row reconstruction from the json envelope\n'
  sed 's/^/        /' "$work/probe.build.err"
  face_fail=1
fi
orc=0
"$DART" analyze --format=machine --no-fatal-warnings "$probe_file" >"$oracle_out" 2>"$work/oracle.err" || orc=$?
chk "oracle machine leg exits inside the issue tiers (got $orc, errors mean 3)" [ "$orc" -le 3 ]
chk "reconstructed rows are byte-identical to the oracle machine output" \
  cmp -s "$oracle_out" "$machine_out"

# The retired faces: the machine value and the verbosity flag are usage
# errors on the CLI now (the freeze that #2606 cast).
rc=0
"$DARTQ_BIN" check --format machine "$fx" >/dev/null 2>"$work/retired-machine.err" || rc=$?
chk "--format machine is retired: usage error 64" [ "$rc" -eq 64 ]
rc=0
"$DARTQ_BIN" check --format json --verbosity concise "$fx" >/dev/null 2>&1 || rc=$?
chk "--verbosity is retired: usage error 64" [ "$rc" -eq 64 ]

clean_dir="$work/clean"
mkdir -p "$clean_dir"
printf 'void main() {}\n' >"$clean_dir/main.dart"
rc=0
"$DARTQ_BIN" check --format json "$clean_dir" >"$work/clean.txt" 2>/dev/null || rc=$?
chk "clean fixture exits 0" [ "$rc" -eq 0 ]
chk "clean json face prints exactly the empty-diagnostics envelope" \
  [ "$(cat "$work/clean.txt")" = '{"version":1,"diagnostics":[]}' ]
end_face

# --- face: cli-check-json ------------------------------------------------------
json_out="$work/check.json"
face cli-check-json
rc=0
"$DARTQ_BIN" check --format json "$fx" >"$json_out" 2>/dev/null || rc=$?
chk "error fixture exits 3 (ERRORS)" [ "$rc" -eq 3 ]
chk "single-line envelope" [ "$(wc -l <"$json_out" | tr -d ' ')" -eq 1 ]
chk "version is the frozen 1 (the envelope's version alias)" jq_eq "$json_out" '.version' 1
chk "three diagnostic rows" jq_eq "$json_out" '.diagnostics | length' 3
chk "rows carry the frozen key set" jq_true "$json_out" \
  '[.diagnostics[] | has("code") and has("severity") and has("type") and has("location") and has("problemMessage") and has("correctionMessage")] | all'
chk "row code/severity/type spellings" jq_true "$json_out" \
  '[.diagnostics[] | .code == "uri_does_not_exist" and .severity == "ERROR" and .type == "COMPILE_TIME_ERROR"] | all'
chk "location names the probe file with the oracle 1-based range" jq_true "$json_out" \
  '([.diagnostics[] | .location.file | endswith("probe/main.dart")] | all) and .diagnostics[0].location.range.start.line == 1 and .diagnostics[0].location.range.start.column == 8 and .diagnostics[0].location.range.start.offset == 7'
end_face

# --- daemon conversation -------------------------------------------------------
# The method names below are the daemon's served request family (see
# crates/dartq-daemon/src/wire.rs for the row/envelope shapes and
# crates/dartq-daemon/src/commands.rs `METHODS` for the family): there is no
# lifecycle handshake — the session answers from frame one and EOF is the
# only end signal. Ids are referenced by the bash assertions below through
# body_<id>.json.
cat >"$work/steps.json" <<'EOF'
[
  {"id": 1, "method": "dartq/index", "params": {}},
  {"id": 2, "method": "dartq/check", "params": {"paths": ["lib/deeply/nested/freeze/probe/main.dart"], "verbosity": "minimal"}},
  {"id": 3, "method": "dartq/check", "params": {"paths": ["lib/deeply/nested/freeze/probe/main.dart"], "verbosity": "compact"}},
  {"id": 4, "method": "dartq/check", "params": {"paths": ["lib/deeply/nested/freeze/probe/main.dart"], "verbosity": "full"}},
  {"id": 5, "method": "dartq/fix", "params": {"paths": ["lib/fixme.dart"], "verbosity": "compact"}},
  {"id": 6, "method": "dartq/fix", "params": {"paths": ["lib/fixme.dart"], "verbosity": "full"}},
  {"id": 7, "method": "dartq/fix", "params": {"paths": ["lib/fixme.dart"], "agent_id": "sweep-probe", "context": "default", "since": 1}},
  {"id": 8, "method": "dartq/assists", "params": {"path": "lib/fixme.dart", "verbosity": "compact"}},
  {"id": 9, "method": "dartq/assists", "params": {"path": "lib/fixme.dart", "since": 1}},
  {"id": 10, "method": "dartq/overlay.set", "params": {"agent_id": "sweep-probe", "files": {}}},
  {"id": 11, "method": "dartq/overlay.clear", "params": {"agent_id": "sweep-probe"}}
]
EOF

cat >"$work/rpc_drive.py" <<'PYEOF'
#!/usr/bin/env python3
"""Drives one scripted dartq-RPC conversation against `dartq daemon` over stdio.

Records every inbound frame (responses and server-initiated notifications) as
one JSON object per line — {"from_server": <frame>} — to the output path, so
the bash layer asserts shapes with jq. Exit 0 on a completed conversation;
exit 2 on transport failure (spawn, timeout, EOF, bad framing).
"""
import json
import os
import select
import subprocess
import sys
import time
from pathlib import Path

def die(msg):
    print("rpc_drive: " + msg, file=sys.stderr)
    sys.exit(2)

def frame(obj):
    body = json.dumps(obj, separators=(",", ":")).encode()
    return b"Content-Length: %d\r\n\r\n%s" % (len(body), body)

class Reader:
    """One stdout's framed reader; leftover bytes between frames survive."""

    def __init__(self, fd):
        self.fd = fd
        self.buf = b""

    def read_frame(self, deadline, what):
        while b"\r\n\r\n" not in self.buf:
            if time.monotonic() > deadline:
                die("timeout waiting for " + what)
            ready, _, _ = select.select([self.fd], [], [], 0.5)
            if not ready:
                continue
            chunk = os.read(self.fd, 65536)
            if not chunk:
                die("daemon closed stdout while waiting for " + what)
            self.buf += chunk
        head, rest = self.buf.split(b"\r\n\r\n", 1)
        length = None
        for line in head.split(b"\r\n"):
            key, _, value = line.partition(b":")
            if key.strip().lower() == b"content-length":
                length = int(value.strip())
        if length is None:
            die("frame without Content-Length: " + head.decode(errors="replace"))
        while len(rest) < length:
            if time.monotonic() > deadline:
                die("timeout reading " + what + " body")
            ready, _, _ = select.select([self.fd], [], [], 0.5)
            if not ready:
                continue
            chunk = os.read(self.fd, length - len(rest))
            if not chunk:
                die("daemon closed stdout mid-body")
            rest += chunk
        self.buf = rest[length:]
        try:
            return json.loads(rest[:length])
        except json.JSONDecodeError as error:
            die("undecodable frame: " + str(error))

def main():
    if len(sys.argv) != 6:
        die("usage: rpc_drive.py <dartq> <fixture> <sdk> <steps.json> <out.jsonl>")
    dartq, fixture, sdk, steps_path, out_path = sys.argv[1:6]
    deadline = time.monotonic() + int(os.environ.get("DARTQ_SWEEP_TIMEOUT", "240"))
    steps = json.loads(Path(steps_path).read_text())
    fx = Path(fixture).resolve()
    err = open(fx.parent / "daemon.err", "wb")
    proc = subprocess.Popen(
        [dartq, "daemon", str(fx), "--sdk", sdk],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=err,
    )
    out = open(out_path, "a")
    reader = Reader(proc.stdout.fileno())
    try:
        for step in steps:
            if "notify" in step:
                payload = frame({
                    "jsonrpc": "2.0", "method": step["notify"],
                    "params": step.get("params", {}),
                })
                try:
                    proc.stdin.write(payload)
                    proc.stdin.flush()
                except BrokenPipeError:
                    die("daemon closed stdin before " + step["notify"])
                continue
            params = step.get("params", {})
            try:
                proc.stdin.write(frame({
                    "jsonrpc": "2.0", "id": step["id"],
                    "method": step["method"], "params": params,
                }))
                proc.stdin.flush()
            except BrokenPipeError:
                die("daemon closed stdin before " + step["method"])
            while True:
                inbound = reader.read_frame(deadline,
                                            "response to id %s" % step["id"])
                out.write(json.dumps({"from_server": inbound}) + "\n")
                out.flush()
                if inbound.get("id") == step["id"]:
                    break
        # EOF is the session's only end signal (there is no shutdown method):
        # closing stdin must end the daemon with EXIT_CLEAN (0).
        proc.stdin.close()
        try:
            proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            proc.kill()
            die("daemon ignored EOF; killed")
        if proc.returncode != 0:
            die("daemon exited %s after EOF (protocol failure)" % proc.returncode)
    finally:
        out.close()
        err.close()
        if proc.poll() is None:
            proc.kill()
            proc.wait()

if __name__ == "__main__":
    main()
PYEOF

rpc_jsonl="$work/rpc.jsonl"
echo
echo "== [$label] driving the daemon session (budget ${TIMEOUT_SECS}s)"
if ! DARTQ_SWEEP_TIMEOUT="$TIMEOUT_SECS" python3 "$work/rpc_drive.py" \
  "$DARTQ_BIN" "$fx" "$SDK" "$work/steps.json" "$rpc_jsonl" \
  2>"$work/transport.err"; then
  echo "schema-freeze-sweep: daemon transport failed (environment error); stderr:" >&2
  sed 's/^/    /' "$work/transport.err" >&2
  if [ -f "$work/daemon.err" ]; then
    sed 's/^/    daemon: /' "$work/daemon.err" >&2
  fi
  exit 2
fi

# Materialize one body file per scripted id; a missing or duplicated response
# is itself a face failure.
for id in 1 2 3 4 5 6 7 8 9 10 11; do
  count="$(jq -s --argjson id "$id" \
    '[.[] | select(.from_server.id == $id)] | length' "$rpc_jsonl")"
  if [ "$count" != "1" ]; then
    echo "schema-freeze-sweep: expected exactly one response for id $id, found $count" >&2
    exit 1
  fi
  jq -s --argjson id "$id" \
    '[.[] | select(.from_server.id == $id)] | first | .from_server' \
    "$rpc_jsonl" >"$work/body_$id.json"
done

# --- face: rpc-session-ready ----------------------------------------------------
# The no-handshake face: the first request of the conversation is a real one
# and answers normally (see crates/dartq-daemon/src/server.rs `run()`).
body="$work/body_1.json"
face rpc-session-ready
chk "the first request (dartq/index) answers without error" jq_true "$body" '.error == null'
chk "the answer is the frozen index envelope (v + the four tallies)" jq_true "$body" \
  '(.result | keys | sort) == ["files", "libraries", "refs", "symbols", "v"]'
chk "v is the frozen 1" jq_eq "$body" '.result.v' 1
end_face

# --- face: rpc-overlay-ack -----------------------------------------------------
face rpc-overlay-ack
chk "overlay.set answers exactly {v: 1}" jq_true "$work/body_10.json" \
  '(.result | keys) == ["v"] and .result.v == 1'
chk "overlay.clear answers exactly {v: 1}" jq_true "$work/body_11.json" \
  '(.result | keys) == ["v"] and .result.v == 1'
end_face

# --- face: rpc-check-tiers -----------------------------------------------------
face rpc-check-tiers
body="$work/body_2.json"
chk "minimal: envelope is exactly {v, diags}" jq_true "$body" \
  '(.result | keys | sort) == ["diags", "v"]'
chk "minimal: v is 1" jq_eq "$body" '.result.v' 1
chk "minimal: rows carry only the red line (code/file/line/col/len)" jq_true "$body" \
  '[.result.diags[] | (keys | sort) == ["code", "col", "file", "len", "line"]] | all'
chk "minimal: three uri_does_not_exist rows on the probe file" jq_true "$body" \
  '(.result.diags | length) == 3 and ([.result.diags[].code] | all(. == "uri_does_not_exist"))'
chk "minimal: paths stay unshortened" jq_eq "$body" '.result.diags[0].file' "$probe_file"

body="$work/body_3.json"
chk "compact: rows carry exactly the frozen 7-field set" jq_true "$body" \
  '[.result.diags[] | (keys | sort) == ["code", "col", "file", "len", "line", "msg", "sev"]] | all'
chk "compact: lowercase sev spelling and non-empty msg" jq_true "$body" \
  '[.result.diags[] | .sev == "error" and (.msg | length > 0)] | all'
chk "compact: no session economics on the answer" jq_true "$body" \
  '(.result | keys | sort) == ["diags", "v"]'

body="$work/body_4.json"
chk "full: the short-id table rides the answer" jq_true "$body" \
  '(.result | has("symbols")) and (.result.symbols | keys | length >= 1)'
chk "full: table keys are the frozen sN form" jq_true "$body" \
  '[.result.symbols | keys[] | test("^s[0-9]+$")] | all'
chk "full: the table's value is the substituted-out path" jq_eq "$body" '.result.symbols.s1' "$probe_file"
chk "full: rows reference the table instead of spelling paths" jq_true "$body" \
  '([.result.diags[].file] | unique) == ["s1"]'
chk "full: rows keep the compact 7-field set" jq_true "$body" \
  '[.result.diags[] | (keys | sort) == ["code", "col", "file", "len", "line", "msg", "sev"]] | all'
end_face

# --- face: rpc-fix-answer ------------------------------------------------------
# Re-frozen 2026-10-18 (rcP adjudication): the fix face's v1 surface here is
# the cross-file document set — the hit file first, the governing options
# file second (the contract rpc_schema.rs
# `dartq_fix_documents_validate_with_their_provenance` has pinned since the
# #631/#632 provenance window), and the provenance spellings are the three
# builtin families of the shared fix assembly (dartq-commands fix.rs:
# `builtin-lint` / `builtin-semantic` / `builtin-ignore`). The semantic rows
# ride the SDK face the daemon drives with, so the answer is wider than a
# bare CLI pass without `--sdk`. Additive-only: no field moved, v1 stands.
face rpc-fix-answer
body="$work/body_5.json"
chk "compact: the intake answer is {v, files}" jq_true "$body" \
  '(.result | keys | sort) == ["files", "v"]'
chk "compact: v is 1" jq_eq "$body" '.result.v' 1
chk "compact: the cross-file document set (hit file first, options second)" jq_true "$body" \
  '(.result.files | length) == 2 and (.result.files[0].file | endswith("fixme.dart")) and (.result.files[1].file | endswith("analysis_options.yaml"))'
chk "compact: file rows are exactly {file, v, edits}" jq_true "$body" \
  '[.result.files[] | (keys | sort) == ["edits", "file", "v"] and .v == 1] | all'
chk "compact: edits ride the red-line floor plus the anchor" jq_true "$body" \
  '[.result.files[].edits[] | (keys | sort) == ["anchor", "len", "off", "src", "text"]] | all'
chk "compact: builtin provenance spellings and a located anchor on every edit" jq_true "$body" \
  '[.result.files[].edits[] | (.src == "builtin-lint" or .src == "builtin-semantic" or .src == "builtin-ignore") and (.anchor | has("line") and has("col"))] | all'
chk "compact: the probe lint deletions are present and builtin-lint" jq_true "$body" \
  '[.result.files[0].edits[] | select(.src == "builtin-lint" and .text == "")] | length >= 1'

body="$work/body_6.json"
chk "full: edits add the builtin rule/msg columns" jq_true "$body" \
  '[.result.files[].edits[] | (keys | sort) == ["anchor", "len", "msg", "off", "rule", "src", "text"]] | all'
chk "full: rule and msg are strings" jq_true "$body" \
  '[.result.files[].edits[] | (.rule | type == "string") and (.msg | type == "string")] | all'
end_face

# --- face: rpc-fix-rejects -----------------------------------------------------
face rpc-fix-rejects
body="$work/body_7.json"
chk "agentId/context/since answer INVALID_PARAMS (-32602)" jq_eq "$body" '.error.code' -32602
chk "the rejection names the disk-view rule and the method" jq_true "$body" \
  '.error.message | test("answers the disk view") and test("dartq/fix")'
chk "no answer payload rides the rejection" jq_true "$body" '.result == null'
end_face

# --- face: rpc-assists ---------------------------------------------------------
face rpc-assists
body="$work/body_8.json"
chk "the document is exactly {file, v, assists}" jq_true "$body" \
  '(.result | keys | sort) == ["assists", "file", "v"]'
chk "v is 1" jq_eq "$body" '.result.v' 1
chk "the document names the probed file" jq_true "$body" '.result.file | endswith("fixme.dart")'
chk "rows (if any) carry the msg/src/edits tuple" jq_true "$body" \
  '[.result.assists[] | ((keys | sort) - ["priority"]) == ["edits", "msg", "src"]] | all'
end_face

# --- face: rpc-assists-rejects -------------------------------------------------
face rpc-assists-rejects
body="$work/body_9.json"
chk "a session field answers INVALID_PARAMS (-32602)" jq_eq "$body" '.error.code' -32602
chk "the rejection names the disk-view rule and the method" jq_true "$body" \
  '.error.message | test("answers the disk view") and test("dartq/assists")'
end_face

# --- summary -------------------------------------------------------------------
echo
echo "== schema-freeze-sweep summary"
printf '  %-16s %s\n' "label" "$label"
printf '  %-16s %s\n' "dartq" "$DARTQ_BIN"
printf '  %-16s %s\n' "sdk" "$SDK"
printf '  %-16s %s\n' "fixture" "$fx (removed; re-run for a fresh one)"
if [ "$any_fail" -eq 0 ]; then
  echo "schema-freeze-sweep: VERDICT PASS (schema v1 frozen surface intact across faces)"
  exit 0
fi
echo "schema-freeze-sweep: VERDICT FAIL (the frozen v1 surface drifted — see the FAILED faces above)"
exit 1
