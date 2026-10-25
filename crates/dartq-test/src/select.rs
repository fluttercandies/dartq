//! Suite-level selection, the runner's way: which suites `package:test`
//! never loads (`Loader.loadFile`). A suite is dropped when the exclude-tag
//! selector matches its `@Tags`, or when its `@TestOn` selector rejects the
//! VM platform of this host. Include tags filter tests, never suites: a
//! suite whose tags miss them still loads (and reports its load).
//!
//! The answer is three-valued: a suite this module cannot read with
//! certainty (an annotation argument that is not a plain literal, an
//! annotation name it cannot resolve) is [`Selection::Unknown`], and the
//! caller lets the runner decide.

use dartq_scanner::{Keyword, ScanConfig, TokenKind, TokenTable};
use dartq_source::FileTable;
use std::path::Path;

/// A boolean selector (`package:boolean_selector`): variables joined by
/// `!`, `&&`, `||`, `? :` and parentheses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    All,
    None,
    Variable(String),
    Not(Box<Selector>),
    And(Box<Selector>, Box<Selector>),
    Or(Box<Selector>, Box<Selector>),
    Conditional(Box<Selector>, Box<Selector>, Box<Selector>),
}

impl Selector {
    /// Parse a selector; `None` when the runner would reject it.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let tokens = lex(text)?;
        let mut parser = Parser { tokens: &tokens, at: 0 };
        let node = parser.conditional()?;
        (parser.at == tokens.len()).then_some(node)
    }

    /// Evaluate against the set of true variables.
    #[must_use]
    pub fn evaluate(&self, var: &dyn Fn(&str) -> bool) -> bool {
        match self {
            Self::All => true,
            Self::None => false,
            Self::Variable(name) => var(name),
            Self::Not(child) => !child.evaluate(var),
            Self::And(left, right) => left.evaluate(var) && right.evaluate(var),
            Self::Or(left, right) => left.evaluate(var) || right.evaluate(var),
            Self::Conditional(cond, yes, no) => {
                if cond.evaluate(var) { yes.evaluate(var) } else { no.evaluate(var) }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Ident(String),
    LParen,
    RParen,
    Question,
    Colon,
    Not,
    Or,
    And,
}

/// The selector scanner: whitespace, `//` and nestable `/* */` comments
/// between tokens; identifiers `[a-zA-Z_-][a-zA-Z0-9_-]*`.
fn lex(text: &str) -> Option<Vec<Tok>> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' | b'\n' => i += 1,
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let mut depth = 0usize;
                loop {
                    if bytes.get(i..i + 2) == Some(b"/*") {
                        depth += 1;
                        i += 2;
                    } else if bytes.get(i..i + 2) == Some(b"*/") {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else if i < bytes.len() {
                        i += 1;
                    } else {
                        return None;
                    }
                }
            }
            b'(' => (out.push(Tok::LParen), i += 1).1,
            b')' => (out.push(Tok::RParen), i += 1).1,
            b'?' => (out.push(Tok::Question), i += 1).1,
            b':' => (out.push(Tok::Colon), i += 1).1,
            b'!' => (out.push(Tok::Not), i += 1).1,
            b'|' if bytes.get(i + 1) == Some(&b'|') => (out.push(Tok::Or), i += 2).1,
            b'&' if bytes.get(i + 1) == Some(&b'&') => (out.push(Tok::And), i += 2).1,
            c if c.is_ascii_alphabetic() || c == b'_' || c == b'-' => {
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'-') {
                    i += 1;
                }
                out.push(Tok::Ident(text[start..i].to_owned()));
            }
            _ => return None,
        }
    }
    Some(out)
}

struct Parser<'t> {
    tokens: &'t [Tok],
    at: usize,
}

impl Parser<'_> {
    fn eat(&mut self, tok: &Tok) -> bool {
        if self.tokens.get(self.at) == Some(tok) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn conditional(&mut self) -> Option<Selector> {
        let cond = self.or()?;
        if !self.eat(&Tok::Question) {
            return Some(cond);
        }
        let yes = self.conditional()?;
        if !self.eat(&Tok::Colon) {
            return None;
        }
        let no = self.conditional()?;
        Some(Selector::Conditional(Box::new(cond), Box::new(yes), Box::new(no)))
    }

    fn or(&mut self) -> Option<Selector> {
        let left = self.and()?;
        if !self.eat(&Tok::Or) {
            return Some(left);
        }
        Some(Selector::Or(Box::new(left), Box::new(self.or()?)))
    }

    fn and(&mut self) -> Option<Selector> {
        let left = self.simple()?;
        if !self.eat(&Tok::And) {
            return Some(left);
        }
        Some(Selector::And(Box::new(left), Box::new(self.and()?)))
    }

    fn simple(&mut self) -> Option<Selector> {
        let tok = self.tokens.get(self.at)?.clone();
        self.at += 1;
        match tok {
            Tok::Not => Some(Selector::Not(Box::new(self.simple()?))),
            Tok::LParen => {
                let child = self.conditional()?;
                self.eat(&Tok::RParen).then_some(child)
            }
            Tok::Ident(name) => Some(Selector::Variable(name)),
            _ => None,
        }
    }
}

/// The tag selectors of one run: `-t`/`--tags` values intersected (each a
/// comma-separated list), `-x`/`--exclude-tags` values united. Only the
/// exclusion selects suites; the inclusion is parsed so a malformed value
/// is left to the runner to report.
#[derive(Debug, Clone)]
pub struct TagFilter {
    exclude: Selector,
}

impl TagFilter {
    /// `None` when a value is not a selector the runner accepts (it
    /// reports the error itself).
    #[must_use]
    pub fn new(include: &[String], exclude: &[String]) -> Option<Self> {
        let parts = |values: &[String]| -> Option<Vec<Selector>> {
            let mut seen: Vec<&str> = Vec::new();
            for value in values {
                for part in value.split(',') {
                    if !seen.contains(&part) {
                        seen.push(part);
                    }
                }
            }
            seen.into_iter().map(Selector::parse).collect()
        };
        parts(include)?;
        let exclude = parts(exclude)?
            .into_iter()
            .fold(Selector::None, |acc, next| if acc == Selector::None { next } else { Selector::Or(Box::new(acc), Box::new(next)) });
        Some(Self { exclude })
    }
}

/// Whether the runner loads a suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Loaded,
    Dropped,
    Unknown,
}

/// The suite-level annotations selection reads.
#[derive(Debug, Default, PartialEq, Eq)]
struct SuiteMeta {
    tags: Vec<String>,
    test_on: Option<String>,
}

/// [`select`] for every file, in parallel; an unreadable file is
/// [`Selection::Unknown`] (the runner reports it).
#[must_use]
pub fn select_all(files: &[std::path::PathBuf], filter: &TagFilter) -> Vec<Selection> {
    use rayon::prelude::*;
    files
        .par_iter()
        .map(|file| std::fs::read(file).map_or(Selection::Unknown, |bytes| select(&bytes, filter)))
        .collect()
}

/// Whether the runner loads the suite whose source is `bytes`, on the VM
/// of this host compiled to kernel.
#[must_use]
pub fn select(bytes: &[u8], filter: &TagFilter) -> Selection {
    // No annotation at all: no suite metadata.
    if !bytes.contains(&b'@') {
        return Selection::Loaded;
    }
    let Some(meta) = std::str::from_utf8(bytes).ok().and_then(suite_meta) else {
        return Selection::Unknown;
    };
    let has = |tag: &str| meta.tags.iter().any(|t| t == tag);
    if filter.exclude.evaluate(&has) {
        return Selection::Dropped;
    }
    match &meta.test_on {
        None => Selection::Loaded,
        Some(text) => match Selector::parse(text) {
            // An invalid `@TestOn` is a load error the runner reports.
            None => Selection::Unknown,
            Some(selector) if selector.evaluate(&vm_platform) => Selection::Loaded,
            Some(_) => Selection::Dropped,
        },
    }
}

/// `PlatformSelector.evaluate` for the VM runtime, kernel compiler and
/// this host's operating system.
fn vm_platform(variable: &str) -> bool {
    let os = if cfg!(target_os = "macos") {
        "mac-os"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "android") {
        "android"
    } else if cfg!(target_os = "ios") {
        "ios"
    } else {
        "linux"
    };
    match variable {
        "vm" | "kernel" | "dart-vm" => true,
        "posix" => os != "windows",
        v => v == os,
    }
}

/// The `@Tags` and `@TestOn` arguments of the annotations on the file's
/// first directive (where the runner reads suite metadata). `None` when an
/// annotation's name or arguments are not certain.
fn suite_meta(text: &str) -> Option<SuiteMeta> {
    let source = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut files = FileTable::new();
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig { enable_triple_shift: true, enable_augmentations: false },
        files.intern(Path::new("suite.dart")),
    );
    let tokens = &scanned.tokens;
    let slice = tokens.as_slice();
    let kind = |index: usize| slice.get(index).map_or(TokenKind::Eof, |token| token.kind);
    let mut index = usize::from(kind(0) == TokenKind::ScriptTag);
    let mut annotations: Vec<(Vec<&str>, Option<(usize, usize)>)> = Vec::new();
    while kind(index) == TokenKind::At {
        let mut names = Vec::new();
        let mut at = index + 1;
        loop {
            if !matches!(kind(at), TokenKind::Identifier | TokenKind::Keyword(_)) {
                return None;
            }
            names.push(tokens.lexeme(source, at));
            at += 1;
            if kind(at) != TokenKind::Period {
                break;
            }
            at += 1;
        }
        if kind(at) == TokenKind::Lt {
            at = tokens.matching_end(at)? + 1;
        }
        let args = if kind(at) == TokenKind::OpenParen {
            let end = tokens.matching_end(at)?;
            let range = (at + 1, end);
            at = end + 1;
            Some(range)
        } else {
            None
        };
        annotations.push((names, args));
        index = at;
    }
    // Annotations count only when they precede a directive.
    let directive = matches!(
        kind(index),
        TokenKind::Keyword(Keyword::Library | Keyword::Import | Keyword::Export | Keyword::Part)
    );
    if !directive || annotations.is_empty() {
        return Some(SuiteMeta::default());
    }
    let prefixes = import_prefixes(source, tokens, index);
    let mut meta = SuiteMeta::default();
    let mut seen_tags = false;
    for (names, args) in annotations {
        // `@Name`, `@prefix.Name`, `@Name.ctor`, `@prefix.Name.ctor`.
        let class = match names.as_slice() {
            [name] => *name,
            [first, second] if prefixes.contains(first) => *second,
            [first, _] => *first,
            [_, second, _] => *second,
            _ => return None,
        };
        match class {
            "Tags" => {
                if seen_tags {
                    return None;
                }
                seen_tags = true;
                meta.tags = string_list(source, tokens, args?)?;
            }
            "TestOn" => {
                if meta.test_on.is_some() {
                    return None;
                }
                let (start, end) = args?;
                meta.test_on = Some(single_string(source, tokens, start, end)?);
            }
            _ => {}
        }
    }
    Some(meta)
}

/// Prefixes of the `import … as p` directives from `at` on.
fn import_prefixes<'s>(source: &'s str, tokens: &TokenTable, mut at: usize) -> Vec<&'s str> {
    let slice = tokens.as_slice();
    let mut out = Vec::new();
    while at < slice.len() {
        match slice[at].kind {
            TokenKind::Keyword(Keyword::Library | Keyword::Import | Keyword::Export | Keyword::Part) => {
                let import = slice[at].kind == TokenKind::Keyword(Keyword::Import);
                while at < slice.len() && slice[at].kind != TokenKind::Semicolon {
                    if import
                        && slice[at].kind == TokenKind::Keyword(Keyword::As)
                        && slice.get(at + 1).is_some_and(|next| next.kind == TokenKind::Identifier)
                    {
                        out.push(tokens.lexeme(source, at + 1));
                    }
                    at += 1;
                }
                at += 1;
            }
            TokenKind::At => {
                at += 1;
                while at < slice.len()
                    && matches!(slice[at].kind, TokenKind::Identifier | TokenKind::Keyword(_) | TokenKind::Period)
                {
                    at += 1;
                }
                if at < slice.len() && slice[at].kind == TokenKind::OpenParen {
                    at = tokens.matching_end(at).map_or(slice.len(), |end| end + 1);
                }
            }
            _ => break,
        }
    }
    out
}

/// `[ 'a', "b", ]` (an optional `const` and type argument included) as
/// the tag names; `None` for anything but string literals.
fn string_list(source: &str, tokens: &TokenTable, (start, end): (usize, usize)) -> Option<Vec<String>> {
    let slice = tokens.as_slice();
    let mut at = start;
    if slice[at].kind == TokenKind::Keyword(Keyword::Const) {
        at += 1;
    }
    if slice.get(at)?.kind == TokenKind::Lt {
        at = tokens.matching_end(at)? + 1;
    }
    if slice.get(at)?.kind != TokenKind::OpenSquare {
        return None;
    }
    let close = tokens.matching_end(at)?;
    // The single positional argument, optionally followed by a comma.
    let after = close + 1;
    if !(after == end || (after + 1 == end && slice[after].kind == TokenKind::Comma)) {
        return None;
    }
    let mut out = Vec::new();
    let mut item = at + 1;
    while item < close {
        let mut next = item;
        while next < close && slice[next].kind != TokenKind::Comma {
            next += 1;
        }
        out.push(single_string(source, tokens, item, next)?);
        item = next + 1;
    }
    Some(out)
}

/// The value of the adjacent string literals filling `start..end` (an
/// optional trailing comma allowed); `None` for interpolation or escapes.
fn single_string(source: &str, tokens: &TokenTable, start: usize, end: usize) -> Option<String> {
    let slice = tokens.as_slice();
    let end = if end > start && slice[end - 1].kind == TokenKind::Comma { end - 1 } else { end };
    if start >= end {
        return None;
    }
    let mut out = String::new();
    for index in start..end {
        if slice[index].kind != TokenKind::String {
            return None;
        }
        out.push_str(literal(tokens.lexeme(source, index))?);
    }
    Some(out)
}

/// A single-line string literal's body; `None` for escapes, triple
/// quotes and unterminated forms.
fn literal(lexeme: &str) -> Option<&str> {
    let raw = lexeme.starts_with('r');
    let quoted = if raw { &lexeme[1..] } else { lexeme };
    let quote = quoted.chars().next()?;
    if !matches!(quote, '\'' | '"') || quoted.len() < 2 || !quoted.ends_with(quote) {
        return None;
    }
    let body = &quoted[1..quoted.len() - 1];
    if body.starts_with(quote) || (!raw && (body.contains('\\') || body.contains('$'))) {
        return None;
    }
    Some(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(include: &[&str], exclude: &[&str]) -> TagFilter {
        let own = |values: &[&str]| values.iter().map(|value| (*value).to_owned()).collect::<Vec<_>>();
        TagFilter::new(&own(include), &own(exclude)).expect("valid selectors")
    }

    #[test]
    fn selectors_parse_with_the_runner_precedence() {
        let sel = Selector::parse("a || b && !c").expect("parses");
        let truth = |set: &'static [&'static str]| move |v: &str| set.contains(&v);
        assert!(sel.evaluate(&truth(&["a"])));
        assert!(sel.evaluate(&truth(&["b"])));
        assert!(!sel.evaluate(&truth(&["b", "c"])));
        let cond = Selector::parse("(x ? y : z) /* c */ // tail").expect("parses");
        assert!(cond.evaluate(&truth(&["x", "y"])));
        assert!(cond.evaluate(&truth(&["z"])));
        assert!(Selector::parse("a &&").is_none());
        assert!(Selector::parse("a & b").is_none());
        assert!(Selector::parse("").is_none());
    }

    #[test]
    fn suite_tags_drive_the_drop() {
        let src = b"@Tags(['slow', \"db\"])\nlibrary;\nimport 'package:test/test.dart';\nvoid main() {}\n";
        assert_eq!(select(src, &filter(&[], &["slow"])), Selection::Dropped);
        assert_eq!(select(src, &filter(&[], &["fast"])), Selection::Loaded);
        assert_eq!(select(src, &filter(&[], &["db && !slow"])), Selection::Loaded);
        assert_eq!(select(src, &filter(&[], &["fast", "db"])), Selection::Dropped);
        // Include selectors filter tests, never suites.
        assert_eq!(select(src, &filter(&["fast"], &[])), Selection::Loaded);
        assert!(TagFilter::new(&["a &&".to_owned()], &[]).is_none());
    }

    #[test]
    fn test_on_is_evaluated_for_the_vm() {
        let src = |sel: &str| format!("@TestOn('{sel}')\nlibrary;\nvoid main() {{}}\n").into_bytes();
        let none = filter(&[], &[]);
        assert_eq!(select(&src("vm"), &none), Selection::Loaded);
        assert_eq!(select(&src("browser"), &none), Selection::Dropped);
        assert_eq!(select(&src("!js"), &none), Selection::Loaded);
        assert_eq!(select(&src("windows"), &none), if cfg!(windows) { Selection::Loaded } else { Selection::Dropped });
        assert_eq!(select(&src("bad selector!"), &none), Selection::Unknown);
    }

    #[test]
    fn uncertain_metadata_is_unknown() {
        let none = filter(&[], &["slow"]);
        assert_eq!(select(b"@Tags(kTags)\nlibrary;\nvoid main() {}\n", &none), Selection::Unknown);
        assert_eq!(select(b"@Tags(['a$b'])\nlibrary;\nvoid main() {}\n", &none), Selection::Unknown);
        assert_eq!(select(b"@Tags(['a'])\n@Tags(['b'])\nlibrary;\n", &none), Selection::Unknown);
        // Annotations on a declaration are not suite metadata.
        assert_eq!(select(b"@Tags(['slow'])\nvoid main() {}\n", &none), Selection::Loaded);
        // A prefixed annotation through an import prefix.
        let prefixed = b"@t.Tags(['slow'])\nlibrary;\nimport 'package:test/test.dart' as t;\nvoid main() {}\n";
        assert_eq!(select(prefixed, &none), Selection::Dropped);
    }
}
