//! The fix surface as data: one [`FixSpec`] row per diagnostic code whose
//! official fix is a single-span mechanical rewrite, keyed by the code —
//! the dispatch the fix face's executors consult ([`crate::fix`]). A row
//! added here widens the surface; the executors never name a code.
//!
//! A row is admissible only when the fix is mechanically provable from the
//! evidence the diagnostic row already carries (the span, the token
//! stream, the message text, the surrounding text): the rewrite
//! re-verifies the shape the code reports before composing the edit, and
//! an unverifiable hit drops rather than patches on faith. Codes whose
//! official fix needs element or name facts (the `create_*` synthesis
//! family, the const-insertion family) or evidence the row lacks have no
//! row and are never claimed; the one type-gated face the table serves is
//! the null-check family, whose assignability gate the diagnostic's
//! message carries as a pair of type displays.
//!
//! # Action kinds
//!
//! Every [`FixAction`] is one of seven kinds — six data shapes that the
//! official producers reduce to, and the verified rewriter:
//!
//! - [`FixAction::DropLexeme`] — the lexeme at the hit start goes with its
//!   trailing horizontal whitespace (the keyword drops; the
//!   `prefer_adjacent_string_concatenation` `+` rides the same shape);
//! - [`FixAction::RemoveLexeme`] — the lexeme at the hit start goes with
//!   exactly one following character (the official `RemoveLexeme` /
//!   `RemoveVar` deletion `[offset, offset + lexeme + 1)`, the parser
//!   modifier family);
//! - [`FixAction::RemoveKeyword`] — the lexeme at the hit start goes with
//!   the whitespace run to the next token (the official `RemoveKeyword`
//!   deletion `startStart(keyword, next)`; a comment ends the run);
//! - [`FixAction::RemoveKeywordAcrossComments`] — the same keyword drop
//!   but the run is the whole trivia run — comments included — to the
//!   next real token (the `startStart(keyword, keyword.next!)` deletion
//!   of `RemoveConst`/`RemoveLate`/`RemoveUnnecessaryConst`);
//! - [`FixAction::ReplaceExact`] — the hit span must slice to the expected
//!   text and is replaced whole (the span-exact rewrites);
//! - [`FixAction::InsertAfter`] — fixed text lands at the hit end (the
//!   official `InsertBody` insertion at the report's end);
//! - [`FixAction::Rewrite`] — a verified rewriter that expands the
//!   diagnostic's span into the edit, re-checking the reported shape
//!   against the text or the token stream (everything structural);
//! - [`FixAction::MultiRewrite`] — the same verified rewriter producing
//!   several disjoint same-file edits at once (the official multi-edit
//!   producers: `OrganizeImports`' block rebuild, the field/parameter
//!   pairs of the initializing-formal family, `AddAsync`'s
//!   marker-plus-`await` pair). The executors emit one `RuleFix` per
//!   edit, and `apply()`'s snapshot-coordinate drift correction lands
//!   them in any order.
//! - [`FixAction::CreateFile`] — a verified producer composing the
//!   cross-file edit: the file the official change creates (the path rides
//!   [`FixInput::path`], the one field the same-file producers ignore) and
//!   the content it is born with, inserted at offset 0 of its empty text.
//!   The executor anchors the row to the target (the `RuleFix::target`
//!   address) and offers it only while the target is absent — the
//!   `uri_does_not_exist` create face.
//!
//! The rewriters themselves live in [`crate::fix`] beside the anchoring
//! substrate they feed; the thin adapters here fit the legacy signatures
//! to the unified [`FixInput`] shape.
//!
//! # Semantic rows
//!
//! The [`FixSource::Semantic`] rows serve the engine's semantic
//! diagnostics in three families. The drop family (the six `unused_*`
//! codes) removes its span-exact row shapes. The null-check family
//! (`unchecked_use_of_nullable_value`, `invalid_assignment`,
//! `argument_type_not_assignable`) mirrors the official `AddNullCheck`
//! producer over the shapes its descent resolves — the `!` insertion at
//! the target expression (wrapped below postfix precedence) and the
//! null-aware `?.` → `!.` rewrite — gated on the message-carried
//! type-display pair: the fix is offered only where the promoted `from`
//! display reads as the `to` display, the nullable-only mismatch the
//! official assignability gate serves. The written-surface family —
//! forty-one emitted codes whose official producer's edit is provable
//! from the report span, the file text, and the token stream — covers
//! `RemoveConst`/`RemoveNew`/`ReplaceNewWithConst`, `RemoveQuestionMark`,
//! `RemoveNonNullAssertion`, `ReplaceWithNotNullAware`, `UseEqEqNull` /
//! `UseNotEqNull`, `ReplaceColonWithEquals`, `RemoveComparison`'s
//! operand-drop arm, `RemoveUnnecessaryCast`, the pattern-face
//! rewrites, `RemoveDeadCode`'s statement-run and catch-clause arms,
//! `RemoveDeadIfNull`, the unreachable switch-member drop, the
//! `RemoveUnusedElement`/`RemoveUnusedField`/`RemoveUnusedParameter`
//! faces, `MoveTypeArgumentsToClass`, `AddReturnNull`,
//! `RemoveNameFromDeclarationClause`, and the multi-edit faces —
//! `AddLate` over the uninitialized non-nullable declarations,
//! `AddNeNull` on the non-bool condition, `AddStatic` on the const
//! instance field, `AddConst` on the non-constant pattern operand,
//! `RemoveConst`'s head drop on the non-constant initializer,
//! `RemoveMethodDeclaration` on the extension-type Object member, and
//! `AddAsync`'s two-edit await pair. Where a diagnostic's official set
//! also lists non-mechanical producers (`ImportLibrary`,
//! `ChangeTo.classOrMixin`, the `if`/`assert`/`conditional` arms of
//! `RemoveComparison`), the row serves the mechanical arm and the
//! comment names the absent legs. The `undefined_*` synthesis family
//! stays unclaimed: every official producer composes its edit from
//! element facts the row lacks — the target declaration of the insertion
//! (the class, extension, or enum resolved from the access, which may sit
//! in another file), the parameter and return types written from the
//! arguments' and the context's static types, or the scope's name
//! inventory (`ChangeTo`'s rename suggestions, `AddMissingParameterNamed`'s
//! callee declaration; `undefined_prefixed_name` is served by data-driven
//! fixes only).
//!
//! The [`FixAction::CreateFile`] family serves the one cross-file face the
//! evidence proves: `uri_does_not_exist`, whose official producer
//! (`CreateFile`, upstream Dart SDK source,
//! pkg/analysis_server/lib/src/services/correction/dart/create_file.dart)
//! creates the missing URI target — the import/export face born with the
//! `// TODO Implement this library.` stub, the `part` face with the
//! `part of '<library>';` head. The row re-verifies the shape the code
//! reports: the span must slice to the directive's single plain string
//! literal whose inner text the message quotes, the directive keyword
//! must stand before it, and the URI must be a relative `.dart` path the
//! hit file's directory can name. The faces whose target needs the
//! resolver's world drop: `package:`/`dart:` and other scheme URIs (the
//! official producer resolves them through the source factory — the
//! known-diff face), `part of` directives (`PartOfDirective` is neither
//! arm of the official parent dispatch), escaped and interpolated URIs
//! (the official `node is SimpleStringLiteral` guard), and conditional-
//! import arms (the official `Configuration` TODO).
//!
//! # Parser rows
//!
//! The [`FixSource::Parser`] rows serve the engine's syntax diagnostics
//! against the official non-lint fix registry (upstream Dart SDK source,
//! pkg/analysis_server/lib/src/services/correction/fix_internal.dart):
//! every parser code the registry serves with a mechanical producer has a
//! row whose edit shape is that producer's — the modifier removals
//! (`RemoveLexeme`), the keyword drops (`RemoveKeyword`, `RemoveConst`,
//! `RemoveLate`), the separator swaps (`ReplaceColonWithIn`,
//! `ReplaceColonWithEquals`, `ReplaceWithWildcard`, `ReplaceVarWithDynamic`),
//! the comma edits (`RemoveComma`, `AddTrailingComma`), the body insertions
//! (`InsertBody`, `ConvertIntoBlockBody.missingBody`), the `expected_token`
//! trio (`InsertSemicolon`, `ReplaceWithArrow`, `InsertOnKeyword`), the
//! structural removals (`RemoveAbstract`, `RemoveConstructor`,
//! `RemoveOnClause`, `RemoveParametersInGetterDeclaration`), the
//! constant-pattern `AddConst` wraps, and `RemoveUnexpectedUnderscores`.
//! The three parser codes whose official producer needs resolved types
//! (`experiment_not_enabled` — `AddTypeName`; `missing_identifier` —
//! `AddMissingClosureParameters`; `missing_const_final_var_or_type` —
//! `AddTypeAnnotation`) have no row. Where the official producer offers
//! several kinds, the row serves the one mechanical kind and the comment
//! names the absent legs.

//! # Lint rows
//!
//! The [`FixSource::Lint`] rows serve the lint face's mechanical producers
//! (upstream Dart SDK source, keyed through the lint registry of
//! fix_internal.dart): every row's edit shape is its official producer's,
//! rebuilt from the diagnostic row's span, the token stream, and the
//! surrounding text, and re-verified before the edit lands. The served
//! families: annotation insertion, directive insertion and naming, the
//! type-drop and `var`/`final`/`const` declaration faces, the boolean
//! flattening (comparison and conditional), the literal pads and raw
//! conversions, the combinator/case/comma list edits, the argument and
//! parameter removals, the null-aware element and truncating-division
//! folds, the comment rebuilds (`///`, flutter TODO), and the multi-edit
//! faces — `OrganizeImports`' directive-block sort, `MoveInitializationToFieldDeclaration`'s
//! initializer carry, and the initializing-formal/declaring-parameter
//! pairs.
//!
//! The census's remaining lint codes have no row; the official producer
//! reads a fact the row does not carry (the #1883 re-adjudication against
//! the landed substrate — each verdict re-traced against the registry and
//! its producer's source):
//!
//! - the inferred type written verbatim (the `AddTypeAnnotation` /
//!   `AddReturnType` family: `always_declare_return_types`,
//!   `always_specify_types`, `prefer_typing_uninitialized_variables`,
//!   `specify_nonobvious_local_variable_types`,
//!   `specify_nonobvious_property_types`, `type_annotate_public_apis`,
//!   `strict_top_level_inference`);
//! - the resolved-type gate or display (`null_check_on_nullable_type_parameter`'s
//!   cast writes the operand's type-parameter display; `null_closures`'s
//!   closure shape reads the parameter's function type; `invalid_case_patterns`'s
//!   and `use_decorated_box`'s producers dispatch on `canBeConst`, the
//!   const-evaluation fact that selects between offer, refuse, and a
//!   `const`-carrying replacement);
//! - rename-producers' scope-wide name inventory
//!   (`avoid_renaming_method_parameters`, `constant_identifier_names`,
//!   `non_constant_identifier_names`,
//!   `no_leading_underscores_for_library_prefixes`,
//!   `no_leading_underscores_for_local_identifiers`);
//! - the package config and directive inventory
//!   (`always_use_package_imports`, `avoid_relative_lib_imports`,
//!   `prefer_relative_imports`, `simple_directive_paths`,
//!   `use_string_in_part_of_directives`' owning-library URI);
//!   `discarded_futures`' `AddAsync` leg rewrites the return type through
//!   the type system and its `WrapInUnawaited` leg adds the `dart:async`
//!   import — a second, disjoint edit;
//! - reference and constant inventories (`comment_references`' import
//!   leg, `diagnostic_describe_all_properties`' property assertions,
//!   `unreachable_from_main`'s `findAllReferences().isEmpty` gate,
//!   `unnecessary_this_alias`'s local-element reference set,
//!   `exhaustive_cases`' enum constants, `hash_and_equals`' field
//!   inventory, `implicit_reopen`'s imported-name dependency check);
//! - structural synthesis (`avoid_unnecessary_containers`' parent-splice,
//!   `use_enums`, `use_primary_constructors`, `use_super_parameters`'
//!   cross-file super-parameter names);
//! - the data-driven replacement texts
//!   (`deprecated_member_use_from_same_package`,
//!   `migrate_design_widgets` — external SDK-change data; the conscious
//!   divergence).
//!
//! The #1883 wave moved five codes out of this register whose producers
//! reduce to the row's own evidence once the lint's gates are read as the
//! shape decision of record: `avoid_function_literals_in_foreach_calls`
//! (was filed under declaration synthesis — an expression rewrite),
//! `prefer_for_elements_to_map_fromiterable` (same-bucket correction; the
//! same-parameter leg is reference-free), `sort_child_properties_last`
//! (was "needs the parameter names" — the names stand in the text),
//! `unnecessary_underscores` (was filed under rename inventory — the
//! rule's unreferenced gate makes the rename reference-free), and
//! `use_key_in_widget_constructors` (the constructor face is token-local;
//! its class-name synthesis face stays bound as a documented absent leg).
//!
//! `empty_statements` carries the official pair wholesale:
//! `RemoveEmptyStatement`'s line drop in a block and the
//! `ReplaceWithBrackets` face on a construct body (`while (x) ;` →
//! `while (x) {}`).

use dartq_diagnostics::Diagnostic;
use dartq_scanner::TokenTable;
use dartq_source::SourceRange;

use crate::fix;
use crate::fix::{
    add_async_await_entry, add_const_pattern_entry, add_const_prefix, add_eol,
    add_explicit_call_entry, add_late_entry, add_leading_newline,
    add_library_directive_entry, add_missing_trailing_comma, add_required_keyword, add_return_null,
    add_static_entry, add_super_key, add_trailing_comma, add_typedef_parameters,
    annotate_overrides_entry, annotate_redeclares_entry, annotation_library_directive_entry,
    append_ne_null_entry, await_future_expression, await_return_entry, bool_comparison,
    bool_conditional, cascade_merge, cascade_with_dot, cast_null_check, catch_to_on_type,
    child_property_last, colon_to_equals, conditional_assignment, conditional_to_if_element,
    const_pattern_binary, const_pattern_generic, const_pattern_literal, const_pattern_negation,
    constructor_first, convert_inner_quotes, convert_to_declaring_param_entry,
    convert_to_init_formal_entry, convert_to_raw_string_entry, create_missing_uri_target,
    curly_braces_body, curly_braces_head, doc_comment_to_line, double_to_int_literal, drop_as_cast,
    drop_bang_expression, drop_bang_pattern, drop_break_statement, drop_clause_name,
    drop_combinator_name, drop_comparison_operand, drop_constructor_type_name, drop_dead_catch,
    drop_dead_code, drop_dead_if_null, drop_empty_container_body, drop_empty_statement,
    drop_enclosing_const, drop_getter_invocation_parens, drop_head_const_entry,
    drop_library_directive, drop_library_name, drop_non_null_assertion, drop_object_member_entry,
    drop_pattern_field_name, drop_primary_constructor_body, drop_switch_member,
    drop_type_question, drop_unused_element, drop_unused_field, drop_unused_parameter,
    drop_wildcard_and_operand, eight_digit_hex, equality_is_check, equality_null_operand,
    expected_token_fix, expression_function_body, field_public, flutter_style_todo,
    for_each_tear_off, for_in_from_for_each, function_type_syntax, generic_alias_syntax,
    if_null_bools, inline_typedef, is_empty_call, is_not_empty_access, is_null_to_eq_null,
    make_declaration_final, make_parameter_final, map_from_iterable_for_element,
    missing_body_block, move_init_to_field_entry, move_type_args_to_class,
    named_constant_reference, new_to_const, not_null_aware_operator, null_aware_element,
    null_check_argument, null_check_assignment, null_check_condition, omit_type_with_var,
    organize_directives_entry, remove_abstract_member, remove_argument_entry, remove_constructor,
    remove_duplicate_case, remove_getter_parameters, remove_hit_lexeme, remove_ignore_comment,
    remove_number_separators, remove_on_clause, remove_override_annotation, remove_override_member,
    remove_parameter_entry, remove_pattern_var_keyword, remove_print_entry, remove_raw_prefix,
    remove_reported_type, remove_returned_value, remove_to_list, replace_null_with_void,
    required_named_parameters_first, rethrow_span, return_type_future, semicolon_body,
    sort_combinator_names, split_declarations, spread_addition, surround_with_parentheses,
    sync_value_entry, tear_off_lambda, truncating_division, unnamed_constructor_first,
    where_type_entry, wildcard_underscore,
};

/// The evidence one fix expansion reads: the file text, the diagnostic's
/// UTF-16 span, its problem message, the token stream when the run
/// parsed one, the hit file's own path — the last only the
/// cross-file producers read — and the file's whole semantic diagnostic
/// set, the one producer that reads it being the whole-file
/// `directives_ordering` organize (the official `ImportOrganizer` takes the
/// resolved unit's diagnostics: the unused-import and shown-name removal
/// legs match rows at URI/name offsets and the unresolved-identifier gate
/// reads the file, not the hit).
pub struct FixInput<'a> {
    pub text: &'a str,
    pub range: core::ops::Range<u32>,
    /// The diagnostic's problem message — the official `InsertSemicolon` /
    /// `ReplaceWithArrow` / `InsertOnKeyword` trio behind `expected_token`
    /// is told apart by the quoted token it names, exactly as the official
    /// producer reads it.
    pub message: &'a str,
    /// Present when a hit of a [`FixSpec::needs_tokens`] family drove the
    /// parse; a rewrite that needs it and finds none drops the fix.
    pub tokens: Option<&'a TokenTable>,
    /// The hit file's own path, as the run discovered it. The same-file
    /// producers ignore it; the [`FixAction::CreateFile`] producers join
    /// their target against it (`uri_does_not_exist`'s missing URI is
    /// file-relative, and the `part` face's `part of` head names the hit
    /// file itself).
    pub path: &'a std::path::Path,
    /// The file's semantic diagnostics (the engine's rows, in production
    /// order). Only the whole-file organize reads it; every other producer
    /// sees an empty slice.
    pub diagnostics: &'a [Diagnostic],
}

/// Which pipeline serves the code — the executors filter on it (a lint hit
/// never consults a semantic row and vice versa; the code namespaces do
/// not collide today and the filter keeps it that way).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixSource {
    /// The dartq-lints layer's wired rules.
    Lint,
    /// The engine's semantic diagnostics.
    Semantic,
    /// The engine's syntax diagnostics — the parser and scanner codes of
    /// the official registry (`syntax-codes.tsv`).
    Parser,
}

/// A verified span rewriter: the [`FixInput`] in, the replacement triple
/// (UTF-16 offset, length, text) out — `None` drops the fix.
pub type Rewrite = fn(&FixInput<'_>) -> Option<(u32, u32, String)>;

/// A verified multi-span rewriter: the [`FixInput`] in, the disjoint
/// replacement triples (UTF-16 offset, length, text) out — `None` drops
/// the fix. The spans must not overlap (the applier's overlap guard drops
/// the later write); the producers verify that at build time by
/// construction.
pub type MultiRewrite = fn(&FixInput<'_>) -> Option<Vec<(u32, u32, String)>>;

/// A verified cross-file producer: the [`FixInput`] in, `(target path,
/// born content)` out — `None` drops the fix. The target is the file the
/// official change creates (absolute, lexically normalized), the content
/// what it is born with; the executor anchors the content at offset 0 of
/// the target's empty text and offers the row only while the target is
/// absent.
pub type CreateFile = fn(&FixInput<'_>) -> Option<(String, String)>;

/// The mechanical transform of one family — the closed action-kind set of
/// the module docs.
#[derive(Clone, Copy)]
pub enum FixAction {
    /// The lexeme at the hit start goes with its trailing horizontal
    /// whitespace (a newline stays — joining lines is a formatting
    /// decision, not a fix).
    DropLexeme(&'static str),
    /// The lexeme at the hit start goes with exactly one following
    /// character — the official `RemoveLexeme` deletion
    /// `[offset, offset + lexeme.length + 1)` (the message-named modifier;
    /// `RemoveVar` deletes the same span over the `var` report). The
    /// lexeme is verified as a whole word; the following character is
    /// whatever stands there, as the official producer deletes it blind.
    RemoveLexeme(&'static str),
    /// The lexeme at the hit start goes with the whitespace run to the next
    /// token — the official `RemoveKeyword` deletion `startStart(keyword,
    /// next-or-its-comment)` (newlines included, a comment ends the run).
    RemoveKeyword(&'static str),
    /// The lexeme at the hit start goes with the whole trivia run —
    /// whitespace AND comments — to the next real token: the
    /// `startStart(keyword, keyword.next!)` deletion of `RemoveConst`,
    /// `RemoveLate` and `RemoveUnnecessaryConst` (a comment rides the next
    /// token's `precedingComments` edge, inside the span).
    RemoveKeywordAcrossComments(&'static str),
    /// The hit span must slice to exactly `expect`; it is replaced whole.
    ReplaceExact {
        expect: &'static str,
        replacement: &'static str,
    },
    /// `text` is inserted at the hit end (the official `InsertBody` /
    /// `AddEmptyArgumentList` insertion at the report's end).
    InsertAfter(&'static str),
    /// A verified rewriter over the diagnostic's span.
    Rewrite(Rewrite),
    /// A verified rewriter producing several disjoint same-file edits —
    /// the official multi-edit producers whose change carries more than
    /// one span (`MoveInitializationToFieldDeclaration`'s delete-plus-
    /// insert, `ConvertToInitializingFormal`'s parameter rewrite plus the
    /// initializer/statement removal, `OrganizeImports`' whole-block
    /// reorder, `AddAsync`'s body-plus-expression pair, the parameter-
    /// rewrite-plus-field-drop faces). The producer returns `None` unless
    /// every edit verifies — the change is offered whole or not at all;
    /// the executors materialize one row per edit, the same shape the
    /// applier's snapshot pass already keeps coherent.
    MultiRewrite(MultiRewrite),
    /// A verified cross-file producer — the change targets a file other
    /// than the hit file, creating it (`uri_does_not_exist`'s official
    /// `CreateFile`: the missing import target is born with the upstream
    /// TODO stub, the missing `part` target with the `part of` head). The
    /// producer composes the target from [`FixInput::path`] and the hit's
    /// own evidence; the executor anchors the row to the target and
    /// offers it only while the target is absent.
    CreateFile(CreateFile),
}

/// One mechanical fix family: the diagnostic code it serves, the pipeline
/// that reports the code, whether the rewrite verifies against the token
/// stream (the parse is paid once per executor call when any hit of the
/// family is present), and the transform.
#[derive(Clone, Copy)]
pub struct FixSpec {
    pub code: &'static str,
    pub source: FixSource,
    pub needs_tokens: bool,
    pub action: FixAction,
}

/// The table, sorted by `code` — [`spec_for`]'s binary search reads the
/// order; the pin test holds it in place.
static FIX_TABLE: &[FixSpec] = &[
    // `abstract` on a class member goes with the whole whitespace run
    // behind it, newlines included (official `RemoveAbstract`'s
    // class-member face: `'abstract '.length` then every whitespace unit).
    FixSpec {
        code: "abstract_class_member",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::Rewrite(remove_abstract_member),
    },
    // `abstract static int x;` — the modifier plus one character (official
    // `RemoveLexeme.modifier`, the message-quoted lexeme).
    FixSpec {
        code: "abstract_static_field",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("abstract"),
    },
    // Registry row the parser does not emit today; the shape is the
    // official `RemoveLexeme.modifier` over the `abstract` report.
    FixSpec {
        code: "abstract_static_method",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("abstract"),
    },
    // The control body on the control head's line wraps into a brace
    // block (the official `UseCurlyBraces` nonBulk face); the report sits
    // on the body's head token, so the statement end is rebuilt first.
    FixSpec {
        code: "always_put_control_body_on_new_line",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(curly_braces_head),
    },
    // The reported `required` named parameter slides ahead of the
    // optional named ones inside the `{…}` group (official
    // `MoveRequiredNamedParametersFirst`: the parameter list reassembles
    // moved-first; the lint's own gate already restricts the report to a
    // `required` parameter standing after an optional one).
    FixSpec {
        code: "always_put_required_named_parameters_first",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(required_named_parameters_first),
    },
    // `@override` lands above the overridden member, doc comments
    // included (official `AddOverride`; the extension-type and
    // redeclaration target faces read resolution and are absent).
    FixSpec {
        code: "annotate_overrides",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(annotate_overrides_entry),
    },
    // `@redeclare` lands above the redeclared member (official
    // `AddRedeclare`; the same target-fact boundary as `AddOverride`).
    FixSpec {
        code: "annotate_redeclares",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(annotate_redeclares_entry),
    },
    // The argument type's nullable-only mismatch takes the `!` after the
    // bare identifier argument, or the argument chain's null-aware
    // operator goes `!`-form (official `AddNullCheck`, its assignability
    // gate read off the message's type-display pair; every other argument
    // shape resolves no target in the official descent, and the cast,
    // wrap-in-text, await, and closure-parameter kinds need resolved
    // types).
    FixSpec {
        code: "argument_type_not_assignable",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(null_check_argument),
    },
    // ` await ` lands after the reported `return`/`=>` (official
    // `AddAwait.return_`; the un-awaited-future gate reads resolution and
    // stays with the lint).
    FixSpec {
        code: "async_return_with_no_await",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(await_return_entry),
    },
    // The written `dynamic` goes with the whitespace to the next token
    // (official `RemoveTypeAnnotation.other`'s parameter face).
    FixSpec {
        code: "avoid_annotating_with_dynamic",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(remove_reported_type),
    },
    // The bool-literal conditional flattens: equal arms keep one literal,
    // an arm literal folds into `||`/`&&` with the condition negated, and
    // `c ? false : true` becomes the negated condition (official
    // `ConvertToBooleanExpression`'s conditional faces; the paren gates
    // read the operands' precedence).
    FixSpec {
        code: "avoid_bool_literals_in_conditional_expressions",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(bool_conditional),
    },
    // `if (b) ; else ;` — the else `;` becomes a block.
    FixSpec {
        code: "avoid_empty_else",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::ReplaceExact {
            expect: ";",
            replacement: "{}",
        },
    },
    // The outer quote moves to the character the value escaped
    // (official `ConvertQuotes`): `'a\'b'` → `"a'b"`.
    FixSpec {
        code: "avoid_escaping_inner_quotes",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(convert_inner_quotes),
    },
    // The parameter's `final` drops (official `RemoveLexeme.modifier` —
    // the message-named lexeme plus one trailing character).
    FixSpec {
        code: "avoid_final_parameters",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::DropLexeme("final"),
    },
    // `xs.forEach((e) { … });` becomes `for (var e in xs) { … }` — the
    // official `ConvertForEachToForLoop` as one whole-statement rewrite
    // (`return` statements fold to `continue;`, nested literals excepted;
    // an async/generator body refuses; the code-style `final`/written-type
    // loop-variable faces read the analysis options' `code` section and
    // are absent).
    FixSpec {
        code: "avoid_function_literals_in_foreach_calls",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(for_in_from_for_each),
    },
    FixSpec {
        code: "avoid_init_to_null",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(drop_null_initializer_entry),
    },
    // The declaration list splits, every declarator re-prefixed with the
    // modifiers and type at the same indent (official
    // `SplitMultipleDeclarations`; comments, metadata, and `for`-part
    // lists refuse).
    FixSpec {
        code: "avoid_multiple_declarations_per_line",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(split_declarations),
    },
    // The reported null-check operand drops its `&&` flank — the official
    // `RemoveComparison` hardcodes the always-true verdict for this code,
    // so the chain is `&&` only; the `||` face, the assert/if/?: arms, and
    // the `?.`→`. ` rewrite on the DotAccess report are absent legs.
    FixSpec {
        code: "avoid_null_checks_in_equality_operators",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(equality_null_operand),
    },
    // The `print` invocation goes — its whole line when it owns it, the
    // invocation plus its `;` otherwise (official `RemovePrint`'s
    // lines-range face).
    FixSpec {
        code: "avoid_print",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(remove_print_entry),
    },
    // The private typedef folds into its one use site — `R Function(p)`
    // replaces the `_F` reference and the `typedef` line goes (official
    // `InlineTypedef`; the multi/zero-use and generic-alias-with-own-
    // type-parameters faces refuse, as upstream).
    FixSpec {
        code: "avoid_private_typedef_functions",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(inline_typedef),
    },
    // The redundant argument leaves its list with one adjacent comma, a
    // named `x: value` whole (official `RemoveArgument`'s nodeInList
    // face).
    FixSpec {
        code: "avoid_redundant_argument_values",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(remove_argument_entry),
    },
    // The setter's written type goes with the whitespace to the next
    // token (official `RemoveTypeAnnotation.other`'s parameter face).
    FixSpec {
        code: "avoid_return_types_on_setters",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(remove_reported_type),
    },
    // `return null;` drops the expression and the `=> null` body becomes
    // a block (official `RemoveReturnedValue`'s two faces; the nullable
    // return-type leg reads resolution and is absent).
    FixSpec {
        code: "avoid_returning_null_for_void",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(remove_returned_value),
    },
    // `x..print();` → `x.print();` — the single section's cascade
    // operator becomes its `.`-equivalent.
    FixSpec {
        code: "avoid_single_cascade_in_expression_statements",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(cascade_with_dot),
    },
    // `catch (T)` → `on T`, `catch (T, s)` → `on T catch (_, s)` — the
    // official `ConvertToOnType` over the reported name; the formal-
    // parameter and type-parameter faces carry sibling codes and the
    // producer drops them.
    FixSpec {
        code: "avoid_types_as_parameter_names",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(catch_to_on_type),
    },
    // The closure parameter's written type goes with the whitespace to
    // the next token (official `RemoveTypeAnnotation.other`'s parameter
    // face).
    FixSpec {
        code: "avoid_types_on_closure_parameters",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(remove_reported_type),
    },
    // The unused parameter leaves the constructor with one adjacent
    // comma; a `[…]`/`{…}` group leaves with its lone member, and an
    // optional `this.x` folds its default into a fresh initializer
    // (official `RemoveUnusedParameter`'s faces).
    FixSpec {
        code: "avoid_unused_constructor_parameters",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(remove_parameter_entry),
    },
    // The `void` return type becomes `Future<void>` (official
    // `ReplaceReturnTypeFuture`; the un-awaited-body leg reads
    // resolution).
    FixSpec {
        code: "avoid_void_async",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(return_type_future),
    },
    // The `await` of an already-`Future`-typed expression drops (official
    // `RemoveKeyword.awaitKeyword` — the keyword plus the whitespace run
    // to the next token or its first comment); the type facts are the
    // lint's own judgment, the edit is a pure keyword drop.
    FixSpec {
        code: "await_only_futures",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::RemoveKeyword("await"),
    },
    // `return null;` lands inside the function body the report covers
    // (official `AddReturnNull`: after the last statement, or inside the
    // empty block with the line's indent).
    FixSpec {
        code: "body_might_complete_normally_nullable",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(add_return_null),
    },
    // The reported statement run folds into cascades on the receiver —
    // `x.f(); x.g();` becomes `x..f()..g();` (official `ConvertToCascade`:
    // each statement's `target`/`operator` collapses, the previous `;`
    // drops, a low-precedence assignment initializer wraps in parens, and
    // the anchor's own `.` doubles when it is an expression statement).
    FixSpec {
        code: "cascade_invocations",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(cascade_merge),
    },
    // The `!` lands after the cast's operand, the cast kept (official
    // `AddNullCheck.withoutAssignabilityCheck`'s as-expression face).
    FixSpec {
        code: "cast_nullable_to_non_nullable",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(cast_null_check),
    },
    // `for (var x : xs)` — the colon becomes `in` (official
    // `ReplaceColonWithIn`: the one-character report replaced).
    FixSpec {
        code: "colon_in_place_of_in",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::ReplaceExact {
            expect: ":",
            replacement: "in",
        },
    },
    // The reported combinator's names sort, the combinator rebuilt
    // verbatim (official `SortCombinatorNames`).
    FixSpec {
        code: "combinators_ordering",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(sort_combinator_names),
    },
    // `const class C {}` — the keyword is not part of the class node, so
    // the official `RemoveConst` reaches it through its compilation-unit
    // leg: the report span plus one character.
    FixSpec {
        code: "const_class",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("const"),
    },
    // `const` heading the creation whose callee faults — the keyword drops
    // with its trivia run (official `RemoveConst`'s keyword deletion); the
    // report spans the invocation, so the keyword is rebuilt from the
    // token stream.
    FixSpec {
        code: "const_eval_method_invocation",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_enclosing_const),
    },
    // `const factory C() => …;` — the constructor's `const` goes through
    // the next token, trivia run included (official `RemoveConst`
    // constructor leg, `startStart(const, next)`).
    FixSpec {
        code: "const_factory",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveKeywordAcrossComments("const"),
    },
    // `const x = v` where `v` is not constant — the list's `const`
    // becomes `final` (upstream `RemoveConst`'s
    // `VariableDeclarationList` arm); a `const`/`new` heading the
    // reported creation itself drops with its trivia run
    // (`startStart(keyword, keyword.next!)`, `RemoveConst`/`RemoveNew`).
    // The push-`const`-down legs read the file's diagnostic inventory
    // and stay absent.
    FixSpec {
        code: "const_initialized_with_non_constant_value",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::MultiRewrite(drop_head_const_entry),
    },
    // A `const` field group's keyword becomes `static ` — the official
    // `AddStatic` insertion at the member's head (`const` stays, the
    // group gains `static`; upstream serves `const_instance_field` with
    // `AddStatic` only).
    FixSpec {
        code: "const_instance_field",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::MultiRewrite(add_static_entry),
    },
    // `const void m() {}` in a class body — the keyword through the next
    // token, trivia run included, the `RemoveConst` keyword-deletion
    // shape (the official producer resolves the covering node; the
    // method's node excludes the dropped keyword, so this row serves the
    // registry's intent where the official may resolve to no edit).
    FixSpec {
        code: "const_method",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveKeywordAcrossComments("const"),
    },
    // `const C()` where `C` declares no const constructor — the keyword
    // drops with its trivia run (official `RemoveConst`'s keyword
    // deletion, `startStart(const, const.next)`).
    FixSpec {
        code: "const_with_non_const",
        source: FixSource::Semantic,
        needs_tokens: false,
        action: FixAction::RemoveKeywordAcrossComments("const"),
    },
    // `const ` lands before the reported pattern operand — the official
    // `AddConst` `ConstantPattern` arm: a const-eligible atom takes the
    // bare insertion; any other expression wraps whole (`const (` +
    // `)`). The type-literal parens arm reads the resolved type and
    // stays absent.
    FixSpec {
        code: "constant_pattern_with_non_constant_expression",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::MultiRewrite(add_const_pattern_entry),
    },
    // `covariant void m() {}` — the modifier plus one character (official
    // `RemoveLexeme.modifier`).
    FixSpec {
        code: "covariant_member",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("covariant"),
    },
    // A single-statement control body wraps into a brace block (official
    // `UseCurlyBraces`): the report spans the body statement.
    FixSpec {
        code: "curly_braces_in_flow_control_structures",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(curly_braces_body),
    },
    // `library;` lands after the attached doc comment (official
    // `AddLibraryDirective`).
    FixSpec {
        code: "dangling_library_doc_comments",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(add_library_directive_entry),
    },
    // The dead interval goes — whole lines when the span heads a
    // statement run, the operator-through-operand tail when it starts
    // mid-line on a `&&`/`||`/`??` (official `RemoveDeadCode`'s
    // `getLinesRange` and `endEnd(leftOperand, node)` faces).
    FixSpec {
        code: "dead_code",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_dead_code),
    },
    // The unreachable `on`/`catch` clause goes whole (official
    // `RemoveDeadCode`'s `CatchClause` arm, `endEnd(previous, clause)`).
    FixSpec {
        code: "dead_code_catch_following_catch",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_dead_catch),
    },
    FixSpec {
        code: "dead_code_on_catch_subtype",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_dead_catch),
    },
    // The dead `?? rhs` / `??= rhs` tail drops — `endEnd(left, right)` on
    // the binary face, the whole statement on the bare-`??=` face
    // (official `RemoveDeadIfNull`).
    FixSpec {
        code: "dead_null_aware_expression",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_dead_if_null),
    },
    // `default => …` in a switch expression — the keyword becomes the
    // wildcard pattern (official `ReplaceWithWildcard`).
    FixSpec {
        code: "default_in_switch_expression",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::ReplaceExact {
            expect: "default",
            replacement: "_",
        },
    },
    // The whole directive block re-sorts — the official
    // `OrganizeImports` face (all four `directives_ordering_*`
    // spellings): `dart:`/`package:`/other/relative groups, exports
    // after imports, `part` last, alphabetical inside a group, leading
    // comments moving with their directive. The unused-import and
    // shown-name removal legs read the file's diagnostic set and stay
    // absent; a `library` directive inside the block bounds the face.
    FixSpec {
        code: "directives_ordering",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::MultiRewrite(organize_directives_entry),
    },
    // The reported combinator name leaves its `show`/`hide` clause — with
    // one adjacent comma edge when listed, the whole combinator when lone
    // (official `RemoveNameFromCombinator`; the report anchors the name).
    // The four codes share the producer.
    FixSpec {
        code: "duplicate_hidden_name",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_combinator_name),
    },
    // The repeated import directive goes whole — the same directive
    // deletion the URI report expands to (official `RemoveUnusedImport`;
    // the report anchors the later repeat's URI).
    FixSpec {
        code: "duplicate_import",
        source: FixSource::Semantic,
        needs_tokens: false,
        action: FixAction::Rewrite(drop_import_directive_entry),
    },
    FixSpec {
        code: "duplicate_shown_name",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_combinator_name),
    },
    // The repeated modifier plus one character (official
    // `RemoveLexeme.modifier` over the message-named lexeme — the report
    // sits on the second occurrence, whose lexeme the span carries).
    FixSpec {
        code: "duplicated_modifier",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::Rewrite(remove_hit_lexeme),
    },
    // The empty catch clause `on T catch (e) {}` goes whole; a shared
    // `try` with sibling clauses keeps the fix (official
    // `RemoveEmptyCatch`).
    FixSpec {
        code: "empty_catches",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(remove_empty_catch_entry),
    },
    // `A() {}` → `A();` — the empty body block is replaced whole.
    FixSpec {
        code: "empty_constructor_bodies",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(semicolon_body),
    },
    // The `{}` body of a class/mixin/extension/extension-type becomes
    // `;` — the gap before the `{` folds too (official
    // `RemoveEmptyContainerBody`: `[prevToken.end, node.end)` → `;`).
    FixSpec {
        code: "empty_container_bodies",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(drop_empty_container_body),
    },
    // `(,)` — the comma of a field-less record literal goes (official
    // `RemoveComma.emptyRecordLiteral`: the report span itself).
    FixSpec {
        code: "empty_record_literal_with_comma",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::ReplaceExact {
            expect: ",",
            replacement: "",
        },
    },
    // `(,) x;` — the comma of a field-less record type goes (official
    // `RemoveComma.emptyRecordType`).
    FixSpec {
        code: "empty_record_type_with_comma",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::ReplaceExact {
            expect: ",",
            replacement: "",
        },
    },
    // The `;` statement in a block goes; as a construct body
    // (`while (x) ;`) it becomes an empty block (official
    // `RemoveEmptyStatement` with the `ReplaceWithBrackets` face).
    FixSpec {
        code: "empty_statements",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_empty_statement),
    },
    // The file ends with exactly one newline (official `AddEolAtEndOfFile`).
    FixSpec {
        code: "eol_at_end_of_file",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(add_eol),
    },
    // `class A` with no body — ` {}` lands after the report's end, the
    // token before the missing `{` (official `InsertBody`, the nine
    // shared-name `expected_*_body` variants).
    FixSpec {
        code: "expected_body",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::InsertAfter(" {}"),
    },
    // The official trio behind the code, told apart by the quoted token:
    // `InsertSemicolon` (`;` after the anchor, never after an `await`
    // identifier), `ReplaceWithArrow` (a switch-expression case's `:`
    // becomes ` =>`), `InsertOnKeyword` (`on ` before the extended type,
    // or before the name when the type is missing).
    FixSpec {
        code: "expected_token",
        source: FixSource::Parser,
        needs_tokens: true,
        action: FixAction::Rewrite(expected_token_fix),
    },
    // The reported `extends` name leaves its clause — the whole clause
    // when it is the only name, `name ,` or `, name` otherwise (official
    // `RemoveNameFromDeclarationClause`; the `ChangeTo.classOrMixin` leg
    // reads the name inventory and is absent).
    FixSpec {
        code: "extends_non_class",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_clause_name),
    },
    // `augment extension E on T {}` — the `on` clause goes through the
    // body's opener (official `RemoveOnClause`, `startStart(on, body)`).
    FixSpec {
        code: "extension_augmentation_has_on_clause",
        source: FixSource::Parser,
        needs_tokens: true,
        action: FixAction::Rewrite(remove_on_clause),
    },
    // The constructor member goes from the previous member's end (or the
    // body's `{`) through its own end token; a primary constructor's
    // `(…)` goes alone (official `RemoveConstructor`).
    FixSpec {
        code: "extension_declares_constructor",
        source: FixSource::Parser,
        needs_tokens: true,
        action: FixAction::Rewrite(remove_constructor),
    },
    // The member declaring an `Object` member name goes — its whole line
    // block, doc comments and annotations with it (official
    // `RemoveMethodDeclaration`'s `getLinesRange` deletion). The report
    // anchors the member's name.
    FixSpec {
        code: "extension_type_declares_member_of_object",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::MultiRewrite(drop_object_member_entry),
    },
    // `external class C {}` — the modifier plus one character (official
    // `RemoveLexeme.modifier`).
    FixSpec {
        code: "external_class",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("external"),
    },
    FixSpec {
        code: "external_enum",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("external"),
    },
    FixSpec {
        code: "external_typedef",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("external"),
    },
    // The misplaced modifier plus one character (official
    // `RemoveLexeme.modifier` over the message-named lexeme, which the
    // report span carries).
    FixSpec {
        code: "extraneous_modifier",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::Rewrite(remove_hit_lexeme),
    },
    // `factory` on a top-level declaration (official `RemoveLexeme.modifier`).
    FixSpec {
        code: "factory_top_level_declaration",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("factory"),
    },
    // Registry row the parser does not emit today (official
    // `RemoveLexeme.modifier` over the `final` report).
    FixSpec {
        code: "final_constructor",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("final"),
    },
    // `final enum E { … }` (official `RemoveLexeme.modifier`).
    FixSpec {
        code: "final_enum",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("final"),
    },
    // Registry row the parser does not emit today (official
    // `RemoveLexeme.modifier` over the `final` report).
    FixSpec {
        code: "final_method",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("final"),
    },
    // `final mixin M {}` / `final mixin class C {}` (official
    // `RemoveLexeme.modifier`).
    FixSpec {
        code: "final_mixin",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("final"),
    },
    FixSpec {
        code: "final_mixin_class",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("final"),
    },
    // `late ` lands at the declaration head — before `final`/`var`, else
    // before the written type, else before the first declarator
    // (official `AddLate`'s `VariableDeclarationList` arm; the
    // `CreateConstructorForFinalFields` synthesis legs and the
    // `MakeVariableNullable` arm stay absent). The report anchors the
    // variable's name.
    FixSpec {
        code: "final_not_initialized",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::MultiRewrite(add_late_entry),
    },
    // The comment rebuilds in the flutter TODO shape — `TODO(author): `
    // with the spacing, case, and backtick normalizations (official
    // `ConvertToFlutterStyleTodo`; a rebuilt comment that still fails the
    // expected shape refuses).
    FixSpec {
        code: "flutter_style_todos",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(flutter_style_todo),
    },
    // The `value` selector renames to `syncValue` (official
    // `ReplaceWithSyncValue`; the sync* gate reads resolution).
    FixSpec {
        code: "future_sync_value",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(sync_value_entry),
    },
    // `C.get()`-style `get` on a constructor — the keyword plus one
    // character (official `RemoveLexeme.keyword`, the correction-named
    // lexeme).
    FixSpec {
        code: "getter_constructor",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("get"),
    },
    // `int get x() => 1;` — the parameter list goes whole (official
    // `RemoveParametersInGetterDeclaration` over the kept
    // `FormalParameterList` node).
    FixSpec {
        code: "getter_with_parameters",
        source: FixSource::Parser,
        needs_tokens: true,
        action: FixAction::Rewrite(remove_getter_parameters),
    },
    // `.call` lands after the reported tear-off, parenthesized below
    // postfix precedence (official `AddExplicitCall`).
    FixSpec {
        code: "implicit_call_tearoffs",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(add_explicit_call_entry),
    },
    // The import of a part file goes whole — the directive deletion the
    // URI report expands to (official `RemoveUnusedImport`; the URI-layer
    // report anchors the URI literal like every directive-URI row).
    FixSpec {
        code: "import_of_non_library",
        source: FixSource::Semantic,
        needs_tokens: false,
        action: FixAction::Rewrite(drop_import_directive_entry),
    },
    // The `: x = expr` initializer moves onto the field's declarator —
    // the initializer leaves its list (`nodeInList`) and ` = expr` lands
    // at the field declaration's end (official
    // `MoveInitializationToFieldDeclaration`; the report anchors the
    // field name).
    FixSpec {
        code: "initialize_in_field_declaration",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::MultiRewrite(move_init_to_field_entry),
    },
    // `interface mixin M {}` / `interface mixin class C {}` (official
    // `RemoveLexeme.modifier`).
    FixSpec {
        code: "interface_mixin",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("interface"),
    },
    FixSpec {
        code: "interface_mixin_class",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("interface"),
    },
    // The assignment's nullable-only mismatch takes the `!` at the
    // right-hand side's end, wrapped when its precedence sits below
    // postfix, or the chain's `?.` goes `!.` (official `AddNullCheck`
    // assignment face, gated on the message's type-display pair; a
    // parenthesized right-hand side stays unfixed — the official fix
    // reaches it only on the statement face, which the row cannot tell
    // apart — and the cast, annotation, nullable-declaration, await, and
    // closure-parameter kinds need resolved types).
    FixSpec {
        code: "invalid_assignment",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(null_check_assignment),
    },
    // `case 1 + 2:` — the constant expression wraps in `const (…)`, or
    // `const ` lands before an enclosing parenthesized pattern (official
    // `AddConst` binary-expression leg).
    FixSpec {
        code: "invalid_constant_pattern_binary",
        source: FixSource::Parser,
        needs_tokens: true,
        action: FixAction::Rewrite(const_pattern_binary),
    },
    // `case List<int>:` — the type literal wraps in `const (…)` (official
    // `AddConst` type-literal leg; a guarded or nested pattern, where the
    // official wraps the enclosing node whole, gets no edit).
    FixSpec {
        code: "invalid_constant_pattern_generic",
        source: FixSource::Parser,
        needs_tokens: true,
        action: FixAction::Rewrite(const_pattern_generic),
    },
    // `case -x:` — the negation wraps in `const (…)` (official `AddConst`
    // prefix-expression leg).
    FixSpec {
        code: "invalid_constant_pattern_negation",
        source: FixSource::Parser,
        needs_tokens: true,
        action: FixAction::Rewrite(const_pattern_negation),
    },
    // `covariant` on a primary-constructor parameter goes through the next
    // token (official `RemoveKeyword.covariantKeyword`; the
    // `ReplaceFinalWithVar` leg over a `final` sibling is not served).
    FixSpec {
        code: "invalid_covariant_modifier_in_primary_constructor",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveKeyword("covariant"),
    },
    // The reported inner pattern wraps in parentheses (official
    // `SurroundWithParentheses` — its two insertions folded into one
    // whole-span replacement).
    FixSpec {
        code: "invalid_inside_unary_pattern",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::Rewrite(surround_with_parentheses),
    },
    // The reported `?.`/`?..`/`?.[`/`...?` becomes its non-null-aware form
    // — `.`/`..`/`[`/`...` (official `ReplaceWithNotNullAware`'s token
    // replacement).
    FixSpec {
        code: "invalid_null_aware_operator",
        source: FixSource::Semantic,
        needs_tokens: false,
        action: FixAction::Rewrite(not_null_aware_operator),
    },
    // `covariant` on an extension member (official `RemoveLexeme.modifier`).
    FixSpec {
        code: "invalid_use_of_covariant_in_extension",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("covariant"),
    },
    // The `()` after the non-function callee goes whole (official
    // `RemoveParenthesesInGetterInvocation`; the report anchors the callee
    // expression).
    FixSpec {
        code: "invocation_of_non_function_expression",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_getter_invocation_parens),
    },
    // `late var (a, b) = …;` — the keyword goes through the next token,
    // trivia run included (official `RemoveLate`, `startStart(late,
    // next)`).
    FixSpec {
        code: "late_pattern_variable_declaration",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveKeywordAcrossComments("late"),
    },
    // The line terminator lands right after the triple-quote opener
    // (official `AddLeadingNewlineToString`).
    FixSpec {
        code: "leading_newlines_in_multiline_strings",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(add_leading_newline),
    },
    // The annotation moves onto a fresh `library;` line (official
    // `AddLibraryDirective`'s annotation face).
    FixSpec {
        code: "library_annotations",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(annotation_library_directive_entry),
    },
    // `new [1]` — the keyword plus one character (official
    // `RemoveLexeme.keyword`).
    FixSpec {
        code: "literal_with_new",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("new"),
    },
    // `required ` lands at the optional named parameter's head, a
    // last-in-metadata `@required` folding into the replacement (official
    // `AddRequiredKeyword`; the report anchors the parameter's name — the
    // positional shapes the shared registry name covers are
    // `MakeVariableNullable`'s legs and stay absent).
    FixSpec {
        code: "missing_default_value_for_parameter",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(add_required_keyword),
    },
    // `enum E` with no body — ` {}` after the report's end (official
    // `InsertBody`).
    FixSpec {
        code: "missing_enum_body",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::InsertAfter(" {}"),
    },
    // `void f();` — the `;` body becomes a block carrying the official
    // `// TODO: implement <name>` line and, for a non-`void` member, a
    // `throw UnimplementedError();` (official
    // `ConvertIntoBlockBody.missingBody`; the `=`/`return` recovery
    // shapes, which the official rewrites from the expression's static
    // type, get no edit).
    FixSpec {
        code: "missing_function_body",
        source: FixSource::Parser,
        needs_tokens: true,
        action: FixAction::Rewrite(missing_body_block),
    },
    // `typedef F<E>;` — `()` lands before the alias's `;` (official
    // `AddEmptyArgumentList`, the end of the token before the terminator).
    FixSpec {
        code: "missing_typedef_parameters",
        source: FixSource::Parser,
        needs_tokens: true,
        action: FixAction::Rewrite(add_typedef_parameters),
    },
    // The constructor member goes from the previous member's end (or the
    // body's `{`) through its own end token; a primary constructor's
    // `(…)` goes alone (official `RemoveConstructor`).
    FixSpec {
        code: "mixin_declares_constructor",
        source: FixSource::Parser,
        needs_tokens: true,
        action: FixAction::Rewrite(remove_constructor),
    },
    // The reported `on` type name goes — a lone name takes the whole `on`
    // clause, otherwise `nodeInList` (official `RemoveNameFromDeclarationClause`).
    FixSpec {
        code: "mixin_super_class_constraint_non_interface",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_clause_name),
    },
    // The duplicated case clause's line goes (official
    // `RemoveDuplicateCase`'s two faces by the previous clause's shape).
    FixSpec {
        code: "no_duplicate_case_values",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(remove_duplicate_case),
    },
    // The comparison keeps its non-literal operand — negated when the
    // literal's value survives the flip, the literal kept on an identity
    // logic operand (official `ConvertToBooleanExpression`'s binary
    // faces).
    FixSpec {
        code: "no_literal_bool_comparisons",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(bool_comparison),
    },
    // ` != null` lands at the reported condition's end (official
    // `AddNeNull`; the `AddAwait.nonBool` arm's resolved-type gate stays
    // absent). The report anchors the whole condition expression.
    FixSpec {
        code: "non_bool_condition",
        source: FixSource::Semantic,
        needs_tokens: false,
        action: FixAction::MultiRewrite(append_ne_null_entry),
    },
    // The reported `new` becomes `const` (official `UseConst`).
    FixSpec {
        code: "non_const_call_to_literal_constructor",
        source: FixSource::Semantic,
        needs_tokens: false,
        action: FixAction::Rewrite(new_to_const),
    },
    // The element's enclosing `const` goes — the producer walks to the
    // typed literal or instance creation's `const` (official
    // `RemoveConst`'s literal/creation faces).
    FixSpec {
        code: "non_constant_list_element",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_enclosing_const),
    },
    FixSpec {
        code: "non_constant_map_element",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_enclosing_const),
    },
    FixSpec {
        code: "non_constant_set_element",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_enclosing_const),
    },
    // A no-op member access on a primitive literal drops
    // (`1.toString()` → `1`, official `RemoveInvocation` — the operator
    // through the argument list).
    FixSpec {
        code: "noop_primitive_operations",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(remove_invocation_entry),
    },
    // `late ` lands at the field's declaration head — before
    // `final`/`var`, else before the written type, else before the
    // declarator (official `AddLate`; the
    // `CreateConstructorForFinalFields` and `MakeVariableNullable` arms
    // stay absent). The report anchors the field's name.
    FixSpec {
        code: "not_initialized_non_nullable_instance_field",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::MultiRewrite(add_late_entry),
    },
    // `late ` lands at the variable's declaration head — the same
    // `AddLate` `VariableDeclarationList` arm (the `MakeVariableNullable`
    // arm stays absent). The report anchors the variable's name.
    FixSpec {
        code: "not_initialized_non_nullable_variable",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::MultiRewrite(add_late_entry),
    },
    // The reported `!` goes (official `RemoveNonNullAssertion` — the
    // operator plus its trailing trivia).
    FixSpec {
        code: "null_check_always_fails",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_bang_expression),
    },
    // The reported `?` on the caught type goes (official
    // `RemoveQuestionMark`).
    FixSpec {
        code: "nullable_type_in_catch_clause",
        source: FixSource::Semantic,
        needs_tokens: false,
        action: FixAction::Rewrite(drop_type_question),
    },
    // The `:` before a named parameter's default becomes `=`, with a
    // leading space when the colon touches the name (official
    // `ReplaceColonWithEquals`).
    FixSpec {
        code: "obsolete_colon_for_default_value",
        source: FixSource::Semantic,
        needs_tokens: false,
        action: FixAction::Rewrite(colon_to_equals),
    },
    // The written type becomes `var`, the type-through-name gap going on
    // a multi-declarator list (official `ReplaceWithVar` + the
    // declaration-list face; a `… = {}` initializer and a parameterized
    // type refuse — the official's own guards).
    FixSpec {
        code: "omit_local_variable_types",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(omit_type_with_var),
    },
    // The written type becomes `var`, the type-through-name gap going on
    // a multi-declarator list (official `ReplaceWithVar` + the
    // declaration-list face; a `… = {}` initializer and a parameterized
    // type refuse — the official's own guards).
    FixSpec {
        code: "omit_obvious_local_variable_types",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(omit_type_with_var),
    },
    // The written type becomes `var`, the type-through-name gap going on
    // a multi-declarator list (official `ReplaceWithVar` + the
    // declaration-list face; a `… = {}` initializer and a parameterized
    // type refuse — the official's own guards).
    FixSpec {
        code: "omit_obvious_property_types",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(omit_type_with_var),
    },
    // The `@override` annotation leaves the member head — the `@` through
    // the trivia run to the next token, its attached comments staying
    // (official `RemoveAnnotation`; the report anchors the member's name).
    FixSpec {
        code: "override_on_non_overriding_member",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(remove_override_annotation),
    },
    // `(var a, b) = …;` — the declared pattern's keyword plus one character
    // goes (official `RemoveVarKeyword`; a typed pattern without a keyword
    // has no edit).
    FixSpec {
        code: "pattern_assignment_declares_variable",
        source: FixSource::Parser,
        needs_tokens: true,
        action: FixAction::Rewrite(remove_pattern_var_keyword),
    },
    // The `+` of an adjacent-string concatenation drops like a keyword.
    FixSpec {
        code: "prefer_adjacent_string_concatenation",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::DropLexeme("+"),
    },
    FixSpec {
        code: "prefer_collection_literals",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(collection_literal_entry),
    },
    // `if (a == null) a = b;` → `a ??= b;` — the null-check `if` becomes
    // the null-aware assignment.
    FixSpec {
        code: "prefer_conditional_assignment",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(conditional_assignment),
    },
    // `const ` lands before the reported creation, a `new` swapping to
    // `const` and inner depth-0 `const`s folding out (official
    // `AddConst`'s insert face + `ReplaceNewWithConst` +
    // `_ConstRangeFinder`).
    FixSpec {
        code: "prefer_const_constructors",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(add_const_prefix),
    },
    // `const ` lands before the reported creation, a `new` swapping to
    // `const` and inner depth-0 `const`s folding out (official
    // `AddConst`'s insert face + `ReplaceNewWithConst` +
    // `_ConstRangeFinder`).
    FixSpec {
        code: "prefer_const_constructors_in_immutables",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(add_const_prefix),
    },
    // `final x = 1;` → `const x = 1;` and the per-initializer inner
    // `const`s drop (official `ReplaceFinalWithConst`).
    FixSpec {
        code: "prefer_const_declarations",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(final_with_const_entry),
    },
    // `const ` lands before the reported literal, inner depth-0 `const`s
    // folding out (official `AddConst`'s insert face +
    // `_ConstRangeFinder`).
    FixSpec {
        code: "prefer_const_literals_to_create_immutables",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(add_const_prefix),
    },
    FixSpec {
        code: "prefer_contains",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(contains_call_entry),
    },
    // The literal's quotes move to `"` with the escapes rebalanced
    // (official `ConvertQuotes` state machine).
    FixSpec {
        code: "prefer_double_quotes",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(convert_to_double_quotes_entry),
    },
    // The `var` keyword becomes `final`, or the written type gains
    // `final ` — single-declarator lists only (official `MakeFinal`'s
    // declaration faces).
    // The `{ return e; }` body becomes `=> e;` — comments ride into the
    // arrow form, generators and constructor bodies refuse (official
    // `ConvertToExpressionFunctionBody`).
    FixSpec {
        code: "prefer_expression_function_bodies",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(expression_function_body),
    },
    FixSpec {
        code: "prefer_final_fields",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(make_declaration_final),
    },
    // `final ` lands at the for-each variable's first content token, a
    // `var` swapping in place (official `MakeFinal`'s parameter faces).
    FixSpec {
        code: "prefer_final_in_for_each",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(make_parameter_final),
    },
    // The `var` keyword becomes `final`, or the written type gains
    // `final ` — single-declarator lists only (official `MakeFinal`'s
    // declaration faces).
    FixSpec {
        code: "prefer_final_locals",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(make_declaration_final),
    },
    // `final ` lands at the parameter's first content token, a `var`
    // swapping in place (official `MakeFinal`'s parameter faces).
    FixSpec {
        code: "prefer_final_parameters",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(make_parameter_final),
    },
    // `Map.fromIterable(xs, key: (k) => k.id, value: (k) => k.name)`
    // becomes `{ for (var k in xs) k.id : k.name }` — the official
    // `ConvertMapFromIterableToForLiteral`'s same-parameter leg; the
    // different-name legs re-spell the closure bodies through a
    // parameter-reference face and are absent.
    FixSpec {
        code: "prefer_for_elements_to_map_fromIterable",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(map_from_iterable_for_element),
    },
    // The `for (var e in it) { f(e); }` statement becomes
    // `it.forEach(f);` — the declaration-form header, the body's single
    // invocation and the loop-variable argument all verified by the lint's
    // own gates (official `ConvertToForEach`).
    FixSpec {
        code: "prefer_foreach",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(for_each_tear_off),
    },
    FixSpec {
        code: "prefer_function_declarations_over_variables",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(function_declaration_entry),
    },
    // The old-form typedef rebuilds as a generic function type —
    // `typedef void F(int);` → `typedef F = void Function(int);` — the
    // written tail verbatim, an untyped parameter refusing (official
    // `ConvertToGenericFunctionSyntax`'s alias face).
    FixSpec {
        code: "prefer_generic_function_type_aliases",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(generic_alias_syntax),
    },
    // The conditional becomes an if-element in place (official
    // `ConvertConditionalExpressionToIfElement`; only list/set element
    // positions serve — maps and arguments are the official's own
    // refusals).
    FixSpec {
        code: "prefer_if_elements_to_conditional_expressions",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(conditional_to_if_element),
    },
    FixSpec {
        code: "prefer_if_null_operators",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(if_null_operators_entry),
    },
    // `this.` lands at the parameter's name — its written type drops
    // when it equals the field's — and the reported `: x = p`
    // initializer entry or `this.x = p;` body statement goes (official
    // `ConvertToInitializingFormal`'s pair; the private-field rename
    // `_x`, doc-comment reference, and extra-reference legs stay
    // absent). The report anchors the assignment.
    FixSpec {
        code: "prefer_initializing_formals",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::MultiRewrite(convert_to_init_formal_entry),
    },
    FixSpec {
        code: "prefer_inlined_adds",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(inlined_adds_entry),
    },
    // The integral double drops its fraction, exponent forms re-rendering
    // from the value (official `ConvertToIntLiteral`).
    FixSpec {
        code: "prefer_int_literals",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(double_to_int_literal),
    },
    FixSpec {
        code: "prefer_interpolation_to_compose_strings",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(compose_interpolation_entry),
    },
    // `list.length == 0` → `list.isEmpty` (and the official matrix's
    // `isNotEmpty` faces; the always-true/false judgments get no edit).
    FixSpec {
        code: "prefer_is_empty",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(is_empty_call),
    },
    // `!list.isEmpty` → `list.isNotEmpty`.
    FixSpec {
        code: "prefer_is_not_empty",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(is_not_empty_access),
    },
    FixSpec {
        code: "prefer_is_not_operator",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(is_not_operator_entry),
    },
    // `x.where((e) => e is T)` becomes `x.whereType<T>()` (official
    // `ConvertToWhereType`).
    FixSpec {
        code: "prefer_iterable_whereType",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(where_type_entry),
    },
    FixSpec {
        code: "prefer_null_aware_operators",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(null_aware_operators_entry),
    },
    // The literal's quotes move to `'` with the escapes rebalanced
    // (official `ConvertQuotes` state machine).
    FixSpec {
        code: "prefer_single_quotes",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(convert_to_single_quotes_entry),
    },
    // The first `..addAll(x)` on a list literal folds into `...x` /
    // `...?x` / `if (c) ...t` (official `ConvertAddAllToSpread`'s
    // non-inline face; a list-literal argument is `prefer_inlined_adds`'s).
    FixSpec {
        code: "prefer_spread_collections",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(spread_addition),
    },
    // The `null`/`Null` type display becomes `void` (official
    // `ReplaceNullWithVoid`).
    FixSpec {
        code: "prefer_void_to_null",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(replace_null_with_void),
    },
    // `const (1)` as a record — `,` lands after the single field, before
    // the report's `)` (official `AddTrailingComma`).
    FixSpec {
        code: "record_literal_one_positional_no_trailing_comma",
        source: FixSource::Parser,
        needs_tokens: true,
        action: FixAction::Rewrite(add_trailing_comma),
    },
    // `(int) x;` as a record type — `,` after the single positional field
    // (official `AddTrailingComma`).
    FixSpec {
        code: "record_type_one_positional_no_trailing_comma",
        source: FixSource::Parser,
        needs_tokens: true,
        action: FixAction::Rewrite(add_trailing_comma),
    },
    // `extension type E(var x)` — `var` goes through the next token
    // (official `RemoveKeyword.varKeyword`; the `AddTypeAnnotation` leg
    // over other modifiers needs the field's type and is not served).
    FixSpec {
        code: "representation_field_modifier",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveKeyword("var"),
    },
    // Registry row the parser does not emit today (official
    // `RemoveComma.representationField`: the report span itself).
    FixSpec {
        code: "representation_field_trailing_comma",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::ReplaceExact {
            expect: ",",
            replacement: "",
        },
    },
    // The comma lands right after the group's last item, before any
    // comment run (official `AddTrailingComma`'s `lastNode.end`).
    FixSpec {
        code: "require_trailing_commas",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(add_missing_trailing_comma),
    },
    // `sealed mixin M {}` / `sealed mixin class C {}` (official
    // `RemoveLexeme.modifier`).
    FixSpec {
        code: "sealed_mixin",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("sealed"),
    },
    FixSpec {
        code: "sealed_mixin_class",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("sealed"),
    },
    // `set` on a constructor (official `RemoveLexeme.keyword`).
    FixSpec {
        code: "setter_constructor",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("set"),
    },
    // `isEven: var isEven` keeps the `: var isEven` — the redundant
    // field-name token drops (official `RemoveUnnecessaryName`, the
    // pattern-field face).
    FixSpec {
        code: "simplify_variable_pattern",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_pattern_field_name),
    },
    // `Container(...)` → `SizedBox(...)` (official
    // `ReplaceContainerWithSizedBox` — the report sits on the
    // constructor name).
    FixSpec {
        code: "sized_box_for_whitespace",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::ReplaceExact {
            expect: "Container",
            replacement: "SizedBox",
        },
    },
    // The `/** … */` block rebuilds as `///` lines (official
    // `ConvertDocumentationIntoLine`'s state machine).
    FixSpec {
        code: "slash_for_doc_comments",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(doc_comment_to_line),
    },
    // The reported `child:`/`children:` argument moves behind the last
    // argument, its same-line comment riding along (official
    // `SortChildPropertyLast`; the widget-identity gate is the lint's own
    // decision of record).
    FixSpec {
        code: "sort_child_properties_last",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(child_property_last),
    },
    // The reported constructor member slides to the class body's head —
    // the official `SortConstructorFirst` moves each out-of-place
    // constructor ahead of the first non-constructor member, the moved
    // text carried with its own leading trivia.
    FixSpec {
        code: "sort_constructors_first",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(constructor_first),
    },
    // The reported named constructor slides ahead of the other named
    // constructors but behind the unnamed one (official
    // `SortUnnamedConstructorFirst` — the unnamed constructor anchors the
    // constructor block's head).
    FixSpec {
        code: "sort_unnamed_constructors_first",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(unnamed_constructor_first),
    },
    // `static C();` (official `RemoveLexeme.keyword`).
    FixSpec {
        code: "static_constructor",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("static"),
    },
    // Registry row the parser does not emit today: the `;` body of a static
    // getter becomes the official block (`ConvertIntoBlockBody.missingBody`).
    FixSpec {
        code: "static_getter_without_body",
        source: FixSource::Parser,
        needs_tokens: true,
        action: FixAction::Rewrite(missing_body_block),
    },
    // `static operator +(…)` (official `RemoveLexeme.keyword`).
    FixSpec {
        code: "static_operator",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("static"),
    },
    // Registry row the parser does not emit today: the `;` body of a static
    // setter becomes the official block (`ConvertIntoBlockBody.missingBody`).
    FixSpec {
        code: "static_setter_without_body",
        source: FixSource::Parser,
        needs_tokens: true,
        action: FixAction::Rewrite(missing_body_block),
    },
    // `if (b)` on a nullable `b` — the producer's target: the bare
    // identifier condition, the outermost `&&`/`||` root's left operand,
    // or the `as` inner expression takes the `!`; a chain's null-aware
    // operator goes `!.`-form (official `AddNullCheck` over the engine's
    // condition face; a `??` or ternary condition and every other shape
    // resolve no target in the official descent).

    // `x is Null` becomes `x == null`, `x is! Null` becomes `x != null` —
    // the type-check tail is replaced (official `UseEqEqNull` /
    // `UseNotEqNull`).
    FixSpec {
        code: "type_check_with_null",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(is_null_to_eq_null),
    },
    // The initializer-formal's written type goes with the whitespace to
    // the next token (official `RemoveTypeAnnotation.other`'s parameter
    // face).
    FixSpec {
        code: "type_init_formals",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(remove_reported_type),
    },
    // The reported type-literal constant pattern wraps in `const (…)`
    // (official `ConvertToConstantPattern` — the first-listed producer of
    // the registry pair).
    FixSpec {
        code: "type_literal_in_constant_pattern",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(const_pattern_literal),
    },
    // `await ` lands at the reported expression's head — the official
    // `AddAwait.unawaited` arm (the lint only fires inside async bodies;
    // the `WrapInUnawaited` face's `dart:async` import edit is absent).
    FixSpec {
        code: "unawaited_futures",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(await_future_expression),
    },
    FixSpec {
        code: "unchecked_use_of_nullable_value",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(null_check_condition),
    },
    // The name the target library does not define leaves its `show`/`hide`
    // clause — the `duplicate_hidden_name`/`duplicate_shown_name` pair's
    // producer (official `RemoveNameFromCombinator`; the report anchors the
    // name).
    FixSpec {
        code: "undefined_hidden_name",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_combinator_name),
    },
    // `await ` lands before the reported miss and `async ` at the
    // enclosing function's body — the official `AddAsync` default face
    // (`convertFunctionFromSyncToAsync` plus the `await` insertion; the
    // return-type rewrite reads the resolved type and stays absent). An
    // already-`async`/`sync*`/`async*` body refuses.
    FixSpec {
        code: "undefined_identifier_await",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::MultiRewrite(add_async_await_entry),
    },
    FixSpec {
        code: "undefined_shown_name",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_combinator_name),
    },
    // `1__0`, `1_e2`, `100_` — the literal is rebuilt without the
    // underscore runs that do not sit between two digits (official
    // `RemoveUnexpectedUnderscores`, its deletions folded into one
    // literal replacement).
    FixSpec {
        code: "unexpected_separator_in_number",
        source: FixSource::Parser,
        needs_tokens: true,
        action: FixAction::Rewrite(remove_number_separators),
    },
    // The `async` marker of a function whose body has no `await` drops
    // (official `RemoveAsync.unnecessary` — reduces to the keyword
    // deletion; the return-type legs serve other faces).
    FixSpec {
        code: "unnecessary_async",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::DropLexeme("async"),
    },
    // A leading `await` in a return inside an `async*` body drops
    // (official `RemoveKeyword.awaitKeyword` — the whitespace run to the
    // next token or its first comment).
    FixSpec {
        code: "unnecessary_await_in_return",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::RemoveKeyword("await"),
    },
    FixSpec {
        code: "unnecessary_brace_in_string_interps",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(unbrace_entry),
    },
    // The last statement of a non-empty block (`break;`) goes whole
    // (official `RemoveBreak` — a lines-range deletion).
    FixSpec {
        code: "unnecessary_breaks",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(drop_break_statement),
    },
    // The reported ` as T` tail goes — the cast's operator through the
    // type (official `RemoveUnnecessaryCast`, `endStart` of the cast).
    FixSpec {
        code: "unnecessary_cast",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_as_cast),
    },
    // The redundant `const` goes with the trivia run to the next token —
    // comments included (official `RemoveUnnecessaryConst`,
    // `startStart(const, const.next)`).
    FixSpec {
        code: "unnecessary_const",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::RemoveKeywordAcrossComments("const"),
    },
    // The redundant `const` of an enum value's invocation goes with the
    // trivia run to the next token — comments included (official
    // `RemoveUnnecessaryConst`'s keyword deletion,
    // `startStart(const, const.next)`; the const-push companion edits
    // serve other codes).
    FixSpec {
        code: "unnecessary_const_in_enum_constructor",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::RemoveKeywordAcrossComments("const"),
    },
    FixSpec {
        code: "unnecessary_constructor_name",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(drop_dot_new_entry),
    },
    // `final` → `var` at a variable declaration, or gone when a type is
    // written (official `ReplaceFinalWithVar`'s two faces).
    FixSpec {
        code: "unnecessary_final",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(final_to_var_entry),
    },
    // The trivial getter/setter pair folds into the field's public name —
    // `int get x => _x;` + `set x(v) => _x = v;` + `_x` become `int x;`
    // (official `ReplaceWithPublicField`: the two member lines drop and
    // the field's `_name` renames in place).
    FixSpec {
        code: "unnecessary_getters_setters",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(field_public),
    },
    // The stale ignore comment's line goes (official `RemoveComment`).
    FixSpec {
        code: "unnecessary_ignore",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(remove_ignore_comment),
    },
    // The lambda reduces to its callee's tear-off — `(x) => f(x)` →
    // `f`, `(x) { return f(x); }` → `f`; a `new`/`const`-marked creation
    // or a dotted head appends `.new` when unnamed (official
    // `ReplaceWithTearOff`; the bare single-name head is ambiguous
    // between function and ctor and drops).
    FixSpec {
        code: "unnecessary_lambdas",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(tear_off_lambda),
    },
    // The redundant `late` goes with the trivia run to the next token —
    // comments included (official `RemoveUnnecessaryLate`,
    // `startStart(late, late.next)`).
    FixSpec {
        code: "unnecessary_late",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::RemoveKeywordAcrossComments("late"),
    },
    // The redundant `library;` directive goes whole (official
    // `RemoveUnnecessaryLibraryDirective`).
    FixSpec {
        code: "unnecessary_library_directive",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(drop_library_directive),
    },
    // The name between the `library` keyword and its `;` goes (official
    // `RemoveLibraryName`).
    FixSpec {
        code: "unnecessary_library_name",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(drop_library_name),
    },
    // The `x.isNaN` comparison's reported operand goes with its `&&`/`||`
    // flank — the whole expression when it stands alone (official
    // `RemoveComparison`).
    FixSpec {
        code: "unnecessary_nan_comparison",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_comparison_operand),
    },
    // The redundant `new` goes with the trivia run to the next token —
    // comments included (official `RemoveUnnecessaryNew`,
    // `startStart(new, new.next)`).
    FixSpec {
        code: "unnecessary_new",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::RemoveKeywordAcrossComments("new"),
    },
    // The reported `!` goes — the operator plus its trailing trivia on the
    // expression face (official `RemoveNonNullAssertion`).
    FixSpec {
        code: "unnecessary_non_null_assertion",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_bang_expression),
    },
    // The reported `!` of a null-assert pattern goes (official
    // `RemoveNonNullAssertion`'s pattern face — the operator alone).
    FixSpec {
        code: "unnecessary_null_assert_pattern",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_bang_pattern),
    },
    FixSpec {
        code: "unnecessary_null_aware_assignments",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(remove_null_aware_assignment_entry),
    },
    // The reported `?.`/`?[`/`?..`/`...?` drops its `?` — the same
    // `ReplaceWithNotNullAware` operator rewrite the semantic face
    // serves.
    FixSpec {
        code: "unnecessary_null_aware_operator_on_extension_on_nullable",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(not_null_aware_operator),
    },
    // The reported `?` of a null-check pattern goes (official
    // `RemoveQuestionMark`'s pattern face).
    FixSpec {
        code: "unnecessary_null_check_pattern",
        source: FixSource::Semantic,
        needs_tokens: false,
        action: FixAction::Rewrite(drop_type_question),
    },
    // The `!` under a nullable written context drops (official
    // `RemoveNonNullAssertion` — the operator character).
    FixSpec {
        code: "unnecessary_null_checks",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(drop_non_null_assertion),
    },
    // The reported `== null`/`!= null` comparison goes with its `&&`/`||`
    // flank — the whole expression when it stands alone (official
    // `RemoveComparison`).
    FixSpec {
        code: "unnecessary_null_comparison",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_comparison_operand),
    },
    FixSpec {
        code: "unnecessary_null_in_if_null_operators",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(drop_if_null_null_entry),
    },
    FixSpec {
        code: "unnecessary_nullable_for_final_variable_declarations",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(drop_type_question_mark_entry),
    },
    // The override member whose body only calls super goes, annotations
    // and doc comments included (official `RemoveOverride`'s lines-range
    // face; the field and cascade shapes read resolution and are
    // absent).
    FixSpec {
        code: "unnecessary_overrides",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(remove_override_member),
    },
    // The expression's redundant parentheses go, keeping the inner text
    // (official `RemoveParentheses` — the operator characters).
    FixSpec {
        code: "unnecessary_parenthesis",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_parentheses_entry),
    },
    // The reported `this` keyword's whole `PrimaryConstructorBody` drops
    // by its lines range (official `RemovePrimaryConstructorBody`,
    // `getLinesRange(body)`).
    FixSpec {
        code: "unnecessary_primary_constructor_body",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_primary_constructor_body),
    },
    // The reported `?` on a type (or `?.` / `?[` form) goes (official
    // `RemoveQuestionMark`'s type/suffix faces).
    FixSpec {
        code: "unnecessary_question_mark",
        source: FixSource::Semantic,
        needs_tokens: false,
        action: FixAction::Rewrite(drop_type_question),
    },
    // The `r` prefix of the reported literal goes (official
    // `RemoveUnnecessaryRawString`).
    FixSpec {
        code: "unnecessary_raw_strings",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(remove_raw_prefix),
    },
    // The needless escape's backslash goes.
    FixSpec {
        code: "unnecessary_string_escapes",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::ReplaceExact {
            expect: "\\",
            replacement: "",
        },
    },
    FixSpec {
        code: "unnecessary_string_interpolations",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(unwrap_string_interpolation_entry),
    },
    FixSpec {
        code: "unnecessary_this",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(drop_this_qualifier_entry),
    },
    // `...a.toList()` → `...a` — the invocation leaves the spread.
    FixSpec {
        code: "unnecessary_to_list_in_spreads",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(remove_to_list),
    },
    // The reported `is T`/`is! T` check goes with its `&&`/`||` flank —
    // the whole expression when it stands alone (official
    // `RemoveComparison`).
    FixSpec {
        code: "unnecessary_type_check",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_comparison_operand),
    },
    // The reported type-name echo drops — `C(...)` → `new()`,
    // `C.named(...)` → `new named(...)`, `C.new(...)` → `new()`, and the
    // `factory`/`new`-led faces (official `RemoveTypeName`'s branches).
    FixSpec {
        code: "unnecessary_type_name_in_constructor",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_constructor_type_name),
    },
    // The `unawaited(...)` wrapper sheds to its argument expression
    // (official `RemoveUnawaited` — the name through `(` and the `)`).
    FixSpec {
        code: "unnecessary_unawaited",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(remove_unawaited_entry),
    },
    // The reported multi-underscore name re-spells `_` — the official
    // `ConvertToWildcardVariable.automatically`'s parameter and local
    // faces; the parameter face keeps the official owner gate (a closure
    // or a `static` method — a top-level function, constructor, instance
    // method, or named group refuses), and the local face needs no
    // references because the rule's own unreferenced gate held.
    FixSpec {
        code: "unnecessary_underscores",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(wildcard_underscore),
    },
    // The reported `_` wildcard pattern's `&&` operand goes — the pattern
    // itself when it stands alone (official
    // `RemoveUnnecessaryWildcardPattern`, `RemoveComparison`'s shape).
    FixSpec {
        code: "unnecessary_wildcard_pattern",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_wildcard_and_operand),
    },
    // The dead `case` member's span goes — head through its statements
    // (official `RemoveDeadCode`'s `SwitchMember` arm).
    FixSpec {
        code: "unreachable_switch_case",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_switch_member),
    },
    FixSpec {
        code: "unreachable_switch_default",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_switch_member),
    },
    // The reported `==`/`!=` becomes `is`/`is!`; when the left operand is
    // the `Type` literal the whole binary reorders (official
    // `ReplaceWithIs` — the `Type` display rides the message).
    FixSpec {
        code: "unrelated_type_equality_checks",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(equality_is_check),
    },
    FixSpec {
        code: "unused_catch_clause",
        source: FixSource::Semantic,
        needs_tokens: false,
        action: FixAction::Rewrite(drop_catch_clause_entry),
    },
    FixSpec {
        code: "unused_catch_stack",
        source: FixSource::Semantic,
        needs_tokens: false,
        action: FixAction::Rewrite(drop_catch_stack_entry),
    },
    // The unused local function's declaration lines go — the return-type
    // through the body (official `RemoveUnusedElement`'s declaration
    // face).
    FixSpec {
        code: "unused_element",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_unused_element),
    },
    // The reported parameter goes with its comma — a sole parameter takes
    // the empty list face (official `RemoveUnusedParameter`).
    FixSpec {
        code: "unused_element_parameter",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_unused_parameter),
    },
    // The unused field's declaration lines go — a multi-declarator list
    // drops only its own declarator (official `RemoveUnusedField`).
    FixSpec {
        code: "unused_field",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(drop_unused_field),
    },
    FixSpec {
        code: "unused_import",
        source: FixSource::Semantic,
        needs_tokens: false,
        action: FixAction::Rewrite(drop_import_directive_entry),
    },
    FixSpec {
        code: "unused_label",
        source: FixSource::Semantic,
        needs_tokens: false,
        action: FixAction::Rewrite(drop_label_entry),
    },
    FixSpec {
        code: "unused_local_variable",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(rewrite_unused_local_entry),
    },
    FixSpec {
        code: "unused_shown_name",
        source: FixSource::Semantic,
        needs_tokens: false,
        action: FixAction::Rewrite(drop_shown_name_entry),
    },
    // The missing URI target is created: an import/export target is born
    // with the upstream TODO stub, a `part` target with the
    // `part of '<library>';` head (official `CreateFile` — the cross-file
    // face the module docs pin; the row's executor offers it only while
    // the target is absent).
    FixSpec {
        code: "uri_does_not_exist",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::CreateFile(create_missing_uri_target),
    },
    // `Container(...)` → `ColoredBox(...)` (official
    // `ReplaceContainerWithColoredBox` — the report sits on the
    // constructor name).
    FixSpec {
        code: "use_colored_box",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::ReplaceExact {
            expect: "Container",
            replacement: "ColoredBox",
        },
    },
    // The reported primary-constructor parameter becomes a declaring
    // parameter — `final `/`var ` by the field's own keyword — and the
    // field's lines go; a `this.f` field formal replaces its `this.`
    // region, a plain parameter assigned by a `: f = p` initializer
    // inserts the keyword and drops the initializer with the field
    // (official `ConvertToDeclaringParameter`; the metadata-move,
    // body-block-assignment, multi-declarator-field, and
    // constructor-adjacent-field legs stay absent). The report anchors
    // the parameter's name.
    FixSpec {
        code: "use_declaring_parameters",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::MultiRewrite(convert_to_declaring_param_entry),
    },
    // The short hex literal pads to eight digits — a value re-render, an
    // in-place separator padding, or the `aa_bb_cc` → `00_aa_bb_cc`
    // triplet (official `ReplaceWithEightDigitHex`'s three faces).
    FixSpec {
        code: "use_full_hex_values_for_flutter_colors",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(eight_digit_hex),
    },
    // The old-style function-typed parameter rebuilds —
    // `void cb(int x)` → `void Function(int x) cb` — modifiers and
    // receiver prefixes kept in order, an untyped inner parameter
    // refusing (official `ConvertToGenericFunctionSyntax`'s parameter
    // face).
    FixSpec {
        code: "use_function_type_syntax_for_parameters",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(function_type_syntax),
    },
    // `x == null` becomes `x ?? false`/`x ?? true`, the outer parens by
    // the flanking precedence (official `ConvertToIfNull`'s boolean
    // faces).
    FixSpec {
        code: "use_if_null_to_convert_nulls_to_bools",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(if_null_bools),
    },
    // The reported constructor gains the `{super.key}` parameter —
    // official `AddKeyToConstructors`'s constructor face, super-parameter
    // leg; the class-name face (the constructor synthesis with its
    // const-ability analysis) and the explicit-super-initializer update
    // leg refuse.
    FixSpec {
        code: "use_key_in_widget_constructors",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(add_super_key),
    },
    // The null-check if-element becomes `?x`/`...?x` (official
    // `ConvertNullCheckToNullAwareElementOrEntry`'s element faces; the
    // map-entry faces need the canonical-element resolution).
    // The reported creation folds into the constant the message names —
    // `const C(0)` becomes `C.zero` (official `ReplaceWithNamedConstant`:
    // the first single-quoted run of the problem message is the
    // replacement; the lint's own gate restricts the report to a creation
    // whose constant exists).
    FixSpec {
        code: "use_named_constants",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(named_constant_reference),
    },
    FixSpec {
        code: "use_null_aware_elements",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(null_aware_element),
    },
    // The literal becomes raw with its `\\`/`\$` escapes folded out
    // (official `ConvertToRawString`; any other escape refuses).
    FixSpec {
        code: "use_raw_strings",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(convert_to_raw_string_entry),
    },
    // `throw e;` → `rethrow;` — the thrown parameter's span is replaced.
    FixSpec {
        code: "use_rethrow_when_possible",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::Rewrite(rethrow_span),
    },
    // `(a / b).truncate()` folds into `a ~/ b`, wrapper parens kept at
    // multiplicative strength (official `UseEffectiveIntegerDivision`'s
    // two faces).
    FixSpec {
        code: "use_truncating_division",
        source: FixSource::Lint,
        needs_tokens: true,
        action: FixAction::Rewrite(truncating_division),
    },
    // `var int x = 1;` — the `var` report plus one character goes (official
    // `RemoveVar`, the leg that holds in every context; the
    // `RemoveTypeAnnotation.fixVarAndType` leg, which keeps `var` and
    // drops the type instead, is not served).
    FixSpec {
        code: "var_and_type",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("var"),
    },
    // `var` in a type position becomes `dynamic` (official
    // `ReplaceVarWithDynamic`: the report span replaced).
    FixSpec {
        code: "var_as_type_name",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::ReplaceExact {
            expect: "var",
            replacement: "dynamic",
        },
    },
    // `var f() {}` — the `var` report plus one character (official
    // `RemoveVar`).
    FixSpec {
        code: "var_return_type",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("var"),
    },
    // The reported `var` keyword plus one character goes — the official
    // `RemoveLexeme.keyword` deletion; the lexeme is fixed (`var` is the
    // only keyword the rule reports).
    FixSpec {
        code: "var_with_no_type_annotation",
        source: FixSource::Lint,
        needs_tokens: false,
        action: FixAction::RemoveLexeme("var"),
    },
    // `var (var a, b) = …;` — the inner `var`/`final` report plus one
    // character (official `RemoveVar`: the report span, whichever keyword
    // it carries).
    FixSpec {
        code: "variable_pattern_keyword_in_declaration_context",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::Rewrite(remove_hit_lexeme),
    },
    // The misplaced `<…>` on a named-creation's constructor segment moves
    // onto the class name — `C.named<int>` → `C<int>.named` (official
    // `MoveTypeArgumentsToClass`; the class-already-typed shape refuses).
    FixSpec {
        code: "wrong_number_of_type_arguments_constructor",
        source: FixSource::Semantic,
        needs_tokens: true,
        action: FixAction::Rewrite(move_type_args_to_class),
    },
    // `[int x: 1]` — the `:` becomes `=`, with a leading space when the
    // colon touches the name (official `ReplaceColonWithEquals`).
    FixSpec {
        code: "wrong_separator_for_positional_parameter",
        source: FixSource::Parser,
        needs_tokens: false,
        action: FixAction::Rewrite(colon_to_equals),
    },
];

/// The table row of `code`, if the code carries a mechanical fix.
#[must_use]
pub fn spec_for(code: &str) -> Option<&'static FixSpec> {
    FIX_TABLE
        .binary_search_by(|spec| spec.code.as_bytes().cmp(code.as_bytes()))
        .ok()
        .map(|index| &FIX_TABLE[index])
}

/// Whether `code` is one of the engine-sourced codes [`crate::fix::from_semantic`]
/// or [`crate::fix::from_parser`] materializes — the engine pass's early
/// filter (the table's semantic and parser rows are the set).
#[must_use]
pub fn is_engine_fix_code(code: &str) -> bool {
    spec_for(code)
        .is_some_and(|spec| matches!(spec.source, FixSource::Semantic | FixSource::Parser))
}

/// The declaration-synthesis producer family a code's evidence serves —
/// the shape the evidence row carries facts for and the follow-up
/// create-family producers dispatch on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SynthesisArm {
    /// A type-name miss (`CreateClass`/`CreateMixin` family): the
    /// producer synthesizes a type declaration carrying the miss's
    /// spelling, inserted at the enclosing compilation-unit member's end.
    TypeName,
    /// A callable miss (`CreateFunction`/`CreateMethod`/accessor family):
    /// the producer synthesizes a member into the enclosing container's
    /// body, or a library callable beside the enclosing unit member.
    Callable,
    /// An override/abstract-member gap (`CreateMissingOverrides`): the
    /// producer synthesizes members into the diagnostic's own container;
    /// no miss name plays a role.
    Override,
    /// A constructor/field gap (`CreateConstructor` family): the producer
    /// synthesizes into the diagnostic's own container.
    Container,
}

/// The synthesis arm of an emitted semantic code, or `None` when the code's
/// official producers read no target-declaration evidence (the mechanical
/// rows above, the written-type family, every non-synthesis face). The set
/// is the D2 census's declaration-synthesis inventory, keyed by the code
/// names the semantic tier emits — the re-routed context faces
/// (`non_type_as_type_argument`, `creation_with_non_type`) included: the
/// diagnoser already decided which code fired, and the evidence face keys
/// on that decision of record. `undefined_identifier_await` is excluded —
/// its official producer is `AddAsync`, a body conversion, not declaration
/// synthesis.
#[must_use]
pub fn synthesis_arm(code: &str) -> Option<SynthesisArm> {
    let arm = match code {
        "undefined_class"
        | "extends_non_class"
        | "implements_non_class"
        | "mixin_of_non_class"
        | "non_type_as_type_argument"
        | "creation_with_non_type" => SynthesisArm::TypeName,
        "undefined_function"
        | "undefined_method"
        | "undefined_getter"
        | "undefined_setter"
        | "undefined_operator"
        | "undefined_identifier"
        | "undefined_extension_method"
        | "undefined_extension_getter"
        | "undefined_extension_setter"
        | "undefined_extension_operator"
        | "undefined_enum_constructor"
        | "undefined_named_parameter" => SynthesisArm::Callable,
        "missing_override_of_must_be_overridden"
        | "non_abstract_class_inherits_abstract_member" => SynthesisArm::Override,
        "extra_positional_arguments"
        | "extra_positional_arguments_could_be_named"
        | "not_initialized_non_nullable_instance_field" => SynthesisArm::Container,
        _ => return None,
    };
    Some(arm)
}

/// Runs a row's transform over `input` — the one place the action kinds
/// execute. The data kinds run the shared shapes; the rewriter runs
/// itself.
pub fn expand(spec: &FixSpec, input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    match spec.action {
        FixAction::DropLexeme(word) => fix::drop_keyword(input.text, input.range.start, word),
        FixAction::RemoveLexeme(word) => fix::remove_lexeme(input.text, input.range.start, word),
        FixAction::RemoveKeyword(word) => fix::remove_keyword(input.text, input.range.start, word),
        FixAction::RemoveKeywordAcrossComments(word) => {
            fix::remove_keyword_across_comments(input.text, input.range.start, word)
        }
        FixAction::ReplaceExact {
            expect,
            replacement,
        } => fix::replace_exact(input.text, &input.range, expect, replacement),
        FixAction::InsertAfter(text) => Some((input.range.end, 0, text.to_owned())),
        FixAction::Rewrite(rewrite) => rewrite(input),
        FixAction::MultiRewrite(rewrite) => rewrite(input).and_then(|edits| {
            // The multi face never answers through this single-edit entry;
            // an executor that forgot `expand_multi` would silently take
            // the first edit. Refuse rather than half-serve the change.
            debug_assert!(edits.is_empty(), "MultiRewrite behind expand()");
            None
        }),
        // The cross-file face never answers through this same-file entry —
        // its edit is addressed to another file. The executors consult
        // [`expand_create`] first; an executor that skipped it would
        // silently drop the change, never misaddress it.
        FixAction::CreateFile(_) => None,
    }
}

/// Runs a row's transform like [`expand`], but the multi-edit kinds keep
/// their full disjoint set — the executors' entry, materializing one row
/// per edit in the producer's own order. A producer returning an empty
/// list is a no-op fix and drops.
#[must_use]
pub fn expand_multi(spec: &FixSpec, input: &FixInput<'_>) -> Option<Vec<(u32, u32, String)>> {
    match spec.action {
        FixAction::MultiRewrite(rewrite) => match rewrite(input)? {
            edits if edits.is_empty() => None,
            edits => Some(edits),
        },
        _ => expand(spec, input).map(|edit| vec![edit]),
    }
}

/// Runs a row's transform when it is a cross-file kind — the created-file
/// face — answering `(target path, born content)`; `None` for every
/// same-file kind, so the executors try this first and fall through to
/// [`expand_multi`]. A `CreateFile` row never serves a same-file edit
/// ([`expand`] matches no data kind for it), and a same-file kind never
/// answers here — the two entries partition the action set.
#[must_use]
pub fn expand_create(spec: &FixSpec, input: &FixInput<'_>) -> Option<(String, String)> {
    match spec.action {
        FixAction::CreateFile(producer) => producer(input),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Adapters
//
// The legacy rewriters in [`crate::fix`] keep their original signatures;
// each adapter fits one to [`FixInput`] — the span form (a hit range or a
// diagnostic's offset/length) and the token stream it reads. A token
// rewriter with no stream drops, exactly as the dispatch it replaces did.
// ---------------------------------------------------------------------------

macro_rules! span_entry {
    ($name:ident, $rewrite:path) => {
        fn $name(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
            $rewrite(input.text, &input.range)
        }
    };
}

macro_rules! token_entry {
    ($name:ident, $rewrite:path) => {
        fn $name(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
            $rewrite(input.tokens?, input.text, &input.range)
        }
    };
}

macro_rules! source_entry {
    ($name:ident, $rewrite:path) => {
        fn $name(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
            $rewrite(
                input.text,
                SourceRange {
                    offset: input.range.start,
                    length: input.range.end - input.range.start,
                },
            )
        }
    };
}

span_entry!(unbrace_entry, fix::unbrace);
span_entry!(
    unwrap_string_interpolation_entry,
    fix::unwrap_string_interpolation
);
span_entry!(drop_if_null_null_entry, fix::drop_if_null_null);
span_entry!(
    remove_null_aware_assignment_entry,
    fix::remove_null_aware_assignment
);
span_entry!(drop_null_initializer_entry, fix::drop_null_initializer);

fn drop_this_qualifier_entry(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    fix::drop_this_qualifier(input.text, input.range.start)
}

fn drop_type_question_mark_entry(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    fix::drop_type_question_mark(input.text, input.range.start)
}

fn drop_dot_new_entry(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    fix::drop_dot_new(input.text, input.range.start)
}

token_entry!(is_not_operator_entry, fix::is_not_operator);
token_entry!(if_null_operators_entry, fix::if_null_operators);
token_entry!(null_aware_operators_entry, fix::null_aware_operators);
token_entry!(contains_call_entry, fix::contains_call);
token_entry!(compose_interpolation_entry, fix::compose_interpolation);
token_entry!(collection_literal_entry, fix::collection_literal);
token_entry!(inlined_adds_entry, fix::inlined_adds);
token_entry!(function_declaration_entry, fix::function_declaration);
token_entry!(remove_empty_catch_entry, fix::remove_empty_catch);
token_entry!(remove_invocation_entry, fix::remove_invocation);
token_entry!(remove_unawaited_entry, fix::remove_unawaited);
token_entry!(drop_parentheses_entry, fix::drop_parentheses);
token_entry!(final_to_var_entry, fix::final_to_var);
token_entry!(final_with_const_entry, fix::final_with_const);
token_entry!(
    convert_to_single_quotes_entry,
    fix::convert_to_single_quotes
);
token_entry!(
    convert_to_double_quotes_entry,
    fix::convert_to_double_quotes
);

source_entry!(drop_import_directive_entry, fix::drop_import_directive);
source_entry!(drop_shown_name_entry, fix::drop_shown_name);
source_entry!(drop_catch_clause_entry, fix::drop_catch_clause);
source_entry!(drop_catch_stack_entry, fix::drop_catch_stack);
source_entry!(drop_label_entry, fix::drop_label);

fn rewrite_unused_local_entry(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    fix::rewrite_unused_local(
        input.tokens,
        input.text,
        SourceRange {
            offset: input.range.start,
            length: input.range.end - input.range.start,
        },
    )
}

#[cfg(test)]
mod tests {
    //! The table's structural contract: the order its lookup reads, the
    //! registries its codes must resolve in, and the legacy surface the
    //! executors' goldens pin. The rewrite behavior itself is pinned per
    //! family in [`crate::fix`]'s suite.

    use dartq_diagnostics::{CodeRegistry, DiagnosticType};
    use dartq_lints::rule_by_name;
    use dartq_semantic::codes::SemanticCode;

    use super::*;

    /// The binary search reads the order: the codes are strictly ascending
    /// (sorted and unique in one).
    #[test]
    fn table_codes_are_strictly_ascending() {
        for pair in FIX_TABLE.windows(2) {
            assert!(
                pair[0].code.as_bytes() < pair[1].code.as_bytes(),
                "codes must be strictly ascending for the binary search: {} then {}",
                pair[0].code,
                pair[1].code,
            );
        }
    }

    /// Every row's code exists in its pipeline's registry — the lint codes
    /// resolve through the wired rule registry, the semantic codes are the
    /// engine's own code-enum spellings, the parser codes are
    /// `SYNTACTIC_ERROR` rows of the official syntax-code table — and the
    /// engine's fix families are fully covered: a semantic code the pass
    /// filters in but the table cannot expand would fix nothing, silently.
    #[test]
    fn every_code_exists_in_its_pipeline_registry() {
        let drop_family: Vec<&str> = [
            SemanticCode::UnusedImport,
            SemanticCode::UnusedLocalVariable,
            SemanticCode::UnusedShownName,
            SemanticCode::UnusedCatchClause,
            SemanticCode::UnusedCatchStack,
            SemanticCode::UnusedLabel,
        ]
        .into_iter()
        .map(|code| code.code_info().name)
        .collect();
        let null_check_family: Vec<&str> = [
            SemanticCode::UncheckedUseOfNullableValue,
            SemanticCode::InvalidAssignment,
            SemanticCode::ArgumentTypeNotAssignable,
        ]
        .into_iter()
        .map(|code| code.code_info().name)
        .collect();
        // The written-surface family: the emitted semantic codes whose
        // official producer's edit is provable from the report span, the
        // file text, and the token stream.
        let written_family: &[&str] = &[
            "body_might_complete_normally_nullable",
            "const_eval_method_invocation",
            "const_initialized_with_non_constant_value",
            "const_instance_field",
            "const_with_non_const",
            "constant_pattern_with_non_constant_expression",
            "dead_code",
            "dead_code_catch_following_catch",
            "dead_code_on_catch_subtype",
            "dead_null_aware_expression",
            "duplicate_hidden_name",
            "duplicate_import",
            "duplicate_shown_name",
            "extends_non_class",
            "extension_type_declares_member_of_object",
            "final_not_initialized",
            "import_of_non_library",
            "invalid_null_aware_operator",
            "invocation_of_non_function_expression",
            "mixin_super_class_constraint_non_interface",
            "missing_default_value_for_parameter",
            "non_bool_condition",
            "non_const_call_to_literal_constructor",
            "non_constant_list_element",
            "non_constant_map_element",
            "non_constant_set_element",
            "not_initialized_non_nullable_instance_field",
            "not_initialized_non_nullable_variable",
            "null_check_always_fails",
            "nullable_type_in_catch_clause",
            "obsolete_colon_for_default_value",
            "override_on_non_overriding_member",
            "type_check_with_null",
            "undefined_hidden_name",
            "undefined_identifier_await",
            "undefined_shown_name",
            "unnecessary_cast",
            "unnecessary_nan_comparison",
            "unnecessary_non_null_assertion",
            "unnecessary_null_assert_pattern",
            "unnecessary_null_check_pattern",
            "unnecessary_null_comparison",
            "unnecessary_question_mark",
            "unnecessary_type_check",
            "unnecessary_wildcard_pattern",
            "unreachable_switch_case",
            "unreachable_switch_default",
            "wrong_number_of_type_arguments_constructor",
            "unused_element",
            "unused_element_parameter",
            "unused_field",
        ];
        // The create-file family: the cross-file face whose official
        // producer's change targets a file other than the hit file — the
        // missing URI target the report names.
        let create_family: &[&str] = &["uri_does_not_exist"];
        let registry = CodeRegistry::builtin();
        for spec in FIX_TABLE {
            match spec.source {
                FixSource::Lint => {
                    assert!(
                        rule_by_name(spec.code).is_some(),
                        "lint code {} is not a wired rule",
                        spec.code,
                    );
                }
                FixSource::Semantic => {
                    assert!(
                        drop_family.contains(&spec.code)
                            || null_check_family.contains(&spec.code)
                            || written_family.contains(&spec.code)
                            || create_family.contains(&spec.code),
                        "semantic code {} is outside the engine fix families",
                        spec.code,
                    );
                }
                FixSource::Parser => {
                    let info = registry.get(spec.code).unwrap_or_else(|| {
                        panic!("parser code {} is not a registry code", spec.code)
                    });
                    assert_eq!(
                        info.dtype,
                        DiagnosticType::SyntacticError,
                        "parser code {} is not a syntax-table row",
                        spec.code,
                    );
                }
            }
        }
        for code in drop_family
            .iter()
            .chain(&null_check_family)
            .chain(written_family)
        {
            assert!(
                spec_for(code).is_some(),
                "engine fix code {code} has no table row",
            );
        }
    }

    /// The parser face serves the official registry's mechanical parser
    /// producers code for code: 60 rows, and the three type-dependent
    /// producers (`AddTypeName`, `AddMissingClosureParameters`,
    /// `AddTypeAnnotation`) stay unclaimed. The semantic face holds its own
    /// count pin: 61 rows over the drop, null-check, and written-surface
    /// families (the #2069 wave-1 combinator/drop/insert additions, the
    /// #2070 wave-2 multi-edit faces, and the `uri_does_not_exist`
    /// create-file face included).
    #[test]
    fn parser_face_covers_the_mechanical_registry() {
        let parser_rows = FIX_TABLE
            .iter()
            .filter(|spec| spec.source == FixSource::Parser)
            .count();
        assert_eq!(parser_rows, 60);
        let semantic_rows = FIX_TABLE
            .iter()
            .filter(|spec| spec.source == FixSource::Semantic)
            .count();
        assert_eq!(semantic_rows, 61);
        for code in [
            "experiment_not_enabled",
            "missing_identifier",
            "missing_const_final_var_or_type",
        ] {
            assert!(spec_for(code).is_none(), "{code} needs resolved types");
        }
        assert!(is_engine_fix_code("expected_token"));
        assert!(is_engine_fix_code("unused_import"));
        assert!(
            !is_engine_fix_code("unnecessary_new"),
            "lint rows are not engine rows"
        );
    }

    /// The lookup is the executors' only entry: a served code resolves with
    /// its pipeline's source, and codes without a mechanical fix carry no
    /// row.
    #[test]
    fn lookup_resolves_the_served_codes_only() {
        let spec = spec_for("unnecessary_new").expect("legacy family");
        assert_eq!(spec.source, FixSource::Lint);
        assert!(!spec.needs_tokens);
        let spec = spec_for("unused_import").expect("legacy drop family");
        assert_eq!(spec.source, FixSource::Semantic);
        let spec = spec_for("final_mixin").expect("parser modifier family");
        assert_eq!(spec.source, FixSource::Parser);
        assert!(!spec.needs_tokens);
        let spec = spec_for("invalid_assignment").expect("null-check family");
        assert_eq!(spec.source, FixSource::Semantic);
        assert!(spec.needs_tokens);
        // A token-verified family carries the parse gate.
        assert!(spec_for("prefer_contains").is_some_and(|spec| spec.needs_tokens));
        assert!(spec_for("expected_token").is_some_and(|spec| spec.needs_tokens));
        // No mechanical rewrite, no row.
        assert!(
            spec_for("camel_case_types").is_none(),
            "rename, not mechanical"
        );
        assert!(
            spec_for("unused_element").is_some(),
            "the written-surface family drops the declaration"
        );
        assert!(
            spec_for("non_type_in_catch_clause").is_none(),
            "the official fix is an import edit, not mechanical"
        );
    }
}
