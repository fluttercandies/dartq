//! Fault-tolerant recursive-descent parser for Dart 3.15: token table in,
//! [`ParserListener`] events out.
//!
//! Parse functions mirror the official CFE parser functions one-to-one (the
//! anchor discipline requires line-level traceability), so they legitimately
//! exceed default line/argument budgets; the corresponding lints are waived
//! at crate level.
#![allow(clippy::too_many_lines, clippy::too_many_arguments)]
//!
//! Behavior mirrors the official CFE parser
//! (`.third/dart-sdk/pkg/_fe_analyzer_shared/lib/src/parser/`, Dart 3.15
//! main). Authoritative design facts: `docs/facts/parser-core-spec.md`
//! (#27) and `docs/facts/parser-grammar-map.md` (#32). Semantic questions
//! are settled against the reference sources, never from memory.
//!
//! # Architecture
//!
//! * **Cursor, not rewriter** — [`TokenTable`] is immutable; the parser never
//!   rewrites the stream (core-spec §5.3). `>>`/`>=`/`>>>`-family closers are
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
//!   [`SourceRange`]).
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
//!   in [`Parser::new`] from a [`FeatureSet`] ([`ParserFeatures`]), matching
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
//! expression through the reduced constant-expression engine, and `;`
//! bodies emit the official empty-body event. Constructor initializers and
//! field initializers still diet-skip their expression internals. Directives
//! are consumed diet-style (group-aware skip to `;`) until the directive
//! milestone. Until #20 lands, statement/initializer expression operands
//! parse through the reduced constant-expression engine (see `expr_const`).
//!
//! # Scanner interplay
//!
//! The scanner never splits `>>` (official `abstract_scanner.dart:1479`);
//! splitting is a parser duty implemented in [`TokenCursor::consume_closer`].
//! Scanner recovery already inserts synthetic closers for unclosed groups
//! (`TokenTable::matching_end` resolves to them, flagged
//! `Token::is_synthetic`); `ensure_close_paren` adopts those without
//! re-reporting (official `moveSynthetic`). Interpolated strings are a linear
//! token flow (`String` segments plus `${`/`}` begin-group pairs), so string
//! literals need no special parser state.

use dartiq_scanner::TokenTable;

mod cursor;
mod declarations;
mod expr_const;
mod features;
mod formal_params;
mod listener;
mod literals;
mod members;
mod message;
mod metadata;
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
    ConstantPatternContext, ConstructorReferenceContext, DeclarationHeaderKind, DeclarationKind,
    ExperimentalFlag, FormalParameterKind, IdentifierContext, MemberKind, NullListener,
    ParserListener, PatternContext,
};
pub use message::Message;
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
    Parser::new(source, tokens, features, NullListener).parse_unit();
}
