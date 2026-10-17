# Regression cases

Persisted differential-testing failures (the charter: every diff between dartq
and the oracle auto-persists as a regression case instead of evaporating
with the run's console output).

## Layout

```
regressions/
  README.md            # this file
  known-diffs.json     # exemption table: divergences already under triage
  cases/               # curated case files (--list-known reads them here)
```

Cases are produced by the runner flag, not by hand:

```bash
# persist cases next to the run's reports (recommended target: the audit dir)
target/debug/harness run --corpus-dir tests/corpus/m1-semantic \
  --dartq-cmd "./target/debug/dartq analyze --format=machine --sdk <sdk>" \
  --persist-regressions <evidence-dir>/gate-m1/<label>/cases

# triage: which known divergences still fail?
target/debug/harness run --list-known
```

## Case files

One file per diffing (code, file): `<hash8>.json` — hash8 is blake3 of the
entry-key file path, first 8 hex — so the same failure always lands on the
same file name (overwrite, never append growth). Schema
`regression-case-v1`:

| field | meaning |
| --- | --- |
| `file` | corpus-relative path (label-free); absolute when the divergence is on a file outside every corpus root |
| `corpus` | merge label (multi-corpus runs) or the corpus dir string |
| `oracle`, `dartq` | both sides' verbatim rows for the file: `{code, line, column, length, message}` (lowercase entry-key codes, sorted) |
| `source` | the file's full Dart text — the case is a self-contained repro |
| `captured_at` | epoch seconds of the capture |
| `source_run` | the capturing run: report path, dartq cmd, oracle cmd |
| `replay_command` | argv reproducing the divergence through `harness replay` (empty for outside-corpus files: replay edits must be corpus-relative) |

Next to each case sits `<hash8>.edits.jsonl` — one replay edit step that
replaces the file with its captured content (a no-op edit), so replaying
re-analyzes exactly the captured state on both faces and the step
comparison surfaces the divergence (replay exits 1 with the step listed).

## Exemption table (`known-diffs.json`)

```json
{
  "entries": [
    {
      "code": "wrong_number_of_type_arguments",
      "file": "lib/foo.dart",
      "message_pattern": "Expected 2 type arguments",
      "reason": "dartq misses nested generic instantiation; fix tracked in goal #NNN"
    }
  ]
}
```

- `code` + `file` are matched exactly (file is corpus-relative and
  label-free, so an entry generalizes across corpus dirs); for a merged
  multi-corpus run whose diff keys are outside every corpus root, `file` is
  the canonical absolute path.
- `message_pattern` is optional and matched as a substring of the diffing
  entry's sample message (substring, not regex — the harness carries no
  regex dependency).
- A matching entry is suppressed from re-persisting. A file whose diffing
  entries are ALL exempt produces no case at all.
- A malformed table is a hard harness error, never silently ignored.

## Workflow

1. **Persist** during gate runs (`--persist-regressions <audit dir>/cases`).
2. **Triage** each case: run its `replay_command` to see the divergence
   live; fix the engine and promote the case's file into a targeted
   corpus fixture (tests/corpus/m0-syntax or m1-semantic) so the gate itself
   covers it — then delete the case.
3. **Exempt** divergences that are real-but-deferred: add an entry to
   `known-diffs.json` with a reason. Copy the case file into `cases/` so
   `--list-known` has evidence.
4. **Prune** with `--list-known`: entries marked STALE match no persisted
   case anymore (fixed or re-titled) — remove them. NO-EVIDENCE means no
   persisted cases exist yet; nothing may be claimed stale off zero data.

## Derivation pin (`cases/` is a hard gate input)

`cases/` is not only evidence for `known-diffs.json`: it is the hard
input of the sem2 derivation pin. The pin test
`sem2_codes_file_matches_derivation` (tests/harness/src/main.rs)
re-derives `tests/harness/sem2_codes.txt` live — the union of the
oracle and dartq row codes over every `cases/*.json` minus the five
frozen gate universes — and asserts set equality against the file. Any
deletion or addition of cases must therefore be derivation-aware:
after touching `cases/`, re-run the pin and consciously re-freeze the
codes file if the derived set moved (deleting a case with unique codes
changes the file). The four keyword-named-declaration error-recovery
nails (`keyword_named_*`: formal parameters, class methods, typedefs,
top-level methods) are intentional one-per-shape coverage of the
keyword matrix, not duplicates — do not consolidate them.
