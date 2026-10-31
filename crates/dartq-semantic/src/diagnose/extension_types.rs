//! The extension-type declaration family's semantic home (#2807).
//!
//! The family's two faces split by trigger site, mirroring the pinned SDK
//! (upstream Dart SDK source, pkg/analyzer):
//!
//! * the *syntactic* representation shapes — `expected_representation_field`
//!   (missing formals, `this.x`/`super.x` initializing formals, delimiters,
//!   function-typed suffixes, the no-constructor-head synthesized list) and
//!   `missing_primary_constructor` — are the PARSER's: their anchors are
//!   parse-time token facts (the `this`/`super` keyword, the formal's begin
//!   token, `insertParens`'s zero-length synthetic `()` at the token after
//!   the type name), reproduced in `dartq-parser`'s
//!   `parse_primary_constructor_opt` with the verifier's decision order
//!   (`error_verifier.dart:5353-5460`). A semantic re-emission would double
//!   every parse row in the merged report, so this layer never reports
//!   them.
//! * the *name-level* checks — representation-depends-on-itself,
//!   `member_with_class_name`, `extension_type_declares_member_of_object`,
//!   the disabled-arm `expected_representation_type` — read the surviving
//!   single representation formal from the collected element model and live
//!   here.
//!
//! The split is the established house posture (recorded here from
//! overrides.rs in #2807): the collector does not carry the syntactic
//! shapes' token facts, and adding them would duplicate what the parser
//! already holds. The one token fact this layer reads for itself is the
//! written-vs-synthesized representation type of the disabled-arm
//! `expected_representation_type` row (#3198): the collector materializes
//! `Object?` for the untyped spelling, so the element model cannot tell
//! the two apart, and a model column would have to ride the persisted
//! payload to survive a warm start — while this pass always holds the
//! tokens the payload was captured from.

use super::*;
use dartq_scanner::{Keyword, TokenKind};

impl Engine<'_> {
    /// The semantic representation checks for one extension type, mounted at
    /// the declaration's name event (the element model is complete —
    /// `collect` parsed before this pass — and the name token anchors the
    /// type-level emissions; the official dispatch order is
    /// depends-on-itself first, the name checks last,
    /// `error_verifier.dart:1184-1193`).
    ///
    /// The syntactic representation shapes (missing formals, `this.x`/`super.x`,
    /// delimiters, function-typed suffixes, modifiers, multiple formals) are
    /// the parser's (`expected_representation_field` /
    /// `multiple_representation_fields` / `representation_field_modifier`);
    /// this side reads the surviving single representation formal.
    pub(crate) fn check_extension_type(&mut self, container: StringId, name_range: SourceRange) {
        // One emission per container per pass: an augmentation declaration
        // re-fires the event for the same container.
        if !self.reported_extension_types.insert(container) {
            return;
        }
        let Some(elements) = self.index.get(self.file) else {
            return;
        };
        let Some(decl) = elements.container(container) else {
            return;
        };
        if decl.kind != ContainerKind::ExtensionType {
            return;
        }
        let Some(rep) = decl.primary_formals.first() else {
            return;
        };
        // The representation can't depend on itself
        // (`error_verifier.dart:5343-5354`, anchored at the type name). M1
        // name-level: a direct `E` reference; a generic `List<E>`-shaped
        // dependency stays an under-report.
        if rep.formal.ty == Ty::named(container) {
            self.emit(
                SemanticCode::ExtensionTypeRepresentationDependsOnItself,
                name_range,
                "",
                "",
            );
        }
        // `_checkForExtensionTypeRepresentationErrorCodes`'s name checks
        // (:5411-5423, both non-returning): the field may not share the
        // type's name, nor an `Object` member's.
        if rep.formal.name == container {
            self.emit(SemanticCode::MemberWithClassName, rep.name_range, "", "");
        } else if self.members.object_members.contains(&rep.formal.name) {
            self.emit(
                SemanticCode::ExtensionTypeDeclaresMemberOfObject,
                rep.name_range,
                "",
                "",
            );
        }
        // The disabled-arm representation-type requirement
        // (:5449-5452): a representation formal with NO WRITTEN type —
        // alongside, not instead of, the modifier check the parser owns.
        if !self.primary_constructors_enabled
            && !self.has_written_representation_type(rep.name_range)
        {
            self.emit(
                SemanticCode::ExpectedRepresentationType,
                rep.name_range,
                "",
                "",
            );
        }
        // `_checkForRepresentationField`'s trailing-comma face
        // (`error_verifier.dart:5009-5015`, anchored at
        // `first.endToken.next`): the same pre-declaring-formal arm. From
        // the primary-constructors feature on, the representation formal is
        // a declaring formal and its list takes a trailing comma like any
        // other (`class A(int x,)`); probe 3.13.7: silent at the current
        // version, `REPRESENTATION_FIELD_TRAILING_COMMA` at `@dart=3.10`.
        if !self.primary_constructors_enabled
            && let Some(comma) = self.representation_trailing_comma(rep.name_range)
        {
            self.emit(
                SemanticCode::RepresentationFieldTrailingComma,
                comma,
                "",
                "",
            );
        }
        // `_verifyExtensionElementImplements`'s representation face
        // (`extension_type_implements_not_supertype`) decides at the
        // file-tail bounds pass ([`Self::finalize_ext_faces`]): the faces'
        // type arguments name declarations the parse has not reached yet.
    }

    /// Whether the representation formal named at `name_range` carries a
    /// WRITTEN type annotation (`extension type E(int x)` over `extension
    /// type E(x)`).
    ///
    /// The oracle decides on the syntax — `inner.type == null`
    /// (`error_verifier.dart:5449-5452`) — never on a semantic type value,
    /// and the collector materializes `Object?` for the untyped spelling,
    /// so the element model cannot answer it. The token table answers it
    /// at the emit site instead of the capture carrying a flag: a model
    /// column would need the persisted-payload lane (and its version bump)
    /// to survive a warm start, while this pass always holds the exact
    /// tokens the payload was captured from.
    ///
    /// Only metadata and the formal's own modifier keywords can precede
    /// the name ahead of a type once the parser has vetted the list, so
    /// "no written type" is "the name is the first token after them".
    /// The shapes whose formals the parser rejects outright (`this.x`,
    /// `super.x`, `[x]`, a function-typed formal) count as written — their
    /// rows are the parser's and never double here.
    fn has_written_representation_type(&self, name_range: SourceRange) -> bool {
        let toks = self.tokens.as_slice();
        let name_at = toks.partition_point(|t| t.utf16_offset < name_range.offset);
        let Some(open) = self.formal_list_open(name_at) else {
            // Recovery left no recognizable opener: stay silent.
            return true;
        };
        let mut at = open + 1;
        while at < name_at {
            match toks[at].kind {
                TokenKind::At => match self.metadata_end(at + 1) {
                    Some(next) => at = next,
                    None => return true,
                },
                TokenKind::Keyword(Keyword::Covariant | Keyword::Final | Keyword::Var) => at += 1,
                _ => return true,
            }
        }
        false
    }

    /// The token index of the `(` opening the formal list that holds the
    /// token at `end` — a backward walk over balanced groups, so a
    /// metadata argument list or a function-typed parameter's parentheses
    /// are stepped over rather than read as the opener.
    fn formal_list_open(&self, end: usize) -> Option<usize> {
        let toks = self.tokens.as_slice();
        let mut depth = 0u32;
        let mut at = end;
        while at > 0 {
            at -= 1;
            match toks[at].kind {
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                    depth += 1;
                }
                TokenKind::OpenParen if depth == 0 => return Some(at),
                TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                    depth = depth.saturating_sub(1);
                }
                _ => {}
            }
        }
        None
    }

    /// The token index just past the metadata entry whose name starts at
    /// `at` (the token after its `@`): the qualified name, its type
    /// arguments, and its argument list when spelled. `None` when the
    /// entry's extent can't be established (recovery residue) — the
    /// caller then stays silent.
    fn metadata_end(&self, at: usize) -> Option<usize> {
        let toks = self.tokens.as_slice();
        let mut at = at + 1;
        while toks.get(at).is_some_and(|t| t.kind == TokenKind::Period) {
            at += 2;
        }
        for kind in [TokenKind::Lt, TokenKind::OpenParen] {
            if toks.get(at).is_some_and(|t| t.kind == kind) {
                at = self.tokens.matching_end(at)? + 1;
            }
        }
        Some(at)
    }

    /// The representation formal list's trailing comma, when it has one.
    /// The formal runs from its name to the list's `)` — a default value
    /// may sit between them, so the scan tracks bracket depth and steps
    /// over string literals and comments. `None` for a list that closes
    /// right after the formal.
    fn representation_trailing_comma(&self, name: SourceRange) -> Option<SourceRange> {
        let bytes = self.source.as_bytes();
        let mut at = self.byte_of(name.end());
        let mut depth = 0u32;
        while at < bytes.len() {
            match bytes[at] {
                b'/' => match bytes.get(at + 1) {
                    Some(b'/') => {
                        while at < bytes.len() && bytes[at] != b'\n' {
                            at += 1;
                        }
                    }
                    Some(b'*') => {
                        at += 2 + self.source.get(at + 2..)?.find("*/")? + 2;
                    }
                    _ => at += 1,
                },
                quote @ (b'\'' | b'"') => at = self.string_end(at, quote)?,
                b'(' | b'[' | b'{' => {
                    depth += 1;
                    at += 1;
                }
                b')' | b']' | b'}' => {
                    if depth == 0 {
                        return None;
                    }
                    depth -= 1;
                    at += 1;
                }
                b',' if depth == 0 => {
                    return Some(SourceRange {
                        offset: self.utf16_of(at),
                        length: 1,
                    });
                }
                _ => at += 1,
            }
        }
        None
    }

    /// `at` (a quote byte) advanced past the string literal it opens —
    /// backslash escapes and the triple-quoted forms included.
    fn string_end(&self, at: usize, quote: u8) -> Option<usize> {
        let bytes = self.source.as_bytes();
        let triple = bytes.get(at + 1) == Some(&quote) && bytes.get(at + 2) == Some(&quote);
        let mut at = at + if triple { 3 } else { 1 };
        while at < bytes.len() {
            match bytes[at] {
                b'\\' => at += 2,
                b if b == quote => {
                    if triple {
                        if bytes.get(at + 1) == Some(&quote) && bytes.get(at + 2) == Some(&quote) {
                            return Some(at + 3);
                        }
                        at += 1;
                    } else {
                        return Some(at + 1);
                    }
                }
                b'\n' if !triple => return None,
                _ => at += 1,
            }
        }
        None
    }
}

// The #3195 pins — the representation-field verifier faces this module
// owns (`expected_representation_type`, the trailing-comma row) at the
// language version that still requires a written representation type.
#[cfg(test)]
#[path = "ticket_3195_tests.rs"]
mod ticket_3195_tests;

// The #3198 pins — the same disabled arm's written-vs-synthesized type
// distinction (the collector materializes `Object?` for the untyped
// spelling, so only the source tokens answer it).
#[cfg(test)]
#[path = "ticket_3198_tests.rs"]
mod ticket_3198_tests;
