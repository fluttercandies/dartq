//! Metadata grammar (official `parseMetadataStar` `parser_impl.dart:1361` and
//! `parseMetadata` :1410).
//!
//! Annotation arguments parse through the full engine
//! (`parse_arguments_rest`, official `parseArgumentsRest`); everything else
//! (shape, events, `@`-loop, type-argument + `(` requirements) mirrors the
//! official productions.

use dartiq_scanner::TokenKind;

use crate::listener::{IdentifierContext, ParserListener};
use crate::message::msg;
use crate::parser::Parser;
use crate::token_id::TokenSpan;

impl<L: ParserListener> Parser<'_, '_, L> {
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
        let name = self.ensure_identifier(IdentifierContext::MetadataReference);
        // Qualified rest.
        while self.peek_kind() == TokenKind::Period && self.peek_kind_ahead_identifier(1) {
            self.cursor.advance();
            self.ensure_identifier(IdentifierContext::MetadataContinuation);
        }
        let has_type_arguments = self.peek_kind() == TokenKind::Lt;
        let type_param = self.compute_type_param_or_arg_on_lt(false);
        type_param.parse_arguments(self);
        let mut period = None;
        if self.peek_kind() == TokenKind::Period {
            let period_span = self.peek_span();
            self.cursor.advance();
            period = Some(period_span);
            self.ensure_identifier(IdentifierContext::MetadataContinuationAfterTypeArguments);
        }
        if has_type_arguments && self.peek_kind() != TokenKind::OpenParen {
            self.report(msg::annotation_with_type_arguments_uninstantiated(), name);
        }
        let end = self.parse_arguments_opt_metadata(has_type_arguments);
        self.listener.end_metadata(at, period, end);
        end
    }

    fn peek_kind_ahead_identifier(&self, n: usize) -> bool {
        crate::recovery::is_identifier(self.cursor.peek_kind_ahead(n))
    }

    /// Official `parseArgumentsOptMetadata` (:9460): optional argument list;
    /// with type arguments the list is required.
    fn parse_arguments_opt_metadata(&mut self, has_type_arguments: bool) -> TokenSpan {
        if self.peek_kind() != TokenKind::OpenParen {
            if has_type_arguments {
                // Reported above via `annotation_with_type_arguments_uninstantiated`.
            }
            let next = self.peek_span();
            self.listener.handle_no_arguments(next);
            return next;
        }
        self.parse_arguments_rest()
    }
}
