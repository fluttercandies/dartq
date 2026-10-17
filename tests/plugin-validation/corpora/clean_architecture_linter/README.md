# clean_architecture_linter corpus

Trigger fixtures for the vendored `clean_architecture_linter` 2.8.0 plugin
(`vendor/clean_architecture_linter-2.8.0`). The package's v2 rules are
layer-sensitive: the upstream harness feeds files at feature paths
(`lib/features/<feature>/{domain,data,presentation}/...`) and keys
diagnostics on those paths, so the corpus mirrors that layout — the preset
trees sync this directory's `lib/` subtree verbatim.

Every registered rule (see `lib/main.dart`'s `register` — 37
`registerWarningRule`/`registerLintRule` calls) that the package's own
`test/src/rules/` suite exercises gets one minimal trigger at the layer
path the rule keys on, distilled from the flagged `analyze(files: ...)`
source to a single shape. Rules keyed on cross-file imports carry a paired
`*_target.dart` or `circular_dependency_{a,b}.dart` second file.

| File (under `lib/features/todo/`) | Rule | Upstream source |
| --- | --- | --- |
| `domain/usecases/allowed_instance_variables.dart` | `allowed_instance_variables` | `cross_layer/allowed_instance_variables_rule_test.dart` |
| `domain/usecases/boundary_crossing.dart` (+ `data/repositories/boundary_crossing_target.dart`) | `boundary_crossing` | `cross_layer/boundary_crossing_rule_test.dart` |
| `domain/circular_dependency_{a,b}.dart` | `circular_dependency` | `cross_layer/circular_dependency_rule_test.dart` |
| `presentation/pages/layer_dependency.dart` (+ `data/repositories/layer_dependency_target.dart`) | `layer_dependency` | `cross_layer/layer_dependency_rule_test.dart` |
| `data/datasources/datasource_abstraction.dart` | `datasource_abstraction` | `data_rules/datasource_abstraction_v2_rule_test.dart` |
| `data/datasources/datasource_exception_types.dart` | `datasource_exception_types` | `data_rules/datasource_exception_types_v2_rule_test.dart` |
| `data/datasources/datasource_no_result_return.dart` | `datasource_no_result_return` | `data_rules/datasource_no_result_return_v2_rule_test.dart` |
| `data/models/model_conversion_methods.dart` | `model_conversion_methods` | `data_rules/model_conversion_methods_v2_rule_test.dart` |
| `data/repositories/model_entity_direct_access.dart` | `model_entity_direct_access` | `data_rules/model_entity_direct_access_v2_rule_test.dart` |
| `data/models/todo_model.dart` | `model_field_duplication` | `data_rules/model_field_duplication_v2_rule_test.dart` |
| `data/models/model_naming_convention.dart` | `model_naming_convention` | `data_rules/model_naming_convention_v2_rule_test.dart` |
| `data/models/model_structure.dart` | `model_structure` | `data_rules/model_structure_v2_rule_test.dart` |
| `data/repositories/repository_implementation.dart` | `repository_implementation` | `data_rules/repository_implementation_v2_rule_test.dart` |
| `data/repositories/repository_no_throw.dart` | `repository_no_throw` | `data_rules/repository_no_throw_v2_rule_test.dart` |
| `data/repositories/todo_repository_impl.dart` | `repository_pass_through` | `data_rules/repository_pass_through_v2_rule_test.dart` |
| `domain/usecases/dependency_inversion.dart` (+ `data/repositories/dependency_inversion_target.dart`) | `dependency_inversion` | `domain_rules/dependency_inversion_v2_rule_test.dart` |
| `domain/entities/domain_purity.dart` | `domain_purity` | `domain_rules/domain_purity_v2_rule_test.dart` |
| `domain/repositories/todo_repository.dart` | `repository_interface` | `domain_rules/repository_interface_v2_rule_test.dart` |
| `domain/usecases/get_todo_usecase.dart` | `usecase_no_result_return` | `domain_rules/usecase_no_result_return_v2_rule_test.dart` |
| `domain/exceptions/exception_naming_convention.dart` | `exception_naming_convention` | `domain_rules/exception_naming_convention_v2_rule_test.dart` |
| `presentation/extensions/extension_location.dart` | `extension_location` | `presentation_rules/extension_location_v2_rule_test.dart` |
| `presentation/states/freezed_usage.dart` | `freezed_usage` | `presentation_rules/freezed_usage_v2_rule_test.dart` |
| `presentation/models/no_presentation_models.dart` | `no_presentation_models` | `presentation_rules/no_presentation_models_v2_rule_test.dart` |
| `presentation/pages/presentation_no_data_exceptions.dart` | `presentation_no_data_exceptions` | `presentation_rules/presentation_no_data_exceptions_v2_rule_test.dart` |
| `presentation/providers/presentation_no_throw.dart` | `presentation_no_throw` | `presentation_rules/presentation_no_throw_rule_test.dart` |
| `presentation/states/presentation_use_async_value.dart` | `presentation_use_async_value` | `presentation_rules/presentation_use_async_value_v2_rule_test.dart` |
| `presentation/providers/ref_mounted_usage.dart` | `ref_mounted_usage` | `presentation_rules/ref_mounted_usage_v2_rule_test.dart` |
| `presentation/providers/riverpod_extension_scoped_dependency.dart` | `riverpod_extension_scoped_dependency` | `presentation_rules/riverpod_extension_scoped_dependency_v2_rule_test.dart` |
| `presentation/providers/riverpod_generator.dart` | `riverpod_generator` | `presentation_rules/riverpod_generator_v2_rule_test.dart` |
| `presentation/providers/riverpod_keep_alive.dart` | `riverpod_keep_alive` | `presentation_rules/riverpod_keep_alive_v2_rule_test.dart` |
| `presentation/providers/riverpod_provider_naming.dart` | `riverpod_provider_naming` | `presentation_rules/riverpod_provider_naming_v2_rule_test.dart` |
| `presentation/providers/riverpod_ref_after_async_gap.dart` | `riverpod_ref_after_async_gap` | `presentation_rules/riverpod_ref_after_async_gap_v2_rule_test.dart` |
| `presentation/providers/riverpod_ref_usage.dart` | `riverpod_ref_usage` | `presentation_rules/riverpod_ref_usage_v2_rule_test.dart` |
| `presentation/providers/riverpod_state_after_async_gap.dart` | `riverpod_state_after_async_gap` | `presentation_rules/riverpod_state_after_async_gap_v2_rule_test.dart` |
| `presentation/providers/riverpod_uncancelled_disposable.dart` | `riverpod_uncancelled_disposable` | `presentation_rules/riverpod_uncancelled_disposable_v2_rule_test.dart` |
| `presentation/pages/widget_no_usecase_call.dart` | `widget_no_usecase_call` | `presentation_rules/widget_no_usecase_call_v2_rule_test.dart` |
| `presentation/pages/widget_ref_read_then_when.dart` | `widget_ref_read_then_when` | `presentation_rules/widget_ref_read_then_when_v2_rule_test.dart` |

Coverage: **37 / 37 registered rules** (36 `registerWarningRule` + 1 opt-in
`registerLintRule`, `riverpod_state_after_async_gap`). `TestCoverageRule`
exists in `lib/src/rules/cross_layer/` with a test file but is **not
registered** in `lib/main.dart`, so it is excluded from the registered set
(no trigger).

Known caveats:

- The v2 harness analyzes a synthetic multi-file package keyed on feature
  paths; several rules are also resolution-flavored (riverpod provider
  typing, `Result` contracts, freezed annotations). Triggers carry free
  names/inline stand-ins so every file parses standalone, and the
  parse-only boundary — not a missing import — decides firing. Ground
  truth comes from the live census run (`scripts/plugin-census.sh`), not
  this table (R2).
- `riverpod_state_after_async_gap` is the package's only opt-in lint
  (registered via `registerLintRule`); its trigger is included for
  completeness but only fires when explicitly enabled.
- `riverpod_*` rules need the riverpod package in the analyzed graph to
  resolve `@riverpod`/`ref` semantics; their shapes reproduce the flagged
  AST surface under the host's execution boundary.
