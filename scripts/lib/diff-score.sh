# diff-score.sh — sourced by the differential gate scripts (m0/m1/m2/m3/
# sem2/lint/sdk-matrix). Single source of truth for the 100% release-caliber
# scoring (ticket #1293): a gate tier PASSES only when the run's flat diff
# lists (report.json oracle_only/dartq_only) are empty after the known-diffs
# exemption — every remaining entry is unattributed divergence and fails the
# gate. The raw (pre-exemption) Jaccard is telemetry only, never a gate.
#
# The flat plane is the gate tier: every consumer gate runs exactly one
# --severity-filter, so report.json's flat diff lists carry exactly that
# tier's entries (they carry no severity field of their own).
#
# Requires jq (every consumer gate already pins that in its own preflight).

# diff_gate_classify <report.json> <known-diffs.json>
#
# Classifies every flat diff entry in <report.json> against the known-diffs
# exemption table. Emits on stdout, one TSV line per entry:
#
#   EXEMPTED|UNATTRIBUTED <TAB> oracle_only|dartq_only <TAB> <code> <TAB> <file>
#
# followed by exactly one final line:
#
#   SUMMARY <TAB> <exempted-count> <TAB> <unattributed-count>
#
# The exemption match mirrors the harness's entry_is_exempt (tests/harness/
# src/report.rs) exactly: code equal, file equal, and message_pattern empty
# (wildcard) or a substring of the entry's sample message. Both sides are
# compared label-free: diff entries carry lowercase entry-key codes and the
# table's codes are lowercased at comparison (the harness lowercases at
# load), while a diff entry's file — merge-label-prefixed for a multi-corpus
# run (`m0/...`, or the full canonical path when corpus basenames collide) —
# is resolved back to its corpus-relative form. That resolution reads the
# sibling raw-outputs.json (the run's own `oracle.corpora[].label`, run order
# preserved) and strips the first `label/` prefix that matches, the same
# first-match-in-run-order rule as the harness's locate_corpus. A missing
# raw-outputs.json means no labels (the single-corpus key shape). A missing
# known-diffs file is an empty table (same as the harness's loader); a
# malformed table or raw file makes the output empty (jq fails), which
# callers must treat as a gate error — a silently ignored table would pass
# divergence.
diff_gate_classify() { # <report.json> <known-diffs.json>
  local kd_file="$2" raw_file
  [ -f "$kd_file" ] || kd_file=/dev/null
  raw_file="$(dirname "$1")/raw-outputs.json"
  [ -f "$raw_file" ] || raw_file=/dev/null
  jq -r --slurpfile kd "$kd_file" --slurpfile raw "$raw_file" '
    def strip_label($labels):
      . as $f |
      ([$labels[] | . as $l | select($f | startswith($l + "/"))] | .[0]) as $label |
      if $label == null then $f else $f[(1 + ($label | length)):] end;
    ($kd[0].entries // []) as $known |
    (($raw[0].oracle.corpora // []) | [.[] | .label // empty]) as $labels |
    ([.oracle_only[] | . + {direction: "oracle_only"}] +
     [.dartq_only[]  | . + {direction: "dartq_only"}]) |
    map(. as $e |
      ($e.file | strip_label($labels)) as $file |
      ($known | any(. as $k |
        (($k.code | ascii_downcase) == $e.code) and ($k.file == $file) and
        ((($k.message_pattern // "") == "") or
         ($e.sample_message | contains($k.message_pattern // ""))))) as $exempt |
      [(if $exempt then "EXEMPTED" else "UNATTRIBUTED" end),
        $e.direction, $e.code, $e.file]) |
    ([.[] | select(.[0] == "EXEMPTED")] | length) as $ex_count |
    ([.[] | select(.[0] == "UNATTRIBUTED")] | length) as $un_count |
    . + [["SUMMARY", ($ex_count | tostring), ($un_count | tostring)]] |
    .[] | @tsv' "$1" 2>/dev/null
}
