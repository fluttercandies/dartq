//! Token model: dual offsets,
//! compact arena elements, and a linear token table.
//!
//! `Token` targets ≤16B: `byte_offset`(4) + `byte_len`(4) + `utf16_offset`(4) +
//! `kind`(1) + `flags`(1) + padding = 16B. Lexemes are not stored — the caller
//! slices them via `&source[t.byte_range()]` ([`Token::byte_range`]).

use crate::keyword::Keyword;

/// Lexical token categories (naming aligned with the official `TokenType`;
/// keywords get no separate kind, see [`TokenKind::Keyword`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// Stream terminator (`byte_offset` = source length, len = 0).
    Eof,
    /// Identifier (includes lexemes judged identifiers in contextual-keyword situations).
    Identifier,
    /// Keyword (official `KeywordToken`; `augment` gated by config).
    Keyword(Keyword),
    /// Decimal integer (including valid digit separators).
    Int,
    /// Floating-point (fraction/exponent forms).
    Double,
    /// `0x` hexadecimal.
    Hexadecimal,
    /// String segment (quotes/escapes kept in the lexeme; interpolated strings are split into segments, see the crate docs).
    String,
    /// Interpolation expression start `${` (begin-group; the closing `}` returns to string scanning).
    StringInterpolationExpression,
    /// Interpolation simple identifier start `$` (followed by Identifier/Keyword).
    StringInterpolationIdentifier,
    /// Leading `#!...` script tag.
    ScriptTag,
    // --- groups ---
    OpenParen,
    CloseParen,
    OpenSquare,
    CloseSquare,
    OpenCurly,
    CloseCurly,
    // --- operators / punctuation (official naming) ---
    Ampersand,
    AmpersandAmpersand,
    AmpersandEq,
    At,
    Bang,
    BangEq,
    /// `===` (illegal but scanned as-is per the official scanner + `unsupported_operator` diagnostic).
    BangEqEq,
    Bar,
    BarBar,
    BarEq,
    Backping,
    Backslash,
    Caret,
    CaretEq,
    Colon,
    Comma,
    Eq,
    EqEq,
    /// `===` (same as above).
    EqEqEq,
    /// `=>` (official TokenType.FUNCTION).
    Function,
    Gt,
    GtEq,
    GtGt,
    GtGtEq,
    /// `>>>` (only with `enable_triple_shift`).
    GtGtGt,
    GtGtGtEq,
    Hash,
    /// `[]` (official INDEX: a single token only when `[` is immediately followed by `]`; does not open a group).
    Index,
    IndexEq,
    Lt,
    LtEq,
    LtLt,
    LtLtEq,
    Minus,
    MinusEq,
    MinusMinus,
    Percent,
    PercentEq,
    Period,
    PeriodPeriod,
    PeriodPeriodPeriod,
    PeriodPeriodPeriodQuestion,
    Plus,
    PlusEq,
    PlusPlus,
    Question,
    QuestionPeriod,
    QuestionPeriodPeriod,
    QuestionQuestion,
    QuestionQuestionEq,
    Semicolon,
    Slash,
    SlashEq,
    Star,
    StarEq,
    Tilde,
    TildeSlash,
    TildeSlashEq,
}

impl TokenKind {
    /// Fixed lexeme (sliceable tokens return `None` — the lexeme is sliced from the source).
    #[must_use]
    pub fn fixed_lexeme(self) -> Option<&'static str> {
        Some(match self {
            TokenKind::Eof => "",
            TokenKind::Identifier
            | TokenKind::Keyword(_)
            | TokenKind::Int
            | TokenKind::Double
            | TokenKind::Hexadecimal
            | TokenKind::String
            | TokenKind::ScriptTag => return None,
            TokenKind::StringInterpolationExpression => r"${",
            TokenKind::StringInterpolationIdentifier => "$",
            TokenKind::OpenParen => "(",
            TokenKind::CloseParen => ")",
            TokenKind::OpenSquare => "[",
            TokenKind::CloseSquare => "]",
            TokenKind::OpenCurly => "{",
            TokenKind::CloseCurly => "}",
            TokenKind::Ampersand => "&",
            TokenKind::AmpersandAmpersand => "&&",
            TokenKind::AmpersandEq => "&=",
            TokenKind::At => "@",
            TokenKind::Bang => "!",
            TokenKind::BangEq => "!=",
            TokenKind::BangEqEq => "!==",
            TokenKind::Bar => "|",
            TokenKind::BarBar => "||",
            TokenKind::BarEq => "|=",
            TokenKind::Backping => "`",
            TokenKind::Backslash => "\\",
            TokenKind::Caret => "^",
            TokenKind::CaretEq => "^=",
            TokenKind::Colon => ":",
            TokenKind::Comma => ",",
            TokenKind::Eq => "=",
            TokenKind::EqEq => "==",
            TokenKind::EqEqEq => "===",
            TokenKind::Function => "=>",
            TokenKind::Gt => ">",
            TokenKind::GtEq => ">=",
            TokenKind::GtGt => ">>",
            TokenKind::GtGtEq => ">>=",
            TokenKind::GtGtGt => ">>>",
            TokenKind::GtGtGtEq => ">>>=",
            TokenKind::Hash => "#",
            TokenKind::Index => "[]",
            TokenKind::IndexEq => "[]=",
            TokenKind::Lt => "<",
            TokenKind::LtEq => "<=",
            TokenKind::LtLt => "<<",
            TokenKind::LtLtEq => "<<=",
            TokenKind::Minus => "-",
            TokenKind::MinusEq => "-=",
            TokenKind::MinusMinus => "--",
            TokenKind::Percent => "%",
            TokenKind::PercentEq => "%=",
            TokenKind::Period => ".",
            TokenKind::PeriodPeriod => "..",
            TokenKind::PeriodPeriodPeriod => "...",
            TokenKind::PeriodPeriodPeriodQuestion => "...?",
            TokenKind::Plus => "+",
            TokenKind::PlusEq => "+=",
            TokenKind::PlusPlus => "++",
            TokenKind::Question => "?",
            TokenKind::QuestionPeriod => "?.",
            TokenKind::QuestionPeriodPeriod => "?..",
            TokenKind::QuestionQuestion => "??",
            TokenKind::QuestionQuestionEq => "??=",
            TokenKind::Semicolon => ";",
            TokenKind::Slash => "/",
            TokenKind::SlashEq => "/=",
            TokenKind::Star => "*",
            TokenKind::StarEq => "*=",
            TokenKind::Tilde => "~",
            TokenKind::TildeSlash => "~/",
            TokenKind::TildeSlashEq => "~/=",
        })
    }

    /// Whether this is a begin-group (`{` `(` `[` `<` `${`; the official `BeginToken` set).
    #[must_use]
    pub fn is_begin_group(self) -> bool {
        matches!(
            self,
            TokenKind::OpenCurly
                | TokenKind::OpenParen
                | TokenKind::OpenSquare
                | TokenKind::Lt
                | TokenKind::StringInterpolationExpression
        )
    }

    /// The matching close token of a begin-group (used to synthesize close
    /// tokens during EOF recovery; official `closeBraceInfoFor`).
    #[must_use]
    pub fn matching_close(self) -> Option<TokenKind> {
        match self {
            TokenKind::OpenCurly | TokenKind::StringInterpolationExpression => {
                Some(TokenKind::CloseCurly)
            }
            TokenKind::OpenParen => Some(TokenKind::CloseParen),
            TokenKind::OpenSquare => Some(TokenKind::CloseSquare),
            TokenKind::Lt => Some(TokenKind::Gt),
            _ => None,
        }
    }

    /// The begin tokens a close token searches for: `}` matches both `{` and
    /// `${` (the official `discardBeginGroupUntil` specialization for
    /// `OPEN_CURLY`); the rest map one-to-one.
    #[must_use]
    pub fn matching_opens(self) -> &'static [TokenKind] {
        match self {
            TokenKind::CloseCurly => &[
                TokenKind::OpenCurly,
                TokenKind::StringInterpolationExpression,
            ],
            TokenKind::CloseParen => &[TokenKind::OpenParen],
            TokenKind::CloseSquare => &[TokenKind::OpenSquare],
            _ => &[],
        }
    }
}

/// Arena element: a 16B compact token (see the module docs).
///
/// `utf16_offset` and `byte_offset` both point at the token start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub byte_offset: u32,
    pub byte_len: u32,
    pub utf16_offset: u32,
    flags: u8,
}

impl Token {
    pub(crate) fn new(kind: TokenKind, byte_offset: u32, byte_len: u32, utf16_offset: u32) -> Self {
        Self {
            kind,
            byte_offset,
            byte_len,
            utf16_offset,
            flags: 0,
        }
    }

    /// Builds a parser-inserted token for [`TokenTable::append_inserted`]: a
    /// closer-split remainder or a recovery synthetic. `synthetic` marks
    /// recovery synthetics ([`Token::is_synthetic`] true); split remainders
    /// materialize valid code and are not synthetic.
    #[must_use]
    pub fn inserted(
        kind: TokenKind,
        byte_offset: u32,
        byte_len: u32,
        utf16_offset: u32,
        synthetic: bool,
    ) -> Self {
        Self {
            kind,
            byte_offset,
            byte_len,
            utf16_offset,
            flags: if synthetic { FLAG_SYNTHETIC } else { 0 },
        }
    }

    pub(crate) fn synthetic(mut self) -> Self {
        self.flags |= FLAG_SYNTHETIC;
        self
    }

    /// Synthetic token inserted during recovery (synthetic closing groups,
    /// synthetic `0` numbers, synthetic closing-quote segments, synthetic empty
    /// interpolation identifiers).
    #[must_use]
    pub fn is_synthetic(&self) -> bool {
        self.flags & FLAG_SYNTHETIC != 0
    }

    /// `[byte_offset, byte_offset + byte_len)`; slicing `&source[r]` yields the lexeme.
    #[must_use]
    pub fn byte_range(&self) -> std::ops::Range<usize> {
        let start = self.byte_offset as usize;
        start..start + self.byte_len as usize
    }
}

const FLAG_SYNTHETIC: u8 = 1;

/// The "no match" sentinel of the `end_of` side table.
pub(crate) const NO_END: u32 = u32::MAX;

/// Linear token table (EOF is always the last element; interpolation nesting is also linear in order).
///
/// `Clone` is a plain copy of the three backing vecs — the table is a pure
/// function of the scanned bytes and the [`crate::ScanConfig`], so a clone is
/// always substitutable for the original (consumers that need to keep the raw
/// table while handing an owned copy to the parse, e.g. the lint fan-out's
/// scan adoption).
#[derive(Clone, Debug)]
pub struct TokenTable {
    tokens: Vec<Token>,
    /// begin token index → matching close token index (`NO_END` = no match;
    /// after recovery an unclosed begin points at its synthetic close token,
    /// consistent with the official endGroup).
    end_of: Vec<u32>,
    /// The unmatched begin groups: `(begin index, synthetic close index)`, in
    /// the order the scanner closed them. The parser reports these rows
    /// post-parse (the official `UnmatchedToken` head-run translation reads
    /// `begin.endGroup.charOffset` after the parse, so an adopted closer
    /// reports at its relocation point); the scanner itself emits nothing for
    /// them.
    unmatched: Vec<(u32, u32)>,
}

impl TokenTable {
    pub(crate) fn new(tokens: Vec<Token>, end_of: Vec<u32>, unmatched: Vec<(u32, u32)>) -> Self {
        Self {
            tokens,
            end_of,
            unmatched,
        }
    }

    /// The scanner's unmatched `(begin, synthetic close)` index pairs, in
    /// scan-close order. Empty for a well-formed file (no allocation on the
    /// clean path). [`Self::append_inserted`] never disturbs them (scan-time
    /// indices only).
    #[must_use]
    pub fn unmatched_groups(&self) -> &[(u32, u32)] {
        &self.unmatched
    }

    /// Total token count (EOF included).
    #[must_use]
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    #[must_use]
    pub fn get(&self, index: usize) -> Option<&Token> {
        self.tokens.get(index)
    }

    /// Full slice (for sequential traversal).
    #[must_use]
    pub fn as_slice(&self) -> &[Token] {
        &self.tokens
    }

    /// First element (always present — EOF guarantees it).
    #[must_use]
    pub fn first(&self) -> &Token {
        &self.tokens[0]
    }

    /// Last element (i.e. EOF).
    #[must_use]
    pub fn last(&self) -> &Token {
        &self.tokens[self.tokens.len() - 1]
    }

    /// Successor index; returns `None` at EOF.
    #[must_use]
    pub fn next(&self, index: usize) -> Option<usize> {
        if index + 1 < self.tokens.len() {
            Some(index + 1)
        } else {
            None
        }
    }

    /// Matching close token index of a begin-group token (`None` if unclosed or not a begin).
    #[must_use]
    pub fn matching_end(&self, begin_index: usize) -> Option<usize> {
        let end = *self.end_of.get(begin_index)?;
        if end == NO_END {
            None
        } else {
            Some(end as usize)
        }
    }

    /// Lexeme (sliced from the source; for the lexeme of synthetic tokens see
    /// [`TokenKind::fixed_lexeme`] — for synthetic sliceable tokens such as the
    /// `0` recovered from `0x`, the source slice is the recovered lexeme).
    #[must_use]
    pub fn lexeme<'s>(&self, source: &'s str, index: usize) -> &'s str {
        let t = &self.tokens[index];
        &source[t.byte_range()]
    }

    /// Appends parser-inserted tokens (closer-split remainders and recovery
    /// synthetics) into the main storage and re-seats the Eof sentinel last,
    /// yielding the one contiguous unified table the AST reads. Appended
    /// token `i` lands at index `old_len - 1 + i` in iteration order — the
    /// first takes the Eof sentinel's former slot — so original indices are
    /// untouched and Eof stays the final element.
    ///
    /// Original tokens keep their indices, offsets and slices exactly, and
    /// `matching_end` stays valid (its entries point at close tokens, never
    /// at Eof). Sequencing contract: this runs once at parse end — before it,
    /// an inserted id resolves only in the parser's side table, and no span
    /// may be derived through the table for one.
    ///
    /// # Degradation
    ///
    /// A scanned table always ends at Eof, so the pop always succeeds; a
    /// missing sentinel (never-scanned table) debug-asserts and appends the
    /// inserted tokens without a re-seat instead of panicking.
    pub fn append_inserted(&mut self, inserted: impl IntoIterator<Item = Token>) {
        match self.tokens.pop() {
            Some(eof) => {
                debug_assert_eq!(eof.kind, TokenKind::Eof, "Eof must be the last token");
                self.tokens.extend(inserted);
                self.tokens.push(eof);
            }
            None => {
                debug_assert!(
                    false,
                    "append_inserted expects a scanned table ending at Eof"
                );
                self.tokens.extend(inserted);
            }
        }
    }
}

const _: () = assert!(
    std::mem::size_of::<Token>() <= 16,
    "Token must stay <= 16 bytes"
);
