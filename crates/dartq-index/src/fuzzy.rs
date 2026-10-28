//! The fuzzy discovery primitive behind the symbol index's discovery face:
//! a case-insensitive subsequence scan over the indexed names with a
//! four-class preference ranking, in the shape of the official editors'
//! symbol search (the upstream rust-analyzer ide-db symbol index serves
//! the same semantics — subsequence matching with case-insensitive
//! default — through an automaton; this crate scans the names directly).
//!
//! # Ranking spec (the implemented contract)
//!
//! A name is a candidate when the query's characters appear in it in
//! order, case-insensitively — contiguous containment (the substring
//! special case) and every camelCase spelling included. Each candidate
//! lands in the best class it qualifies for, best first:
//!
//! 1. [`MatchClass::ExactPrefix`] — the name starts with the query.
//! 2. [`MatchClass::WordBoundary`] — the query aligns to a strictly
//!    increasing run of word-start positions: index 0, the byte after a
//!    `_`/`$`, or an ASCII uppercase whose predecessor is not an ASCII
//!    uppercase (the lowerCamelCase/UpperCamelCase hump — Dart's naming
//!    convention, which the style lints keep to `HttpServer` spellings,
//!    so no acronym-internal boundary rule).
//! 3. [`MatchClass::Substring`] — the query occurs contiguously, not as a
//!    prefix.
//! 4. [`MatchClass::Subsequence`] — the greedy leftmost alignment
//!    completes, none of the above.
//!
//! Ties break by name length (shorter first), then by name bytes; rows of
//! one matched name keep their file-then-offset order. The ranking is a
//! total order, so the directory scan's map iteration order never reaches
//! the answer, and the capped top slice is deterministic.
//!
//! # Cost posture
//!
//! The candidate scan is one linear pass per distinct indexed name — no
//! automaton, no auxiliary structure. At the index's scale (distinct
//! declared names, 10^4-10^5 at the 100k-LOC corpus) the scan is
//! sub-millisecond, and discovery is a cold query face, not the
//! incremental hot path; an automaton would pay for itself only orders of
//! magnitude beyond that. Names are byte-scanned: Dart identifiers are
//! ASCII-only (the official scanner's law), so `to_ascii_lowercase`
//! folding is exact and a non-ASCII query character can never match.

use dartq_source::{StringId, StringTable};

/// The discovery answer's cap: the top slice a search serves, applied
/// after the full rank sort (never a scan-time early exit, so the answer
/// is the same top-32 whatever the directory's iteration order).
pub const RESULT_CAP: usize = 32;

/// The preference class of a fuzzy match, best first — the derived
/// [`Ord`] is the ranking order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchClass {
    /// The name starts with the query.
    ExactPrefix,
    /// The query aligns to word-start positions only (the camelCase
    /// humps / snake-case segments).
    WordBoundary,
    /// The query occurs contiguously, not as a prefix.
    Substring,
    /// The query's characters appear in order, none of the above.
    Subsequence,
}

/// One classified candidate: a name the query fuzzy-matches and the best
/// class it qualified for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// The best class the name qualified for.
    pub class: MatchClass,
    /// The matched name.
    pub name: StringId,
}

/// Folds a query once for the scan loop — ASCII lowercase, byte-wise
/// (module docs: identifiers are ASCII-only, so the fold is exact and a
/// non-ASCII query byte can never match).
#[must_use]
pub fn fold_query(query: &str) -> Vec<u8> {
    query.bytes().map(|byte| byte.to_ascii_lowercase()).collect()
}

/// Classifies `name` against the folded `query`: [`None`] when the name
/// is not a candidate (the query's characters do not appear in order), or
/// the best class it qualified for (module docs for the spec). An empty
/// query names no pattern and matches nothing.
#[must_use]
pub fn classify(query: &[u8], name: &str) -> Option<MatchClass> {
    if query.is_empty() || name.len() < query.len() {
        return None;
    }
    let bytes = name.as_bytes();
    if query.iter().zip(bytes).all(|(q, n)| eq_fold(*q, *n)) {
        return Some(MatchClass::ExactPrefix);
    }
    if align(query, bytes, true) {
        return Some(MatchClass::WordBoundary);
    }
    if bytes
        .windows(query.len())
        .any(|window| window.iter().zip(query).all(|(n, q)| eq_fold(*n, *q)))
    {
        return Some(MatchClass::Substring);
    }
    if align(query, bytes, false) {
        return Some(MatchClass::Subsequence);
    }
    None
}

/// Ranks the classified candidates best-first (class, then name length,
/// then name bytes — the module docs' tiebreaks) in place and caps at
/// [`RESULT_CAP`]. The total order absorbs whatever order the scan fed
/// the candidates in; duplicate names are the caller's to dedupe (the
/// directory scans yield each name once, tier merges excepted).
pub fn rank(candidates: &mut Vec<Candidate>, strings: &StringTable) {
    candidates.sort_unstable_by(|a, b| {
        let (a_name, b_name) = (strings.resolve(a.name), strings.resolve(b.name));
        a.class
            .cmp(&b.class)
            .then_with(|| a_name.len().cmp(&b_name.len()))
            .then_with(|| a_name.cmp(b_name))
    });
    candidates.truncate(RESULT_CAP);
}

/// ASCII case-insensitive byte equality.
fn eq_fold(a: u8, b: u8) -> bool {
    a.to_ascii_lowercase() == b.to_ascii_lowercase()
}

/// Whether `at` is a word-start position of `name` (module docs: index 0,
/// after a `_`/`$`, or a hump — an ASCII uppercase whose predecessor is
/// not one).
fn is_word_start(name: &[u8], at: usize) -> bool {
    if at == 0 {
        return true;
    }
    let prev = name[at - 1];
    if prev == b'_' || prev == b'$' {
        return true;
    }
    name[at].is_ascii_uppercase() && !prev.is_ascii_uppercase()
}

/// The greedy leftmost alignment of `query` over `name`, each matched
/// position required to be a word start when `at_starts` holds. Leftmost
/// greedy is complete for existence (an earlier-or-equal choice only
/// widens the remaining suffix, so any valid alignment implies the greedy
/// one), and each step advances the scan cursor, so the walk terminates.
fn align(query: &[u8], name: &[u8], at_starts: bool) -> bool {
    let mut from = 0;
    for &q in query {
        let Some(at) = (from..name.len())
            .find(|&i| eq_fold(name[i], q) && (!at_starts || is_word_start(name, i)))
        else {
            return false;
        };
        from = at + 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four preference classes rank in spec order for one query: the
    /// exact prefix, the camelCase word-boundary match, the contiguous
    /// substring, and the sparse subsequence — none of the candidates
    /// qualifies for a better class than its asserted one.
    #[test]
    fn ranking_prefers_prefix_then_word_boundary_then_substring_then_subsequence() {
        let query = fold_query("fb");
        let matched = |name: &str| classify(&query, name).expect(name);
        assert_eq!(matched("fb"), MatchClass::ExactPrefix);
        assert_eq!(matched("FooBar"), MatchClass::WordBoundary);
        assert_eq!(matched("xfbx"), MatchClass::Substring);
        assert_eq!(matched("foob"), MatchClass::Subsequence);
        assert!(matched("fb") < matched("FooBar"));
        assert!(matched("FooBar") < matched("xfbx"));
        assert!(matched("xfbx") < matched("foob"));
    }

    /// Every class matches case-insensitively in both directions, and a
    /// name that lacks a query character stays out.
    #[test]
    fn case_insensitive_matching_across_classes() {
        let upper = fold_query("FB");
        assert_eq!(classify(&upper, "fb"), Some(MatchClass::ExactPrefix));
        assert_eq!(classify(&upper, "FooBar"), Some(MatchClass::WordBoundary));
        assert_eq!(classify(&upper, "XFBX"), Some(MatchClass::Substring));
        let mixed = fold_query("fB");
        assert_eq!(classify(&mixed, "FooBar"), Some(MatchClass::WordBoundary));
        assert_eq!(classify(&mixed, "FB"), Some(MatchClass::ExactPrefix));
        // No `z` anywhere: not a candidate under any class.
        assert_eq!(classify(&fold_query("fz"), "FooBar"), None);
        assert_eq!(classify(&fold_query("fz"), "FZ"), Some(MatchClass::ExactPrefix));
    }

    /// Within one class, shorter names win, then byte order — the total
    /// order that makes the capped answer deterministic.
    #[test]
    fn ties_break_by_length_then_lexicographic() {
        let strings = StringTable::new();
        let names = ["xbcq", "xbc", "xbcd", "zbc"];
        let query = fold_query("bc");
        let mut candidates: Vec<Candidate> = names
            .iter()
            .map(|&name| Candidate {
                class: classify(&query, name).expect(name),
                name: strings.intern(name),
            })
            .collect();
        rank(&mut candidates, &strings);
        let spelled: Vec<&str> = candidates
            .iter()
            .map(|candidate| strings.resolve(candidate.name))
            .collect();
        // Length first: the two len-3 names, then the len-4 pair in byte
        // order.
        assert_eq!(spelled, ["xbc", "zbc", "xbcd", "xbcq"]);
    }

    /// An empty query names no pattern, a name shorter than the query can
    /// never contain it, and the cap truncates the fully ranked order —
    /// whatever order the candidates were fed in.
    #[test]
    fn empty_query_short_names_and_cap() {
        assert_eq!(classify(&fold_query(""), "anything"), None);
        assert_eq!(classify(&fold_query("abcdef"), "abc"), None);
        let strings = StringTable::new();
        let query = fold_query("x");
        // `xNNN` names rank lexically ascending, so the expected top slice
        // is the lowest-numbered spellings; the feed is reversed.
        let names: Vec<String> = (0..RESULT_CAP + 8).map(|i| format!("x{i:03}")).collect();
        let mut candidates: Vec<Candidate> = names
            .iter()
            .rev()
            .map(|name| Candidate {
                class: classify(&query, name).expect("prefix match"),
                name: strings.intern(name),
            })
            .collect();
        rank(&mut candidates, &strings);
        assert_eq!(candidates.len(), RESULT_CAP, "capped");
        assert_eq!(candidates[0].class, MatchClass::ExactPrefix);
        let spelled: Vec<&str> = candidates
            .iter()
            .map(|candidate| strings.resolve(candidate.name))
            .collect();
        let expected: Vec<String> = (0..RESULT_CAP).map(|i| format!("x{i:03}")).collect();
        assert_eq!(spelled, expected, "the cap keeps the ranked top slice");
    }

    /// The word-start rule covers the snake-case segments and the hump of
    /// a `Upper`-then-lower container spelling; a mid-word position is no
    /// boundary and keeps its candidate in the substring or subsequence
    /// class instead.
    #[test]
    fn snake_case_and_humps_are_word_starts() {
        let query = fold_query("fb");
        assert_eq!(classify(&query, "foo_bar"), Some(MatchClass::WordBoundary));
        assert_eq!(classify(&query, "FooBaz"), Some(MatchClass::WordBoundary));
        // `HttpServer`: the `S` hump after a lowercase is a word start.
        assert_eq!(classify(&fold_query("hs"), "HttpServer"), Some(MatchClass::WordBoundary));
        // `f` then `b` around a mid-word hop: neither match position
        // after the first is a word start, and the pair is not contiguous
        // — the sparse class.
        assert_eq!(classify(&query, "fxbo"), Some(MatchClass::Subsequence));
    }

    /// The discovery fixture's source: the four rank classes for the
    /// query `fb`, plus enough substring candidates to overflow the cap.
    fn discovery_source() -> String {
        let mut source = String::from(
            "class FooBar {}\nvoid foo_bar() {}\nvoid foob() {}\nvoid fb() {}\nvoid xfbx() {}\n",
        );
        for i in 0..35 {
            source.push_str(&format!("void xfbx{i:02}() {{}}\n"));
        }
        source
    }

    /// The index face: best-first classes, the lexicographic ties inside
    /// one class, and the cap applied to the fully ranked order (the
    /// sparse-subsequence candidate falls past the cap).
    #[test]
    fn search_fuzzy_ranks_classes_and_caps() {
        use crate::library::tests::capture_one;
        use dartq_source::FileTable;

        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut files = FileTable::new();
        let capture = capture_one(strings, &mut files, "f.dart", &discovery_source());
        let mut index = crate::SymbolIndex::default();
        index.insert_file(capture.file, capture.index);

        let hits = index.search_fuzzy(strings, "fb");
        assert_eq!(hits.len(), RESULT_CAP, "the cap applies");
        let names: Vec<&str> = hits.iter().map(|hit| strings.resolve(hit.decl.name)).collect();
        let expected: Vec<String> = ["fb", "FooBar", "foo_bar", "xfbx"]
            .iter()
            .map(|name| (*name).to_owned())
            .chain((0..28).map(|i| format!("xfbx{i:02}")))
            .collect();
        assert_eq!(names, expected, "rank order: classes, then ties");
        assert!(
            !names.contains(&"foob"),
            "the sparse candidate ranks past the cap"
        );
    }

    /// Determinism and exact-path neutrality: files inserted in opposite
    /// orders answer identically, and a query that IS a full exact name
    /// ranks that name's rows first — the rows the exact face serves.
    #[test]
    fn search_fuzzy_is_insertion_order_independent_and_exact_name_neutral() {
        use crate::library::tests::capture_one;
        use dartq_source::FileTable;

        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut files = FileTable::new();
        let capture_a = capture_one(strings, &mut files, "a.dart", &discovery_source());
        let capture_b = capture_one(strings, &mut files, "b.dart", "void fbx() {}\n");
        let mut forward = crate::SymbolIndex::default();
        forward.insert_file(capture_a.file, capture_a.index);
        forward.insert_file(capture_b.file, capture_b.index);
        let capture_a = capture_one(strings, &mut files, "a.dart", &discovery_source());
        let capture_b = capture_one(strings, &mut files, "b.dart", "void fbx() {}\n");
        let mut backward = crate::SymbolIndex::default();
        backward.insert_file(capture_b.file, capture_b.index);
        backward.insert_file(capture_a.file, capture_a.index);

        let project = |hits: &[crate::SymbolDecl<'_>]| {
            hits.iter()
                .map(|hit| {
                    (
                        strings.resolve(hit.decl.name).to_owned(),
                        hit.file,
                        hit.decl.range.offset,
                    )
                })
                .collect::<Vec<_>>()
        };
        let a = forward.search_fuzzy(strings, "fb");
        let b = backward.search_fuzzy(strings, "fb");
        assert_eq!(project(&a), project(&b), "insertion order never leaks");
        // `fbx` rides the prefix class of the second file, ahead of every
        // substring candidate of the first.
        assert_eq!(strings.resolve(a[1].decl.name), "fbx");

        // A query that spells a full exact name: that name's rows rank
        // first and are exactly the exact face's answer — the exact face
        // itself is untouched by the discovery face.
        let hits = forward.search_fuzzy(strings, "xfbx07");
        assert_eq!(strings.resolve(hits[0].decl.name), "xfbx07");
        let via_exact: Vec<u32> = forward
            .symbols(strings.intern("xfbx07"))
            .map(|hit| hit.decl.range.offset)
            .collect();
        let head: Vec<u32> = hits
            .iter()
            .take_while(|hit| strings.resolve(hit.decl.name) == "xfbx07")
            .map(|hit| hit.decl.range.offset)
            .collect();
        assert_eq!(head, via_exact, "the exact face's rows, unchanged");
    }
}
