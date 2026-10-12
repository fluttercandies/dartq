# Valid-syntax acceptance corpus

"all syntax supported" as a measurable invariant: every file here is valid
Dart for the 3.15 grammar window, and the gate asserts the engine reports
**zero diagnostics of every tier** over this directory. One-sided by design —
no oracle is involved; validity is asserted by construction, with every file
modeled on the official language tests cited in its header comment.

Gate: `scripts/acceptance-gate.sh` (repo root) — runs
`./target/release/dartq check --format=machine` over this directory and fails
on any row (rule rationale in the script header). Ladder position: alongside
the differential gates (m0 → m1 → m2 → m3 → lint → sdk-matrix → mega); as a
one-sided gate it can also run standalone at any time.

## Gate rule

Fail on ANY machine row (`SEVERITY|TYPE|CODE|FILE|LINE|COL|LENGTH|MESSAGE`)
of any tier. The check face is syntax-tier only (scan → parse, no semantic
engine), so a valid corpus must yield zero rows of every severity — there are
no known-deliberate advisory rows in this corpus.

## Families

| File | Family | Official provenance (`​.third/dart-sdk/`) |
| --- | --- | --- |
| `lib/patterns.dart` | all pattern forms (constant, variable, wildcard, parenthesized, logical-or/and, relational, cast, null-check, null-assert, list+rest, map, record, object, guards, switch statement/expression, if-case, declarations, assignments, for-in) | `tests/language/patterns/`, `pkg/front_end/parser_testcases/patterns/` |
| `lib/records.dart` | record literals, positional/named/mixed types, field access, nested and empty records | `tests/language/records/simple/literals_and_field_access_test.dart`, `tests/language/record_type_test.dart` |
| `lib/class_modifiers.dart` | sealed / base / final / interface / abstract / mixin class / base mixin, in-library use, sealed exhaustive switch | `tests/language/class_modifiers/`, `tests/language/sealed_class/` |
| `lib/primary_constructors.dart` | primary constructor headers, `;` bodies, `class const`, `new` constructors, mixin/extension-type forms | `tests/language/primary_constructors/{feature_enabled_test,syntax/class_header_test,syntax/empty_body_test,short_syntax/new_test}.dart` |
| `lib/dot_shorthands.dart` | enum/static/extension-type/constructor/const shorthands, chains, equality, relational-pattern face | `tests/language/dot_shorthands/` (feature_enabled, helper, member/, equality/, multiple/) |
| `lib/null_aware_elements.dart` | `?e` list/set elements, `?k: v`, `k: ?v`, mutable + const | `tests/language/null_aware_elements/const_literals_test.dart` |
| `lib/triple_shift.dart` | `>>>` declaration/use, `>>>=`, precedence, `#>>>` symbol, extension `>>>` | `tests/language/operator/operator_triple_shift_test.dart` |
| `lib/digit_separators.dart` | `_`/`__` separators in int/hex/double/exponent literals, coercion, const | `tests/language/number/separators_test.dart` |
| `lib/kitchen_sink.dart` | all shipped families combined | per-family tests cited above |

The corpus is a valid package (`pubspec.yaml`, `environment: sdk: ^3.15.0`).
No `analysis_options.yaml` on purpose: the `dartq check` face is options-free
by design (`crates/dartq-commands/src/check.rs`, `analyze_file` — the feature
set comes from the language version only). `dart pub get` is NOT required by
the gate; run it (with a >= 3.15 SDK) only if you want the package to resolve
for tooling.

## Pending until enabled (excluded from this gate)

Syntax the 3.15 window accepts only behind experiments that the `dartq check`
face cannot enable (no `--enable-experiment` flag, no analysis_options
support; `ScanConfig::enable_augmentations` / `ParserFeatures::variance_enabled`
stay off). Do not add files for these families until the check face grows
experiment plumbing or the features ship:

- **augmentations** — `augment` declarations (lexical + syntactic gate;
  oracle-facing faces need the analysis_options double-enable, measured in
  `docs/facts/version-skew.md` §1.3 T5)
- **variance** — `in` / `out` / `inout` type-parameter modifiers
  (`docs/facts/version-skew.md` §1.3 T3/T4)
- **enhanced-parts**, **macros**, **data-assets**, **unquoted-imports**,
  **const-functions**, **static-extensions**, **anonymous-methods**,
  **this-promotion**, **inference-update-4** — experimental windows with no
  `enabledIn` in `crates/dartq-language/data/experimental_features.yaml`

## Needs first-run confirmation

(none) — every included file mirrors shipped-feature syntax taken verbatim
from the official language tests, and shipped features are enabled at the
check face's default language version. Any first-run row is a gate defect to
fix at its reported (code, file), not a corpus guess to adjudicate.
