//! Full Dart 3.15 lexical scanning: UTF-8 source → linear token table + syntax
//! diagnostics.
//!
//! Behavior is aligned with the official scanner
//! (`.third/dart-sdk/pkg/_fe_analyzer_shared/lib/src/scanner/`, Dart 3.15
//! main). Key behavioral contracts (all verified against the reference source):
//!
//! # Interpolated string token sequence shape (parser contract)
//!
//! The token stream is **linear and flat** (aligned with the official
//! scanner); interpolation produces no tree structure:
//!
//! ```text
//! 'a${expr}b'  →  String("'a")  StringInterpolationExpression("${")  <expr tokens...>
//!                 CloseCurly("}")  String("b'")
//! 'a$id.c'    →  String("'a")  StringInterpolationIdentifier("$")  Identifier/Keyword(id)
//!                 String(".c'")
//! ```
//!
//! The prefix `String` segment ends **before** `$` (it does not contain the
//! `$`: the SII/SIE token starts at `$`, and the `${` form covers `$` + `{` —
//! official corpus `scanner_test.dart:1034-1051`, `"Hello "` [0..7) with `${`
//! at 7); the suffix `String` segment includes the closing quote. Correction
//! history: an earlier revision of this doc claimed the prefix includes `$`,
//! copied from the outdated doc comment at `abstract_scanner.dart:2203-2207`
//! (`StringToken("a $)`); the authoritative sources are the code path and the
//! official test corpus above. `${` is a begin-group
//! (paired with `}`, see [`TokenTable::matching_end`]); after `}` closes the
//! `${`, scanning returns to the string body (an equivalent implementation of
//! the official `$STX` signal mechanism). The identifier form of interpolation
//! allows keywords (`'$this'` produces `Keyword::This`).
//!
//! # `>>>` gating conclusion
//!
//! The official scanner (`abstract_scanner.dart`, `tokenizeGreaterThan` :1479)
//! **never splits `>>`** — regardless of the triple-shift switch, `>>` is
//! always a single token; `>>>`/`>>>=` become a single token only when
//! [`ScanConfig::enable_triple_shift`] is true, otherwise they scan as
//! `>>` + `>` (or `>>` + `>=`). Splitting `>>` in `List<List<int>>` generics is
//! the parser layer's responsibility. The switch is derived by the caller from
//! the language version: `dartq_language::triple_shift_enabled(v)`
//! (>= 2.14, generated yaml `enabledIn`) or `FeatureSet`.
//!
//! # `@dart` version comment window rules
//!
//! Only version comments inside the **file-header comment window** are
//! recognized (bigHeaderSwitch :1076 + `_scanHeaderLookingForLanguageVersion`
//! :972): starting from the file origin (the decode faces strip a leading
//! BOM, so the scanner text starts at the true content), consecutive
//! whitespace/comments (including nested block comments)
//! do not end the window; comments after an optional very-first script tag
//! (`#!`, only when it is the file's first byte sequence) are still
//! inside the window. Emitting any real token closes the window. The syntax is
//! strict: `//` + space* + `@dart` + space* + `=` + space* + digit+ + `.` +
//! digit+ + space* + (LF|CR|EOF); `///`, tab indentation, and trailing garbage
//! never match. When the window contains multiple matches, the **last** one
//! wins (oracle basis and the CFE split: see "consumer contract" below).
//!
//! # `language_version_override` consumer contract
//!
//! If [`ScanResult::language_version_override`] differs from the language
//! version the caller assumed when building [`ScanConfig`], the caller **must**
//! re-derive the config from the override and rescan the file before parsing.
//! The oracle scanner folds the override into its configuration mid-scan
//! (`scanner.configuration = buildConfig(_featureSet)`, analyzer-8.4.1
//! scanner.dart:222), so a single pass suffices there; dartq's scanner takes
//! a fixed [`ScanConfig`], so re-issuing it and rescanning is the consumer's
//! job (currently nobody; this is the interface law for the CLI/engine lanes).
//! Rescanning is cheap relative to parse/analysis.
//!
//! Multiple-match precedence: the **last** valid version comment in the
//! header window wins, because the oracle's `_languageVersionChanged` runs on
//! every languageVersionChanged and unconditionally reassigns
//! `_overrideVersion` (:206) and `scanner.configuration` (:222)
//! (analyzer-8.4.1 scanner.dart:195-222). The CFE pipeline keeps the **first**
//! match instead (`registerExplicitLanguageVersion`,
//! `_languageVersion.isExplicit → return`, `source_compilation_unit.dart:486`);
//! that split is real and expected — dartq's parity basis for this behavior
//! is the analyzer consumer path, and the CFE citation must not be reused for
//! oracle alignment here.
//!
//! # Known divergences from the official scanner (recorded, revisit triggers)
//!
//! - Invalid-input synthetic recovery token spans: ours emit zero-length or
//!   clamped forms where the official scanner keeps `[start, stop)` partial
//!   lexemes (abstract_scanner.dart:334-343, 1581-1592, 1697-1714).
//!   Adjudicated KEEP — Jaccard-neutral (the oracle axis scores `(code, file)`
//!   entries, not invalid-input token spans) and the shapes are pinned by
//!   tests; `[start, stop)` partial-lexeme alignment remains a future option.
//!
//! # Identifiers
//!
//! Dart identifiers are ASCII-only (`a-zA-Z0-9_$`, official `isIdentifierChar`,
//! `internal_utils.dart`:4). Non-ASCII characters in code take the official
//! `unexpected` :2470 path: they produce an `illegal_character` diagnostic;
//! identifier-like characters merge with adjacent identifiers into a single
//! `Identifier` token for recovery (the official `NonAsciiIdentifierToken`
//! recovery shape).
//!
//! # Recovery strategy list (single pass + one conditional re-scan)
//!
//! | Input | Behavior |
//! |---|---|
//! | Unclosed `{`/`(`/`[`/`${` (EOF) | Pop the stack one by one, inserting synthetic close tokens + `expected_token` diagnostics (official `unmatchedBeginGroup`) |
//! | Exactly one unclosed `{` at EOF + a misaligned `{`/`${` in the stream | Two-pass recovery (official `scan`/`scanString` re-scan): the blamed brace's offset feeds a second full scan in which a `}` closing it is replaced by a synthetic closer + `expected_token` (`tokenizeCloseBrace`), and a blamed `${` is discarded with a synthetic `}` at the `$` while the string body resumes after the `{` (`tokenizeInterpolatedExpression`) — the "user is typing" shape recovers to the oracle's outline |
//! | Extra/mismatched `}`/`)`/`]` | No scanner diagnostic, emitted as normal tokens (when official `discardBeginGroupUntil` finds no match it emits as-is; the parser reports the error) |
//! | Close token matching deep in the stack | Dual-option recovery (official `discardBeginGroupUntil` :787-826): both postures run the remaining input to EOF on cloned state (100-step cap, nested deep matches insert unconditionally) — insert a synthetic closer + `expected_token` per popped begin, or drop the closer as a plain token with the stack untouched. Drop wins only when strictly cheaper (`recoveryCount` + final unclosed-stack size, +1 for the dropped closer); ties insert: `{[}` → `{[]}`, but the oracle's `bracket_mismatch` corpus drops (`[(])]` → `[(])]` all real) |
//! | Unterminated string | Synthetic closing-quote STRING segment + `unterminated_string_literal` (range pinned to the last character, aligned with the analyzer's translation of `ErrorToken`) |
//! | Unterminated block comment | `unterminated_multi_line_comment` (pinned to the last character) |
//! | `0x` with no digits | Synthetic `0` HEX token + `missing_hex_digit` |
//! | `1e` with no exponent digits | Synthetic `0` DOUBLE token + `missing_digit` |
//! | Digit separator at an illegal position | `unexpected_separator_in_number` (position = number start; the official scanner reports each occurrence) |
//! | `===`/`!==` | Token emitted as-is (`EqEqEq`/`BangEqEq`) + `unsupported_operator` |
//! | Non-identifier after interpolation `$` | Synthetic empty `Identifier` + `missing_identifier` |
//! | Illegal character | `illegal_character` + skip by character class or merge into an Identifier |
//!
//! `>` / `>>` / `>>>` **never error** when closing a `<` group (they are also
//! operators; official appendGt/appendGtGt/appendGtGtGt semantics); `;`, `=`,
//! `{`, `[`, `${` silently discard the top-of-stack `<` group (type parameters
//! cannot contain these; official discardOpenLt).
//!
//! # Comments
//!
//! Consistent with the official main stream, **comments (including `///` and
//! `/** */` doc comments) produce no tokens** (officially they form a
//! `CommentToken` chain attached to the preceding token and never enter the
//! main stream; the M0 parser does not consume them — land them as an additive
//! field when needed). Block comments nest (depth counted).
//!
//! # Signature revision notes (relative to the original frozen draft)
//!
//! `scan` takes `file: dartq_source::FileId`: the final shape of
//! `dartq_diagnostics::Diagnostic` from #12/#17 carries a separate `file`
//! field, and a `FileId` can only be issued by a `FileTable` — so the caller
//! (parser/engine lane) passes it in. The rest of the frozen surface is
//! unchanged.

use dartq_diagnostics::Diagnostic;
use dartq_language::LanguageVersion;
use dartq_source::{FileId, LineIndex, SourceRange};

mod diag;
mod keyword;
mod lexer;
mod token;

#[cfg(test)]
mod tests;

pub use diag::{
    ILLEGAL_CHARACTER, MISSING_DIGIT, MISSING_HEX_DIGIT, UNEXPECTED_SEPARATOR_IN_NUMBER,
    UNSUPPORTED_OPERATOR, UNTERMINATED_MULTI_LINE_COMMENT,
};
pub use keyword::{Keyword, KeywordStyle, keyword_from_str};
pub use token::{Token, TokenKind, TokenTable};

/// Scan configuration (isomorphic to the official `ScannerConfiguration`; two
/// booleans only, derived by the caller from the `FeatureSet`; the scanner does
/// not depend on `FeatureSet` itself).
#[derive(Clone, Copy, Debug)]
pub struct ScanConfig {
    /// Whether `>>>` / `>>>=` form a single token (when false, `>>>` scans as `>>` + `>`).
    pub enable_triple_shift: bool,
    /// Whether `augment` is a built-in keyword (when false, treated as a plain
    /// identifier — official tokenizeKeywordOrIdentifier,
    /// `abstract_scanner.dart:2106`; pinned by `front_end` scanner_test.dart:480-489).
    pub enable_augmentations: bool,
}

/// Scan result.
#[derive(Debug)]
pub struct ScanResult {
    /// Token table, always terminated by EOF (linear order, interpolation nesting included).
    pub tokens: TokenTable,
    /// Lexical diagnostics (syntax codes, `SyntacticError` kind); ranges are UTF-16 offsets.
    pub diagnostics: Vec<Diagnostic>,
    /// The last explicit `// @dart = x.y` matched within the header window
    /// (later matches overwrite earlier ones — see "consumer contract" in the
    /// module docs: a differing override obliges the consumer to re-issue
    /// [`ScanConfig`] and rescan).
    pub language_version_override: Option<LanguageVersion>,
    /// UTF-16 range of the whole version comment lexeme behind
    /// [`ScanResult::language_version_override`] — from the `//` through the
    /// last character before the line terminator (trailing spaces included,
    /// CR excluded), isomorphic to the official `LanguageVersionToken`
    /// offset/length that the analyzer's above-latest warning pins at
    /// (`analyzer/lib/src/dart/scanner/scanner.dart:124`). `None` exactly when
    /// `language_version_override` is `None`; with multiple matches it points
    /// at the last (winning) one.
    pub language_version_override_span: Option<SourceRange>,
    /// Line index (authoritative dartq-source implementation).
    pub line_index: LineIndex,
}

/// Scans `source` (valid UTF-8, guaranteed by the caller; BOM-less — the
/// decode faces strip a leading U+FEFF, so a scanner-visible BOM is content
/// and takes the ordinary illegal-character path).
///
/// `file` is used for diagnostic attribution (see "Signature revision notes" in
/// the module docs).
#[must_use]
pub fn scan(source: &str, config: ScanConfig, file: FileId) -> ScanResult {
    lexer::Lexer::new(source, config, file).run()
}
