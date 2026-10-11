//! The doc-comment family: `slash_for_doc_comments`,
//! `unintended_html_in_doc_comment`, `dangling_library_doc_comments`.
//!
//! Doc comments are the suffix run of doc-form comments (`///`-line or
//! `/**`-block) attached to a node's span-head token — the syntax-level
//! image of the analyzer's `documentationComment` (the comment chain
//! attached to the declaration's begin token, non-doc orphans excluded).
//!
//! Offsets are `u32` throughout the substrate and bounded by the source
//! length; the `usize` to `u32` casts below mirror the engine's own
//! convention.

#![allow(clippy::cast_possible_truncation)]

use crate::comments::Comment;
use dartq_ast::NodeKind;

use crate::engine::Check;
use crate::registry::{Group, RuleSpec};

/// The declaration kinds whose doc comments the doc-comment rules face —
/// the official registry of `slash_for_doc_comments.dart:59-82`
/// (`ClassTypeAlias` → `NamedMixinApplication`, `FieldDeclaration` →
/// `Fields`, `TopLevelVariableDeclaration` → `TopLevelFields`,
/// `FunctionDeclaration` → `TopLevelMethod`,
/// `FunctionDeclarationStatement` → `LocalFunctionDeclaration`, both alias
/// forms → `Typedef`; primary-constructor bodies are outside the current
/// substrate and skipped).
pub(crate) const DOC_KINDS: &[NodeKind] = &[
    NodeKind::ClassDeclaration,
    NodeKind::MixinDeclaration,
    NodeKind::NamedMixinApplication,
    NodeKind::ExtensionDeclaration,
    NodeKind::EnumDeclaration,
    NodeKind::EnumElement,
    NodeKind::Typedef,
    NodeKind::Constructor,
    NodeKind::Method,
    NodeKind::TopLevelMethod,
    NodeKind::LocalFunctionDeclaration,
    NodeKind::Fields,
    NodeKind::TopLevelFields,
    NodeKind::CompilationUnit,
];

pub(crate) mod slash_for_doc_comments {
    use super::{
        Check, DOC_KINDS, Group, NodeKind, RuleSpec, comment_lexeme, doc_group, group_span_utf16,
    };

    /// Official: `pkg/linter/lib/src/rules/slash_for_doc_comments.dart:26-30`
    /// — a Java-style (`/** ... */`) doc comment on a registered declaration
    /// reports at the whole doc comment. `isJavaStyle` (:22-24) checks the
    /// first doc token's lexeme. Message: `diagnostic.g.dart`
    /// `slashForDocComments`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "slash_for_doc_comments",
        group: Group::Recommended,
        problem: "Use the end-of-line form ('///') for doc comments.",
        correction: "Try rewriting the comment to use '///'.",
        kinds: DOC_KINDS,
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // The unit face checks only the first directive's doc (:66-70) —
        // leading comments on declarations belong to the declaration faces,
        // so the same group never reports twice.
        let target = if c.kind(node) == NodeKind::CompilationUnit {
            let first = c.direct_children(node).first().copied();
            match first.map(|kid| c.kind(kid)) {
                Some(
                    NodeKind::Import
                    | NodeKind::Export
                    | NodeKind::Part
                    | NodeKind::PartOf
                    | NodeKind::LibraryName,
                ) => first.expect("kind matched above"),
                _ => return,
            }
        } else {
            node
        };
        let docs = doc_group(c, target);
        let Some(first) = docs.first() else {
            return;
        };
        // `isJavaStyle` (:22-24): first doc token starts `/**`.
        if !comment_lexeme(c, first).starts_with("/**") {
            return;
        }
        let range = group_span_utf16(c, docs);
        c.report(range, &[]);
    }
}

pub(crate) mod unintended_html_in_doc_comment {
    use super::{Check, Comment, Group, NodeKind, RuleSpec, comment_lexeme, is_doc_form};

    /// Official: `pkg/linter/lib/src/rules/unintended_html_in_doc_comment.dart:118-186`
    /// — doc-comment lines are scanned for angle-bracket text that Markdown
    /// would treat as raw HTML, reporting each unintended tag at its
    /// `<`. The official single regex is replayed as one non-overlapping
    /// left-to-right pass with the same alternative order: escapes, code
    /// spans, autolinks, HTML comments/declarations/PIs/CDATA, `DartDoc`
    /// `[...]` links, whitelisted tags (`CommonMark` raw-HTML list, :17-125),
    /// and finally the invalid `</?tag…>` shape — the report. Lines inside a
    /// markdown code region are skipped, mirroring the analyzer's
    /// `codeBlocks` construction (`doc_comment_builder.dart`):
    /// fenced regions toggle on ``` lines (a documented approximation —
    /// no tick-count matching), and an indented code block opens after an
    /// empty line when the content is indented by four or more characters,
    /// lasting while it stays so. Remaining approximations: the official
    /// models no other container blocks (lists, quotes) either —
    /// `isPreviousLineEmpty` alone decides — and the scan faces
    /// declaration/directive doc groups (the official dispatch is the
    /// `Comment` AST node, i.e. documentation comments only). Message:
    /// `diagnostic.g.dart` `unintendedHtmlInDocComment`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "unintended_html_in_doc_comment",
        group: Group::Core,
        problem: "Angle brackets will be interpreted as HTML.",
        correction: "Try using backticks around the content with angle brackets, or try replacing `<` with `&lt;` and `>` with `&gt;`.",
        kinds: &[NodeKind::CompilationUnit],
        check,
    };

    fn check(c: &mut Check<'_>) {
        // The official face is per documentation comment
        // (`registry.addComment`, :135) — here, the run of consecutive
        // doc-form comments attached to one token. Each run scans exactly
        // once, its code-region state spanning every line of the run (the
        // official computes `node.codeBlocks` over the whole comment).
        let comments = c.comments.all();
        let mut idx = 0;
        while idx < comments.len() {
            if !is_doc_form(c, &comments[idx]) {
                idx += 1;
                continue;
            }
            let run_start = idx;
            let run_token = comments[idx].token;
            idx += 1;
            while idx < comments.len()
                && comments[idx].token == run_token
                && is_doc_form(c, &comments[idx])
            {
                idx += 1;
            }
            let run = &comments[run_start..idx];
            // Fast reject first (the official :120-122): most doc comments
            // contain no `<` at all.
            if !run.iter().any(|cm| comment_lexeme(c, cm).contains('<')) {
                continue;
            }
            scan_run(c, run);
        }
    }

    /// Scans one doc-comment run, skipping lines inside a markdown code
    /// region — the analyzer's `codeBlocks` construction
    /// (`doc_comment_builder.dart:_parseDocComment`, :270-308): fenced
    /// regions toggle on ``` lines (the documented approximation — no
    /// tick-count matching), and an indented code block opens after an
    /// empty line when the line content is indented by four or more
    /// characters, lasting while it stays so; its terminating line is
    /// re-dispatched as a normal line. The official models no other
    /// container blocks (lists, quotes) either — `isPreviousLineEmpty`
    /// alone decides — so a tightly indented list-item continuation is
    /// prose while a blank-line-separated indented block is code.
    fn scan_run(c: &mut Check<'_>, run: &[Comment]) {
        let mut fenced = false;
        let mut indented = false;
        // `isPreviousLineEmpty` (:273) — true before the first line.
        let mut prev_empty = true;
        for cm in run {
            let lexeme = comment_lexeme(c, cm);
            let comment_start = cm.range.start as usize;
            let block_form = matches!(cm.kind, crate::comments::CommentKind::Block);
            let mut line_start = 0usize;
            for (line_index, line) in lexeme.split('\n').enumerate() {
                let content = line_content(line);
                // Raw content emptiness; the builder counts a `/**` opening
                // line as empty so an indented block can start on the next
                // line (:298-302).
                let empty =
                    content.is_empty() || (block_form && line_index == 0 && content == "/**");
                let indent = content.len() - content.trim_start_matches([' ', '\t']).len();
                if fenced {
                    // Every line of a fenced block is code, the closing
                    // fence line included; the block leaves
                    // `isPreviousLineEmpty` false (:287-289).
                    if is_fence_line(line) {
                        fenced = false;
                    }
                } else if indented && indent >= 4 {
                    // Indented code block continuation (:560-563).
                } else if prev_empty && indent >= 4 {
                    // An indented code block opens after an empty line —
                    // the official dispatch checks this before the fence
                    // check (:279-285).
                    indented = true;
                } else {
                    if indented {
                        // The block's terminating line is re-dispatched as
                        // a normal line (:564-574).
                        indented = false;
                    }
                    if is_fence_line(line) {
                        fenced = true;
                        prev_empty = false;
                    } else {
                        if line.contains('<') {
                            for (start, end) in unintended_tags(line) {
                                let from = c.unit.lines.byte_to_utf16(
                                    c.unit.source,
                                    (comment_start + line_start + start) as u32,
                                );
                                let to = from + utf16_len(&line[start..end]);
                                c.report(from..to, &[]);
                            }
                        }
                        prev_empty = empty;
                    }
                }
                line_start += line.len() + 1; // +1 for the split `\n`
            }
        }
    }

    /// The markdown line content: the line after the comment marker
    /// (`///`, or leading whitespace plus `*` inside a block comment) and
    /// one optional space, with a trailing `\r` stripped — the analyzer's
    /// `_CharacterSequence` content
    /// (`doc_comment_builder.dart:982-1087`).
    fn line_content(line: &str) -> &str {
        let content = line.trim_start_matches([' ', '\t']);
        let content = content
            .strip_prefix("///")
            .or_else(|| content.strip_prefix('*'))
            .unwrap_or(content);
        let content = content.strip_prefix(' ').unwrap_or(content);
        content.strip_suffix('\r').unwrap_or(content)
    }

    /// A Markdown fence toggle: the line's content after the comment marker
    /// starts with a three-backtick fence (the opening/closing fence lines
    /// themselves are skipped along with the block).
    fn is_fence_line(line: &str) -> bool {
        let rest = line.trim_start();
        let content = rest
            .strip_prefix("///")
            .or_else(|| rest.strip_prefix("//"))
            .or_else(|| rest.strip_prefix("/*"))
            .or_else(|| rest.strip_prefix("*"))
            .unwrap_or(rest);
        content.trim_start().starts_with("```")
    }

    /// `CommonMark` raw-HTML tag names (`unintended_html_in_doc_comment.dart:17-125`),
    /// sorted for lookup.
    const VALID_TAGS: &[&str] = &[
        "a",
        "abbr",
        "address",
        "area",
        "article",
        "aside",
        "audio",
        "b",
        "bdi",
        "bdo",
        "blockquote",
        "br",
        "button",
        "canvas",
        "caption",
        "cite",
        "code",
        "col",
        "colgroup",
        "data",
        "datalist",
        "dd",
        "del",
        "dfn",
        "div",
        "dl",
        "dt",
        "em",
        "fieldset",
        "figcaption",
        "figure",
        "footer",
        "form",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "header",
        "hr",
        "i",
        "iframe",
        "img",
        "input",
        "ins",
        "kbd",
        "keygen",
        "label",
        "legend",
        "li",
        "link",
        "main",
        "map",
        "mark",
        "meta",
        "meter",
        "nav",
        "noscript",
        "object",
        "ol",
        "optgroup",
        "option",
        "output",
        "p",
        "param",
        "pre",
        "progress",
        "q",
        "s",
        "samp",
        "script",
        "section",
        "select",
        "small",
        "source",
        "span",
        "strong",
        "style",
        "sub",
        "sup",
        "table",
        "tbody",
        "td",
        "template",
        "textarea",
        "tfoot",
        "th",
        "thead",
        "time",
        "title",
        "tr",
        "track",
        "u",
        "ul",
        "var",
        "video",
        "wbr",
    ];

    /// One non-overlapping pass replaying the official `_markdownTokenPattern`
    /// alternative order (:135-176); returns the byte ranges of unintended
    /// tags (the regex's `nh` group matches).
    fn unintended_tags(line: &str) -> Vec<(usize, usize)> {
        let bytes = line.as_bytes();
        let mut tags = Vec::new();
        let mut p = 0usize;
        while p < bytes.len() {
            match bytes[p] {
                b'\\' if p + 1 < bytes.len() => p += 2, // `\.`, `\<`, `\\`
                b'`' => {
                    // Code span: a backtick run closes at a run of the same
                    // length; an unterminated run matches alone (atomic).
                    let open = run_len(bytes, p);
                    let close = find_run(bytes, p + open + 1, open);
                    p = match close {
                        Some(q) => q + open,
                        None => p + open,
                    };
                }
                b'<' => {
                    if let Some(end) = non_tag_angle(line, p) {
                        p = end;
                    } else if let Some(end) = valid_tag(bytes, p) {
                        p = end;
                    } else if let Some(end) = invalid_tag(bytes, p) {
                        tags.push((p, end));
                        p = end;
                    } else {
                        p += 1;
                    }
                }
                b'[' if p == 0 || bytes[p - 1] != b']' => {
                    // DartDoc link `[...]`: no `]` inside, not followed by
                    // `(` or `[` (:163-165).
                    match bytes[p + 1..].iter().position(|&b| b == b']') {
                        Some(rel) if !matches!(bytes.get(p + 1 + rel + 1), Some(b'(' | b'[')) => {
                            p = p + rel + 2;
                        }
                        _ => p += 1,
                    }
                }
                _ => p += 1,
            }
        }
        tags
    }

    fn run_len(bytes: &[u8], from: usize) -> usize {
        bytes[from..].iter().take_while(|&&b| b == b'`').count()
    }

    /// The first position ≥ `from` starting a backtick run of `n`.
    fn find_run(bytes: &[u8], from: usize, n: usize) -> Option<usize> {
        let mut q = from;
        while q + n <= bytes.len() {
            if bytes[q] == b'`' {
                if run_len(bytes, q) >= n {
                    return Some(q);
                }
                q += run_len(bytes, q);
            } else {
                q += 1;
            }
        }
        None
    }

    /// The non-tag `<…>` constructs, in regex order: autolink (:154-156),
    /// HTML comment (:158-159), declaration (:161-162), processing
    /// instruction (:164-165), CDATA (:167-168). Returns the position past
    /// the construct, or `None` when it does not match.
    fn non_tag_angle(line: &str, p: usize) -> Option<usize> {
        let bytes = line.as_bytes();
        let rest = &line[p..];
        // Autolink `<scheme:...>`: scheme `[a-z][a-z\d\-+.]+`, body without
        // whitespace/control/angle bytes, closed by `>`.
        if let Some(colon) = rest.find(':') {
            let scheme = &rest[1..colon];
            if scheme.len() >= 2
                && scheme.as_bytes()[0].is_ascii_alphabetic()
                && scheme.as_bytes()[1..]
                    .iter()
                    .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'+' | b'.'))
                && colon > 1
            {
                let body_end = rest[colon + 1..]
                    .bytes()
                    .position(|b| b <= b' ' || b == 0x7f || b == b'<' || b == b'>')
                    .map_or(rest.len(), |rel| colon + 1 + rel);
                if body_end < rest.len() && rest.as_bytes()[body_end] == b'>' {
                    return Some(p + body_end + 1);
                }
            }
        }
        if let Some(body) = rest.strip_prefix("<!--") {
            if body.starts_with("->") {
                return Some(p + 6);
            }
            return body.find("-->").map(|rel| p + 4 + rel + 3);
        }
        if bytes.get(p + 1) == Some(&b'!') {
            if matches!(bytes.get(p + 2), Some(&b) if b.is_ascii_alphabetic()) {
                // Declaration `<!x ... !>` (:161-162).
                return rest[2..].find("!>").map(|rel| p + 2 + rel + 2);
            }
            if rest[2..].to_ascii_lowercase().starts_with("[cdata") {
                return rest[2..].find("]>").map(|rel| p + 2 + rel + 2);
            }
            return None;
        }
        if bytes.get(p + 1) == Some(&b'?') {
            return rest[2..].find("?>").map(|rel| p + 2 + rel + 2);
        }
        None
    }

    /// A whitelisted tag with one of the accepted terminators (:170-176):
    /// `>`, `/>`, ` attrs>`, `</tag>`, `</tag attrs>`. Returns the position
    /// past it, or `None`.
    fn valid_tag(bytes: &[u8], p: usize) -> Option<usize> {
        let mut i = p + 1;
        let end_tag = bytes.get(i) == Some(&b'/');
        if end_tag {
            i += 1;
        }
        let name_start = i;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric()) {
            i += 1;
        }
        let name = &line_name(bytes, name_start, i);
        if name.is_empty() || !VALID_TAGS.iter().any(|tag| tag.eq_ignore_ascii_case(name)) {
            return None;
        }
        // Terminator alternatives, regex order: `/(?=et)>` (only for start
        // tags in practice — an end tag's `/` must be followed by another
        // `/`), `>`, or whitespace + anything until `>`.
        match bytes.get(i) {
            Some(b'>') => Some(i + 1),
            Some(b'/') if !end_tag && bytes.get(i + 1) == Some(&b'>') => Some(i + 2),
            Some(b' ' | b'\r' | b'\n' | b'\t') => bytes[i..]
                .iter()
                .position(|&b| b == b'>')
                .map(|rel| i + rel + 1),
            _ => None,
        }
    }

    fn line_name(bytes: &[u8], start: usize, end: usize) -> &str {
        core::str::from_utf8(&bytes[start..end]).unwrap_or("")
    }

    /// The invalid-tag shape `</?[a-z][^]*?>` (:178-181) — the report.
    fn invalid_tag(bytes: &[u8], p: usize) -> Option<usize> {
        let mut i = p + 1;
        if bytes.get(i) == Some(&b'/') {
            i += 1;
        }
        if !bytes.get(i).is_some_and(|&b| b.is_ascii_alphabetic()) {
            return None;
        }
        bytes[i..]
            .iter()
            .position(|&b| b == b'>')
            .map(|rel| i + rel + 1)
    }

    fn utf16_len(text: &str) -> u32 {
        text.chars().map(|ch| ch.len_utf16() as u32).sum()
    }
}

pub(crate) mod dangling_library_doc_comments {
    use super::{
        Check, Group, NodeKind, RuleSpec, blank_between, comment_span_utf16, group_span_utf16,
        is_doc_form, line_of,
    };

    /// Official: `pkg/linter/lib/src/rules/dangling_library_doc_comments.dart:42-134`
    /// — a doc comment that reads like library documentation reports. The
    /// unit faces: (1) a `library` or `part of` first directive silences
    /// everything (:46-55); (2) a doc comment on the first non-library
    /// directive reports at its begin (:57-66); (3) a comment-only file
    /// reports trailing doc comments at EOF (:69-81); (4) otherwise the
    /// first declaration's doc comment reports when a blank line separates
    /// it from the declaration — within the group (report at the comment
    /// before the gap, :95-110) or after it (report the whole group,
    /// :112-134). The "comments following the doc comment" walk runs over
    /// every comment attached to the declaration's begin token (:112-131).
    /// Message: `diagnostic.g.dart` `danglingLibraryDocComments`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "dangling_library_doc_comments",
        group: Group::Core,
        problem: "Dangling library doc comment.",
        correction: "Add a 'library' directive after the library comment.",
        kinds: &[NodeKind::CompilationUnit],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let kids = c.collect_direct(node);
        if let Some(&first) = kids.first() {
            // Face 1: a `library`/`part of` first directive silences
            // (:46-55); face 2: a doc comment on any other first directive
            // reports (:57-66).
            match c.kind(first) {
                NodeKind::LibraryName | NodeKind::PartOf => {}
                _ if is_directive(c, first) => {
                    if let Some(first_doc) = doc_start(c, first) {
                        let at = comment_span_utf16(c, first_doc).start;
                        c.report(at..at, &[]);
                    }
                }
                _ => check_first_declaration(c, first),
            }
            return;
        }
        // Face 3: a comment-only file reports doc comments attached to EOF
        // (:69-81).
        let eof = c.unit.tokens.len() as u32 - 1;
        for comment in c.comments.attached(eof as usize) {
            if is_doc_form(c, comment) {
                let at = comment_span_utf16(c, comment).start;
                c.report(at..at, &[]);
            }
        }
    }

    fn is_directive(c: &Check<'_>, node: u32) -> bool {
        matches!(
            c.kind(node),
            NodeKind::LibraryName
                | NodeKind::LibraryAugmentation
                | NodeKind::Part
                | NodeKind::PartOf
                | NodeKind::Import
                | NodeKind::Export
                | NodeKind::ScriptTag
        )
    }

    /// The first doc-form comment attached to `node`'s head token, if any.
    fn doc_start<'a>(c: &'a Check<'_>, node: u32) -> Option<&'a crate::comments::Comment> {
        c.comments
            .attached(c.head_token(node) as usize)
            .iter()
            .find(|comment| is_doc_form(c, comment))
    }

    fn check_first_declaration(c: &mut Check<'_>, node: u32) {
        let head = c.head_token(node);
        let comments = c.comments;
        let attached = comments.attached(head as usize);
        // The doc group is the suffix run of doc-form comments.
        let docs_end = attached
            .iter()
            .rposition(|comment| is_doc_form(c, comment))
            .map_or(0, |last| last + 1);
        if docs_end == 0 {
            return;
        }
        // Within-doc pairs: a blank reports at the comment before the gap
        // (:95-110).
        for pair in attached[..docs_end].windows(2) {
            if blank_between(c, &pair[0], &pair[1]) {
                let at = comment_span_utf16(c, &pair[0]).start;
                c.report(at..at, &[]);
                return;
            }
        }
        // From the last doc comment through the remaining attached comments
        // and finally the declaration token: any blank reports the whole
        // group (:112-134).
        let declaration_line = line_of(
            c,
            c.unit
                .tokens
                .get(head as usize)
                .expect("head token")
                .byte_offset as usize,
        );
        let mut previous = &attached[docs_end - 1];
        for comment in &attached[docs_end..] {
            if blank_between(c, previous, comment) {
                let range = group_span_utf16(c, &attached[..docs_end]);
                c.report(range, &[]);
                return;
            }
            previous = comment;
        }
        if declaration_line > line_of(c, previous.range.end as usize) + 1 {
            let range = group_span_utf16(c, &attached[..docs_end]);
            c.report(range, &[]);
        }
    }
}

fn blank_between(c: &Check<'_>, a: &Comment, b: &Comment) -> bool {
    line_of(c, b.range.start as usize) > line_of(c, a.range.end as usize) + 1
}

/// The whole doc-comment group span (first start .. last end) in UTF-16.
fn group_span_utf16(c: &Check<'_>, docs: &[Comment]) -> core::ops::Range<u32> {
    let start = comment_span_utf16(c, &docs[0]).start;
    let end = comment_span_utf16(c, &docs[docs.len() - 1]).end;
    start..end
}

fn comment_span_utf16(c: &Check<'_>, comment: &Comment) -> core::ops::Range<u32> {
    let from = c
        .unit
        .lines
        .byte_to_utf16(c.unit.source, comment.range.start);
    let to = c.unit.lines.byte_to_utf16(c.unit.source, comment.range.end);
    from..to
}

fn line_of(c: &Check<'_>, byte_offset: usize) -> u32 {
    c.unit.lines.byte_to_line_col(byte_offset as u32).0
}

fn comment_lexeme<'a>(c: &Check<'a>, comment: &Comment) -> &'a str {
    &c.unit.source[comment.range.start as usize..comment.range.end as usize]
}

/// `///`-line or `/**`-block — the analyzer's `DocumentationCommentToken`
/// shapes.
fn is_doc_form(c: &Check<'_>, comment: &Comment) -> bool {
    let lexeme = comment_lexeme(c, comment);
    match comment.kind {
        crate::comments::CommentKind::Line => lexeme.starts_with("///"),
        crate::comments::CommentKind::Block => lexeme.starts_with("/**"),
    }
}

/// The doc comment of `node`: the suffix run of doc-form comments attached
/// to its span-head token.
pub(crate) fn doc_group<'a>(c: &Check<'a>, node: u32) -> &'a [Comment] {
    let comments = c.comments;
    let attached = comments.attached(c.head_token(node) as usize);
    // The documentation comment is the trailing CONSECUTIVE run of doc-form
    // comments; non-doc orphans never join it.
    let start = attached
        .iter()
        .rposition(|comment| !is_doc_form(c, comment))
        .map_or(0, |last| last + 1);
    &attached[start..]
}

#[cfg(test)]
mod tests {
    use dartq_ast::NodeKind;

    use crate::testsupport::{Fixture, NONE, identifier, one_rule, unit_of};

    /// A `class A {}` unit — the doc-comment attachment target for the
    /// declaration faces.
    fn class_fixture(source: &str) -> Fixture {
        Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.end(NodeKind::DeclarationBody, 4);
                b.end(NodeKind::ClassDeclaration, 5);
                b.end(NodeKind::TopLevelDeclaration, 5);
            }),
        )
    }

    /// A `var x = 1;` top-level declaration (the first-declaration face).
    fn var_fixture(source: &str) -> Fixture {
        Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelFields, 0);
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, 0);
                b.begin(NodeKind::InitializedIdentifier, 1);
                b.end(NodeKind::InitializedIdentifier, 2);
                b.end(NodeKind::VariablesDeclaration, 3);
                b.end(NodeKind::TopLevelFields, 3);
            }),
        )
    }

    // —— slash_for_doc_comments ———————————————————————————————————————————

    /// `test/rules/slash_for_doc_comments_test.dart` `test_javaStyle`.
    #[test]
    fn slash_reports_java_style() {
        let f = class_fixture("/** doc */\nclass A {}");
        let lints = f.run(&one_rule("slash_for_doc_comments"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use the end-of-line form ('///') for doc comments."
        );
        assert_eq!(lints[0].range, 0..10);
    }

    /// The `///` form never reports (`test_doughStyle` family).
    #[test]
    fn slash_line_style_passes() {
        let f = class_fixture("/// doc\nclass A {}");
        assert!(f.run(&one_rule("slash_for_doc_comments")).is_empty());
    }

    /// Orphan (non-doc) comments do not join the doc group — the Java-style
    /// comment still reports at its own span.
    #[test]
    fn slash_skips_orphan_comments() {
        let f = class_fixture("// c\n/** doc */\nclass A {}");
        let lints = f.run(&one_rule("slash_for_doc_comments"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range, 5..15);
    }

    // —— unintended_html_in_doc_comment ———————————————————————————————————

    /// `test/rules/unintended_html_in_doc_comment_test.dart`
    /// `test_typeParameters` — `<List>` reports at its `<` with tag length.
    #[test]
    fn html_reports_a_non_html_tag() {
        let f = class_fixture("/// See <List>.\nclass A {}");
        let lints = f.run(&one_rule("unintended_html_in_doc_comment"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Angle brackets will be interpreted as HTML."
        );
        assert_eq!(lints[0].range, 8..14);
    }

    /// Whitelisted tags pass (`test_validHtmlTags`).
    #[test]
    fn html_valid_tags_pass() {
        let f = class_fixture("/// See <p> and <b>bold</b>.\nclass A {}");
        assert!(
            f.run(&one_rule("unintended_html_in_doc_comment"))
                .is_empty()
        );
    }

    /// A code span hides the brackets (`test_codeSpan`).
    #[test]
    fn html_code_span_passes() {
        let f = class_fixture("/// Use `<T>` here.\nclass A {}");
        assert!(
            f.run(&one_rule("unintended_html_in_doc_comment"))
                .is_empty()
        );
    }

    /// `DartDoc` `[...]` links pass (`test_squareBrackets`).
    #[test]
    fn html_dartdoc_link_passes() {
        let f = class_fixture("/// See [List<int>].\nclass A {}");
        assert!(
            f.run(&one_rule("unintended_html_in_doc_comment"))
                .is_empty()
        );
    }

    /// Autolinks pass (`test_autolinks`).
    #[test]
    fn html_autolink_passes() {
        let f = class_fixture("/// See <http://example.com>.\nclass A {}");
        assert!(
            f.run(&one_rule("unintended_html_in_doc_comment"))
                .is_empty()
        );
    }

    /// Fenced code blocks pass (official `codeBlocks`; the fence toggle is
    /// the documented approximation).
    #[test]
    fn html_fenced_block_passes() {
        let f = class_fixture("/// ```dart\n/// <T>\n/// ```\nclass A {}");
        assert!(
            f.run(&one_rule("unintended_html_in_doc_comment"))
                .is_empty()
        );
    }

    /// An indented code block after an empty line passes (official
    /// `codeBlocks` indented type, `test_codeBlock_indented`): the line
    /// content — marker and one space stripped — is indented by four.
    #[test]
    fn html_indented_block_passes() {
        let f = class_fixture("/// Example:\n///\n///     var x = a <T> b;\nclass A {}");
        assert!(
            f.run(&one_rule("unintended_html_in_doc_comment"))
                .is_empty()
        );
    }

    /// `isPreviousLineEmpty` starts true, so an indented line at the very
    /// start of a doc comment is code.
    #[test]
    fn html_indented_block_at_start_passes() {
        let f = class_fixture("///     var x = a <T> b;\nclass A {}");
        assert!(
            f.run(&one_rule("unintended_html_in_doc_comment"))
                .is_empty()
        );
    }

    /// An indented line with no preceding blank line is a list item's
    /// paragraph continuation, not code — the official models no list
    /// containers and `_isPreviousLineEmpty` decides, so the tag reports.
    #[test]
    fn html_indented_list_continuation_reports() {
        let f = class_fixture("/// - item:\n///     var x = a <T> b;\nclass A {}");
        let lints = f.run(&one_rule("unintended_html_in_doc_comment"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range, 30..33);
    }

    /// A blank-line-separated indented block after a list item is code —
    /// no report (the official `_parseIndentedCodeBlock` verdict).
    #[test]
    fn html_indented_after_blank_list_passes() {
        let f = class_fixture("/// - item:\n///\n///     var x = a <T> b;\nclass A {}");
        assert!(
            f.run(&one_rule("unintended_html_in_doc_comment"))
                .is_empty()
        );
    }

    /// Prose scanning resumes once the indented block's terminator line
    /// is re-dispatched (`_parseIndentedCodeBlock` returning the line).
    #[test]
    fn html_reports_after_indented_block_ends() {
        let f = class_fixture("///     var x = 1;\n/// Uses a <T> here.\nclass A {}");
        let lints = f.run(&one_rule("unintended_html_in_doc_comment"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range, 30..33);
    }

    /// A Java-style block doc comment's indented block passes (official
    /// `test_unintendedHtml_javaDoc_multiline_indented`): ` *` counts as an
    /// empty line and ` *     ` yields four content-space indentation.
    #[test]
    fn html_java_doc_indented_block_passes() {
        let f = class_fixture("/**\n * Text\n *\n *     List<int>\n *\n * Text\n */\nclass A {}");
        assert!(
            f.run(&one_rule("unintended_html_in_doc_comment"))
                .is_empty()
        );
    }

    /// The `/**` opening line counts as empty, so an indented block may
    /// start on the next line (official
    /// `test_unintendedHtml_javaDoc_multiline_indented_atStart`).
    #[test]
    fn html_java_doc_indented_at_start_passes() {
        let f = class_fixture("/**\n *     List<int>\n */\nclass A {}");
        assert!(
            f.run(&one_rule("unintended_html_in_doc_comment"))
                .is_empty()
        );
    }

    // —— dangling_library_doc_comments —————————————————————————————————————

    /// A blank line between the doc comment and the first declaration makes
    /// the doc dangling (`test_danglingDocComment`).
    #[test]
    fn dangling_blank_after_doc_reports() {
        let f = var_fixture("/// doc\n\nvar x = 1;");
        let lints = f.run(&one_rule("dangling_library_doc_comments"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Dangling library doc comment.");
        assert_eq!(lints[0].range, 0..7);
    }

    /// Attached directly to the declaration, the doc documents it — no
    /// report (`test_attachedDocComment`).
    #[test]
    fn dangling_attached_doc_passes() {
        let f = var_fixture("/// doc\nvar x = 1;");
        assert!(f.run(&one_rule("dangling_library_doc_comments")).is_empty());
    }

    /// A blank line within the doc group reports at the comment before the
    /// gap (`test_blankLineBetweenDocComments`).
    #[test]
    fn dangling_blank_within_group_reports_at_first() {
        let f = var_fixture("/// a\n\n/// b\nvar x = 1;");
        let lints = f.run(&one_rule("dangling_library_doc_comments"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range, 0..0);
    }

    /// A `library` directive silences everything (:46-49).
    #[test]
    fn dangling_library_directive_silences() {
        let f = Fixture::build(
            "library foo;\n\n/// doc\nvar x = 1;",
            unit_of(|b, _t| {
                b.begin(NodeKind::LibraryName, 0);
                b.patch(0, 0);
                identifier(b, 1, 2);
                b.end(NodeKind::LibraryName, 3);
                b.begin(NodeKind::TopLevelFields, 4);
                b.begin(NodeKind::VariablesDeclaration, 4);
                b.patch(0, 4);
                b.begin(NodeKind::InitializedIdentifier, 5);
                b.end(NodeKind::InitializedIdentifier, 6);
                b.end(NodeKind::VariablesDeclaration, 7);
                b.end(NodeKind::TopLevelFields, 7);
            }),
        );
        assert!(f.run(&one_rule("dangling_library_doc_comments")).is_empty());
    }

    /// A doc comment on the first non-library directive reports (:57-66).
    #[test]
    fn dangling_reports_first_directive_doc() {
        let f = Fixture::build(
            "/// d\nimport 'a.dart';\nvar x = 1;",
            unit_of(|b, _t| {
                b.begin(NodeKind::Import, 0);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.leaf_span(NodeKind::StringLiteral, 1, 2);
                b.end(NodeKind::Import, 3);
                b.begin(NodeKind::TopLevelFields, 4);
                b.begin(NodeKind::VariablesDeclaration, 4);
                b.patch(0, 4);
                b.begin(NodeKind::InitializedIdentifier, 5);
                b.end(NodeKind::InitializedIdentifier, 6);
                b.end(NodeKind::VariablesDeclaration, 7);
                b.end(NodeKind::TopLevelFields, 7);
            }),
        );
        let lints = f.run(&one_rule("dangling_library_doc_comments"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range, 0..0);
    }

    /// A `part of` file never reports (:50-55).
    #[test]
    fn dangling_part_of_silences() {
        let f = Fixture::build(
            "/// d\npart of foo;",
            unit_of(|b, _t| {
                b.begin(NodeKind::PartOf, 0);
                b.patch(0, 1);
                b.patch(1, 0);
                b.leaf_span(NodeKind::DottedName, 2, 3);
                b.end(NodeKind::PartOf, 4);
            }),
        );
        assert!(f.run(&one_rule("dangling_library_doc_comments")).is_empty());
    }

    /// A comment-only file reports trailing doc comments (:69-81).
    #[test]
    fn dangling_eof_only_reports() {
        let f = Fixture::build("/// d", unit_of(|_b, _t| {}));
        let lints = f.run(&one_rule("dangling_library_doc_comments"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range, 0..0);
    }

    /// A blank line between the doc group and later attached comments makes
    /// the doc look library-level (:112-131).
    #[test]
    fn dangling_chain_blank_reports_group() {
        let f = var_fixture("/// doc\n// c\n\nvar x = 1;");
        let lints = f.run(&one_rule("dangling_library_doc_comments"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range, 0..7);
    }
}
