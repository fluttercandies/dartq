# dart_code_linter corpus

Trigger fixtures for the vendored `dart_code_linter` 4.4.2 plugin
(`vendor/dart_code_linter-4.4.2`), one file per rule. Every registered rule
(see `lib/src/analysis_server_plugin/dart_code_linter_plugin.dart`, which
registers every id in `rules_factory.dart`'s `_implementedRules`) that the
package's own suite exercises gets one minimal trigger, distilled from the
rule's `test/src/analyzers/lint_analyzer/rules/rules_list/<rule>/examples/`
fixture to a single flagged shape.

| File | Rule id | Upstream source |
| --- | --- | --- |
| `always_remove_listener.dart` | `always-remove-listener` | `rules_list/always_remove_listener/examples/example.dart` |
| `arguments_ordering.dart` | `arguments-ordering` | `rules_list/arguments_ordering/examples/*_example.dart` |
| `avoid_banned_imports.dart` | `avoid-banned-imports` | `rules_list/avoid_banned_imports/examples/example.dart` |
| `avoid_border_all.dart` | `avoid-border-all` | `rules_list/avoid_border_all/examples/example.dart` |
| `avoid_cascade_after_if_null.dart` | `avoid-cascade-after-if-null` | `rules_list/avoid_cascade_after_if_null/examples/example.dart` |
| `avoid_collection_methods_with_unrelated_types.dart` | `avoid-collection-methods-with-unrelated-types` | `rules_list/avoid_collection_methods_with_unrelated_types/examples/*.dart` |
| `avoid_creating_vector_in_update.dart` | `avoid-creating-vector-in-update` | `rules_list/avoid_creating_vector_in_update/examples/example.dart` |
| `avoid_double_slash_imports.dart` | `avoid-double-slash-imports` | `rules_list/avoid_double_slash_imports/examples/example.dart` |
| `avoid_duplicate_exports.dart` | `avoid-duplicate-exports` | `rules_list/avoid_duplicate_exports/examples/example.dart` |
| `avoid_dynamic.dart` | `avoid-dynamic` | `rules_list/avoid_dynamic/examples/example.dart` |
| `avoid_expanded_as_spacer.dart` | `avoid-expanded-as-spacer` | `rules_list/avoid_expanded_as_spacer/examples/example.dart` |
| `avoid_global_state.dart` | `avoid-global-state` | `rules_list/avoid_global_state/examples/example.dart` |
| `avoid_ignoring_return_values.dart` | `avoid-ignoring-return-values` | `rules_list/avoid_ignoring_return_values/examples/example.dart` |
| `avoid_initializing_in_on_mount.dart` | `avoid-initializing-in-on-mount` | `rules_list/avoid_initializing_in_on_mount/examples/example.dart` |
| `avoid_late_keyword.dart` | `avoid-late-keyword` | `rules_list/avoid_late_keyword/examples/example.dart` |
| `avoid_missing_enum_constant_in_map.dart` | `avoid-missing-enum-constant-in-map` | `rules_list/avoid_missing_enum_constant_in_map/examples/example.dart` |
| `avoid_nested_conditional_expressions.dart` | `avoid-nested-conditional-expressions` | `rules_list/avoid_nested_conditional_expressions/examples/example.dart` |
| `avoid_non_ascii_symbols.dart` | `avoid-non-ascii-symbols` | `rules_list/avoid_non_ascii_symbols/examples/example.dart` |
| `avoid_non_configurable_callbacks_in_init_state.dart` | `avoid-non-configurable-callbacks-in-init-state` | `rules_list/avoid_non_configurable_callbacks_in_init_state/examples/example.dart` |
| `avoid_non_exhaustive_switch_on_sealed_classes.dart` | `avoid-non-exhaustive-switch-on-sealed-classes` | `rules_list/avoid_non_exhaustive_switch_on_sealed_classes/examples/example.dart` |
| `avoid_non_null_assertion.dart` | `avoid-non-null-assertion` | `rules_list/avoid_non_null_assertion/examples/example.dart` |
| `avoid_passing_async_when_sync_expected.dart` | `avoid-passing-async-when-sync-expected` | `rules_list/avoid_passing_async_when_sync_expected/examples/example.dart` |
| `avoid_redundant_async.dart` | `avoid-redundant-async` | `rules_list/avoid_redundant_async/examples/example.dart` |
| `avoid_redundant_async_on_load.dart` | `avoid-redundant-async-on-load` | `rules_list/avoid_redundant_async_on_load/examples/example.dart` |
| `avoid_returning_widgets.dart` | `avoid-returning-widgets` | `rules_list/avoid_returning_widgets/examples/example.dart` |
| `avoid_shrink_wrap_in_lists.dart` | `avoid-shrink-wrap-in-lists` | `rules_list/avoid_shrink_wrap_in_lists/examples/example.dart` |
| `avoid_substring.dart` | `avoid-substring` | `rules_list/avoid_substring/examples/example.dart` |
| `avoid_throw_in_catch_block.dart` | `avoid-throw-in-catch-block` | `rules_list/avoid_throw_in_catch_block/examples/example.dart` |
| `avoid_top_level_members_in_tests.dart` | `avoid-top-level-members-in-tests` | `rules_list/avoid_top_level_members_in_tests/examples/example.dart` |
| `avoid_unnecessary_conditionals.dart` | `avoid-unnecessary-conditionals` | `rules_list/avoid_unnecessary_conditionals/examples/example.dart` |
| `avoid_unnecessary_setstate.dart` | `avoid-unnecessary-setstate` | `rules_list/avoid_unnecessary_setstate/examples/example.dart` |
| `avoid_unnecessary_type_assertions.dart` | `avoid-unnecessary-type-assertions` | `rules_list/avoid_unnecessary_type_assertions/examples/*.dart` |
| `avoid_unnecessary_type_casts.dart` | `avoid-unnecessary-type-casts` | `rules_list/avoid_unnecessary_type_casts/examples/example.dart` |
| `avoid_unrelated_type_assertions.dart` | `avoid-unrelated-type-assertions` | `rules_list/avoid_unrelated_type_assertions/examples/example.dart` |
| `avoid_unused_parameters.dart` | `avoid-unused-parameters` | `rules_list/avoid_unused_parameters/examples/*.dart` |
| `avoid_wrapping_in_padding.dart` | `avoid-wrapping-in-padding` | `rules_list/avoid_wrapping_in_padding/examples/example.dart` |
| `ban_name.dart` | `ban-name` | `rules_list/ban_name/examples/example.dart` |
| `binary_expression_operand_order.dart` | `binary-expression-operand-order` | `rules_list/binary_expression_operand_order/examples/example.dart` |
| `check_for_equals_in_render_object_setters.dart` | `check-for-equals-in-render-object-setters` | `rules_list/check_for_equals_in_render_object_setters/examples/example.dart` |
| `consistent_update_render_object.dart` | `consistent-update-render-object` | `rules_list/consistent_update_render_object/examples/*.dart` |
| `correct_game_instantiating.dart` | `correct-game-instantiating` | `rules_list/correct_game_instantiating/examples/example.dart` |
| `double_literal_format.dart` | `double-literal-format` | `rules_list/double_literal_format/examples/example.dart` |
| `format_comment.dart` | `format-comment` | `rules_list/format_comment/examples/example.dart` |
| `list_all_equatable_fields.dart` | `list-all-equatable-fields` | `rules_list/list_all_equatable_fields/examples/example.dart` |
| `member_ordering.dart` | `member-ordering` | `rules_list/member_ordering/examples/example.dart` |
| `missing_test_assertion.dart` | `missing-test-assertion` | `rules_list/missing_test_assertion/examples/*.dart` |
| `newline_before_return.dart` | `newline-before-return` | `rules_list/newline_before_return/examples/example.dart` |
| `no_blank_line_before_single_return.dart` | `no-blank-line-before-single-return` | `rules_list/no_blank_line_before_single_return/examples/example.dart` |
| `no_boolean_literal_compare.dart` | `no-boolean-literal-compare` | `rules_list/no_boolean_literal_compare/examples/example.dart` |
| `no_empty_block.dart` | `no-empty-block` | `rules_list/no_empty_block/examples/example.dart` |
| `no_equal_arguments.dart` | `no-equal-arguments` | `rules_list/no_equal_arguments/examples/example.dart` |
| `no_equal_then_else.dart` | `no-equal-then-else` | `rules_list/no_equal_then_else/examples/example.dart` |
| `no_magic_number.dart` | `no-magic-number` | `rules_list/no_magic_number/examples/example.dart` |
| `no_object_declaration.dart` | `no-object-declaration` | `rules_list/no_object_declaration/examples/example.dart` |
| `only_barrel_import.dart` | `only-barrel-import` | `rules_list/only_barrel_import/examples/*.dart` |
| `prefer_async_await.dart` | `prefer-async-await` | `rules_list/prefer_async_await/examples/example.dart` |
| `prefer_commenting_analyzer_ignores.dart` | `prefer-commenting-analyzer-ignores` | `rules_list/prefer_commenting_analyzer_ignores/examples/example.dart` |
| `prefer_conditional_expressions.dart` | `prefer-conditional-expressions` | `rules_list/prefer_conditional_expressions/examples/example.dart` |
| `prefer_const_border_radius.dart` | `prefer-const-border-radius` | `rules_list/prefer_const_border_radius/examples/example.dart` |
| `prefer_correct_edge_insets_constructor.dart` | `prefer-correct-edge-insets-constructor` | `rules_list/prefer_correct_edge_insets_constructor/examples/*.dart` |
| `prefer_correct_identifier_length.dart` | `prefer-correct-identifier-length` | `rules_list/prefer_correct_identifier_length/examples/*.dart` |
| `prefer_correct_test_file_name.dart` | `prefer-correct-test-file-name` | `rules_list/prefer_correct_test_file_name/examples/example.dart` |
| `prefer_correct_type_name.dart` | `prefer-correct-type-name` | `rules_list/prefer_correct_type_name/examples/*.dart` |
| `prefer_define_hero_tag.dart` | `prefer-define-hero-tag` | `rules_list/prefer_define_hero_tag/examples/example.dart` |
| `prefer_dot_shorthands.dart` | `prefer-dot-shorthands` | `rules_list/prefer_dot_shorthands/examples/example.dart` |
| `prefer_enums_by_name.dart` | `prefer-enums-by-name` | `rules_list/prefer_enums_by_name/examples/example.dart` |
| `prefer_extracting_callbacks.dart` | `prefer-extracting-callbacks` | `rules_list/prefer_extracting_callbacks/examples/example.dart` |
| `prefer_first.dart` | `prefer-first` | `rules_list/prefer_first/examples/example.dart` |
| `prefer_first_or_null.dart` | `prefer-first-or-null` | `rules_list/prefer_first_or_null/examples/example.dart` |
| `prefer_immediate_return.dart` | `prefer-immediate-return` | `rules_list/prefer_immediate_return/examples/example.dart` |
| `prefer_intl_name.dart` | `prefer-intl-name` | `rules_list/prefer_intl_name/examples/example.dart` |
| `prefer_iterable_of.dart` | `prefer-iterable-of` | `rules_list/prefer_iterable_of/examples/*.dart` |
| `prefer_last.dart` | `prefer-last` | `rules_list/prefer_last/examples/*.dart` |
| `prefer_match_file_name.dart` | `prefer-match-file-name` | `rules_list/prefer_match_file_name/examples/example.dart` |
| `prefer_media_query_direct_access.dart` | `prefer-media-query-direct-access` | `rules_list/prefer_media_query_direct_access/examples/example.dart` |
| `prefer_moving_to_variable.dart` | `prefer-moving-to-variable` | `rules_list/prefer_moving_to_variable/examples/example.dart` |
| `prefer_named_record_fields.dart` | `prefer-named-record-fields` | `rules_list/prefer_named_record_fields/examples/example.dart` |
| `prefer_provide_intl_description.dart` | `prefer-provide-intl-description` | `rules_list/prefer_provide_intl_description/examples/example.dart` |
| `prefer_single_quotes.dart` | `prefer-single-quotes` | `rules_list/prefer_single_quotes/examples/example.dart` |
| `prefer_single_widget_per_file.dart` | `prefer-single-widget-per-file` | `rules_list/prefer_single_widget_per_file/examples/*.dart` |
| `prefer_static_class.dart` | `prefer-static-class` | `rules_list/prefer_static_class/examples/*.dart` |
| `prefer_trailing_comma.dart` | `prefer-trailing-comma` | `rules_list/prefer_trailing_comma/examples/*.dart` |
| `prefer_using_list_view.dart` | `prefer-using-list-view` | `rules_list/prefer_using_list_view/examples/example.dart` |
| `provide_correct_intl_args.dart` | `provide-correct-intl-args` | `rules_list/provide_correct_intl_args/examples/example.dart` |
| `tag_name.dart` | `tag-name` | `rules_list/tag_name/examples/example.dart` |
| `use_design_system_item.dart` | `use-design-system-item` | `rules_list/use_design_system_item/examples/example.dart` |
| `use_setstate_synchronously.dart` | `use-setstate-synchronously` | `rules_list/use_setstate_synchronously/examples/example.dart` |

Coverage: **87 / 87 registered rule ids** (the complete `_implementedRules`
set); every id has an upstream `examples/` fixture, so none are skipped.

Known caveats:

- Rule ids are kebab-case; the daemon's config face matches
  case-insensitively, both spellings resolve.
- Several rules are resolution-flavored (typed collections, Flutter/Bloc
  subjects) or config-driven (`ban-name`, `use-design-system-item`,
  `arguments-ordering` parameter maps, member/named-parameter ordering).
  Their triggers carry free-name stand-ins or default-posture shapes, so
  the parse-only boundary decides firing — ground truth comes from the
  live census run (`scripts/plugin-census.sh`), never this table.
- `prefer-correct-test-file-name`, `prefer-match-file-name`,
  `avoid-top-level-members-in-tests`, and `prefer-mirror-test-structure`
  are name/path-sensitive: the trigger encodes the flagged *shape* and the
  harness feeds it under the mismatched name/path the rule keys on.
- File-level rules that need a whole-file form (`format-comment`,
  `no-blank-line-before-single-return`) keep the smallest snippet that
  still carries the flagged form.
