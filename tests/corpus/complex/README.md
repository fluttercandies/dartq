# complex corpus

A single coherent Dart package simulating a realistically complex project:
a warehouse inventory and order-processing domain, organized as `lib/src`
modules exactly like production code. It exercises the Dart 3.13 language
surface at depth — the deliverable "complete test cases for a
super-complex project" — plus a `broken_*` fixture set carrying intentional
diagnostics for oracle-differential runs.

Package facts:

- `corpus_complex`, `environment.sdk: ^3.13.0`, effective language version
  3.13 (package_config self entry, harness preflight floor).
- One nested path dependency: `packages/stock_policy` (`stock_policy`), the
  cross-package import edge. The `.dart_tool/package_config.json` is
  hand-materialized (same convention as mega; both packages pin 3.13).
- Clean modules analyze with ZERO diagnostics under plain
  `dart analyze` at Dart 3.13.1 (no analysis_options, no lints, no
  experiment flags) — every diagnostic in the corpus is deliberate.

## Intentional-diagnostics convention

No `broken_`/`expect:` convention existed in m0/m1/mega, so this corpus
establishes one (m0-syntax names files after targeted codes; m1-semantic
uses header comments; both are single-code fixtures, too rigid for a
many-feature corpus):

- Intentionally-broken files live in `lib/src/broken/`, are named
  `broken_<topic>.dart`, are fully self-contained, and are never imported
  by any clean module nor by the barrel.
- Every intentionally-failing line carries its own one-line comment
  directly above it:

  ```dart
  // expect: undefined_identifier
  final total = unreportedCount + 1;
  ```

- `<CODE>` is the machine diagnostic name in lowercase
  (`DiagnosticCode.name` lowercased, same normalization as
  `syntax-codes.tsv` / `m1_codes.txt`). One comment per diagnostic; a line
  with two diagnostics would carry two stacked comments in the order the
  oracle reports them (current fixtures avoid that case).
- Trigger shapes are copied from the m1-semantic fixtures (gate-proven
  against the Dart 3.13.1 oracle at ERROR-tier Jaccard >= 0.95), including
  the measured variant drift inside the `wrong_number_of_type_arguments`
  family: invocations report `_element`, property-access tear-offs report
  `_function`, plain type annotations report the base code.

## Coverage matrix (feature -> files)

| Feature | Files |
| --- | --- |
| Extension types (members, operators, statics, const named ctors, generic extension over them) | core/identifiers, core/money |
| Generics: bounded, F-bounded, nested type args (depth >= 3), generic methods/extensions | domain/option_matrix, pricing/pricing_engine, pricing/tax_rules, reporting/table, core/identifiers |
| Sealed hierarchies + exhaustive switches (incl. across a part file) | domain/product (+product_views), domain/discount, domain/events, domain/shipment |
| Records: positional+named+mixed typedefs, literals, field access, destructuring (irrefutable record patterns mirror the type's positional/named shape; per-field annotations are ignored by the irrefutable check), pattern assignment, equality | domain/order, warehouse/route, reporting/summary, matching/destructuring, validation/validator, domain/customer |
| Pattern grammar: object/record/list/map/relational/logical-or/logical-and/cast/null-check/null-assert/wildcard/constant/parenthesized/guards | matching/pattern_matrix, matching/destructuring, domain/discount, domain/shipment, text/search |
| Promotion chains: is/is!, && / \|\| joins, ?? chains, if-case, private-final field promotion, closure interplay, catch-clause typing | flow/promotion_chains |
| Definite assignment: late final branches, ??=, exhaustive switch-expression init | flow/definite_assignment |
| Const evaluation: const ctors, initializer-list asserts, const collections (map of lists, sets), const tables, adjacent metadata | domain/catalog, core/timestamps, core/units, text/label_format, annotations/annotations |
| Class modifiers: abstract/interface/base/final/sealed/mixin class/base on-clause mixin/mixin application | domain/shipment, registry/component_registry, warehouse/picker |
| Mixins (stateful, generic, on-clause, lifecycle hooks) | registry/lifecycle, registry/component_registry, domain/shipment |
| Constructors: named, factory, redirecting factory, private, initializing formals, super params, initializer asserts | domain/customer, domain/shipment, warehouse/picker, core/timestamps |
| Operator overload matrix: + - * unary- < <= > >= == [] []= & \| ^ ~ << >> >>> ~/ % call | operators/operator_matrix |
| Extensions with complex on-clauses (Iterable, Map<K,List<V>>, generic bounded on-clause) | pricing/tax_rules, text/label_format, text/search, reporting/table |
| Typedefs: generic aliases, function types, record types, class-aliased usage | core/identifiers, pricing/discount_pipeline, domain/order, validation/validator, reporting/table, warehouse/route |
| Enum surface: enhanced enums (fields, methods, statics, implements), constant-pattern switches | domain/order_status, core/units, matching/destructuring, flow/definite_assignment, warehouse/zoning, text/search |
| Nullability: nullable generics fields, ?? / ??= / ?. / ...? / null-aware spread, promoted private finals | flow/promotion_chains, pricing/discount_pipeline, domain/customer |
| Primary constructors (stable 3.13) | warehouse/bin |
| Dot shorthands (stable 3.10): assignment, comparison, argument, collection-element and map-value positions (the context type must carry the static type — an `Object?` index parameter does not) | warehouse/zoning |
| Null-aware elements (stable 3.8): list value `?e`, map `k: ?v` and `?k:` with a genuinely nullable key variable (a constant key makes `?k:` dead code) | warehouse/zoning |
| Digit separators (stable 3.6) | core/units, warehouse/zoning, domain/catalog, annotations/annotations |
| Metadata: on library directive, classes, members, params, type params; adjacent annotations | annotations/annotations |
| String surface: interpolation nesting, adjacent literals, raw strings, multiline const template | text/label_format |
| Async: async/await, async* streams, await-for, Future composition, timeouts | notifications/audit_log |
| Import graph: relative chains, show/hide, prefixed imports, re-export, package: self-import, deferred import | import_graph/deep_core..deep_edge, import_graph/deferred_report, lib/complex.dart |
| Cross-package import (nested packages/ layout per mega) | validation/validator -> packages/stock_policy |
| parts | domain/product.dart + product_views.dart |
| Cascades, spread, collection-if/for | reporting/table, domain/catalog, pricing/discount_pipeline |

## Intentional diagnostics (broken_*)

| File | Codes |
| --- | --- |
| broken_undefined_identifier.dart | undefined_identifier x2 |
| broken_undefined_class.dart | undefined_class x2 |
| broken_undefined_member.dart | undefined_method, undefined_getter, undefined_setter |
| broken_argument_types.dart | argument_type_not_assignable, invalid_assignment, return_of_invalid_type |
| broken_type_arguments.dart | wrong_number_of_type_arguments, wrong_number_of_type_arguments_element, wrong_number_of_type_arguments_function |
| broken_undefined_extension.dart | undefined_extension_method, undefined_extension_setter |
| broken_null_aware.dart | invalid_null_aware_operator x2 |
| broken_undefined_enum.dart | undefined_enum_constant x2 |
| broken_missing_uri.dart | uri_does_not_exist x2 |
| broken_duplicate.dart | duplicate_definition |
| broken_named_parameter.dart | undefined_named_parameter, undefined_function |

All codes are in the frozen M1 universe (`tests/harness/m1_codes.txt`).

## Measured oracle facts (r1 fix round, dart 3.13.1)

Behavior facts the r1 oracle run measured on these exact shapes; the corpus
follows them, and any future shape change must too:

- Extension-type representation constructors are NOT const: `Money(0)` is
  rejected in const contexts (CONST_WITH_NON_CONST), including parameter
  defaults. A user-declared const named constructor on an extension type
  (`const CustomerRef.anonymous()`) is accepted.
- Extension types cannot declare `toString`/`hashCode` (Object member names
  are reserved; EXTENSION_TYPE_DECLARES_MEMBER_OF_OBJECT). Formatting goes
  through an explicit `Money.display()`.
- A relational pattern (or a relational operator in a guard) is checked
  against the CURRENT matched type: on `Object?` it is UNDEFINED_OPERATOR.
  Relationals need a type context — an and-pattern left side (`int amount
  && > 0`) or an object-pattern field (`Money(cents: > 0)`).
- A `base` supertype forces `base` on any on-clause mixin and on any mixin
  application that extends it (SUBTYPE_OF_BASE_OR_FINAL_IS_NOT_BASE_...).
- F-bounded generics: `PricingRule<Object>` violates the bound
  `Self extends PricingRule<Self>` (TYPE_ARGUMENT_NOT_MATCHING_BOUNDS);
  heterogeneous collections hold the erased non-generic base instead.
- Irrefutable record patterns: the checker builds the required type with
  `Object?` for every field (hardcoded in the shared analyzer's
  `analyzeRecordPattern`) and keys the whole check on the pattern's
  positional/named SHAPE matching the matched record type. On a shape
  mismatch every field is dispatched as `Object?`
  (PATTERN_TYPE_MISMATCH_IN_IRREFUTABLE_CONTEXT on the outer pattern and on
  each field). Per-field annotations neither fix nor influence the check —
  destructure with bare variable patterns in the type's exact shape.
- Guard-usage counts as binding use, but an unused binding anywhere in an
  arm is UNUSED_LOCAL_VARIABLE; a relational pattern arm can make a later
  same-shape arm unreachable, and cast patterns never fail (they throw), so
  an arm after a cast arm must not only cover the cast's failure cases.
- Code attribution drift vs the M1 fixtures: initializer/assignment type
  mismatches report invalid_assignment (argument_type_not_assignable needs
  an ARGUMENT position); an undefined name in type-argument position
  reports non_type_as_type_argument (outside M1 — avoided here).

## Deliberately deferred

- **Augmentations (`augment`)**: at the 3.13 oracle caliber the feature is
  disabled by default (`augmentations` has no `enabledIn` in
  `experimental_features.yaml` @ 3.13.1; the scanner lexes `augment` as an
  identifier — abstract_scanner.dart:2106). No per-file enablement exists:
  `// @dart` banners can only LOWER the version, and experiments need
  `--enable-experiment`, which the recorded oracle caliber does not carry.
  Adding `augment` fixtures now would only produce oracle output that is
  not statically predictable (identifier-vs-keyword recovery cascades).
  Revisit when the corpus gains an experiment-flag caliber.
- **Experiment-gated families** (enhanced parts, macros): same reasoning as
  augmentations.
- **record-use (3.13)**: shipped, but its observable static surface
  (output-argument use) is resolution-heavy and adds oracle-noise risk with
  no parser-side payoff; deferred.
- **Legacy language versions**: owned by `tests/corpus/mixed-versions`;
  this corpus deliberately pins 3.13 everywhere (harness preflight floor).

## Wiring notes

Not wired into the m0/m1 gates yet. A future gate can consume it as-is:
the harness corpus preflight is satisfied (package_config self entry,
languageVersion 3.13), the oracle runs plain `dart analyze`, and the
`// expect:` comments are greppable for a three-way check (oracle vs dartq
vs expect-file).
