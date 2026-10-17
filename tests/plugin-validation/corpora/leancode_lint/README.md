# leancode_lint corpus

Trigger fixtures for the vendored `leancode_lint` 27.0.0 plugin
(`vendor/leancode_lint-27.0.0`), one file per rule, kept standalone: the
upstream suites embed snippets in test literals behind mocked package
imports, so each trigger de-templatizes one flagged case and replaces the
mocks with inline stand-ins.

Every registered rule (see `lib/plugin.dart`'s `register` — 21
`registerWarningRule` entries plus the config-generated
`UseDesignSystemItem` family, and 1 opt-in `registerLintRule`) that the
package's own `test/test_cases/` suite exercises gets one minimal trigger.

| File | Rule | Upstream source |
| --- | --- | --- |
| `add_cubit_suffix_for_your_cubits.dart` | `add_cubit_suffix_for_your_cubits` | `test_cases/add_cubit_suffix_for_cubits_test.dart` |
| `avoid_build_context_in_blocs.dart` | `avoid_build_context_in_blocs` | `test_cases/avoid_build_context_in_blocs_test.dart` |
| `avoid_catch_error.dart` | `avoid_catch_error` | `test_cases/avoid_catch_error_test.dart` |
| `avoid_conditional_hooks.dart` | `avoid_conditional_hooks` | `test_cases/avoid_conditional_hooks_test.dart` |
| `avoid_context_read_in_build.dart` | `avoid_context_read_in_build` | `test_cases/avoid_context_read_in_build_test.dart` |
| `avoid_direct_collection_equality_checks.dart` | `avoid_direct_collection_equality_checks` | `test_cases/avoid_direct_collection_equality_checks_test.dart` |
| `avoid_single_child_in_multi_child_widgets.dart` | `avoid_single_child_in_multi_child_widgets` | `test_cases/avoid_single_child_in_multi_child_widget_test.dart` |
| `bloc_related_class_naming.dart` | `bloc_related_class_naming` | `test_cases/bloc_related_class_naming_test.dart` |
| `bloc_subclasses_naming.dart` | `bloc_subclasses_naming` | `test_cases/bloc_subclasses_naming_test.dart` |
| `catch_parameter_names.dart` | `catch_parameter_names` | `test_cases/catch_parameter_names_test.dart` |
| `constructor_parameters_and_fields_should_have_the_same_order.dart` | `constructor_parameters_and_fields_should_have_the_same_order` | `test_cases/constructor_parameters_and_fields_should_have_the_same_order_test.dart` |
| `hook_widget_does_not_use_hooks.dart` | `hook_widget_does_not_use_hooks` | `test_cases/hook_widget_does_not_use_hooks_test.dart` |
| `missing_equatable_props.dart` | `missing_equatable_props` | `test_cases/missing_equatable_props_test.dart` |
| `never_discard_build_context.dart` | `never_discard_build_context` | `test_cases/never_discard_build_context_test.dart` |
| `prefer_abstract_final_class.dart` | `prefer_abstract_final_class` | `test_cases/prefer_abstract_final_class_test.dart` |
| `prefer_equatable_mixin.dart` | `prefer_equatable_mixin` | `test_cases/prefer_equatable_mixin_test.dart` |
| `prefix_widgets_returning_slivers.dart` | `prefix_widgets_returning_slivers` | `test_cases/prefix_widgets_returning_slivers_test.dart` |
| `start_comments_with_space.dart` | `start_comments_with_space` | `test_cases/start_comments_with_space_test.dart` |
| `use_align.dart` | `use_align` | `test_cases/use_align_test.dart` |
| `use_dedicated_media_query_methods.dart` | `use_dedicated_media_query_methods` | `test_cases/use_dedicated_media_query_methods_test.dart` |
| `use_design_system_item.dart` | `use_design_system_item_<item>` | `test_cases/use_design_system_item_test.dart` |
| `use_padding.dart` | `use_padding` | `test_cases/use_padding_test.dart` |

Coverage: **22 / 22 registered rules** (20 named `registerWarningRule` codes
+ the config-driven `use_design_system_item_<item>` family + 1 opt-in
`registerLintRule` `constructor_parameters_and_fields_should_have_the_same_order`).
`PreferCenterOverAlign` is not registered (its registration line is
commented out in `lib/plugin.dart`), so it is excluded.

Known caveats:

- The inline `Bloc`/`BuildContext`/`HookWidget`/`Equatable` stand-ins are
  name-level: rules whose checks need resolved supertype information may
  not fire under the host's parse-only execution (R2). Expected findings
  come from the live census run (`scripts/plugin-census.sh`) and the
  differential, never from this table.
- `use_design_system_item_<item>` is config-generated: the registered code
  name embeds the preferred item (e.g. `use_design_system_item_LftText`);
  the trigger reproduces the flagged usage shape under default posture.
- `prefer_abstract_final_class`, `catch_parameter_names`, and the
  collection-equality / comment shapes are pure-AST and parse-eligible by
  construction.
