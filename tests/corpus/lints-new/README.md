# lints-new — the 19-name promotion-wave differential corpus (#1160)

Differential corpus for the 19 lint rules promoted in the 2026-10 wave
(#1106/#1150/#1151/#1152 family, final promotion #1153). Every `lib/`
fixture is transcribed from the upstream linter test corpus (upstream Dart
SDK source, `pkg/linter/test/rules/<rule>_test.dart`, vendored SDK 3.15
main face); block headers carry each fixture's inventory (POSITIVE /
NEGATIVE / SKIPPED counts) and its expected oracle-leg row count, with
per-block provenance comments.

## The 19 names

`avoid_redundant_argument_values`, `avoid_slow_async_io`,
`cancel_subscriptions`, `close_sinks`, `comment_references`,
`deprecated_member_use_from_same_package`,
`future_sync_value`, `matching_super_parameters`,
`prefer_const_constructors`, `prefer_const_declarations`,
`prefer_const_literals_to_create_immutables`, `prefer_mixin`,
`remove_deprecations_in_breaking_versions`, `tighten_type_of_initializing_formals`,
`unnecessary_ignore`, `unnecessary_this_alias`, `use_enums`,
`use_named_constants`, `use_test_throws_matchers`.

`analysis_options.yaml` enables exactly these 19 and nothing else.

## Legs and expectations

`expectations.json` is the N5 gate's ground truth (serial window,
`scripts/serial-gates.sh` step `lints-new-gate`):

- `rules` — the 19 names (the gate filters machine rows to these codes;
  everything else on either leg is out of face).
- `expected` — the DARTQ-leg expectation, (code, file, line) rows, the
  multiset the gate enforces against `dartq analyze --format=machine`.
- `oracle` — the informational oracle-leg rows derived from the fixture
  headers (verified against the upstream tests' diagnostic markers).
- `adjudications` — the documented dartq-leg deviation families below.

### Adjudication families (dartq leg structurally silent)

1. `unnecessary_ignore` — the official rule body is the analyzer's
   `IgnoreValidator` post-pass (needs the whole diagnostic set of a
   location); dartq mounts the spec `not_dispatched` until an engine
   post-pass exists. Oracle: 1 row each in `unnecessary_ignore.dart` and
   `unnecessary_ignore_line.dart`; dartq: 0.
2. `remove_deprecations_in_breaking_versions` — registration is gated on
   the containing package's pubspec `version:` (the pubspec family's
   mounting seam). `lib/remove_deprecations_in_breaking_versions.dart` is
   the no-version control (0 rows both legs); the breaking twin
   `packages/version_breaking` fires 2 rows on the oracle leg and stays
   structurally silent on the dartq leg; the non-breaking twin
   `packages/version_minor` is 0 on both legs.
3. Generated-file parity probe — `test.g.dart` expects 0 rows on a face
   where generated-file lint suppression applies; a dartq-only row there
   is the known under-report approximation (upstream suppresses lints in
   `*.g.dart` files; whichever way each leg resolves it, both must agree).

## Ignore-comment interaction family

Fixtures transcribe upstream's `// ignore:` / `// ignore_for_file:`
comments verbatim. Where the ignored diagnostic is not produced on a leg
(e.g. `unused_field` on a used field), the ignore is itself unnecessary —
that is exactly the `unnecessary_ignore` rule's subject on the oracle leg.
These rows ride adjudication family 1 (dartq structurally silent); the
remaining rule rows are unaffected because the gate filters to the 19
codes.

## Language faces that cannot share a library

A `// @dart = N` comment governs the whole file, so language-gated faces
live in dedicated twin fixtures:

- `future_sync_value_pre.dart` (`// @dart = 3.9`): the rule's 3.10
  language gate bails — 0 rows both legs. A dartq row here is a @dart
  language-version resolution divergence.
- `prefer_mixin_pre.dart` (`// @dart = 2.19`): the pre-mixin-class
  language where `with` a plain class lints — 2 rows on the oracle leg,
  expected on the dartq leg too.

## Consuming this corpus

The root package and the `packages/` twins are hand-materialized into
`.dart_tool/package_config.json` (path deps into `vendor/`) — never
`pub get` this corpus. The N5 gate drives only the dartq leg:
`dartq analyze --format=machine --sdk <sdk> tests/corpus/lints-new`,
filtered to the 19 codes, multiset-compared against `expected`.
