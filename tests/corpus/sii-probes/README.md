# sii-probes corpus

Materialized replay target for the 17 scanner string-interpolation
regression cases in `tests/corpus/regressions/cases` (triage #1048). The
capturing probes ran on 2026-10-12 against a scratch corpus outside the
repo and an oracle SDK `dart analyze --format=machine`; the case files are
the durable evidence (both sides' rows plus the full Dart source inline),
and this corpus makes them replayable in-repo: each `lib/*.dart` here is
the byte-identical `source` of exactly one persisted case.

Read-only, like the other assembled corpora. The companion
`<hash8>.edits.jsonl` per case rewrites the whole fixture with its
captured content (a no-op splice), so

```bash
target/debug/harness replay \
  --corpus-dir tests/corpus/sii-probes \
  --edits tests/corpus/regressions/cases/<hash8>.edits.jsonl \
  --dartq-cmd "./target/release/dartq analyze --format=machine" \
  --daemon-cmd "./target/release/dartq daemon --dir {dir}"
```

re-analyzes exactly the captured state on both faces and surfaces the
divergence in the step comparison (exit 1 with the step listed). A green
run (exit 0) is the retirement evidence for that case.
