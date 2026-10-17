# solid_lints corpus

Trigger fixtures for the vendored `solid_lints` 2.0.0 plugin
(`vendor/solid_lints-2.0.0`), one file per rule, defaults-only posture —
the package's nonstandard `solid_lints:` options key is not a dartq v1
config face, so every rule runs at its built-in default (recon R4/R5).

Every registered rule (see `lib/main.dart`'s `lintRules` list — 35 rules
registered via `registerWarningRule`/`registerLintRule` proxies) that the
package's own `test/src/lints/` suite exercises gets one minimal trigger,
distilled from the flagged `assertDiagnostics` source to a single shape.

| File | Rule |
| --- | --- |
| `avoid_debug_print_in_release.dart` | `avoid_debug_print_in_release` |
| `avoid_duplicate_code.dart` | `avoid_duplicate_code` |
| `avoid_final_with_getter.dart` | `avoid_final_with_getter` |
| `avoid_global_state.dart` | `avoid_global_state` |
| `avoid_late_keyword.dart` | `avoid_late_keyword` |
| `avoid_multiple_declarations_per_file.dart` | `avoid_multiple_declarations_per_file` |
| `avoid_non_null_assertion.dart` | `avoid_non_null_assertion` |
| `avoid_returning_widgets.dart` | `avoid_returning_widgets` |
| `avoid_similar_names.dart` | `avoid_similar_names` |
| `avoid_unnecessary_return_variable.dart` | `avoid_unnecessary_return_variable` |
| `avoid_unnecessary_setstate.dart` | `avoid_unnecessary_setstate` |
| `avoid_unnecessary_type_assertions.dart` | `avoid_unnecessary_type_assertions` |
| `avoid_unrelated_type_assertions.dart` | `avoid_unrelated_type_assertions` |
| `avoid_unused_parameters.dart` | `avoid_unused_parameters` |
| `avoid_using_api.dart` | `avoid_using_api` |
| `cyclomatic_complexity.dart` | `cyclomatic_complexity` |
| `double_literal_format.dart` | `double_literal_format` |
| `feature_envy.dart` | `feature_envy` |
| `function_lines_of_code.dart` | `function_lines_of_code` |
| `member_ordering.dart` | `member_ordering` |
| `named_parameters_ordering.dart` | `named_parameters_ordering` |
| `newline_before_return.dart` | `newline_before_return` |
| `no_empty_block.dart` | `no_empty_block` |
| `no_equal_then_else.dart` | `no_equal_then_else` |
| `no_magic_number.dart` | `no_magic_number` |
| `number_of_parameters.dart` | `number_of_parameters` |
| `prefer_conditional_expressions.dart` | `prefer_conditional_expressions` |
| `prefer_early_return.dart` | `prefer_early_return` |
| `prefer_first.dart` | `prefer_first` |
| `prefer_last.dart` | `prefer_last` |
| `prefer_match_file_name.dart` | `prefer_match_file_name` |
| `prefer_mirror_test_structure.dart` | `prefer_mirror_test_structure` |
| `proper_super_calls.dart` | `proper_super_calls` |
| `use_descriptive_names_for_type_parameters.dart` | `use_descriptive_names_for_type_parameters` |
| `use_nearest_context.dart` | `use_nearest_context` |

Coverage: **35 / 35 registered rules**. Every rule in `main.dart`'s
`lintRules` list has an upstream `test/src/lints/<rule>/` suite, so none
are skipped.

Known caveats:

- The package registers through `registerLintRule`/`registerWarningRule`
  proxies (`ProxyAnalysisRule`/`ProxyMultiAnalysisRule`); rule kind and
  parse eligibility are read from the live census
  (`scripts/plugin-census.sh`), not from this table.
- Threshold-driven rules (`cyclomatic_complexity`, `function_lines_of_code`,
  `number_of_parameters`, `avoid_similar_names`) carry a default-posture
  shape sized past the default bound; `member_ordering` and
  `named_parameters_ordering` are config/order-driven and use the default
  order.
- `prefer_match_file_name` and `prefer_mirror_test_structure` are
  name/path-sensitive: the trigger encodes the flagged *shape* and the
  harness feeds it under the mismatched name/`lib` path the rule keys on.
- `avoid_using_api` and `avoid_duplicate_code` are config/cross-file by
  nature; their triggers encode the flagged AST surface under default
  posture.
