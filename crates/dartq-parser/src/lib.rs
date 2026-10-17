//! Fault-tolerant recursive-descent parser for Dart 3.15: token table in,
//! [`ParserListener`] events out.
//!
//! Parse functions mirror the official CFE parser functions one-to-one (the
//! anchor discipline requires line-level traceability), so they legitimately
//! exceed default line/argument budgets; the corresponding lints are waived
//! at crate level.
#![allow(clippy::too_many_lines, clippy::too_many_arguments)]
//!
//! Behavior mirrors the official CFE parser (upstream Dart SDK source,
//! `pkg/_fe_analyzer_shared/lib/src/parser/`, Dart 3.15 main); semantic
//! questions are settled against those reference sources, never from
//! memory. The authoritative design contracts are the architecture
//! invariants documented below.
//!
//! # Architecture
//!
//! * **Cursor, not rewriter** — [`TokenTable`] is immutable; the parser never
//!   rewrites the stream. `>>`/`>=`/`>>>`-family closers are
//!   split at consume time into parser-inserted tokens, and recovery inserts
//!   synthetic tokens into a parser-local side table (`TokenId`s past
//!   `TokenTable::len()`). Speculation (rolling the cursor back) is therefore
//!   free.
//! * **Events, not return values** — the parser collects no diagnostics. All
//!   errors flow through [`ParserListener::handle_recoverable_error`]; this
//!   keeps speculation free of phantom errors (a returned diagnostic cannot
//!   be un-returned, but an un-emitted event is trivially absent). Event
//!   names are the official listener names mechanically transliterated to
//!   `snake_case`; token positions carry [`TokenSpan`] (token id + UTF-16
//!   [`SourceRange`](dartq_source::SourceRange)).
//! * **Termination on any input** — three structural guarantees keep the
//!   parser finite (and memory input-bounded) for arbitrary bytes: every
//!   recovery loop consumes at least one real token per iteration or bails
//!   (no-progress guards force the consume); structural recursion is capped
//!   at the official `recoverFromStackOverflow` depth; and a per-file
//!   diagnostics cap latches a terminal bailout (one bounded diagnostic, no
//!   further consumption) before a runaway recovery loop can flood memory
//!   with reports, events, and synthetic tokens.
//! * **Zero cost without a listener** — the parser is generic over `L:
//!   ParserListener`; [`NullListener`] (or any all-default impl) monomorphizes
//!   to dead code. The parser itself allocates nothing per event.
//! * **Feature folding at construction** — grammar variants are decided once
//!   in [`Parser::new`] from a [`FeatureSet`](dartq_language::FeatureSet) ([`ParserFeatures`]), matching
//!   `parser_impl.dart:316-373`.
//!
//! # Implemented grammar surface (this milestone)
//!
//! Type annotations (aligned with `type_info.dart`: the full `computeType`
//! shape analysis, type arguments/variables with `>>` splitting, record
//! types, generalized function types), metadata, full Dart 3 patterns (the
//! `parsePattern` family), and the recovery skeleton (synthetic insertion,
//! move-synthetic, skip-to-sync, no-progress guard).
//!
//! Declarations (#18) cover the full top-level surface: classes, named
//! mixin applications, mixins, enums, extensions, extension types, typedefs
//! (both styles), top-level members, and class members with **full function
//! bodies** (#19): `{ ... }` bodies parse the statement grammar
//! (`statements.rs`, official `parseFunctionBody`), `=>` bodies parse their
//! expression through the full expression engine, and `;`
//! bodies emit the official empty-body event. Directives (#300) parse the
//! full family (`directives.rs`): library/library-augmentation, import
//! (conditional URIs, `deferred`/`as` prefix, show/hide combinators,
//! out-of-order-clause recovery), export, part, and part-of, with the
//! official `DirectiveContext` ordering checks.
//! Statement, initializer, and default-value expression operands parse
//! through the full expression engine (`expressions.rs`, official
//! `parseExpression` family, #20); patterns keep the reduced
//! constant-expression engine (see `expr_const`).
//!
//! # Scanner interplay
//!
//! The scanner never splits `>>` (official `abstract_scanner.dart:1479`);
//! splitting is a parser duty implemented in `TokenCursor::consume_closer`.
//! Scanner recovery inserts synthetic closers for unclosed groups and
//! records the unmatched pairs on the table (`TokenTable::matching_end`
//! resolves to them, flagged `Token::is_synthetic`; the pairs are
//! `TokenTable::unmatched_groups`) — `ensure_close_paren` adopts those with
//! the row at its relocation point and the parse-end pass reports the rest
//! (official `moveSynthetic` + the post-parse `UnmatchedToken` translation).
//! Interpolated strings are a linear
//! token flow (`String` segments plus `${`/`}` begin-group pairs), so string
//! literals need no special parser state.

use dartq_scanner::{Token, TokenTable};

mod cursor;
mod declarations;
mod directives;
mod expr_const;
mod expressions;
mod features;
mod formal_params;
mod listener;
mod literals;
mod members;
mod message;
mod metadata;
mod mux;
mod parser;
mod patterns;
mod recovery;
mod statements;
mod token_id;
mod type_args;
mod type_parse;
mod types;

#[cfg(test)]
mod tests;

pub use cursor::TokenCursor;
pub use features::ParserFeatures;
pub use listener::{
    AssertKind, BlockKind, ConstantPatternContext, ConstructorReferenceContext,
    DeclarationHeaderKind, DeclarationKind, ExperimentalFlag, FormalParameterKind,
    IdentifierContext, MemberKind, NullListener, ParserListener, PatternContext,
};
pub use message::{Message, experiment_not_enabled_message};
pub use mux::MultiplexListener;
pub use parser::Parser;
pub use token_id::{TokenId, TokenSpan};
pub use types::{TypeInfo, TypeParamOrArg};

/// Parses one compilation unit through the recovery skeleton.
///
/// The skeleton emits `begin/end_compilation_unit` and consumes the whole
/// stream under the no-progress guard. Declaration dispatch arrives with
/// #18; until then nothing inside the unit is analyzed, so by design the
/// parser produces no diagnostics here.
pub fn parse_unit(source: &str, tokens: &TokenTable, features: ParserFeatures) {
    let mut null = NullListener;
    parse_unit_with(source, tokens, features, &mut null);
}

/// Parses one compilation unit through `listener` and returns the
/// parser-inserted tokens as scanner [`Token`]s, in insertion-id order (the
/// id order of the parser's side table, `table.len() + i`).
///
/// The listener is borrowed: the caller keeps ownership and reads the
/// collected events back after this call returns.
///
/// Closer splits (`>>` → two `>`) and recovery synthetics live only in the
/// parser's private side table: until they are folded in, their ids resolve
/// nowhere and AST span derivation panics. The caller folds them with
/// [`TokenTable::append_inserted`] — once, at parse end, before any AST span
/// read (the unified-table sequencing contract).
pub fn parse_unit_with<L: ParserListener>(
    source: &str,
    tokens: &TokenTable,
    features: ParserFeatures,
    listener: &mut L,
) -> Vec<Token> {
    let mut parser = Parser::new(source, tokens, features, listener);
    parser.parse_unit();
    let inserted = parser.cursor.take_inserted();
    cursor::materialize_inserted(source, &inserted)
}
