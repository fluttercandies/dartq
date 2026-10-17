# AI-face goldens

The AI-first faces' JSON envelopes, promoted from the in-crate pinned tests
to repo-root artifacts (the release gate's strongest evidence form). Each
file is one envelope exactly as the producer emits it, one JSON object per
line.

| golden | producer | fixture |
| --- | --- | --- |
| `query_symbol_compact.json` | `dartq_commands::query::to_json` over the `Widget` symbol answer | the golden fixture below |
| `query_symbol_minimal.json` | `dartq_commands::query::to_json_at_tier(.., Minimal)` over the same answer | the golden fixture below |
| `index_run.json` | `dartq_commands::index::IndexRun::to_json` | the three-library query fixture (a/b/c.dart) |
| `impact_report.json` | `dartq_commands::impact::to_json` | the canonical fixture report hand-built in the shaper tests — every row shape in one document (both signature verdicts, both impact levels, the test section, a two-row environment matrix); the live runs are the CLI tests' |
| `compat_matrix.json` | `dartq_commands::compat::to_json` | the canonical fixture matrix hand-built in the shaper tests — all three axis row shapes (clean, analyzed-with-failure, isolated load failure) |

## The query fixture

Three files, one package (`fixture`):

- `lib/a.dart` — the signature-surface source: an `@Deprecated('use Other')`
  container `Widget` with a `static final int limit` member, a top-level
  `int top(int a, {String? b})`, and an unnamed extension (the shared
  signature fixture of `dartq-commands`' query tests).
- `lib/b.dart` — `import 'a.dart';` and `var w = Widget(); top(1);` (the
  constructor site lands at UTF-16 offset 41, the pinned cross-file
  offset of the shared A/B fixtures).
- `tool/note.dart` — `import 'package:fixture/a.dart';` and
  `Widget? probe() => null;` — a **non-lib** member file.

## What the goldens pin

- The element-face columns (#1560): the declaration row carries
  `annotations` verbatim; members carry their rendered `signature` in the
  symbol tests' pinned spellings.
- The package-form reference labels (#1556): `package:fixture/<rel>` for
  every `lib/` file, in declarations and both reference columns.
- The non-lib fallback: `tool/note.dart` keeps the canonical path. Its
  spelling is workspace-root dependent, so the golden carries the
  `FIXTURE_ROOT` placeholder token; the verifying test substitutes the
  canonical fixture root before comparing. Every other byte is literal.
- The verbosity tiers: minimal keeps the red-line locators and drops
  `container`, the element-face columns, the use-file column, and the
  reserved kind arrays (the canonical tables' query rows).

## Regeneration

The envelopes are derived from the pinned test values (offsets, signatures,
annotation texts) and verified by the `dartq-commands` golden tests against
live producer output. A deliberate shape change updates the golden in the
same change; an unexpected mismatch is a regression, not a golden to edit.
