//! The collection literal's write face: a list, set, or map literal on
//! an assignment's right side (`chunk ??= []`, `names = <String>{}`)
//! is non-null, and the oracle infers its arguments from the write
//! target's declared type (downward inference on the assignment's
//! context), so the write carries the target's non-null declared type —
//! which promotes a nullable local exactly as a creation would. The
//! name-level row model keeps the literal opaque (a value row with no
//! type), so the staging reads the spelling instead.
//!
//! The spelling test covers only the shapes whose inferred type *is* the
//! target's declared type. A literal under a context that is not its own
//! head (`C<int> c = <int>{}` with `C` a user class, `SplayTreeSet s = {}`,
//! `T t = {}`) infers `Set<...>`/`List<...>`/`Map<...>` upstream and the
//! assignment is then decided against that type — the oracle reports an
//! `invalid_assignment` this predicate suppresses by answering "the target
//! type". Narrowing it needs the literal's own element inference, which
//! lives outside this helper.

use super::*;

impl Engine<'_> {
    /// Whether a collection literal (`[`, `{`, or a `<T>`-prefixed one,
    /// optionally under `const`) starts at or after `at`, past trivia.
    ///
    /// The probe rides the token table: the scanner already dropped
    /// whitespace and comments, so a head separated from the operator by a
    /// comment (`xs = /* note */ [1]`) is still seen, and the anchors stay
    /// UTF-16 offsets (slicing the source bytes at `at` drifts by the
    /// non-ASCII byte surplus of everything written before it).
    pub(crate) fn collection_literal_starts_at(&self, at: u32) -> bool {
        let tokens = self.tokens.as_slice();
        let mut index = tokens.partition_point(|token| token.utf16_offset < at);
        if tokens.get(index).is_some_and(|token| {
            token.kind == dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Const)
        }) {
            index += 1;
        }
        matches!(
            tokens.get(index).map(|token| token.kind),
            Some(
                dartq_scanner::TokenKind::OpenSquare
                    | dartq_scanner::TokenKind::OpenCurly
                    | dartq_scanner::TokenKind::Lt
            )
        )
    }
}
