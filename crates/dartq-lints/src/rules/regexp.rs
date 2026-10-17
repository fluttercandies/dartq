//! The `valid_regexps` rule and its regex-syntax validator.
//!
//! Official: `pkg/linter/lib/src/rules/valid_regexps.dart:31-72` — the first
//! positional argument of a `RegExp(...)` instance creation, when it is a
//! string literal without interpolation, is validated by constructing the
//! regex; a `FormatException` reports at the literal. Message:
//! `diagnostic.g.dart` `validRegexps`.
//!
//! Approximations (documented):
//! - The creation face is written-name matching. The official resolves the
//!   constructor (`element.name == 'RegExp' && element.library.isDartCore`,
//!   :48-50); here the callee head must be exactly the bare identifier
//!   `RegExp`, over the three creation shapes the parser produces — the
//!   implicit call `RegExp(...)` (a `Send` reading the owner slice,
//!   `rules::slice`), `const RegExp(...)` (the same `Send` under a
//!   `ConstExpression`), and `new RegExp(...)` (a `ConstructorReference`
//!   inside the `NewExpression`). Prefixed and chained callees trail a
//!   same-span `DotAccess` marker and stay silent; a user class named
//!   `RegExp` over-reports.
//! - The validation face ports the runtime parser's decision order
//!   (`RegExp(source, unicode: unicode)` throwing `FormatException`) as a
//!   recursive-descent accept/reject walk over the ECMAScript pattern
//!   grammar: alternation and group balance, quantifier placement and
//!   `{n,m}` ordering, class ranges and class escapes, and the unicode-mode
//!   escape strictness (annex-B legacy identity escapes are accepted only
//!   with the flag off). Corner decisions fixed to the shipped engine:
//!   `{n,m}` with `n > m` throws in both modes (so does the literal-brace
//!   reading of it, e.g. the non-unicode `\u{2,1}`), quantified lookbehinds
//!   throw in both modes, quantified anchors/lookaheads throw only in
//!   unicode mode, duplicate named groups are accepted, and a numeric `\N`
//!   escape is a backreference iff `N` does not exceed the pattern's total
//!   capturing-group count (forward references parse).

use dartq_ast::NodeKind;

use crate::engine::Check;
use crate::registry::RuleSpec;
use crate::rules::slice;

/// The domain's incoming registrations: a new standalone rule appends its
/// `SPEC` reference here (name order) and joins the registry through the
/// aggregated tail — adding a rule touches only this file. Batch close
/// promotes the block into the registry's frozen skeleton together with
/// the pin-test wave list (`tests/registry_pin.rs`).
pub(crate) static INCOMING_RULES: &[&RuleSpec] = &[];

pub(crate) mod valid_regexps {
    use super::{creation_parts, is_valid, slice, string_value};
    use crate::engine::Check;
    use crate::registry::{Group, RuleSpec};
    use dartq_ast::NodeKind;

    /// Official: `pkg/linter/lib/src/rules/valid_regexps.dart:40-71` — the
    /// first positional argument of a `RegExp(...)` creation, when it is a
    /// plain string literal, must parse as a regex under the `unicode:`
    /// flag's value. Message: `diagnostic.g.dart` `validRegexps`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "valid_regexps",
        group: Group::Core,
        problem: "Invalid regular expression syntax.",
        correction: "Try correcting the regular expression.",
        kinds: &[NodeKind::Send, NodeKind::NewExpression],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let (callee, args) = creation_parts(c, node);
        // The callee head is exactly the bare identifier `RegExp` (the
        // documented name-level approximation of the resolved-element test).
        let Some(callee) = callee else {
            return;
        };
        if c.kind(callee) != NodeKind::Identifier || c.lexeme(c.head_token(callee)) != "RegExp" {
            return;
        }
        let Some(args) = args else {
            return; // argument-less tear-off shape: nothing to validate
        };
        let pieces = c.collect_direct(args);
        let arguments = slice::arguments(c, &pieces);
        if arguments.is_empty() {
            return;
        }
        // `unicode: true` named argument (:52-60) — the value must be the
        // `true` literal (`isTrue`); the named group is
        // `name value NamedArgument`.
        let unicode = arguments.iter().any(|group| {
            let marker = group.end - 1;
            group.len() == 3
                && c.kind(pieces[marker]) == NodeKind::NamedArgument
                && c.kind(pieces[group.start]) == NodeKind::Identifier
                && c.lexeme(c.head_token(pieces[group.start])) == "unicode"
                && c.kind(pieces[group.start + 1]) == NodeKind::LiteralBool
                && c.lexeme(c.head_token(pieces[group.start + 1])) == "true"
        });
        // `args.first` (:62) — the first argument; named arguments come
        // last in legal Dart, so a leading named group is not a string
        // literal and the face skips.
        let first = &arguments[0];
        if c.kind(pieces[first.end - 1]) == NodeKind::NamedArgument {
            return;
        }
        let source = pieces[first.start];
        // `sourceExpression is StringLiteral` + `stringValue != null` (:63-65):
        // interpolation makes the value unknown and the face skips.
        if c.kind(source) != NodeKind::StringLiteral {
            return;
        }
        let Some(value) = string_value(c, source) else {
            return;
        };
        if !is_valid(&value, unicode) {
            let range = c.utf16_span(source);
            c.report(range, &[]);
        }
    }
}

/// The callee head and the argument list of a creation-shaped call: a
/// `Send` reads the owner slice (`rules::slice` — the argument list sits at
/// `pos - 1`, the callee head before that), a `NewExpression` from its
/// children. The callee is `Some` only for the single-name forms the rule
/// matches — one `Identifier` head without a trailing `DotAccess` (a
/// prefixed or chained callee stays silent — the conservative family), or
/// one `ConstructorReference` holding exactly one `Identifier` (the `new`
/// face).
fn creation_parts(c: &Check<'_>, node: u32) -> (Option<u32>, Option<u32>) {
    match c.kind(node) {
        NodeKind::Send => {
            let Some((kids, pos)) = slice::owner_slice(c) else {
                return (None, None);
            };
            // A `None` argument list is a tear-off (`RegExp;`) — no creation.
            let Some((head, Some(args))) = slice::send_parts(c, &kids, pos) else {
                return (None, None);
            };
            // A prefixed or chained callee trails a same-span `DotAccess`
            // marker — the conservative family stays silent.
            if slice::access_marker(c, &kids, pos, NodeKind::DotAccess).is_some() {
                return (None, None);
            }
            (
                (c.kind(kids[head]) == NodeKind::Identifier).then_some(kids[head]),
                Some(args),
            )
        }
        NodeKind::NewExpression => {
            let kids = c.collect_direct(node);
            let Some(args) = kids
                .iter()
                .rev()
                .copied()
                .find(|&kid| c.kind(kid) == NodeKind::Arguments)
            else {
                return (None, None);
            };
            let args_pos = kids.iter().position(|&kid| kid == args).unwrap_or(0);
            let callee = match args_pos {
                1 => match c.kind(kids[0]) {
                    // One bare identifier, or a constructor reference that
                    // holds exactly one identifier (unqualified, unnamed).
                    NodeKind::Identifier => Some(kids[0]),
                    NodeKind::ConstructorReference => {
                        let window = c.children(kids[0]);
                        let mut ids = (window.start..window.end)
                            .filter(|&id| c.kind(id) == NodeKind::Identifier);
                        ids.next().filter(|_| ids.next().is_none())
                    }
                    _ => None,
                },
                _ => None,
            };
            (callee, Some(args))
        }
        _ => (None, None),
    }
}

/// The `stringValue` of a string-literal node: `None` for the interpolated
/// (multi-token) form, the unescaped contents of the single token otherwise
/// (raw strings verbatim).
fn string_value(c: &Check<'_>, node: u32) -> Option<String> {
    let range = c.unit.ast.token_range(node);
    if range.len() != 1 {
        return None;
    }
    let lexeme = c.lexeme(range.start);
    let bytes = lexeme.as_bytes();
    let raw = bytes.first() == Some(&b'r');
    let quote = if raw {
        bytes.get(1).copied()?
    } else {
        bytes.first().copied()?
    };
    let lead = usize::from(raw) + 1;
    // Multiline literals keep their 3-quote runs; contents sit between them.
    let open = bytes[lead..].iter().take_while(|&&b| b == quote).count();
    let close = bytes[lead..]
        .iter()
        .rev()
        .take_while(|&&b| b == quote)
        .count();
    let contents = lexeme.get(lead + open..bytes.len() - close)?;
    if raw {
        return Some(contents.to_owned());
    }
    Some(unescape(contents))
}

/// The Dart string-escape rules over scanner-validated contents: control
/// escapes, `\xHH`, `\uHHHH`, `\u{H+}`, line continuations, and the quoted
/// metacharacters. (The scanner rejects unknown escapes, so the catch-all
/// pass-through only covers recovery shapes.)
fn unescape(contents: &str) -> String {
    let chars: Vec<char> = contents.chars().collect();
    let mut out = String::with_capacity(contents.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '\\' || i + 1 >= chars.len() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        i += 1;
        match chars[i] {
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            'f' => out.push('\u{c}'),
            'b' => out.push('\u{8}'),
            't' => out.push('\t'),
            'v' => out.push('\u{b}'),
            'x' => {
                let value = hex_value(&chars, i + 1, 2);
                out.push(char::from_u32(value).unwrap_or('\u{fffd}'));
                i += 2;
            }
            'u' if chars.get(i + 1) == Some(&'{') => {
                let mut j = i + 2;
                let mut value = 0u32;
                while let Some(digit) = chars.get(j).and_then(|ch| ch.to_digit(16)) {
                    value = value * 16 + digit;
                    j += 1;
                }
                out.push(char::from_u32(value).unwrap_or('\u{fffd}'));
                i = j; // the closing `}` is a plain char on the next loop pass
            }
            'u' => {
                let value = hex_value(&chars, i + 1, 4);
                out.push(char::from_u32(value).unwrap_or('\u{fffd}'));
                i += 4;
            }
            '\n' => {}
            '\r' => {
                if chars.get(i + 1) == Some(&'\n') {
                    i += 1;
                }
            }
            ch => out.push(ch),
        }
        i += 1;
    }
    out
}

fn hex_value(chars: &[char], from: usize, width: usize) -> u32 {
    chars
        .get(from..from + width)
        .unwrap_or(&[])
        .iter()
        .fold(0u32, |acc, ch| acc * 16 + ch.to_digit(16).unwrap_or(0))
}

// —— the regex syntax validator —————————————————————————————————————————————
// A recursive-descent port of the ECMAScript pattern grammar's decision
// order (the runtime `RegExp` constructor's `FormatException` face). One
// pass, no backtracking, linear in the source — the rule only needs
// accept/reject.

/// Whether `source` parses as a regex under the `unicode` flag's strictness.
fn is_valid(source: &str, unicode: bool) -> bool {
    let mut scan = Scan::new(source, unicode);
    // A recorded `\N` is a backreference iff `N` does not exceed the
    // pattern's total capture count; the verdict waits for the walk because
    // forward references are legal (regexp-parser.cc
    // ParseBackReferenceIndex).
    scan.disjunction(false) && scan.backrefs.iter().all(|&n| n <= scan.groups)
}

/// One escape's class face: a single character (usable as a class-range
/// endpoint), a set escape (never a range endpoint), or an anchor (outside
/// classes only).
enum Escape {
    Char(u32),
    Set,
    Anchor,
}

/// What the last atom was, for the quantifier placement rules: a plain atom,
/// an anchor (`^ $ \b \B` — quantifiable only in annex B), a lookahead
/// (annex B only), or a lookbehind (never).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Atom {
    Term,
    Anchor,
    Lookahead,
    Lookbehind,
}

struct Scan {
    s: Vec<char>,
    pos: usize,
    unicode: bool,
    /// Capturing groups opened so far; never popped, so at walk end this is
    /// the pattern's total capture count — the `\N` validity bound in
    /// unicode mode.
    groups: usize,
    /// Numeric `\N` backreference values in walk order; each is checked
    /// against the final [`Self::groups`] total after the walk.
    backrefs: Vec<usize>,
    /// Names of closed named capturing groups.
    named: Vec<String>,
}

impl Scan {
    fn new(source: &str, unicode: bool) -> Self {
        Self {
            s: source.chars().collect(),
            pos: 0,
            unicode,
            groups: 0,
            backrefs: Vec::new(),
            named: Vec::new(),
        }
    }

    fn peek(&self) -> Option<char> {
        self.s.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += 1;
        Some(ch)
    }

    /// `Disjunction`: alternatives separated by `|`; `in_group` leaves the
    /// closing `)` for the group to consume.
    fn disjunction(&mut self, in_group: bool) -> bool {
        loop {
            if !self.alternative() {
                return false;
            }
            match self.peek() {
                Some('|') => self.pos += 1,
                Some(')') if in_group => return true,
                None => return !in_group,
                _ => return false,
            }
        }
    }

    /// `Alternative`: terms until `|`, `)`, or the end.
    fn alternative(&mut self) -> bool {
        loop {
            match self.peek() {
                None | Some('|' | ')') => return true,
                Some(_) => {
                    if !self.term() {
                        return false;
                    }
                }
            }
        }
    }

    /// One `Term` plus its optional quantifier.
    fn term(&mut self) -> bool {
        let atom = match self.peek() {
            Some('^' | '$') => {
                self.pos += 1;
                Atom::Anchor
            }
            Some('(') => match self.group() {
                Some(atom) => atom,
                None => return false,
            },
            Some('[') => {
                if !self.class() {
                    return false;
                }
                Atom::Term
            }
            Some('\\') => {
                self.pos += 1;
                match self.escape(false) {
                    Some(Escape::Anchor) => Atom::Anchor,
                    Some(_) => Atom::Term,
                    None => return false,
                }
            }
            // Quantifiers with nothing to repeat, or end of input; lone
            // `{`/`}`/`]` are literal only in annex B. `.` and every other
            // character are plain literal terms.
            Some('*' | '+' | '?') | None => return false,
            Some('{' | '}' | ']') if self.unicode => return false,
            Some(_) => {
                self.pos += 1;
                Atom::Term
            }
        };
        if self.at_quantifier() {
            let quantifiable = match atom {
                Atom::Term => true,
                // Annex B quantifies anchors and lookaheads; unicode mode
                // quantifies neither; lookbehinds never.
                Atom::Lookbehind => false,
                Atom::Anchor | Atom::Lookahead => !self.unicode,
            };
            if !quantifiable || !self.quantifier() {
                return false;
            }
        }
        true
    }

    /// Whether a quantifier begins at the cursor (`{`-forms only count when
    /// they are complete; a malformed brace falls back to a literal atom).
    fn at_quantifier(&self) -> bool {
        match self.peek() {
            Some('*' | '+' | '?') => true,
            Some('{') => self.brace_quantifier_len(),
            _ => false,
        }
    }

    /// Whether a complete `{n}` / `{n,}` / `{n,m}` production starts at the
    /// cursor's `{`.
    fn brace_quantifier_len(&self) -> bool {
        let mut i = self.pos + 1;
        if !self.digits_at(&mut i) {
            return false;
        }
        match self.s.get(i) {
            Some('}') => true,
            Some(',') => {
                i += 1;
                if self.s.get(i) == Some(&'}') {
                    return true;
                }
                self.digits_at(&mut i) && self.s.get(i) == Some(&'}')
            }
            _ => false,
        }
    }

    /// Consumes one quantifier; `false` on `{n,m}` with `n > m` (the runtime
    /// throws "numbers out of order" in both modes). The complete form was
    /// already gated by [`Self::at_quantifier`].
    fn quantifier(&mut self) -> bool {
        match self.peek() {
            Some('*' | '+' | '?') => self.pos += 1,
            Some('{') => {
                let mut i = self.pos + 1;
                let min = self.digits_value(&mut i);
                let (max, close) = match self.s.get(i) {
                    Some('}') => (Some(min), i),
                    Some(',') if self.s.get(i + 1) == Some(&'}') => (None, i + 1),
                    Some(',') => {
                        i += 1;
                        let max = self.digits_value(&mut i);
                        (Some(max), i)
                    }
                    // at_quantifier gated a complete form.
                    _ => return true,
                };
                if max.is_some_and(|max| min > max) {
                    return false;
                }
                self.pos = close + 1;
            }
            _ => return true,
        }
        if self.peek() == Some('?') {
            self.pos += 1; // lazy
        }
        true
    }

    fn digits_at(&self, i: &mut usize) -> bool {
        let start = *i;
        while self.s.get(*i).is_some_and(char::is_ascii_digit) {
            *i += 1;
        }
        *i > start
    }

    fn digits_value(&self, i: &mut usize) -> u64 {
        let mut value = 0u64;
        while let Some(digit) = self.s.get(*i).and_then(|ch| ch.to_digit(10)) {
            value = value.saturating_mul(10).saturating_add(u64::from(digit));
            *i += 1;
        }
        value
    }

    /// `(` — capturing, non-capturing, lookahead/lookbehind, named, and
    /// modifier groups; returns the atom kind the completed group leaves.
    fn group(&mut self) -> Option<Atom> {
        self.pos += 1; // `(`
        if self.peek() == Some('?') {
            self.pos += 1;
            return match self.peek() {
                Some(':') => {
                    self.pos += 1;
                    self.group_body().map(|()| Atom::Term)
                }
                Some('=') => {
                    self.pos += 1;
                    self.group_body().map(|()| Atom::Lookahead)
                }
                Some('!') => {
                    self.pos += 1;
                    self.group_body().map(|()| Atom::Lookahead)
                }
                Some('<') if matches!(self.s.get(self.pos + 1), Some('=' | '!')) => {
                    self.pos += 2;
                    self.group_body().map(|()| Atom::Lookbehind)
                }
                // Named capture: the name opens a capturing group.
                Some('<') => {
                    self.pos += 1;
                    let start = self.pos;
                    while self.peek().is_some_and(|ch| ch != '>' && ch != ')') {
                        self.pos += 1;
                    }
                    if self.peek() != Some('>') || self.pos == start {
                        return None;
                    }
                    let name: String = self.s[start..self.pos].iter().collect();
                    self.pos += 1; // `>`
                    self.groups += 1;
                    self.group_body()?;
                    self.named.push(name);
                    Some(Atom::Term)
                }
                Some('i' | 'm' | 's' | '-') => self.modifier_group(),
                _ => None,
            };
        }
        self.groups += 1;
        self.group_body()?;
        Some(Atom::Term)
    }

    /// The `Disjunction)` of an opened group, `)` consumed.
    fn group_body(&mut self) -> Option<()> {
        if !self.disjunction(true) || self.peek() != Some(')') {
            return None;
        }
        self.pos += 1;
        Some(())
    }

    /// `(?flags)` / `(?flags:...)` with optional `-flags` — `i`, `m`, `s`.
    fn modifier_group(&mut self) -> Option<Atom> {
        let flag = |ch: char| matches!(ch, 'i' | 'm' | 's');
        let mut seen = 0usize;
        while self.peek().is_some_and(flag) {
            self.pos += 1;
            seen += 1;
        }
        let mut negated = 0usize;
        if self.peek() == Some('-') {
            self.pos += 1;
            while self.peek().is_some_and(flag) {
                self.pos += 1;
                negated += 1;
            }
        }
        if seen == 0 && negated == 0 {
            return None;
        }
        match self.peek() {
            Some(')') => {
                self.pos += 1;
                Some(Atom::Term)
            }
            Some(':') => {
                self.pos += 1;
                self.group_body().map(|()| Atom::Term)
            }
            _ => None,
        }
    }

    /// `[` — a character class with ranges; `z-a` and set-escape endpoints
    /// throw in both modes (the runtime's "range out of order" and "class
    /// escape in range" faces).
    fn class(&mut self) -> bool {
        self.pos += 1; // `[`
        if self.peek() == Some('^') {
            self.pos += 1;
        }
        loop {
            let Some(ch) = self.peek() else {
                return false; // unterminated
            };
            if ch == ']' {
                self.pos += 1;
                return true;
            }
            let Some(lo) = self.class_atom() else {
                return false;
            };
            if self.peek() == Some('-') && self.s.get(self.pos + 1) != Some(&']') {
                match lo {
                    Escape::Char(lo_char) => {
                        self.pos += 1; // `-`
                        let Some(hi) = self.class_atom() else {
                            return false;
                        };
                        match hi {
                            Escape::Char(hi_char) if lo_char <= hi_char => {}
                            _ => return false,
                        }
                    }
                    // `[\d-x]`: a set before `-` is a range error in unicode
                    // mode and three plain atoms in annex B.
                    Escape::Set => {
                        if self.unicode {
                            return false;
                        }
                        self.pos += 1;
                    }
                    Escape::Anchor => return false,
                }
            }
        }
    }

    /// One class atom: an escape or a literal character.
    fn class_atom(&mut self) -> Option<Escape> {
        match self.bump()? {
            '\\' => {
                let atom = self.escape(true)?;
                Some(match atom {
                    Escape::Anchor => Escape::Char(0x8), // class `\b` = backspace
                    other => other,
                })
            }
            ch => Some(Escape::Char(u32::from(ch))),
        }
    }

    /// One `\` escape (the cursor sits past the backslash); `None` is the
    /// `FormatException` face. `in_class` adjusts the `\b`/`\B`/digit faces.
    /// `Char` carries the escape's real codepoint — class-range ordering
    /// compares actual values.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per escape mirrors the official FormatException \
                  decision order; splitting it would scatter that pinned table"
    )]
    fn escape(&mut self, in_class: bool) -> Option<Escape> {
        match self.bump()? {
            'f' => Some(Escape::Char(0xc)),
            'n' => Some(Escape::Char(0xa)),
            'r' => Some(Escape::Char(0xd)),
            't' => Some(Escape::Char(0x9)),
            'v' => Some(Escape::Char(0xb)),
            'b' if in_class => Some(Escape::Char(0x8)),
            'B' if in_class && self.unicode => None,
            'b' | 'B' => Some(Escape::Anchor),
            'c' => {
                match self.peek() {
                    Some(ch) if ch.is_ascii_alphabetic() => {
                        self.pos += 1;
                        return Some(Escape::Char(u32::from(ch) & 0x1f));
                    }
                    // Annex-B legacy control letters include digits and `_`.
                    Some(ch @ ('0'..='9' | '_')) if !self.unicode => {
                        self.pos += 1;
                        return Some(Escape::Char(u32::from(ch) & 0x1f));
                    }
                    // `\c` alone / `\c<other>`: identity `c` in annex B, a
                    // syntax error in unicode mode.
                    Some(_) if !self.unicode => {}
                    _ => return None,
                }
                Some(Escape::Char(u32::from(b'c')))
            }
            'x' => match self.hex_value_run(2) {
                Some(value) => Some(Escape::Char(value)),
                None if !self.unicode => Some(Escape::Char(u32::from(b'x'))),
                None => None,
            },
            'u' if self.peek() == Some('{') => {
                if !self.unicode {
                    // Legacy: literal `u`; the brace content re-parses as
                    // plain pattern characters.
                    return Some(Escape::Char(u32::from(b'u')));
                }
                let value = self.brace_codepoint()?;
                char::from_u32(value)?;
                Some(Escape::Char(value))
            }
            'u' => match self.hex_value_run(4) {
                Some(value) => Some(Escape::Char(value)),
                None if !self.unicode => Some(Escape::Char(u32::from(b'u'))),
                None => None,
            },
            '0' => {
                if !self.unicode {
                    // Legacy octal continuation (annex B).
                    return Some(Escape::Char(self.octal_run(0)));
                } else if self.peek().is_some_and(|ch: char| ch.is_ascii_digit()) {
                    return None;
                }
                Some(Escape::Char(0))
            }
            '1'..='9' => {
                if in_class {
                    // Class digit escapes: legacy octal in annex B (`\8`/`\9`
                    // are the literal digits), none in unicode mode (no
                    // backreferences inside classes).
                    let first = self.s[self.pos - 1];
                    return match first {
                        '8' | '9' if !self.unicode => Some(Escape::Char(u32::from(first))),
                        _ if !self.unicode => {
                            Some(Escape::Char(self.octal_run(first.to_digit(8)?)))
                        }
                        _ => None,
                    };
                }
                if !self.unicode {
                    while self.peek().is_some_and(|ch: char| ch.is_ascii_digit()) {
                        self.pos += 1;
                    }
                    return Some(Escape::Char(0)); // backreference or legacy octal
                }
                let mut group = u64::from(self.s[self.pos - 1].to_digit(10)?);
                while let Some(digit) = self.peek().and_then(|ch| ch.to_digit(10)) {
                    group = group.saturating_mul(10).saturating_add(u64::from(digit));
                    self.pos += 1;
                }
                // A numeric escape is a backreference iff `N` does not exceed
                // the pattern's TOTAL capture count (regexp-parser.cc
                // ParseBackReferenceIndex): forward references and references
                // into still-open groups are legal, so the verdict waits for
                // the end of the walk (`is_valid`).
                let group = usize::try_from(group).ok()?;
                self.backrefs.push(group);
                Some(Escape::Char(0))
            }
            'd' | 'D' | 's' | 'S' | 'w' | 'W' => Some(Escape::Set),
            'p' | 'P' => {
                if !self.unicode {
                    return Some(Escape::Char(u32::from(b'p'))); // identity `p`
                }
                if self.peek() != Some('{') {
                    return None;
                }
                self.pos += 1;
                let start = self.pos;
                while self.peek().is_some_and(|ch| ch != '}') {
                    self.pos += 1;
                }
                if self.peek() != Some('}') || self.pos == start {
                    return None;
                }
                self.pos += 1;
                Some(Escape::Set)
            }
            'k' if !in_class => {
                if self.peek() == Some('<') {
                    let start = self.pos + 1;
                    let mut i = start;
                    while self.s.get(i).is_some_and(|ch| *ch != '>') {
                        i += 1;
                    }
                    if self.s.get(i) != Some(&'>') || i == start {
                        return None;
                    }
                    if self.unicode {
                        let name: String = self.s[start..i].iter().collect();
                        if !self.named.contains(&name) {
                            return None;
                        }
                    }
                    self.pos = i + 1;
                    Some(Escape::Char(0))
                } else {
                    (!self.unicode).then_some(Escape::Char(u32::from(b'k')))
                }
            }
            '-' if self.unicode => None,
            ch => {
                // Identity escapes: annex B accepts any character; unicode
                // mode only the syntax characters.
                if self.unicode
                    && !matches!(
                        ch,
                        '^' | '$'
                            | '\\'
                            | '.'
                            | '*'
                            | '+'
                            | '?'
                            | '('
                            | ')'
                            | '['
                            | ']'
                            | '{'
                            | '}'
                            | '|'
                            | '/'
                    )
                {
                    None
                } else {
                    Some(Escape::Char(u32::from(ch)))
                }
            }
        }
    }

    /// Consumes an annex-B octal escape run and returns its value; `first`
    /// is the already-consumed head digit.
    fn octal_run(&mut self, first: u32) -> u32 {
        let mut value = first;
        while let Some(digit) = self.peek().and_then(|ch| ch.to_digit(8)) {
            value = value * 8 + digit;
            self.pos += 1;
        }
        value
    }

    /// Consumes `width` hex digits and returns their value, `None` without
    /// consuming when fewer are present.
    fn hex_value_run(&mut self, width: usize) -> Option<u32> {
        let mut value = 0u32;
        for k in 0..width {
            let digit = self.s.get(self.pos + k).and_then(|ch| ch.to_digit(16))?;
            value = value * 16 + digit;
        }
        self.pos += width;
        Some(value)
    }

    /// The `{HexDigit+}` value of a unicode-mode `\u{...}` escape (the
    /// cursor sits on `{`); the closing `}` is consumed.
    fn brace_codepoint(&mut self) -> Option<u32> {
        let mut i = self.pos + 1;
        let mut value = 0u32;
        while let Some(digit) = self.s.get(i).and_then(|ch| ch.to_digit(16)) {
            value = value.checked_mul(16)?.checked_add(digit)?;
            i += 1;
        }
        if self.s.get(i) != Some(&'}') || i == self.pos + 1 {
            return None;
        }
        self.pos = i + 1;
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::is_valid;
    use crate::engine::narrow_u32;

    #[test]
    fn valid_and_invalid_shapes() {
        for source in ["", "abc", "a|b", "(a|b)*c", "[a-z]+", r"\b\w+\b"] {
            assert!(is_valid(source, false), "{source} should be valid");
        }
        for source in ["(", ")", "[", "a)", "*a", "+a", "?a", "a**", "[z-a]"] {
            assert!(!is_valid(source, false), "{source} should be invalid");
        }
    }

    #[test]
    fn quantifier_ordering_throws_in_both_modes() {
        assert!(!is_valid("a{2,1}", false));
        assert!(!is_valid("a{2,1}", true));
        assert!(is_valid("a{2,3}", false));
        assert!(is_valid("a{2,}", true));
    }

    #[test]
    fn annex_b_lenience_and_unicode_strictness() {
        assert!(is_valid(r"\q", false));
        assert!(!is_valid(r"\q", true));
        assert!(is_valid(r"\8", false));
        assert!(!is_valid(r"\8", true));
        assert!(is_valid("a{2", false));
        assert!(!is_valid("a{2", true));
        assert!(is_valid("^*", false));
        assert!(!is_valid("^*", true));
        assert!(is_valid("(?=a)*", false));
        assert!(!is_valid("(?=a)*", true));
        assert!(!is_valid("(?<=a)*", false));
        assert!(is_valid("}", false));
        assert!(!is_valid("}", true));
    }

    #[test]
    fn unicode_mode_escapes() {
        assert!(is_valid(r"\u{10ffff}", true));
        assert!(!is_valid(r"\u{110000}", true));
        assert!(is_valid(r"\u{110000}", false));
        assert!(is_valid(r"\p{L}", true));
        assert!(!is_valid(r"\pL", true));
        assert!(is_valid(r"\pL", false));
        assert!(is_valid(r"\-", false));
        assert!(!is_valid(r"\-", true));
        assert!(is_valid(r"(?<a>x)\k<a>", true));
        assert!(!is_valid(r"\k<a>(?<a>x)", true));
        assert!(is_valid(r"\1(a)", false));
        // `\1` is a backreference iff 1 <= the TOTAL capture count, so the
        // forward reference parses in unicode mode (regexp-parser.cc
        // ParseBackReferenceIndex).
        assert!(is_valid(r"\1(a)", true));
        assert!(is_valid(r"(a)\1", true));
        // Surrogate halves are valid `\uHHHH` in both modes.
        assert!(is_valid(r"\uD800", false));
        assert!(is_valid(r"\uD800", true));
    }

    #[test]
    fn unicode_backreferences_bound_by_total_captures() {
        // The VM parser accepts `\N` iff `N` does not exceed the pattern's
        // TOTAL capturing-group count (regexp-parser.cc
        // ParseBackReferenceIndex); only `N` beyond the total hits the
        // unicode-mode `kInvalidEscape` throw.
        assert!(is_valid(r"(a\1)", true)); // its own group is still open
        assert!(is_valid(r"((a))\1", true)); // nested captures, outer referenced
        assert!(is_valid(r"(?<a>(?<b>c))\1", true)); // nested named captures
        assert!(!is_valid(r"\2(a)", true)); // `N` exceeds the total
        assert!(!is_valid(r"(a)\2", true));
    }

    #[test]
    fn class_faces() {
        assert!(is_valid(r"[A-\x41]", false));
        assert!(!is_valid(r"[a-\x41]", false), "range out of order");
        assert!(!is_valid(r"[\d-a]", true));
        assert!(is_valid(r"[\d-a]", false));
        assert!(is_valid(r"[]]", false));
        assert!(is_valid(r"[^a-z0-9_]", true));
    }

    #[test]
    fn modifier_groups_and_named_duplicates() {
        assert!(is_valid("(?i:abc)", false));
        assert!(is_valid("(?im-s:abc)", true));
        assert!(!is_valid("(?-:abc)", false));
        assert!(is_valid("(?<a>x)(?<a>y)", false));
    }

    // —— the rule against hand-built trees —————————————————————————————————
    // The rule reads the sibling-leaf call shape (the
    // `Send` leaf completes after its callee and `Arguments`) and the
    // `NewExpression` child shape. Fixtures mount the creation under an
    // `ExpressionStatement`.

    use crate::testsupport::{Fixture, NONE, identifier, one_rule};
    use dartq_ast::NodeKind;

    fn rule_hits(f: &Fixture) -> Vec<crate::engine::Lint> {
        f.run(&one_rule("valid_regexps"))
    }

    /// The implicit-call face: `ExpressionStatement[Identifier, Arguments[
    /// literal], Send]`.
    fn call_fixture(source: &str) -> Fixture {
        // 0:var 1:s 2:= 3:callee 4:( 5:literal 6:) 7:;
        Fixture::build(source, |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::ExpressionStatement, 0);
            identifier(b, 3, 4);
            b.begin(NodeKind::Arguments, 4);
            b.leaf_span(NodeKind::StringLiteral, 5, 6);
            b.end(NodeKind::Arguments, 7);
            b.leaf_span(NodeKind::Send, 3, 7);
            b.end(NodeKind::ExpressionStatement, 8);
            b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
        })
    }

    /// `RegExp('(')` reports at the literal (the official `test_invalid`).
    #[test]
    fn unbalanced_group_reports() {
        let f = call_fixture("var s = RegExp('(');");
        let lints = rule_hits(&f);
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Invalid regular expression syntax.");
        assert_eq!(
            lints[0].correction,
            Some("Try correcting the regular expression.".to_owned())
        );
        // The report sits on the string literal `'('` (bytes 15..18).
        assert_eq!(lints[0].range.start, 15);
        assert_eq!(lints[0].range.end, 18);
    }

    /// A well-formed pattern passes (`test_valid`).
    #[test]
    fn valid_pattern_passes() {
        let f = call_fixture("var s = RegExp('[(]');");
        assert!(rule_hits(&f).is_empty());
    }

    /// Interpolation skips the face — the value is unknown
    /// (`test_interpolation`): the literal spans four tokens.
    #[test]
    fn interpolated_source_skips() {
        // 3:RegExp 4:( 5:'( $ 6:$ 7:r 8:' 9:) 10:;
        let f = Fixture::build("var s = RegExp('( $r');", |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::ExpressionStatement, 0);
            identifier(b, 3, 4);
            b.begin(NodeKind::Arguments, 4);
            b.leaf_span(NodeKind::StringLiteral, 5, 9);
            b.end(NodeKind::Arguments, 10);
            b.leaf_span(NodeKind::Send, 3, 10);
            b.end(NodeKind::ExpressionStatement, 11);
            b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
        });
        assert!(rule_hits(&f).is_empty());
    }

    /// Non-string first arguments skip (`stringValue` is `null`).
    #[test]
    fn non_string_argument_skips() {
        // 3:RegExp 4:( 5:1 6:) 7:;
        let f = Fixture::build("var s = RegExp(1);", |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::ExpressionStatement, 0);
            identifier(b, 3, 4);
            b.begin(NodeKind::Arguments, 4);
            b.leaf(NodeKind::LiteralInt, 5);
            b.end(NodeKind::Arguments, 7);
            b.leaf_span(NodeKind::Send, 3, 7);
            b.end(NodeKind::ExpressionStatement, 8);
            b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
        });
        assert!(rule_hits(&f).is_empty());
    }

    /// Non-`RegExp` callees never match (the resolution boundary).
    #[test]
    fn other_callees_skips() {
        let f = call_fixture("var s = foo('(');");
        assert!(rule_hits(&f).is_empty());
    }

    /// An argument-less tear-off shape never matches.
    #[test]
    fn argumentless_send_skips() {
        let f = Fixture::build("var s = RegExp;", |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::ExpressionStatement, 0);
            identifier(b, 3, 4);
            b.leaf_span(NodeKind::Send, 3, 4);
            b.end(NodeKind::ExpressionStatement, 5);
            b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
        });
        assert!(rule_hits(&f).is_empty());
    }

    /// The `new` creation face dispatches through the `ConstructorReference`
    /// + `Arguments` children.
    #[test]
    fn new_expression_face_reports() {
        // 3:new 4:RegExp 5:( 6:'( 7:) 8:;
        let f = Fixture::build("var s = new RegExp('(');", |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::ExpressionStatement, 0);
            b.begin(NodeKind::NewExpression, 3);
            b.begin(NodeKind::ConstructorReference, 4);
            b.patch(0, NONE);
            b.patch(1, 0);
            identifier(b, 4, 5);
            b.end(NodeKind::ConstructorReference, 5);
            b.begin(NodeKind::Arguments, 5);
            b.leaf_span(NodeKind::StringLiteral, 6, 7);
            b.end(NodeKind::Arguments, 8);
            b.end(NodeKind::NewExpression, 8);
            b.end(NodeKind::ExpressionStatement, 9);
            b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
        });
        let lints = rule_hits(&f);
        assert_eq!(lints.len(), 1);
        // The literal `'('` sits at bytes 19..22.
        assert_eq!(lints[0].range.start, 19);
    }

    /// `unicode: true` flips the validator into strict mode, through the
    /// production bridge: the named argument flattens as
    /// `name value NamedArgument` pieces in the argument list and the name
    /// re-anchors two tokens back (`rules::slice`).
    #[test]
    fn unicode_flag_is_read() {
        let f = Fixture::parse("var s = RegExp(r'\\q', unicode: true);\n");
        assert_eq!(
            rule_hits(&f).len(),
            1,
            "unicode mode rejects the identity escape"
        );
        // Without the flag the same literal is a valid annex-B pattern.
        let lenient = Fixture::parse("var s = RegExp(r'\\q');\n");
        assert!(rule_hits(&lenient).is_empty());
    }

    /// A prefixed callee trails the same-span `DotAccess` marker — the
    /// conservative family stays silent (the official resolves the element
    /// to dart:core).
    #[test]
    fn prefixed_callee_skips() {
        let f = Fixture::parse("var s = core.RegExp('(');\n");
        assert!(rule_hits(&f).is_empty());
    }

    /// The callee head reads off the owner slice: a nested creation still
    /// reports (`print(RegExp('('))` — the head walk skips the outer
    /// argument list).
    #[test]
    fn nested_callee_reports() {
        let f = Fixture::parse("void f() { print(RegExp('(')); }\n");
        assert_eq!(rule_hits(&f).len(), 1);
    }
}
