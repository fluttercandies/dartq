//! The collection literal's write face: a list, set, or map literal on
//! an assignment's right side (`chunk ??= []`, `names = <String>{}`)
//! is non-null, and the oracle infers its arguments from the write
//! target's declared type (downward inference on the assignment's
//! context), so the write carries the target's non-null declared type —
//! which promotes a nullable local exactly as a creation would. The
//! name-level row model keeps the literal opaque (a value row with no
//! type), so the staging reads the spelling instead.

use super::*;

impl Engine<'_> {
    /// Whether a collection literal (`[`, `{`, or a `<T>`-prefixed one,
    /// optionally under `const`) starts at or after `at`, past whitespace.
    pub(crate) fn collection_literal_starts_at(&self, at: u32) -> bool {
        let mut at = self.skip_whitespace(at);
        if self
            .source
            .get(at as usize..)
            .is_some_and(|rest| rest.starts_with("const"))
        {
            at = self.skip_whitespace(at + 5);
        }
        matches!(self.byte_at(at), Some(b'[' | b'{' | b'<'))
    }
}
