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

### Oracle-verified additions — 2026-10-12

27 new files widened the family map below (ticket #918). Every file was
admitted through the oracle protocol: copied into a `/tmp` probe package
(`environment: sdk: ^3.13.0`), then run under fvm dart 3.13.1
`dart analyze --format=machine` — **zero rows of any severity** is the
admission bar. The file contents are byte-identical to the oracle-verified
probe copies modulo header comments. All are also parseable by the 3.13.1
oracle because every shipped feature of the 3.13–3.15 window landed at or
before 3.13 (primary constructors `enabledIn 3.13.0`, private named
parameters 3.12.0, dot shorthands 3.10.0); see the `modern/` note below.

| File | Family | Official provenance (`.third/dart-sdk/`) | Verified |
| --- | --- | --- | --- |
| `lib/enhanced_enums.dart` | generative/const/factory enum constructors, fields, mixins/implements, generic enums with bounds, static members | `tests/language/enum/` (enhanced_enums_basic_test, syntax_test) | 2026-10-12 |
| `lib/mixins.dart` | mixin declarations (on/super, base mixin), mixin application classes, multi-mixin chains | `tests/language/mixin_declaration/`, `tests/language/mixin/` | 2026-10-12 |
| `lib/generics.dart` | generic classes/methods, extends/F-bounded/nullable bounds, nested generics, instantiate-to-bound use | `tests/language/generic/`, `tests/language/generic_methods/`, `tests/language/instantiate_to_bound/` | 2026-10-12 |
| `lib/typedefs.dart` | old function aliases, new general typedefs, generic/nullable typedefs, typedefs of records | `tests/language/typedef/`, `tests/language/nonfunction_type_aliases/` | 2026-10-12 |
| `lib/extension_types.dart` | representation declarations, const/private/factory constructors, generic + nullable representations, implements | `tests/language/extension_type/` | 2026-10-12 |
| `lib/extensions.dart` | named/unnamed/generic extensions, static members, explicit application, conflict resolution by name | `tests/language/extension_methods/` | 2026-10-12 |
| `lib/late_final.dart` | late top-level/static/instance fields, late final lazy init, initializer lists, definite assignment | `tests/language/final/`, `tests/language/variable/`, `tests/language/local_null_initialization.dart` | 2026-10-12 |
| `lib/constructors.dart` | const/initializing formals, initializer lists with asserts, factory + redirecting factory + const factory redirect, super parameters | `tests/language/constructor/`, `tests/language/factory/`, `tests/language/redirecting/`, `tests/language/initializing_formal/` | 2026-10-12 |
| `lib/operators.dart` | full overloadable-operator set incl. `>>>`, `[]`/`[]=`, `call`, unary `-`/`~`, `==`, `compareTo`, `++`/`--` usage | `tests/language/operator/` | 2026-10-12 |
| `lib/string_literals.dart` | raw/adjacent/multiline strings, simple/braced/nested interpolation, escapes, const interpolation | `tests/language/string/` | 2026-10-12 |
| `lib/number_literals.dart` | decimal/hex/leading-zero ints, doubles, signed exponents, mantissa-exponent forms | `tests/language/number/`, `tests/language/double_literals/` | 2026-10-12 |
| `lib/collection_literals.dart` | typed/const collections, spread + null-aware spread, collection-if (with else), collection-for | `tests/language/control_flow_collections/`, `tests/language/spread_collections/`, `tests/language/set_literals/` | 2026-10-12 |
| `lib/labels.dart` | labels on loops/blocks, labeled break/continue, labels across switch | `tests/language/label/` | 2026-10-12 |
| `lib/cascades.dart` | cascade sections over methods/getters/setters/index, nested cascades, null-aware cascades | `tests/language/cascade/` | 2026-10-12 |
| `lib/async_await.dart` | async/async*/sync* functions, await in bodies/expressions, await-for, yield/yield*, async closures | `tests/language/async/`, `tests/language/async_star/`, `tests/language/sync_star/` | 2026-10-12 |
| `lib/null_safety_edges.dart` | local + private-final field promotion, `??`/`??=`/`?.`/`!`, null-aware chains, is/is! narrowing | `tests/language/null/`, `tests/language/null_aware/`, `tests/language/type_promotion/` | 2026-10-12 |
| `lib/control_flow.dart` | switch statements (grouped/empty/constant/default cases), loops, try/on/catch/finally, rethrow, assert | `tests/language/switch/`, `tests/language/if/`, `tests/language/loop/`, `tests/language/exception/` | 2026-10-12 |
| `lib/metadata.dart` | annotations on library/classes/type params/members/params/enum values/typedefs/locals/loop vars | `tests/language/metadata/` | 2026-10-12 |
| `lib/getters_setters.dart` | top-level/static/instance accessor pairs, abstract accessors, covariant setters | `tests/language/getter/`, `tests/language/setter/` | 2026-10-12 |
| `lib/external_abstract.dart` | external top-level functions/accessors/constructors/statics, abstract fields and members | `tests/language/external_abstract_fields/`, `tests/language/abstract/` | 2026-10-12 |
| `lib/constants.dart` | const constructors/fields/collections, canonicalization, const ternary + interpolation, const record sets | `tests/language/const/`, `tests/language/constants_2018/` | 2026-10-12 |
| `lib/functions.dart` | closures over all parameter shapes, constructor tear-offs + generic instantiation, callable classes, named args anywhere | `tests/language/function/`, `tests/language/named_arguments_anywhere/` | 2026-10-12 |
| `lib/wildcard_variables.dart` | non-binding `_` (repeated params, for-in, list/map patterns, catch), binding `_`-prefixed locals | `tests/language/wildcard_variables/` | 2026-10-12 |
| `lib/private_named_parameters.dart` | primary-ctor declaring params + `this._x` formals with private names, public-name invocation | `tests/language/private_named_parameters/` | 2026-10-12 |
| `lib/directives.dart` | library, deferred import + `loadLibrary`, import combinators, export combinators, part directive | `tests/language/library/`, `tests/language/part/`, `tests/language/import/`, `tests/language/prefix/`, `tests/language/deferred/` | 2026-10-12 |
| `lib/directives_part.dart` | companion `part` unit (string `part of`, private shared declarations) | `tests/language/part/`, `tests/language/library/` | 2026-10-12 |
| `lib/deferred_impl.dart` | companion deferred-import target | `tests/language/deferred/` | 2026-10-12 |

### The `modern/` sub-suite (empty as of 2026-10-12)

Rule for future additions: a file that is valid for the 3.15 grammar window
but NOT parseable by the 3.13.1 oracle (3.14/3.15-only shipped syntax) goes
to `modern/` instead of `lib/`, and its README row must state that its
oracle face is the implement face (3.15-main source-anchored), not the
3.13.1 differential. As of 2026-10-12 the suite is empty: no shipped feature
in the window has `enabledIn` above 3.13 (`tools/experimental_features.yaml`),
so every admitted file passed the 3.13.1 differential. Experiment-gated
families remain excluded per the section below.

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

Every included file mirrors shipped-feature syntax taken from the official
language tests, and shipped features are enabled at the check face's default
language version. Any first-run row is a gate defect to fix at its reported
(code, file), not a corpus guess to adjudicate. One such defect is already
known from the 2026-10-12 admission sweep (oracle-clean file, current-binary
row — a fix ticket, not a corpus change):

- `lib/constants.dart` line 54 — `const canonicalPairs = <(int, int)>{(1, 2),
  (3, 4)};`: dartq emits `ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|...|54|24|1|
  Expected '[' before this.` on the record type argument `<(int, int)>` of a
  const set literal. fvm dart 3.13.1 reports no issues for this file.
