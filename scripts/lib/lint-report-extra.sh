# lint-report-extra.sh — sourced by lint-gate.sh; the additive #1998 report
# faces (instance-count axis + pre-filter severity tally). Requires jq
# (the caller's preflight already pins it). Schema contract: the two faces
# are NEW top-level report.json keys (`count_mismatches`,
# `severity_stats_prefilter`) and two report.md sections; every pre-existing
# key/section keeps its value and semantics — consumers of the old schema
# are unaffected.

_LINT_REPORT_EXTRA_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# lint_report_extra_faces <report.json> <raw-outputs.json>
#
# Computes the two additive faces; prints them as one JSON object on stdout.
# Empty output = failure (unreadable inputs, malformed schema) — callers
# must treat that as a gate error, never as "no mismatches".
lint_report_extra_faces() { # <report.json> <raw-outputs.json>
  jq -s -f "$_LINT_REPORT_EXTRA_DIR/lint_report_extra.jq" "$1" "$2"
}

# lint_report_enrich_json <report.json> <extra-faces-json>
#
# Merges the extra faces into report.json in place (temp file + mv, so a
# failed merge never leaves a truncated report behind).
lint_report_enrich_json() { # <report.json> <extra-faces-json>
  local report="$1" extra="$2" tmp
  tmp="$(mktemp "$report.enrich.XXXXXX")" || return 1
  jq --argjson extra "$extra" '. + $extra' "$report" >"$tmp" ||
    { rm -f "$tmp"; return 1; }
  mv "$tmp" "$report" || { rm -f "$tmp"; return 1; }
}

# lint_report_md_extra <extra-faces-json>
#
# Renders the two additive markdown sections (pre-filter severity tally
# first — the primary per-face tally — then the instance-count mismatches)
# for insertion ahead of the harness's post-filter "## Jaccard per severity"
# section.
lint_report_md_extra() { # <extra-faces-json>
  local extra="$1" out rows line
  out='## Jaccard per severity (pre-filter — primary tally)

Computed over the raw machine outputs of both sides: all severities, before
the gate'"'"'s severity/type/codes filters. The harness table below this
section is the post-filter gate face (under a severity-filtered run its
off-tier rows are empty-population artifacts).

| severity | jaccard | intersect | union | oracle-only | dartq-only |
| --- | --- | --- | --- | --- | --- |
'
  rows="$(jq -r '.severity_stats_prefilter[] | [.severity, .jaccard, .intersection, .union, .oracle_only, .dartq_only] | @tsv' <<<"$extra")"
  if [ -z "$rows" ]; then
    out+='(none)
'
  else
    while IFS="$(printf '\t')" read -r sev jac inter uni oonly donly; do
      printf -v line '| %s | %.3f | %s | %s | %s | %s |\n' \
        "$sev" "$jac" "$inter" "$uni" "$oonly" "$donly"
      out+="$line"
    done <<<"$rows"
  fi
  out+='
## Instance-count mismatches (matched triples, gate face)

Matched (severity, code, file) triples — present on both sides post-filter —
whose per-side instance counts differ; invisible to the set-based Jaccard.
delta = dartq - oracle.

| severity | code | file | oracle | dartq | delta |
| --- | --- | --- | --- | --- | --- |
'
  rows="$(jq -r '.count_mismatches[] | [.severity, .code, .file, .oracle_count, .dartq_count, .delta] | @tsv' <<<"$extra")"
  if [ -z "$rows" ]; then
    out+='(none)
'
  else
    while IFS="$(printf '\t')" read -r sev code file oc dc delta; do
      printf -v line '| %s | %s | %s | %s | %s | %+d |\n' \
        "$sev" "$code" "$file" "$oc" "$dc" "$delta"
      out+="$line"
    done <<<"$rows"
  fi
  printf '%s' "$out"
}
