# Getting Started

## When dartq Is the Right Tool

- You are an agent working in a Dart or Flutter repo and want diagnostics,
  symbol navigation, impact answers, or tests without spawning `dart analyze`
  or an IDE per question.
- The repo is large and `dart analyze` per-command latency is the bottleneck.
- You need machine output by default, not prose to strip.

If dartq is absent, `dart analyze` / `flutter analyze` still work — but the
rest of this skill assumes dartq from here on.

## Install

dartq is a single static binary. Build or obtain it, put it on `PATH`, and
point it at a Dart SDK:

```bash
dartq version                      # verify: prints "dartq 0.1.0" or similar
export DART_SDK=/path/to/dart-sdk # or pass --sdk-path / --sdk per command
```

Engine faces resolve the SDK through `$DART_SDK` (or the per-command
`--sdk-path` / `--sdk` spelling); FVM worktrees are picked up through the
session identity, so `.fvmrc` switches are respected per worktree.

Flutter packages need no new flags: when the package graph contains
`sky_engine`, its `lib/_embedder.yaml` replaces the SDK's `dart:` library
table (analyzer `EmbedderSdk` semantics), so `dart:ui` and the embedder
libraries resolve automatically — `--sdk-path`/`$DART_SDK` still name the
Dart SDK.

## First Run in a Repo

```bash
cd /path/to/repo
dart pub get                       # engine faces need .dart_tool/package_config.json
dartq analyze | head -5            # the default: one canonical LON document
dartq analyze --format json        # the frozen oracle envelope (jq consumers)
dartq analyze --format text        # the human grouping (sev code line:col msg rows)
dartq analyze --fast               # parser-only sweep: no engine, pure syntax
```

The first command also elects the worktree's resident session (see
[sessions-and-monorepos.md](sessions-and-monorepos.md)). It is automatic — no
daemon to babysit.

## Orientation Pass (unfamiliar repo)

One command per question, all machine-readable by default:

```bash
dartq packages                      # {v root pkgs bounds}: every package root,
                                    #   kind, resolved state, pruned boundary
dartq packages --format json        # same facts, one JSON object per line
dartq query symbol main             # where is main declared, who references it
dartq explain MyAppService          # symbol report: declaration, supertypes, ctors, members, refs, importers, tests
```

`packages` is the navigation face for multi-package trees: run it before
touching a monorepo you do not know.

## The Output Contract

Every output-bearing verb carries one `--format <lon|json|text>` axis whose
default is the constant `lon` — piped or on a TTY, the same bytes:

| Verb | `lon` (default) | `json` | `text` |
| --- | --- | --- | --- |
| `analyze` | `{v diags:[…]}` document | oracle envelope | human groups |
| `query` / `resolve` / `complete` | vocabulary answers | frozen envelopes | indented |
| `packages` | `{v root pkgs bounds}` | one JSON object per line | aligned table |
| `fix` | vocabulary patch documents | frozen documents | human listing |
| `session` / `events` | vocabulary rows | JSON rows | identity lines |

Pin the face explicitly in scripts; the short field vocabulary (`v`, `diags`,
`sev`, `msg`, `decls`, `refs`, `off`, `len`, `sym`, …) is the lon face's
spelling — the json face keeps the long frozen names for jq.

## Verb Families at a Glance

- **Diagnostics:** `analyze` (full engine), `--fast` (parser tier), `fix`
- **Navigation:** `packages`, `query`, `explain`, `resolve`, `complete`
- **Change planning:** `impact`, `rename`
- **Verification:** `test` (drop-in `dart test` / `flutter test`)
- **Infrastructure:** `session`, `events`, `compat`, `mcp`

## Verify the Setup Works End to End

```bash
dartq analyze --fast lib/           # must print diagnostics or a clean envelope
dartq query symbol <SomeSymbol>     # must answer DECL/REF rows or JSON
echo 'void f() { print("x" }' > /tmp/t.dart && dartq analyze --fast /tmp/t.dart
```

If `analyze` reports a missing `package_config.json`, run `dart pub get`
inside the package directory it named. If the SDK is wrong, set `$DART_SDK`.

## Next Stops

- Editing code: [analysis-loop.md](analysis-loop.md) (diagnose → understand → scope → verify)
- Running tests: [test-driving.md](test-driving.md)
- Agent host integration: [mcp.md](mcp.md)
