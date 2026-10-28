# The Analysis Loop

The loop for every code change in an analyzed workspace:

```
diagnose (analyze) -> understand (query/explain) -> scope (impact)
    -> edit -> execute (rename/fix) -> verify (analyze/test)
```

## 1. Diagnose

```bash
dartq analyze                      # default: {v root? diags:[{code sev file line col len msg}]}
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
dartq query callers UserService         # the invoked reference rows only (call sites, constructor refs, tear-offs); decls stay under `decls`
dartq query implementations Repository  # subtypes / overriding members
dartq query dependencies UserService  # the declaring library's import edges
dartq explain UserService               # the symbol report (JSON)
dartq query batch --symbol Foo --symbol Bar --kinds callers --kinds references
```

`query` answers "where is X"; `explain` answers the symbol's shape and reach
in one report — sections `declaration bases ifaces ctors methods refs deps
tests`. For a name declared in several places the primary declaration picks
the non-test file first (test = a path under `test/` or `*_test.dart`), the
public container before a `_`-private one, then path/line; `others` lists
the remaining candidates `{file kind container? line}` (absent when empty,
`counts.others` at minimal), and a `Container.name` spelling disambiguates
up front (a miss = the empty report, exit 0). Use query rows for cheap
lookups, explain when you need the shape and the reach in one answer.
`batch` runs every symbol × kind in one workspace assembly — one graph
build, one JSON envelope per line.

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

`--symbol` takes three spellings: a bare `name` (the union of every
same-named declaration, constructor selectors included), `Container.name`
(the member must exist inside that container — `C.C` names the unnamed
constructor), and `path/to/file.dart:line` (the declaration whose name
token starts on that line; a relative path tries the cwd, the workspace
root, then the target's directory). A spelling that resolves to nothing
is a usage error naming it.

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
# the edit set = the declaration's name token + the resolved reference rows
# the index records; an anchor the index does not cover (an unresolved bare
# name, an import prefix, an unmodeled member, a non-identifier position)
# answers the empty edit set (target:null, exit 0)

# Fixes: see them, then apply
dartq fix --dry-run                  # every candidate, each row with its role key
dartq fix --code prefer_final_locals # comma-separated codes, case-insensitive
dartq fix                            # apply (writes only the primary role)
dartq fix --role ignore_line         # opt into a suppression spelling
```

`--role <primary|ignore_line|ignore_file|suppress>` (default `primary`)
selects which candidates an apply writes: `primary` = one code change per
diagnostic; the ignore comments and `analysis_options` suppressions are
never auto-applied — opt in by naming their role. A dry run lists every
candidate with a required `role` key; an apply record omits the rows the
selection filtered out, and the patch schema
(`dartq/schema/v1/fix-patch.json`) requires `role` on every row. The
daemon `dartq/fix` and MCP `dartq_fix` take the same `role` (exact
spellings, default `primary`).

Rename anchors are `PATH:LINE:COL` (1-based; column counts UTF-16 code
units — the same units an editor reports) or `PATH:OFFSET` (0-based
UTF-16 code units — the same unit every `off` in the edit rows; on
pure-ASCII files it equals the byte count).

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
