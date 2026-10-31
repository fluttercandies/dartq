//! Metadata grammar (official `parseMetadataStar` `parser_impl.dart:1361` and
//! `parseMetadata` :1410).
//!
//! Annotation arguments parse through the full engine
//! (`parse_arguments_rest`, official `parseArgumentsRest`); everything else
//! (shape, events, `@`-loop, type-argument + `(` requirements) mirrors the
//! official productions.

use dartq_scanner::TokenKind;

use crate::listener::{IdentifierContext, ParserListener};
use crate::message::msg;
use crate::parser::Parser;
use crate::token_id::TokenSpan;

impl<L: ParserListener> Parser<'_, '_, '_, L> {
    /// Official `parseMetadataStar`: `@`-prefixed annotations until the next
    /// non-`@` token. Returns the last consumed span.
    pub(crate) fn parse_metadata_star(&mut self) -> TokenSpan {
        if self.peek_kind() != TokenKind::At {
            // Official `parseMetadataStar` always emits the star pair (count
            // 0) unless a `>`-split remnant is next.
            if self.cursor.lexeme(self.peek_id()) != ">" {
                let next = self.peek_span();
                self.listener.begin_metadata_star(next);
                self.listener.end_metadata_star(0);
            }
            return self.peek_span();
        }
        let begin = self.peek_span();
        self.listener.begin_metadata_star(begin);
        let mut count = 0usize;
        let mut last = begin;
        while self.peek_kind() == TokenKind::At {
            last = self.parse_metadata();
            count += 1;
        }
        self.listener.end_metadata_star(count);
        last
    }

    /// Official `parseMetadata`:
    /// `@` qualified (typeArgs (`.` id)?)? arguments?.
    fn parse_metadata(&mut self) -> TokenSpan {
        let at = self.peek_span();
        self.cursor.advance();
        self.listener.begin_metadata(at);
        let _name = self.ensure_identifier(IdentifierContext::MetadataReference);
        // Qualified rest — official `parseQualifiedRestOpt`/`parseQualifiedRest`
        // (parser_impl.dart:2562-2585): the `.` face is unconditional;
        // `ensureIdentifier(period, context)` reports `missing_identifier` on a
        // non-identifier follower and splices the synthetic name without
        // consuming the junk (`@A.;` → one row on the `;`, never a
        // declaration-loop junk cascade).
        while self.peek_kind() == TokenKind::Period {
            self.cursor.advance();
            self.ensure_identifier(IdentifierContext::MetadataContinuation);
        }
        let has_type_arguments = self.peek_kind() == TokenKind::Lt;
        let type_param = self.compute_type_param_or_arg_on_lt(false);
        let group_parsed = !matches!(type_param, crate::type_args::TypeParamOrArg::None);
        let type_args_end = type_param.parse_arguments(self);
        let mut period = None;
        if self.peek_kind() == TokenKind::Period {
            let period_span = self.peek_span();
            self.cursor.advance();
            period = Some(period_span);
            self.ensure_identifier(IdentifierContext::MetadataContinuationAfterTypeArguments);
        }
        let prev_span = self.last_consumed_span();
        if has_type_arguments && self.peek_kind() != TokenKind::OpenParen {
            // Official `reportRecoverableError(token, …)`
            // (parser_impl.dart:1430): `token` is parseArguments' returned
            // group end — the real `>`-family closer for a committed group
            // (`@A<int> x;` anchors on the `>`), or the name itself when the
            // group scan abandoned (`noTypeParamOrArg.parseArguments`
            // returns `token` unchanged, `@A<int;` anchors on the `A`). A
            // recovered group's zero-width synthetic end carries the
            // analyzer translation's zero-length law — length 1 at the
            // synthetic's own offset (translate_error_token.dart:28
            // `atOffset(offset: charOffset, length: 1)`, the same law
            // recovery.rs `report_unmatched_row` encodes) — so `@A<int,;`
            // anchors on the synthetic `>` parked at the `;`.
            let anchor = if !group_parsed {
                prev_span
            } else if type_args_end.range.length == 0 {
                TokenSpan::new(
                    type_args_end.id,
                    dartq_source::SourceRange {
                        offset: type_args_end.range.offset,
                        length: 1,
                    },
                )
            } else {
                type_args_end
            };
            self.report(msg::annotation_with_type_arguments_uninstantiated(), anchor);
        }
        // `parseArgumentsOptMetadata` (parser_impl.dart:9459): the argument
        // list must hug the annotation name; a gap only recovers as arguments
        // when the parse requires them (type arguments) or the follower is a
        // reserved `class`/`enum` head.
        let name_end = prev_span.range.offset + prev_span.range.length;
        let adjacent =
            self.peek_kind() == TokenKind::OpenParen && self.peek_span().range.offset == name_end;
        let end = self.parse_arguments_opt_metadata(has_type_arguments, adjacent);
        self.listener.end_metadata(at, period, end);
        end
    }

    /// Official `parseArgumentsOptMetadata` (:9459): optional argument list;
    /// with type arguments the list is required. A spaced `(` only parses as
    /// the annotation's arguments when required (`has_type_arguments`) or
    /// when a `class`/`enum` declaration follows the group.
    fn parse_arguments_opt_metadata(
        &mut self,
        has_type_arguments: bool,
        adjacent: bool,
    ) -> TokenSpan {
        if self.peek_kind() != TokenKind::OpenParen {
            let next = self.peek_span();
            self.listener.handle_no_arguments(next);
            // Official returns the last consumed token (:9463), not the peeked
            // follower — `parse_arguments_opt`'s peeked-return contract (#585)
            // does not apply to the metadata variant.
            return self.last_consumed_span();
        }
        if !adjacent {
            if has_type_arguments {
                // Required arguments: report + parse anyway (:9473).
                self.report(msg::annotation_space_before_parenthesis(), self.peek_span());
                return self.parse_arguments_rest();
            }
            let open_id = self.peek_id();
            if let Some(close) = self.cursor.table_matching_end(open_id) {
                let after = self.kind_at(self.offset_of(close) + 1);
                if matches!(
                    after,
                    TokenKind::Keyword(
                        dartq_scanner::Keyword::Class | dartq_scanner::Keyword::Enum
                    )
                ) {
                    // Recovery is unambiguous before `class`/`enum` (:9480-9486).
                    self.report(msg::annotation_space_before_parenthesis(), self.peek_span());
                    return self.parse_arguments_rest();
                }
            }
            let next = self.peek_span();
            self.listener.handle_no_arguments(next);
            return self.last_consumed_span();
        }
        self.parse_arguments_rest()
    }
}
