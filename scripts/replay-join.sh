#!/usr/bin/env bash
# replay-join.sh — N2 known-diffs re-adjudication joiner (goal #1301).
#
# Joins the FRESH dartq-vs-oracle diff sets of the three serial-window
# differential legs against the frozen evidence table
# (tests/corpus/regressions/known-diffs.json) and writes
# $REPLAY_DIR/replay-results.json (schema known-diffs-replay-v1): one verdict
# row per table entry — already-fixed | still-red | oracle-ambiguous.
# Retirement decisions come ONLY from this artifact; the serial gate checks
# its mtime freshness and its known-diffs sha256 pin externally.
#
# Legs consumed (produced by the serial window's N2 phase, never by this
# script):
#   $REPLAY_DIR/m0-merged/report.json + raw-outputs.json
#       ./target/release/harness run --corpus-dir tests/corpus/m0
#         --corpus-dir tests/corpus/m0-syntax
#         --corpus-dir tests/corpus/pub-null-sections
#         --corpus-dir tests/corpus/pub-universe-shadow
#         --oracle-dart "$DART_BIN"
#         --dartq-cmd "./target/release/dartq analyze --format=machine --sdk $SDK_ROOT"
#         --out $REPLAY_DIR/m0-merged
#   $REPLAY_DIR/m1/report.json + raw-outputs.json   (same shape over
#       --corpus-dir tests/corpus/m1-semantic)
#   $REPLAY_DIR/sii.oracle.machine + sii.dartq.machine  (bare `analyze
#       --format=machine` dump pair over tests/corpus/sii-probes)
#
#   The #1347 pub-nested-example / pub-corrupt-config dump pairs
#   (pub-*.oracle.machine / pub-*.dartq.machine, same directory) are
#   EVIDENCE ONLY: their corpora ride bare dumps below the run-face floor
#   or past a deliberately corrupt config, carry no known-diffs entries,
#   and are adjudicated at N2 triage — this joiner never reads them.
#
# Join caliber (the N2 rule): fresh LOWERCASE (code, corpus-relative file)
# diff sets, severity-insensitive. Harness-leg diff sets are the union of
# report.json's oracle_only + dartq_only arrays — the comparator's own diff
# output (codes already lowercase entry-key form, files label-prefixed for
# multi-corpus runs, `m0/...` and `m0-syntax/...`, unlabeled for single
# corpus). The sii leg diffs the two machine files on the same caliber.
#
# Decisions baked in (zero-questions defaults):
#   - Entry -> leg mapping by the corpus path embedded in the entry's reason
#     field (`case <hex8> [tests/corpus/<name>]`): m0, m0-syntax,
#     pub-null-sections and pub-universe-shadow -> the m0-merged leg,
#     m1-semantic -> m1,
#     sii-probes -> sii. known-diffs.json
#     has no dedicated corpus field; the reason bracket IS the corpus record.
#     An entry with a missing/unknown corpus aborts the join (exit 2) —
#     guessing a leg would poison retirement decisions (an entry citing the
#     pub-nested-example / pub-corrupt-config dump-pair corpora aborts here
#     until their leg is wired into this joiner).
#   - Label prefix resolution comes from each leg's raw-outputs.json corpora
#     sections (dir + label verbatim from the run), matched by corpus-dir
#     suffix — not from a hardcoded label convention.
#   - oracle-ambiguous: the leg's ORACLE side produced zero rows (any
#     severity) for the entry's file — the comparison proves nothing and
#     adjudicates nothing. Checked BEFORE still-red: a dartq-only row on a
#     dead-oracle file is ambiguity, not a verdict. Ambiguity for a genuinely
#     fixed file (its only diagnostic was the fixed divergence) is expected
#     and conservative — fix the environment, rerun the leg.
#   - message_pattern does not participate in the join: the N2 caliber is
#     flat (code, file); every current table entry carries an empty pattern.
#     The field is copied through verbatim for the adjudicator.
#   - Malformed input is fatal (exit 1/2), never skipped: a silently dropped
#     diff row could turn a still-red entry into a phantom already-fixed.
#   - Output is deterministic: verdict rows sorted by (case, file, code); no
#     timestamps inside the artifact (freshness is enforced externally).
#
# Usage: scripts/replay-join.sh
# Env:   REPLAY_DIR   default target/evidence/regressions-replay
#        KNOWN_DIFFS  default tests/corpus/regressions/known-diffs.json
# Exit:  0 joined | 2 preflight/input problem (nothing written) | 1 parse error

set -Eeuo pipefail
cd "$(dirname "$0")/.." || exit 2

REPLAY_DIR="${REPLAY_DIR:-target/evidence/regressions-replay}"
KNOWN_DIFFS="${KNOWN_DIFFS:-tests/corpus/regressions/known-diffs.json}"
export REPLAY_DIR KNOWN_DIFFS

exec python3 - <<'PY'
import hashlib
import json
import os
import re
import sys

REPLAY_DIR = os.environ["REPLAY_DIR"]
KNOWN_DIFFS = os.environ["KNOWN_DIFFS"]
RESULTS_PATH = os.path.join(REPLAY_DIR, "replay-results.json")
SCHEMA = "known-diffs-replay-v1"

# Corpus -> (leg, label-free) mapping fixed by the N2 leg composition.
# pub-null-sections (#1347) and pub-universe-shadow (#1377) ride the
# m0-merged harness leg; the pub-nested-example / pub-corrupt-config roots
# have bare dump pairs only (see the header) and deliberately stay unmapped.
CORPUS_TO_LEG = {
    "tests/corpus/m0": "m0-merged",
    "tests/corpus/m0-syntax": "m0-merged",
    "tests/corpus/pub-null-sections": "m0-merged",
    "tests/corpus/pub-universe-shadow": "m0-merged",
    "tests/corpus/m1-semantic": "m1",
    "tests/corpus/sii-probes": "sii",
}
HARNESS_LEGS = ("m0-merged", "m1")
SII_CORPUS = "tests/corpus/sii-probes"

# reason template: "case <hex8> [tests/corpus/<name>] oracle=N rows, ..."
REASON_CORPUS = re.compile(r"^case ([0-9a-f]+) \[([^\]]+)\]")


def die(msg, code=2):
    print(f"replay-join: {msg}", file=sys.stderr)
    sys.exit(code)


def preflight_path(path, what, kind="file"):
    if not os.path.exists(path):
        die(
            f"missing {what}: {path} — produce the leg output first, then "
            f"rerun this joiner (see scripts/serial-gates.sh phase N2 for "
            f"the exact leg commands)"
        )
    if kind == "file" and os.path.getsize(path) == 0:
        die(
            f"empty (0 bytes) {what}: {path} — the leg produced no output "
            f"(oracle or dartq side failed); rerun the leg, never join "
            f"against a dead dump"
        )


def load_json(path, what):
    try:
        with open(path, "r", encoding="utf-8") as fh:
            return json.load(fh)
    except (OSError, json.JSONDecodeError) as exc:
        die(f"cannot read {what} {path}: {exc}", 1)


def split_unescaped_pipe(line):
    # Escape-aware field split: `\|` is field content, not a separator
    # (same contract as the harness machine parser).
    fields, start, i, n = [], 0, 0, len(line)
    while i < n:
        c = line[i]
        if c == "\\":
            i += 2
            continue
        if c == "|":
            fields.append(line[start:i])
            start = i + 1
        i += 1
    fields.append(line[start:])
    return fields


def unescape_field(raw):
    out, i, n = [], 0, len(raw)
    while i < n:
        c = raw[i]
        if c != "\\":
            out.append(c)
            i += 1
            continue
        nxt = raw[i + 1] if i + 1 < n else ""
        out.append({"n": "\n", "r": "\r", "\\": "\\", "|": "|"}.get(nxt, "\\" + nxt))
        i += 2 if nxt else 1
    return "".join(out)


def parse_machine_file(path, what):
    rows = []
    try:
        with open(path, "r", encoding="utf-8") as fh:
            for idx, line in enumerate(fh.read().splitlines()):
                fields = split_unescaped_pipe(line)
                if len(fields) != 8 or fields[0] not in ("ERROR", "WARNING", "INFO"):
                    die(
                        f"{what} {path} line {idx + 1} is not an 8-field "
                        f"machine row with a known severity: {line!r}",
                        1,
                    )
                rows.append((fields[2].lower(), unescape_field(fields[3])))
    except OSError as exc:
        die(f"cannot read {what} {path}: {exc}", 1)
    except UnicodeDecodeError as exc:
        die(f"{what} {path} is not valid UTF-8: {exc}", 1)
    return rows


def sii_rel_file(path):
    # Normalize a bare-dump machine row path to corpus-relative: strip a
    # leading ./ then the corpus-dir component (handles both the
    # path-as-given and the oracle's symlink-resolved absolute form).
    p = path[2:] if path.startswith("./") else path
    marker = SII_CORPUS + "/"
    if p.startswith(marker):
        return p[len(marker):]
    at = p.rfind(marker)
    if at >= 0:
        return p[at + len(marker):]
    return p


def corpus_label(labels, corpus):
    # Resolve the run's label for a corpus by suffix-matching the section
    # dir (raw-outputs.json records canonical dirs + labels verbatim).
    hits = [
        label
        for section_dir, label in labels.items()
        if section_dir == corpus or section_dir.endswith("/" + corpus)
    ]
    if len(hits) != 1:
        die(
            f"corpus {corpus} matches {len(hits)} corpus sections in the "
            f"leg's raw outputs (expected exactly 1) — the leg artifact "
            f"does not cover this corpus; rerun the leg over the right "
            f"corpus dirs",
            1,
        )
    return hits[0]


def oracle_file_rows(raw_side):
    # Oracle-side rows (all severities, unfiltered) per entry-key file.
    counts = {}
    for section in raw_side["corpora"]:
        label = section["label"]
        for diag in section["diagnostics"]:
            # raw-outputs file fields already carry the label prefix for
            # multi-corpus runs (same convention as report.json's diff
            # entries) — compose nothing here or keys double-prefix and
            # every lookup misses.
            key = diag["file"]
            counts[key] = counts.get(key, 0) + 1
    return counts


def load_harness_leg(name):
    out_dir = os.path.join(REPLAY_DIR, name)
    report_path = os.path.join(out_dir, "report.json")
    raw_path = os.path.join(out_dir, "raw-outputs.json")
    preflight_path(out_dir, f"leg output directory for leg {name}", "dir")
    preflight_path(report_path, f"report.json of leg {name}")
    preflight_path(raw_path, f"raw-outputs.json of leg {name}")
    report = load_json(report_path, f"report.json of leg {name}")
    raw = load_json(raw_path, f"raw-outputs.json of leg {name}")
    # The comparator's own diff output: union of both sides' diff arrays,
    # reduced to the flat (lowercase code, file) caliber.
    diff = {
        (entry["code"].lower(), entry["file"])
        for side in ("oracle_only", "dartq_only")
        for entry in report[side]
    }
    return {
        "report_path": report_path,
        "raw_path": raw_path,
        "diff": diff,
        "oracle_rows": oracle_file_rows(raw["oracle"]),
        "oracle_instances": raw["oracle"]["instances"],
        "dartq_instances": raw["dartq"]["instances"],
        "oracle_diff_rows": len(report["oracle_only"]),
        "dartq_diff_rows": len(report["dartq_only"]),
        "labels": {s["dir"]: s["label"] for s in raw["oracle"]["corpora"]},
    }


def leg_file_key(corpus, file, label):
    # Entry-key file form: label-prefixed for a merged (multi-corpus) run,
    # plain corpus-relative otherwise — the same convention the comparator
    # applies to every diff/raw key.
    return f"{label}/{file}" if label else file


def main():
    preflight_path(KNOWN_DIFFS, "known-diffs table")
    sii_oracle_path = os.path.join(REPLAY_DIR, "sii.oracle.machine")
    sii_dartq_path = os.path.join(REPLAY_DIR, "sii.dartq.machine")

    table = load_json(KNOWN_DIFFS, "known-diffs table")
    entries = table.get("entries")
    if not isinstance(entries, list):
        die(
            f"{KNOWN_DIFFS} has no entries list — the table schema is a "
            f"top-level object with an entries array; refusing to join "
            f"against an unknown shape",
            2,
        )

    with open(KNOWN_DIFFS, "rb") as fh:
        known_sha = hashlib.sha256(fh.read()).hexdigest()

    legs = {name: load_harness_leg(name) for name in HARNESS_LEGS}

    preflight_path(sii_oracle_path, "sii oracle machine dump")
    preflight_path(sii_dartq_path, "sii dartq machine dump")
    sii_oracle = parse_machine_file(sii_oracle_path, "sii oracle machine dump")
    sii_dartq = parse_machine_file(sii_dartq_path, "sii dartq machine dump")
    sii_oracle_set = {(code, sii_rel_file(f)) for code, f in sii_oracle}
    sii_dartq_set = {(code, sii_rel_file(f)) for code, f in sii_dartq}
    sii_oracle_rows = {}
    for _, rel in sii_oracle_set:
        sii_oracle_rows[rel] = sii_oracle_rows.get(rel, 0) + 1
    sii_diff = sii_oracle_set.symmetric_difference(sii_dartq_set)
    legs["sii"] = {
        "oracle_path": sii_oracle_path,
        "dartq_path": sii_dartq_path,
        "diff": sii_diff,
        "oracle_rows": sii_oracle_rows,
        "oracle_instances": len(sii_oracle),
        "dartq_instances": len(sii_dartq),
        "diff_rows": len(sii_diff),
    }

    verdicts = []
    counts = {"already-fixed": 0, "still-red": 0, "oracle-ambiguous": 0}
    for index, entry in enumerate(entries):
        if not isinstance(entry, dict) or "code" not in entry or "file" not in entry:
            die(
                f"{KNOWN_DIFFS} entry {index} is not an object with code and "
                f"file fields — fix the table entry ({entry!r})",
                2,
            )
        reason = entry.get("reason", "")
        match = REASON_CORPUS.match(reason)
        if not match:
            die(
                f"{KNOWN_DIFFS} entry {index} ({entry.get('code')}, "
                f"{entry.get('file')}) carries no `case <hex> [<corpus>]` "
                f"record in its reason — the corpus cannot be resolved to a "
                f"leg; fix the table entry instead of guessing "
                f"(reason: {reason!r})",
                2,
            )
        case_id, corpus = match.group(1), match.group(2)
        leg_name = CORPUS_TO_LEG.get(corpus)
        if leg_name is None:
            die(
                f"{KNOWN_DIFFS} entry {index} ({entry.get('code')}, "
                f"{entry.get('file')}) cites corpus {corpus}, which no N2 "
                f"leg covers (expected one of: "
                f"{', '.join(sorted(CORPUS_TO_LEG))})",
                2,
            )
        leg = legs[leg_name]

        if leg_name == "sii":
            file_key = sii_rel_file(entry["file"])
        else:
            label = corpus_label(leg["labels"], corpus)
            file_key = leg_file_key(corpus, entry["file"], label)

        if leg["oracle_rows"].get(file_key, 0) == 0:
            verdict = "oracle-ambiguous"
        elif (entry["code"].lower(), file_key) in leg["diff"]:
            verdict = "still-red"
        else:
            verdict = "already-fixed"
        counts[verdict] += 1

        row = dict(entry)
        row["verdict"] = verdict
        row["leg"] = leg_name
        row["case"] = case_id
        verdicts.append(row)

    verdicts.sort(key=lambda r: (r["case"], r["file"], r["code"]))

    # Retirement coupling: a case (hash8) retires only when ALL table rows
    # citing it read already-fixed. The verdicts above are per-entry; the
    # aggregation below is the gate — retiring on a single already-fixed row
    # destroys the evidence (case json + edits + rows) of its still-red /
    # oracle-ambiguous siblings sharing the same hash8. Cases with zero
    # already-fixed rows (e.g. the class-row sentinels) never retire and are
    # not listed as blocked — only mixed cases are the hazard.
    by_case = {}
    for row in verdicts:
        agg = by_case.setdefault(row["case"], {"rows": 0, "already_fixed": 0})
        agg["rows"] += 1
        if row["verdict"] == "already-fixed":
            agg["already_fixed"] += 1
    retirable_cases = sorted(
        case for case, agg in by_case.items() if agg["already_fixed"] == agg["rows"]
    )
    blocked_cases = {
        case: agg
        for case, agg in sorted(by_case.items())
        if 0 < agg["already_fixed"] < agg["rows"]
    }

    results = {
        "schema": SCHEMA,
        "known_diffs_sha256": known_sha,
        "legs": {
            **{
                name: {
                    "report": legs[name]["report_path"],
                    "raw_outputs": legs[name]["raw_path"],
                    "oracle_diff_rows": legs[name]["oracle_diff_rows"],
                    "dartq_diff_rows": legs[name]["dartq_diff_rows"],
                    "oracle_instances": legs[name]["oracle_instances"],
                    "dartq_instances": legs[name]["dartq_instances"],
                }
                for name in HARNESS_LEGS
            },
            "sii": {
                "oracle_machine": sii_oracle_path,
                "dartq_machine": sii_dartq_path,
                "oracle_rows": len(sii_oracle),
                "dartq_rows": len(sii_dartq),
                "diff_rows": legs["sii"]["diff_rows"],
            },
        },
        "verdicts": verdicts,
        "retirement": {
            "rule": "a case retires only when ALL its known-diffs rows read already-fixed",
            "retirable_cases": retirable_cases,
            "blocked_cases": blocked_cases,
        },
        "summary": {
            "total": len(verdicts),
            "already-fixed": counts["already-fixed"],
            "still-red": counts["still-red"],
            "oracle-ambiguous": counts["oracle-ambiguous"],
        },
    }

    os.makedirs(REPLAY_DIR, exist_ok=True)
    with open(RESULTS_PATH, "w", encoding="utf-8") as fh:
        json.dump(results, fh, indent=2, ensure_ascii=False)
        fh.write("\n")

    print(
        f"replay-join: {len(verdicts)} entries -> already-fixed="
        f"{counts['already-fixed']} still-red={counts['still-red']} "
        f"oracle-ambiguous={counts['oracle-ambiguous']}; wrote {RESULTS_PATH}"
    )
    print(
        f"replay-join: retirement gate — {len(retirable_cases)} case(s) fully "
        f"already-fixed (retirable); {len(blocked_cases)} mixed case(s) MUST "
        f"STAY (never retire a case on a single already-fixed row)"
    )


main()
PY
