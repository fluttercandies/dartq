# mixed-versions corpus

A multi-package language-version matrix: every subpackage pins a different
Dart language version, with per-file `// @dart=X.Y` overrides in both
directions and one cross-package import edge across a version boundary.
Purpose: exercise the engine's language-version resolution and per-feature
version gating (corpus axis — the other corpus tiers all pin
`sdk: ^3.13.0`).

This corpus is NOT wired into the m0/m1 gates. See "Wiring" at the bottom.

## Version ladder

| Package | pubspec `environment.sdk` | Effective language version | Gated faces |
| --- | --- | --- | --- |
| pkg212 | `^2.12.0` | 2.12 | null safety / `late` / `required` boundary (positive control at 2.12); legacy-mode override file |
| pkg217 | `^2.17.0` | 2.17 | enhanced enums (positive); same shape under a `// @dart=2.12` override (below-version face) |
| pkg219 | `^2.19.0` | 2.19 | pre-records baseline; records/patterns/sealed under a `// @dart=3.0` override (higher-than-package face) |
| pkg300 | `^3.0.0` | 3.0 | records, patterns, class modifiers (positive); same shapes under `// @dart=2.19` (below-version face); imports pkg219 (cross-version edge) |
| pkg330 | `^3.3.0` | 3.3 | extension types (positive control) |
| pkg360 | `^3.6.0` | 3.6 | digit separators (positive control) |
| pkg313 | `^3.13.0` | 3.13 | everything-valid baseline; malformed/inconsistent override file producing the language-version-family codes |
| pkg315 | `^3.15.0` | 3.15 (future) | anticipation package: shapes all valid at 3.13; a 3.13 oracle clamps the future version, a 3.15-aware engine accepts it as-is |

## Resolution and override anchors (ground truth)

- `languageVersion` field in `package_config.json` entries is read as a
  string per package: `upstream pub source, lib/src/package_config.dart:236-238`
  (non-string rejected). The pubspec-derived default comes from the
  `environment: sdk:` lower bound: `upstream pub source, lib/src/pubspec.dart:141`.
- `// @dart` version-comment recognition lives in the scanner:
  `upstream Dart SDK source, pkg/_fe_analyzer_shared/lib/src/scanner/abstract_scanner.dart:1837`
  (`"@dart"` prefix check) and `token.dart:809` (accepted form
  `'// @dart = 2.1'`, spacing around `=` optional).
- Override semantics: `upstream Dart SDK source, pkg/front_end/lib/src/source/
  source_compilation_unit.dart:469-478` — the package_config version is the
  default, a source-level version annotation overrides it, and only the
  FIRST registered version applies (later ones are ignored).
- Feature gating is version-driven:
  `source_compilation_unit.dart:348-353` builds `LibraryFeatures` from
  `languageVersion.version`. Example per-feature gate: digit separators,
  `pkg/front_end/lib/src/kernel/body_builder.dart:7372`
  (`libraryFeatures.digitSeparators.isEnabled`) while the scanner accepts
  separators unconditionally (`abstract_scanner.dart:1533-1541`).
- Parser-gated experiment families (enhanced parts, primary constructors,
  augmentations, patterns-adjacent experiments):
  `pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart:361-373`.

## Expectation classes

1. **Positive controls** — a shape inside a package whose version is at or
   above the feature's release: must analyze clean (pkg212 null safety,
   pkg217 enhanced enums, pkg300 records/patterns/modifiers, pkg330
   extension types, pkg360 separators, pkg313 everything).
2. **Below-version faces** — the same shapes under a lower override or in a
   lower package (pkg217's `// @dart=2.12` enhanced enum, pkg300's
   `// @dart=2.19` records/patterns, pkg212's legacy file): the engine must
   apply the overridden version. The ORACLE's emission for below-version
   released features follows its own feature table and is intentionally not
   pinned here; the engine-gating decision (which version applied) is the
   thing under test.
3. **Legacy-mode face** — pkg212/lib/legacy.dart at `// @dart=2.9`: the
   non-nullable feature is off for that file, so `int?` is outside the
   file's grammar and `int y = null;` is legal (the reverse of the 2.12
   package default).
4. **Family-code products (documented exclusion)** —
   pkg313/lib/malformed_override.dart intentionally produces the three
   language-version-family codes (`invalid_language_version_override.*`,
   `illegal_language_version_override.*`,
   `inconsistent_language_version_override.*`). These sit OUTSIDE the
   255-code syntax gate universe by design (the gate's codes file excludes
   them); the differential filter accounting handles out-of-set codes.
5. **Future version** — pkg315 pins 3.15 with 3.13-valid shapes: a 3.13
   oracle clamps the future version to its supported maximum; a 3.15-aware
   engine resolves it natively. No `enable-experiment` content here (that
   is a separate corpus gap).

## Cross-package edge

pkg300 (`3.0`) imports pkg219 (`2.19`) — `lib/uses_pkg219.dart` — so one
analysis run must keep two language versions live across an import edge.
pkg300's pubspec declares the path dependency and its package_config entry
carries pkg219's languageVersion.

## Wiring (recommendation, not implemented)

Not wired into the m0/m1 gates. If adopted later: the harness preflight
requires every corpus package's package_config to carry
`languageVersion >= 3.13`, which this corpus deliberately violates (that
is its purpose) — wiring requires a dedicated gate caliber (a per-package
analysis loop, not the single `dart analyze <dir>` shape) plus a decision
on how the differential filter treats the family-code products. Until
then the corpus is a standalone engine-gating fixture.
