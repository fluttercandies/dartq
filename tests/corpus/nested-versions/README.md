# nested-versions corpus

An arbitrarily nested pub workspace with mixed settled language versions in
one shared resolution: a workspace root, members at three directory depths,
and vendored path-dependency packages pinned to older language versions —
all mapped by a single hand-written root `package_config.json` in pub's
output shape (the `tests/corpus/` idiom: the config file IS the resolution
result, no `pub get` needed). Companion to the flat sibling layout of
`../mixed-versions/`; this corpus adds workspace nesting, the
`workspace_ref.json` artifact, a root lockfile, member-level analysis
options, and the same-dependency-two-versions split.

## Layout

```
nested-versions/
├── pubspec.yaml                        # workspace root `monorepo` (3.7)
├── pubspec.lock                        # external deps only (members excluded, per pub)
├── analysis_options.yaml               # root lint set (always_declare_return_types on)
├── .dart_tool/
│   ├── package_config.json             # THE resolution: 7 packages, pub field shape
│   └── pub/workspace_ref.json          # root -> itself ("../..")
├── lib/
│   ├── monorepo.dart                   # root hub across members (clean)
│   └── wildcard_reference.dart         # 3.7 gated error
├── apps/shell/                         # member, depth 2, 3.7, options override
│   ├── pubspec.yaml                    # resolution: workspace; deps core/deep_ui + 2 path deps
│   ├── analysis_options.yaml           # includes root; silence + add (different strictness)
│   ├── .dart_tool/pub/workspace_ref.json
│   └── lib/                            # shell.dart, wildcards.dart, uses_ext_lib.dart,
│                                       # wildcard_closure.dart
├── packages/core/                      # member, depth 2, 3.6
│   ├── pubspec.yaml                    # resolution: workspace; dep ext_lib_compat (path)
│   ├── .dart_tool/pub/workspace_ref.json
│   └── lib/                            # core.dart, wildcard_repeat.dart, uses_compat.dart
├── packages/nested/deep_ui/            # member, depth 4, 3.7
│   ├── pubspec.yaml                    # resolution: workspace; dep core (plain constraint)
│   ├── .dart_tool/pub/workspace_ref.json
│   └── lib/                            # deep_ui.dart, wildcard_reference.dart
└── third_party/                        # vendored, NOT workspace members (see below)
    ├── ext_lib/                        # path dep, 3.0  — "ext_lib major 2"
    │   └── lib/                        # ext_lib.dart, gated_wildcards.dart
    ├── ext_lib_compat/                 # path dep, 2.19 — "ext_lib major 1"
    │   └── lib/                        # ext_lib_compat.dart, gated_wildcards.dart
    └── legacy/legacy_lib/              # path dep, 2.19, depth 3 — pre-records baseline
        └── lib/                        # legacy_lib.dart, gated_records.dart
```

## Version matrix

| Package | Path (depth) | `environment.sdk` | Settled version | Workspace role |
| --- | --- | --- | --- | --- |
| monorepo | `.` (0) | `^3.7.0` | 3.7 | root (`workspace:` list) |
| shell | `apps/shell` (2) | `^3.7.0` | 3.7 | member (`resolution: workspace`) |
| core | `packages/core` (2) | `^3.6.0` | 3.6 | member (`resolution: workspace`) |
| deep_ui | `packages/nested/deep_ui` (4) | `^3.7.0` | 3.7 | member (`resolution: workspace`) |
| ext_lib | `third_party/ext_lib` (2) | `^3.0.0` | 3.0 | path dep (NOT a member) |
| ext_lib_compat | `third_party/ext_lib_compat` (2) | `^2.19.0` | 2.19 | path dep (NOT a member) |
| legacy_lib | `third_party/legacy/legacy_lib` (3) | `^2.19.0` | 2.19 | path dep (NOT a member) |

`languageVersion` in each package_config entry is the lower bound of the
pubspec sdk constraint, written `major.minor` — matching what pub derives
(upstream pub source, `lib/src/pubspec.dart` language-version getter) and
emits (`lib/src/entrypoint.dart` `_packageConfigFile`).

## Why the 2.19/3.0 packages are not workspace members

Pub gates `workspace:` and `resolution:` on the declaring package's own
language version >= 3.5 (`lib/src/pubspec.dart:71,110`,
`lib/src/language_version.dart:114` `firstVersionWithWorkspaces`). A
member pubspec at 2.19 or 3.0 declaring `resolution: workspace` is a pub
parse error, so a pub-valid corpus cannot pin those versions through
membership. They are vendored path dependencies instead: all four settled
versions (2.19, 3.0, 3.6, 3.7) still coexist in ONE package_config, which
is the guarantee under test. The gate itself is ground truth worth
differencing — if the engine accepted a 2.19 `resolution: workspace`, that
is a bug this layout motivates but does not encode (all files here are
valid for their declared versions).

## Resolution ground truth (what the artifacts pin)

- One `package_config.json`, at the workspace root only. Members carry
  `.dart_tool/pub/workspace_ref.json` pointing at the root
  (`{"workspaceRoot": "<relative>"}`, pub `entrypoint.dart`
  `writePackageConfigFiles`): root `../..`, depth-2 packages `../../../..`,
  depth-4 `../../../../..`.
- Entry order mirrors pub's `_packageConfigFile`: lockfile packages sorted
  by name first (`ext_lib`, `ext_lib_compat`, `legacy_lib`), then the
  workspace closure root-first in DFS declaration order (`monorepo`,
  `shell`, `core`, `deep_ui`).
- Members are absent from `pubspec.lock` (pub locks only external deps);
  the lock holds the three path deps plus the `sdks` section.
- Member-to-member deps are plain version constraints (`core: ^1.0.0`);
  pub canonicalizes member refs through the workspace root
  (`lib/src/solver/partial_solution.dart:20-21`).
- No pubspec sits in `packages/nested/` — interstitial pubspecs between
  root and member are a pub workspace error (`lib/src/package.dart:531-592`);
  the root walk must pass through cleanly.
- `packages/*` style globs are deliberately NOT used; the member list is
  explicit directories (glob expansion is a separate face with its own
  tests).

## Expectation inventory (per file)

The corpus is 33 files on disk: 17 Dart sources (inventoried below) plus
the resolution/options fixtures — 7 pubspecs (root + 3 members + 3
vendored), `pubspec.lock`, 2 `analysis_options.yaml`, the root
`package_config.json`, 4 `workspace_ref.json` (root + 3 members), and
this README.

Version-gated flips used (stable features only, no experiments):

| Feature | Ships | Below-version verdict | At/above-version verdict |
| --- | --- | --- | --- |
| records + patterns | 3.0 | rejected (parse/summary) | clean |
| digit separators | 3.6 | rejected | clean |
| wildcard (non-binding) formals | 3.7 | repeated `_` = `duplicate_definition` | repeated `_` clean; body reference to `_` = `undefined_identifier` |

| File | Version | Verdict |
| --- | --- | --- |
| lib/monorepo.dart | 3.7 | clean |
| lib/wildcard_reference.dart | 3.7 | `undefined_identifier` (body `_`) |
| apps/shell/lib/shell.dart | 3.7 | no errors; `prefer_final_locals` on `visits` (shell override set) |
| apps/shell/lib/wildcards.dart | 3.7 | clean (wildcard positive control) |
| apps/shell/lib/uses_ext_lib.dart | 3.7 | clean (S3 positive, ext_lib 2.x) |
| apps/shell/lib/wildcard_closure.dart | 3.7 | `undefined_identifier` (closure body `_`) |
| packages/core/lib/core.dart | 3.6 | clean (digit separators, patterns) |
| packages/core/lib/wildcard_repeat.dart | 3.6 | `duplicate_definition` (second `_`) |
| packages/core/lib/uses_compat.dart | 3.6 | clean (S3 positive, ext_lib_compat 1.x) |
| packages/nested/deep_ui/lib/deep_ui.dart | 3.7 | no errors; `always_declare_return_types` on `runDemo` (inherited root set at depth 4) |
| packages/nested/deep_ui/lib/wildcard_reference.dart | 3.7 | `undefined_identifier` (method body `_`) |
| third_party/ext_lib/lib/ext_lib.dart | 3.0 | clean (records/sealed/patterns positive control) |
| third_party/ext_lib/lib/gated_wildcards.dart | 3.0 | `duplicate_definition` (second `_`) |
| third_party/ext_lib_compat/lib/ext_lib_compat.dart | 2.19 | clean (pre-records) |
| third_party/ext_lib_compat/lib/gated_wildcards.dart | 2.19 | `duplicate_definition` (second `_`) |
| third_party/legacy/legacy_lib/lib/legacy_lib.dart | 2.19 | clean (pre-records baseline) |
| third_party/legacy/legacy_lib/lib/gated_records.dart | 2.19 | records/patterns rejected below 3.0 (oracle-pinned codes) |

Lint expectations (both options sets are options-driven standalone rules,
so each diagnostic pins which options file applied):

- Root set `{always_declare_return_types}`: fires in
  `packages/nested/deep_ui/lib/deep_ui.dart` (`runDemo`) — proving the
  root file reaches depth 4.
- Shell override set `{prefer_final_locals}` (+ root rule silenced): fires
  in `apps/shell/lib/shell.dart` (`visits`) — proving the member override
  replaces the effective set for its subtree only.

## Same-dependency-two-versions (S3) face

A pub package_config keys entries by name, so one shared resolution holds
ONE version per package name; two majors of a dependency family coexist
only as differently named packages. Here the family is `ext_lib` 2.x
(`third_party/ext_lib`, consumed by shell) vs `ext_lib_compat` 1.x
(`third_party/ext_lib_compat`, consumed by core), with disjoint API
surfaces (`ExtGreeter`/`Shape` vs `CompatGreeter`/`Buffer`). Note for the
harness: an import of an undeclared-but-config-present package still
resolves for the analyzer (`uri_does_not_exist` fires only when the name
is absent from the config; the missing-dependency check is the
`depend_on_referenced_packages` lint, off here) — the zero-cross-
contamination assertion is therefore POSITIVE: each member must resolve
its own major's symbols from the right directory and only there
(apps/shell/lib/uses_ext_lib.dart and packages/core/lib/uses_compat.dart).

## Consuming this corpus (differential step)

- Analysis root: this directory. It is ONE context root — only the root
  has `.dart_tool/package_config.json`; member files attach to it through
  the workspace walk (`find_workspace_root`) or `workspace_ref.json`.
  Do not expect per-member package configs; their absence is the shape.
- The per-package language versions come from the config's
  `languageVersion` fields (2.19/2.19/3.0/3.6/3.7), not from any
  `// @dart` comments (there are none — that override face lives in
  `../mixed-versions/`).
- A harness preflight requiring every `languageVersion >= 3.13` (see
  `../mixed-versions/` DESIGN.md Wiring) will reject this corpus by
  design; it needs the per-package analysis caliber (each package
  analyzed at its settled version, oracle and engine alike), not a single
  root-version pass.
- Lint diagnostics are part of the expectations (two rules above); the
  comparison filter must admit lint codes for the two pinned files, or
  drop them on BOTH sides symmetrically.
- Structural self-check only was performed at build time (config↔pubspec
  version match, target dirs, workspace_ref reachability). The oracle run
  happens in the serial validation window.
