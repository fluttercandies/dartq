# The Analysis Loop

The loop for every code change in an analyzed workspace:

```
diagnose (analyze) -> understand (query/explain) -> scope (impact)
    -> edit -> execute (rename/fix) -> verify (analyze/test)
```

## 1. Diagnose

```bash
dartq analyze                      # default: {v diags:[{code sev file line col len msg doc?}]}
dartq analyze --format json        # the frozen oracle envelope (jq consumers)
dartq analyze --format text        # grouped human listing
dartq analyze --fast               # parser tier only — pure syntax, no engine
```

The diagnostic **set** is complete on every face — faces only change the
encoding. Exit codes: 0 clean/info-only, 1 fatal-infos, 2
fatal-warnings, 3 errors, 4 internal, 64 usage.

Diagnose a code, not a file:

```bash
dartq explain --diagnostic invalid_use_of_protected_member
dartq explain --diagnostic unused_import --file lib/api.dart:12
```

## 2. Understand the Symbol

```bash
dartq query symbol UserService          # declarations + resolved references
dartq query callers UserService         # invocation sites only
dartq query implementations Repository  # subtypes / overriding members
dartq query dependencies UserService  # the declaring library's import edges
dartq explain UserService               # the one-stop report (JSON)
dartq query batch --symbol Foo --symbol Bar --kinds callers --kinds references
```

`query` answers "where is X"; `explain` answers "everything about X". Use
query rows for cheap lookups, explain when you need the shape, docs, and
related diagnostics in one answer. `batch` runs every symbol × kind in one
workspace assembly — one graph build, one JSON envelope per line.

Cursor-shaped questions:

```bash
dartq resolve lib/main.dart:42:15      # what is under this cursor (1-based, UTF-16 cols)
dartq complete lib/main.dart:42:15     # visible-name candidates at this position
```

## 3. Scope the Change

```bash
dartq impact lib/src/user_service.dart            # file-level: what a change affects
dartq impact lib/src/api.dart --symbol UserService # narrowed to one container's face
```

`impact` answers the counterfactual — dependent files, signatures, and
tests a hypothetical change touches. Run it before edits whose blast radius
is unclear; it is the difference between "rename a method" and "rename a
method and fix 40 call sites you did not know about".

## 4. Execute

```bash
# Rename: dry run first — the edit set is the lon document
dartq rename --new-name UserAccount lib/src/user.dart:8:10
# then apply (refuses any file outside the workspace)
dartq rename --write --new-name UserAccount lib/src/user.dart:8:10

# Fixes: see them, then apply
dartq fix --dry-run                  # piped: one JSON document per file
dartq fix --code prefer_final_locals # comma-separated codes, case-insensitive
dartq fix                            # apply
```

Rename anchors are `PATH:LINE:COL` (1-based; column counts UTF-16 code
units — the same units an editor reports) or `PATH:OFFSET` (0-based bytes).

## 5. Verify

```bash
dartq analyze              # the same diagnostics dart analyze would print
dartq test --impacted      # only the suites your change can reach
```

## Overlays: Analyze an Edit Before Writing It

```bash
dartq analyze --overlay lib/main.dart=/tmp/edited.dart    # analyze PATH with FILE's bytes
dartq analyze --overlay lib/main.dart=-                   # ...with stdin's bytes
```

Repeatable. The zero-risk pre-apply check: analyze the would-be file, then
write it only if clean.

## Depth

Formats, the lon vocabulary, and tiers per verb:
[formats-and-tiers.md](formats-and-tiers.md).
