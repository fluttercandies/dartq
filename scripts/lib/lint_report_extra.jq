# lint_report_extra.jq — the additive #1998 report faces for the lint gate.
# Input: a slurped array of exactly two documents:
#   .[0] = the harness run's report.json
#   .[1] = the run's raw-outputs.json (both sides' complete machine
#          diagnostics, all severities, before any gate filter)
# Emits one object with two faces; the caller merges it into report.json
# with `. + $extra` — additive only, no existing key is read or replaced.
#
#   count_mismatches: matched (severity, code, file) triples — present on
#     BOTH sides post-filter (report.json filters.*.entries) — whose
#     per-side instance counts differ. delta = dartq − oracle. The gate is
#     set-based, so these rows are invisible to every Jaccard face; this
#     channel makes the instance-count axis measurable.
#
#   severity_stats_prefilter: per-severity entry-set Jaccard over the RAW
#     machine outputs (all severities, before the gate's severity/type/
#     codes filters) — the real per-face numbers the post-filter `severities`
#     array cannot show under a severity-filtered run (there an off-tier
#     row degenerates to jaccard 1.000 over an empty intersection/union).
#     Row shape mirrors the harness's `severities` entries.

def triples: [.corpora[].diagnostics[] | [.severity, .code, .file]] | unique;

(.[0].filters.dartq.entries
  | map({key: ([.severity, .code, .file] | tojson), value: .count})
  | from_entries) as $dmap
| [ .[0].filters.oracle.entries[]
    | . as $e
    | ($dmap[([$e.severity, $e.code, $e.file] | tojson)]) as $dc
    | select($dc != null)
    | select($dc != $e.count)
    | {
        severity: $e.severity,
        code: $e.code,
        file: $e.file,
        oracle_count: $e.count,
        dartq_count: $dc,
        delta: ($dc - $e.count)
      }
  ] as $mismatches
| (.[1].oracle | triples) as $o
| (.[1].dartq | triples) as $d
| ([ $o[], $d[] ]
    | group_by(.[0])
    | map(
        (.[0][0]) as $sev
        | [$o[] | select(.[0] == $sev)] as $os
        | [$d[] | select(.[0] == $sev)] as $ds
        | ($os - ($os - $ds)) as $inter
        | (($os + $ds) | unique | length) as $uni
        | {
            severity: $sev,
            jaccard: (if $uni == 0 then 1.0 else (($inter | length) / $uni) end),
            intersection: ($inter | length),
            union: $uni,
            oracle_only: (($os - $ds) | length),
            dartq_only: (($ds - $os) | length)
          }
      )
    | sort_by(
        if .severity == "ERROR" then 0
        elif .severity == "WARNING" then 1
        elif .severity == "INFO" then 2
        else 3 end,
        .severity)
  ) as $prefilter
| {count_mismatches: $mismatches, severity_stats_prefilter: $prefilter}
