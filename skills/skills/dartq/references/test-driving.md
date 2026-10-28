# Test Driving

## The Rule

Use `dartq test` wherever you would use `dart test`; add
`dartq test --flutter` wherever you would use `flutter test`. Same arguments,
same reporters (`compact`, `expanded`, `json`, ...), same output, same exit
codes (0 pass, 1 fail, 64 usage, 65 data error, 79 no tests). dartq decides
where each suite actually runs; when it cannot reproduce a run exactly, it
delegates to the stock runner with the terminal attached — the verdict is
never invented.

## The Three dartq Extras

```bash
dartq test                          # plain drop-in
dartq test --impacted               # only suites the changes since HEAD can reach
dartq test --impacted=main          # ...since an explicit ref
dartq test --no-cache               # run everything, store nothing
dartq test --stats                  # one stderr line: where suites ran
```

`--impacted` is the edit-test loop verb: change code, run
`dartq test --impacted`, get the reachable subset. A dirty tree with no git
baseline (or a ref that cannot be diffed) prints a warning and runs
everything — it never silently skips.

## Reading the `--stats` Line

```
dartq test: 42 suites — 30 replayed, 9 grouped, 3 stock (8.2s)
```

- **replayed** — proven green earlier with the same key; the recorded events
  are reprinted, nothing executes. This is the free tier.
- **grouped** — compiled into one program, run as isolates of that program's
  group in a few memory-leased processes.
- **stock** — the plain `dart test` runner (dartq could not prove the
  outcome, or the suite observes how it was launched).

High `stock` counts are the signal to investigate: read
[cache-and-markers.md](cache-and-markers.md) for why a
suite refuses replay and which marker opts it back in.

## When dartq Delegates to the Stock Runner

These run as plain `dart test` (or `flutter test`) unchanged — expect stock
latency, not a bug: `--help`, `--version`, malformed arguments, `--debug`,
`--pause-after-load`, coverage, `--fail-fast`, a randomized test order,
`--configuration`, non-VM platforms, and a `dart_test.yaml` that names a
reporter.

## Flutter Path Facts

`dartq test --flutter` keeps the tool's verdict authoritative: suites are
compiled by `frontend_server`, run in `flutter_tester`, with the asset
bundle, fonts, golden comparator, and `flutter_test_config.dart` wrapping —
dartq only decides which suites run and where (replay / shared-compile
chunks / parallel shards balanced on recorded durations). It resolves
packages once and runs later shards with `--no-pub`, so parallel shards
never contend on `pubspec.lock`. `--platform tester` runs here; any other
platform, web, devices, and integration tests delegate to the stock tool.

Replay works the same as on the Dart path: a green suite re-runs its key
— the import closure plus `flutter_test_config`, bundled assets and
fonts, golden files, the resolution, the Flutter SDK and engine stamps,
the arguments and environment — and the second run reprints the recorded
events. The Flutter SDK's own packages (the framework, `flutter_test`)
ride as the toolchain tier: still content-keyed (editing the SDK
re-keys) but never refused for their internal clock or environment
reads. A suite refuses replay only through its own closure — its files
and the package's `lib/` (see
[cache-and-markers.md](cache-and-markers.md)).

## Plain `dartq test` inside a Flutter package

Without `--flutter`, `dartq test` applies `dart test` semantics even in a
Flutter package: suites compile and run on the plain Dart VM, where
`dart:ui` is unavailable — widget tests cannot run this way (use
`dartq test --flutter`). Positional path arguments resolve against the
package root — the current directory, which must hold `pubspec.yaml` and
`.dart_tool/package_config.json`; any other shape delegates to the stock
runner.

## Cache Discipline for Agents

- The result cache is per user and content-keyed: every worktree of the same
  content shares it. Re-running a suite in a fresh checkout can replay.
- Failures are never stored — a red suite always really runs.
- Suspect a false replay? `dartq test --no-cache` is the escape hatch.
- Want a suite cached despite drifting reads? That is what the
  `// dartq: deterministic` / `// dartq: portable` attestations are for —
  see the reference file before adding one: a wrong attestation can
  replay a stale verdict.

## Quick Reference

| Situation | Command |
| --- | --- |
| Full run, drop-in | `dartq test` |
| Flutter suite | `dartq test --flutter` |
| After an edit | `dartq test --impacted` |
| Against a base branch | `dartq test --impacted=main` |
| Where did time go | `dartq test --stats` |
| Trust nothing | `dartq test --no-cache` |
| One suite, by name | `dartq test -n "suite name"` (runner args pass through) |
