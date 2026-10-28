//! The expression-shape family: `unnecessary_new`,
//! `unnecessary_null_in_if_null_operators`, `prefer_is_not_operator`,
//! `prefer_adjacent_string_concatenation`,
//! `avoid_single_cascade_in_expression_statements`,
//! `prefer_conditional_assignment`, `prefer_if_null_operators`,
//! `prefer_null_aware_operators`.

use dartq_ast::{NodeKind, PayloadField};

use crate::engine::{Check, narrow_u32};
use crate::registry::{Group, RuleSpec};
use crate::rules::{NONE_TOKEN, slice, token_seq_equal};

/// The domain's incoming registrations: a new standalone rule appends its
/// `SPEC` reference here (name order) and joins the registry through the
/// aggregated tail — adding a rule touches only this file. Batch close
/// promotes the block into the registry's frozen skeleton together with
/// the pin-test wave list (`tests/registry_pin.rs`).
pub(crate) static INCOMING_RULES: &[&RuleSpec] = &[];

/// A row's own token span as the semantic query face's `SourceRange` —
/// the analysis queries' span contract (a row and its fold row share the
/// identifier token's own range). UTF-16 — the fold rows' coordinate
/// convention; the queries match on the start offset, the length rides
/// along for construction.
pub(crate) fn query_site(c: &Check<'_>, row: u32) -> dartq_source::SourceRange {
    let span = c.utf16_span(row);
    dartq_source::SourceRange {
        offset: span.start,
        length: span.end - span.start,
    }
}

/// The enclosing switch's scrutinee read row (the pattern-matched-type
/// seam the `null_check_on_nullable_type_parameter` and
/// `unrelated_type_equality_checks` pattern faces share): the scrutinee
/// pieces tile flat as the switch statement/expression's direct rows
/// before the case block (the production tiling) — a lone `Identifier`
/// row is the bare variable-read scrutinee the matched-value face
/// answers.
fn switch_scrutinee_read(c: &Check<'_>) -> Option<u32> {
    for &ancestor in c.path.iter().rev().skip(1) {
        let boundary = match c.kind(ancestor) {
            NodeKind::SwitchStatement => NodeKind::SwitchBlock,
            NodeKind::SwitchExpression => NodeKind::SwitchExpressionBlock,
            _ => continue,
        };
        let kids = c.collect_direct(ancestor);
        let block_pos = kids.iter().position(|&kid| c.kind(kid) == boundary)?;
        // The bare-scrutinee read tiles as a send group (4 pieces) with a
        // trailing `ParenthesizedCondition` wrapper — the access run strips
        // the send pieces; exactly one surviving identifier answers (a
        // member-access scrutinee carries two and stays silent).
        let run = slice::access_run(c, &kids[..block_pos]);
        let mut ids = run
            .iter()
            .copied()
            .filter(|&row| c.kind(row) == NodeKind::Identifier);
        return match (ids.next(), ids.next()) {
            (Some(only), None) => Some(only),
            _ => None,
        };
    }
    None
}

/// The span end of an expression run: the last non-completion-wrapper row
/// (the statement-level `Send` and its argument/type-argument group
/// markers finalize at the statement's `;`, which the container row
/// swallows — the value expression's own rows bound the span).
fn value_run_end(c: &Check<'_>, rows: &[u32], fallback: u32) -> u32 {
    rows.iter()
        .rev()
        .find(|&&row| {
            !matches!(
                c.kind(row),
                NodeKind::Send
                    | NodeKind::NoArguments
                    | NodeKind::NoTypeArguments
                    | NodeKind::TypeArguments
                    | NodeKind::DotAccess
            )
        })
        .map_or_else(|| c.utf16_span(fallback).end, |&row| c.utf16_span(row).end)
}

/// The written-type row of a variable: the `Type`/`FunctionType` sibling of
/// the variable row, over the two production shapes. Hand-built trees tile
/// the type inside the declaration window before the variable row; the
/// production parser opens the declaration at the NAME (the local-declaration
/// lane passes the name token as the declaration head), so the type
/// completes first and tiles as the declaration's left sibling in the
/// enclosing window.
fn written_type_sibling(c: &Check<'_>, var: u32, kinds: &[NodeKind]) -> Option<u32> {
    let is_type = |row: u32| kinds.contains(&c.kind(row));
    let owner = c.unit.ast.parent(var)?;
    let members = c.collect_direct(owner);
    if let Some(index) = members.iter().position(|&m| m == var)
        && index > 0
        && is_type(members[index - 1])
    {
        return Some(members[index - 1]);
    }
    let outer = c.unit.ast.parent(owner)?;
    let outer_kids = c.collect_direct(outer);
    let index = outer_kids.iter().position(|&m| m == owner)?;
    let before = *outer_kids.get(index.checked_sub(1)?)?;
    is_type(before).then_some(before)
}

// Per-rule modules (mechanical split 2026-10-19; registry paths unchanged).
pub(crate) mod literal_only_boolean_expressions;
pub(crate) mod avoid_bool_literals_in_conditional_expressions;
pub(crate) mod unnecessary_parenthesis;
pub(crate) mod prefer_int_literals;
pub(crate) mod unnecessary_new;
pub(crate) mod unnecessary_null_in_if_null_operators;
pub(crate) mod prefer_is_not_operator;
pub(crate) mod unnecessary_type_check;
pub(crate) mod prefer_adjacent_string_concatenation;
pub(crate) mod avoid_single_cascade_in_expression_statements;
pub(crate) mod prefer_conditional_assignment;
pub(crate) mod prefer_if_null_operators;
pub(crate) mod prefer_null_aware_method_calls;
pub(crate) mod prefer_null_aware_operators;
pub(crate) mod unnecessary_null_aware_assignments;
pub(crate) mod prefer_contains;
pub(crate) mod unnecessary_const;
pub(crate) mod prefer_is_empty;
pub(crate) mod prefer_is_not_empty;
pub(crate) mod avoid_function_literals_in_foreach_calls;
/// Same-file declaration facts for the rules that stand in for an
/// element-model face (`null_closures` type chains, `unnecessary_this`
/// shadow bail). Everything here is same-file only: a name that resolves to
/// another file stays `Unknown` and lands in the caller's documented
/// fallback family. `prefer_collection_literals` (collections.rs) reads the
/// same scan for its typedef/shadow exemptions.
pub(crate) mod same_file;
pub(crate) mod null_closures;
pub(crate) mod unnecessary_this;
pub(crate) mod prefer_interpolation_to_compose_strings;
pub(crate) mod prefer_final_fields;
pub(crate) mod await_only_futures;
pub(crate) mod unrelated_type_equality_checks;
pub(crate) mod no_literal_bool_comparisons;
pub(crate) mod void_checks;
pub(crate) mod null_check_on_nullable_type_parameter;
pub(crate) mod implicit_call_tearoffs;
pub(crate) mod invalid_runtime_check_with_js_interop_types;
pub(crate) mod avoid_double_and_int_checks;
pub(crate) mod avoid_js_rounded_ints;
pub(crate) mod unnecessary_statements;
pub(crate) mod avoid_null_checks_in_equality_operators;
pub(crate) mod noop_primitive_operations;
pub(crate) mod unnecessary_lambdas;
pub(crate) mod prefer_expression_function_bodies;
#[cfg(test)]
mod tests;
/// The W3 (#992) production-bridge pins: `no_literal_bool_comparisons` over
/// real parser trees.
#[cfg(test)]
mod w3_pins;
#[cfg(test)]
mod w4_pins;
/// The wildcard-name family: a multi-underscore parameter or local whose
/// name the wildcard-variables feature (Dart 3.7) makes redundant.
pub(crate) mod unnecessary_underscores;
pub(crate) mod cascade_invocations;
pub(crate) mod do_not_use_environment;
pub(crate) mod only_throw_errors;
pub(crate) mod unnecessary_await_in_return;
pub(crate) mod use_if_null_to_convert_nulls_to_bools;
pub(crate) mod use_is_even_rather_than_modulo;
/// The static-type-consumer family's shared faces: the piece-run static
/// type over the semantic query row (`Ty`), the exact single-name
/// receiver of a member selector, the unparenthesized root, and the
/// explicit-cast bail the official dynamic rules read. Every consumer
/// keeps the wave posture: the face absent means the rule stays silent.
pub(crate) mod static_type_faces;
pub(crate) mod avoid_dynamic_calls;
pub(crate) mod no_dynamic_casts;
pub(crate) mod avoid_type_to_string;
pub(crate) mod cast_nullable_to_non_nullable;
pub(crate) mod unnecessary_null_checks;
pub(crate) mod unnecessary_unawaited;
/// The GAP-1 static-type-consumer wave's production-bridge pins: each rule
/// runs with the semantic face mounted (the #1049 face construction), so
/// the pins decide the tiling reading, not just the registration.
#[cfg(test)]
mod gap1_pins;
pub(crate) mod discarded_futures;
pub(crate) mod unawaited_futures;
/// The shared walk behind `discarded_futures` and `unawaited_futures` (the
/// upstream `util/unused_futures.dart` visitor): a `Future`/`FutureOr`-typed
/// expression dropped as a statement reports at its selector. `want_async`
/// is the context gate — `discarded_futures` wants sync bodies,
/// `unawaited_futures` async ones.
mod unused_futures;
pub(crate) mod unnecessary_null_aware_operator_on_extension_on_nullable;
pub(crate) mod use_truncating_division;
/// The GAP-1 tranche-2 static-type-consumer wave's production-bridge pins:
/// each rule runs with the semantic face mounted (the #1049 face
/// construction), so the pins decide the tiling reading, not just the
/// registration.
#[cfg(test)]
mod gap1_pins_t2;
pub(crate) mod avoid_print;
pub(crate) mod no_runtimetype_tostring;
pub(crate) mod prefer_const_constructors;
pub(crate) mod use_named_constants;
pub(crate) mod avoid_redundant_argument_values;
pub(crate) mod avoid_slow_async_io;
pub(crate) mod future_sync_value;
pub(crate) mod unnecessary_this_alias;
/// The W2-remainder expression rules' production-bridge pins: each rule
/// runs with the semantic face mounted (the gap1 face construction), so
/// the pins decide the tiling reading, not just the registration.
#[cfg(test)]
mod w2_expr_pins;
/// The #589 W-A B4 wave's production-bridge pins: the four rules turned
/// resolution-bound (`literal_only_boolean_expressions`,
/// `noop_primitive_operations`, `unnecessary_statements`,
/// `prefer_int_literals`) run with the semantic face mounted — the same
/// single-file shape as `gap1_pins`, plus a two-file face for the
/// cross-library closures.
#[cfg(test)]
mod b4_expr_pins;
