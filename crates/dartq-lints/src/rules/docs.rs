//! The doc-comment family: `slash_for_doc_comments`,
//! `unintended_html_in_doc_comment`, `dangling_library_doc_comments`,
//! `document_ignores`, `flutter_style_todos`,
//! `missing_code_block_language_in_doc_comment`,
//! `public_member_api_docs`, `comment_references`,
//! `unnecessary_ignore`.
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
use crate::registry::RuleSpec;
use crate::rules::NONE_TOKEN;

/// The domain's incoming registrations: a new standalone rule appends its
/// `SPEC` reference here (name order) and joins the registry through the
/// aggregated tail — adding a rule touches only this file. Batch close
/// promotes the block into the registry's frozen skeleton together with
/// the pin-test wave list (`tests/registry_pin.rs`).
pub(crate) static INCOMING_RULES: &[&RuleSpec] = &[];

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
    use super::{Check, DOC_KINDS, NodeKind, RuleSpec, comment_lexeme, doc_group, group_span_utf16};

    /// Official: `pkg/linter/lib/src/rules/slash_for_doc_comments.dart:26-30`
    /// — a Java-style (`/** ... */`) doc comment on a registered declaration
    /// reports at the whole doc comment. `isJavaStyle` (:22-24) checks the
    /// first doc token's lexeme. Message: `diagnostic.g.dart`
    /// `slashForDocComments`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "slash_for_doc_comments",
    DOC_KINDS,
    check,
);

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
    use super::{Check, Comment, NodeKind, RuleSpec, comment_lexeme, is_doc_form};

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
    /// fenced regions open on a leading backtick run of three or more and
    /// close only when the closer satisfies the opener's tick count
    /// (`_fencedCodeBlockDelimiter` :244-257, `_parseFencedCodeBlock`
    /// :492-547), and an indented code block
    /// opens after an empty line when the content is indented by four or
    /// more characters,
    /// lasting while it stays so. Remaining approximations: the official
    /// models no other container blocks (lists, quotes) either —
    /// `isPreviousLineEmpty` alone decides — and the scan faces
    /// declaration/directive doc groups (the official dispatch is the
    /// `Comment` AST node, i.e. documentation comments only). Message:
    /// `diagnostic.g.dart` `unintendedHtmlInDocComment`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unintended_html_in_doc_comment",
    &[NodeKind::CompilationUnit],
    check,
);

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
    /// regions open on a leading backtick run of three or more
    /// (`_parseFencedCodeBlock` :492-510 counts the opener's ticks) and
    /// close on the line satisfying the opener's tick count
    /// (`_fencedCodeBlockDelimiter` :244-257), and an indented code block
    /// opens after an empty line when the line content is indented by four
    /// or more characters, lasting while it stays so; its terminating line is
    /// re-dispatched as a normal line. The official models no other
    /// container blocks (lists, quotes) either — `isPreviousLineEmpty`
    /// alone decides — so a tightly indented list-item continuation is
    /// prose while a blank-line-separated indented block is code.
    fn scan_run(c: &mut Check<'_>, run: &[Comment]) {
        let mut fenced = false;
        // The open fence's tick count (`tickCount`, :500-510); `0` = open
        // region.
        let mut fence_ticks = 0usize;
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
                    // `isPreviousLineEmpty` false (:287-289). The closer
                    // must satisfy the opener's tick count.
                    if fence_closes(line, fence_ticks) {
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
                    if let Some(ticks) = fence_opens(line) {
                        fenced = true;
                        fence_ticks = ticks;
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

    /// The line's fence content: after the comment marker (`///`, or the
    /// leading whitespace plus `*`/`/*` inside a block comment).
    fn fence_content(line: &str) -> &str {
        let rest = line.trim_start();
        rest.strip_prefix("///")
            .or_else(|| rest.strip_prefix("//"))
            .or_else(|| rest.strip_prefix("/*"))
            .or_else(|| rest.strip_prefix('*'))
            .unwrap_or(rest)
    }

    /// The fence a line opens, as its tick count — the leading backtick
    /// run after the whitespace (`_parseFencedCodeBlock` :500-510), `None`
    /// below three or when the line opens nothing. The whitespace face is
    /// the shared `isWhitespace` set (space, tab, carriage return within a
    /// line).
    fn fence_opens(line: &str) -> Option<usize> {
        let rest = fence_content(line)
            .trim_start_matches([' ', '\t', '\r'])
            .as_bytes();
        let ticks = rest.iter().take_while(|&&b| b == b'`').count();
        (ticks >= 3).then_some(ticks)
    }

    /// Whether the line closes a fence opened with `opener_ticks` — the
    /// upstream `_fencedCodeBlockDelimiter(content, minimumTickCount:)`
    /// (:244-257) compares a three-character substring against a backtick
    /// run of the opener's length, so only a three-tick opener ever closes
    /// (any other run length cannot equal the fixed 3-character window —
    /// the documented upstream face).
    fn fence_closes(line: &str, opener_ticks: usize) -> bool {
        if opener_ticks != 3 {
            return false;
        }
        let rest = fence_content(line)
            .trim_start_matches([' ', '\t', '\r'])
            .as_bytes();
        // A byte slice behind a length gate — no char-boundary hazard.
        rest.len() >= 3 && &rest[..3] == b"```"
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
            // `rest[2..]` sits past the ASCII `<!` pair — always a char
            // boundary; the same holds for the `<?` branch below.
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
    use super::{Check, Comment, NodeKind, RuleSpec, blank_between, comment_span_utf16, group_span_utf16, is_doc_form, line_of};

    /// Official: `pkg/linter/lib/src/rules/dangling_library_doc_comments.dart:42-134`
    /// — a doc comment that reads like library documentation reports. The
    /// unit faces: (1) a `library` or `part of` first directive silences
    /// everything (:46-55); (2) a doc comment on the first non-library
    /// directive reports at its begin (:57-66); (3) a comment-only file
    /// reports trailing doc comments at EOF (:69-81); (4) otherwise the
    /// first declaration's doc comment reports when a blank line separates
    /// it from the declaration — within the doc tokens (report at the
    /// comment before the gap, :83-97) or after them (report the doc
    /// comment, :99-139). The group anchors at `findDartDoc` — the first `///`
    /// (a later `/**`, then a `///` after it, re-anchor); plain `//`
    /// comments never join, and the span is the anchor's start through the
    /// last doc token's end. The "comments following the doc comment" walk
    /// runs over every comment attached to the declaration's begin token
    /// (:112-131).
    /// Message: `diagnostic.g.dart` `danglingLibraryDocComments`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "dangling_library_doc_comments",
    &[NodeKind::CompilationUnit],
    check,
);

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

    /// The doc comment of `node`: the official `findDartDoc` winner among
    /// the comments attached to its head token.
    fn doc_start<'a>(c: &'a Check<'_>, node: u32) -> Option<&'a crate::comments::Comment> {
        let attached = c.comments.attached(c.head_token(node) as usize);
        find_dart_doc(c, attached).map(|anchor| &attached[anchor])
    }

    /// The official `findDartDoc` winner (`parser_impl.dart:11546-11564`):
    /// the first `///` attached comment, unless a later `/**` block
    /// re-anchors (and a `///` after that re-anchors again); plain `//`
    /// comments never anchor. Returns the winner's index in `attached`.
    fn find_dart_doc(c: &Check<'_>, attached: &[Comment]) -> Option<usize> {
        let mut anchor: Option<usize> = None;
        let mut multiline = false;
        for (index, comment) in attached.iter().enumerate() {
            if !is_doc_form(c, comment) {
                continue;
            }
            if matches!(comment.kind, crate::comments::CommentKind::Block) {
                anchor = Some(index);
                multiline = false;
            } else if !multiline {
                anchor = Some(index);
                multiline = true;
            }
        }
        anchor
    }

    fn check_first_declaration(c: &mut Check<'_>, node: u32) {
        let head = c.head_token(node);
        let comments = c.comments;
        let attached = comments.attached(head as usize);
        // The doc group anchors at the `findDartDoc` winner; plain `//`
        // comments never anchor and never join the group — they ride the
        // attached chain before, between, or after the doc tokens. The
        // group ends at the last doc-form comment, and its span runs from
        // the anchor's start to that last doc token's end
        // (`reportAtNode(docComment)`).
        let Some(anchor) = find_dart_doc(c, attached) else {
            return;
        };
        let docs_end = attached
            .iter()
            .rposition(|comment| is_doc_form(c, comment))
            .map_or(0, |last| last + 1);
        // Within-doc pairs over the doc tokens only (`docComment.tokens`): a
        // blank reports at the comment before the gap (:95-110).
        let mut previous: Option<&Comment> = None;
        for comment in &attached[anchor..docs_end] {
            if !is_doc_form(c, comment) {
                continue;
            }
            if let Some(before) = previous
                && blank_between(c, before, comment)
            {
                let at = comment_span_utf16(c, before).start;
                c.report(at..at, &[]);
                return;
            }
            previous = Some(comment);
        }
        // From the last doc comment through the remaining attached comments
        // and finally the declaration token: any blank reports the whole
        // group (:112-134).
        let Some(declaration_offset) = c.token(head).map(|t| t.byte_offset as usize) else {
            return;
        };
        let declaration_line = line_of(c, declaration_offset);
        let mut previous = &attached[docs_end - 1];
        for comment in &attached[docs_end..] {
            if blank_between(c, previous, comment) {
                let range = group_span_utf16(c, &attached[anchor..docs_end]);
                c.report(range, &[]);
                return;
            }
            previous = comment;
        }
        if declaration_line > line_of(c, previous.range.end as usize) + 1 {
            let range = group_span_utf16(c, &attached[anchor..docs_end]);
            c.report(range, &[]);
        }
    }
}

pub(crate) mod document_ignores {
    use super::{Check, NodeKind, RuleSpec, comment_lexeme};

    /// Official: `pkg/linter/lib/src/rules/document_ignores.dart:59-104` —
    /// an `// ignore:`/`// ignore_for_file:` comment citing at least one
    /// diagnostic with no trailing free text reports unless the line above
    /// is itself a comment (the documentation face). The element walk is
    /// the analyzer's `CommentTokenExtension.ignoredElements`
    /// (`pkg/analyzer/lib/src/ignore_comments/ignore_info.dart`): a
    /// comma-separated diagnostic-name list whose unstructured remainder —
    /// the trailing [`IgnoredDiagnosticComment`] — documents the ignore.
    /// An empty citation (`// ignore:` or `// ignore: 42`) never reports.
    /// Message: `diagnostic.g.dart` `documentIgnores`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "document_ignores",
    &[NodeKind::CompilationUnit],
    check,
);

    fn check(c: &mut Check<'_>) {
        for comment in c.comments.all() {
            let lexeme = comment_lexeme(c, comment);
            if !is_ignore_comment(lexeme) && !is_ignore_for_file_comment(lexeme) {
                continue;
            }
            // `ignoredElements` empty (`// ignore:` with nothing cited) is
            // the analyzer's continue face; trailing free text (the last
            // element being an `IgnoredDiagnosticComment`) documents the
            // ignore in place (:66-71). An empty citation
            // (`ignoredElements.isEmpty`, :66-67) has nothing to document.
            let Some((cited, trailing_text)) = ignored_elements(lexeme) else {
                continue;
            };
            if !cited || trailing_text {
                continue;
            }
            // Only look at the line above when the ignore comment is not on
            // the first line (:80-95).
            let line = c.unit.lines.byte_to_line_col(comment.range.start).0;
            if line > 1 {
                // The scanner mints comment ranges at the leading `/` byte
                // — always a char boundary.
                let before = &c.unit.source[..comment.range.start as usize];
                if let Some(previous_line) = previous_line(before)
                    && starts_with_line_comment(previous_line)
                {
                    continue;
                }
            }
            let span = super::comment_span_utf16(c, comment);
            c.report(span, &[]);
        }
    }

    /// The start of the line preceding the text ending at `before`'s end —
    /// the `getOffsetOfLine(ignoreCommentLine - 2)` face. `None` when the
    /// ignore comment sits on the first line.
    fn previous_line(before: &str) -> Option<&str> {
        let line_end = before.rfind('\n')?;
        let start = before[..line_end].rfind('\n').map_or(0, |p| p + 1);
        Some(&before[start..line_end])
    }

    /// Optional whitespace then `//` — the
    /// `_startsWithEndOfLineComment` face (`isSpace` = space or tab).
    fn starts_with_line_comment(line: &str) -> bool {
        let trimmed = line.trim_start_matches([' ', '\t']);
        trimmed.starts_with("//")
    }

    /// `//`+slashes, then spaces, then `ignore:` — `IgnoreInfo
    /// .isIgnoreComment` (the space skip is `0x20` only there).
    fn is_ignore_comment(lexeme: &str) -> bool {
        ignore_marker(lexeme, "ignore:")
    }

    /// The `ignore_for_file:` shape of [`is_ignore_comment`].
    fn is_ignore_for_file_comment(lexeme: &str) -> bool {
        ignore_marker(lexeme, "ignore_for_file:")
    }

    fn ignore_marker(lexeme: &str, marker: &str) -> bool {
        let body = lexeme.trim_start_matches('/');
        if lexeme.len() - body.len() < 2 {
            return false;
        }
        let body = body.trim_start_matches(' ');
        body.starts_with(marker)
    }

    /// The citation walk of `CommentTokenExtension.ignoredElements`: the
    /// first element kind is irrelevant here, so the walk answers `(any
    /// element cited, last element is trailing text)`. `None` when the
    /// lexeme carries no citation list at all (no `:` — unreachable behind
    /// the marker gate, kept for totality).
    fn ignored_elements(lexeme: &str) -> Option<(bool, bool)> {
        let bytes = lexeme.as_bytes();
        let mut offset = lexeme.find(':')? + 1;
        let mut has_element = false;

        let skip_whitespace = |mut o: usize| -> usize {
            while o < bytes.len() && matches!(bytes[o], b' ' | b'\t' | b'\r' | b'\n') {
                o += 1;
            }
            o
        };
        let read_word = |mut o: usize| -> usize {
            if !bytes
                .get(o)
                .is_some_and(|&b| b.is_ascii_alphabetic() || b == b'_')
            {
                return o;
            }
            o += 1;
            while o < bytes.len() && (bytes[o].is_ascii_alphanumeric() || bytes[o] == b'_') {
                o += 1;
            }
            o
        };

        loop {
            offset = skip_whitespace(offset);
            if offset == bytes.len() {
                return Some((has_element, false));
            }
            let word_start = offset;
            offset = read_word(offset);
            if word_start == offset {
                // A non-word at the citation position: trailing text when
                // anything was cited before it, otherwise nothing.
                return Some((has_element, has_element));
            }
            let word = &lexeme[word_start..offset];
            if word.eq_ignore_ascii_case("type") {
                offset = skip_whitespace(offset);
                if offset == bytes.len() || bytes[offset] != b'=' {
                    return Some((has_element, false));
                }
                offset = skip_whitespace(offset + 1);
                if offset == bytes.len() {
                    return Some((has_element, false));
                }
                let type_start = offset;
                offset = read_word(offset);
                if type_start == offset {
                    return Some((has_element, has_element));
                }
                if offset < bytes.len()
                    && !matches!(bytes[offset], b' ' | b'\t' | b'\r' | b'\n' | b',')
                {
                    return Some((has_element, has_element));
                }
            } else {
                // A plugin-name-prefixed code (`plugin/code`) reads its
                // second word; either way the word must end at a space or
                // comma (`ignore: http://…` cites nothing).
                if bytes.get(offset) == Some(&b'/') {
                    offset = skip_whitespace(offset + 1);
                    if offset == bytes.len() {
                        return Some((has_element, false));
                    }
                    let name_start = offset;
                    offset = read_word(offset);
                    if name_start == offset {
                        return Some((has_element, has_element));
                    }
                }
                if offset < bytes.len()
                    && !matches!(bytes[offset], b' ' | b'\t' | b'\r' | b'\n' | b',')
                {
                    return Some((has_element, has_element));
                }
            }
            has_element = true;
            if offset == bytes.len() {
                return Some((has_element, false));
            }
            offset = skip_whitespace(offset);
            if offset == bytes.len() {
                return Some((has_element, false));
            }
            if bytes[offset] != b',' {
                // Unstructured comment text follows the citations.
                return Some((has_element, true));
            }
            offset += 1;
        }
    }
}

pub(crate) mod flutter_style_todos {
    use super::{Check, NodeKind, RuleSpec, comment_lexeme};

    /// Official: `pkg/linter/lib/src/rules/flutter_style_todos.dart:24-84`
    /// — a comment whose lexeme starts with the trigger face (`//+`, one
    /// regex-`\s` run, `TODO` case-insensitive, word boundary) but not the
    /// expected face (`//`, one `\s` run, `TODO`, `(<username>)`, `: `)
    /// reports at the whole comment. Both regexes anchor at index 0
    /// (`String.startsWith`), so a `TODO` after other text never fires and
    /// `/// TODO(…)…` reports (the expected `//` cannot cross the third
    /// slash). Message: `diagnostic.g.dart` `flutterStyleTodos`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "flutter_style_todos",
    &[NodeKind::CompilationUnit],
    check,
);

    fn check(c: &mut Check<'_>) {
        for comment in c.comments.all() {
            let lexeme = comment_lexeme(c, comment);
            if invalid_todo(lexeme) {
                let span = super::comment_span_utf16(c, comment);
                c.report(span, &[]);
            }
        }
    }

    /// The official `invalidTodo` (:41-43): the trigger matches and the
    /// expected shape does not.
    fn invalid_todo(content: &str) -> bool {
        matches_trigger(content) && !matches_expected(content)
    }

    /// `_todoRegExp = //+\s*TODO\b`, case-insensitive. The greedy
    /// slash/space runs need no backtracking: `TODO` starts with neither
    /// character, so the maximal-run assignment is the only candidate.
    fn matches_trigger(content: &str) -> bool {
        let slash_run = content.len() - content.trim_start_matches('/').len();
        if slash_run < 2 {
            return false;
        }
        let rest = &content[slash_run..];
        let ws = rest
            .find(|ch: char| !is_regex_space(ch))
            .unwrap_or(rest.len());
        let rest = &rest[ws..];
        // The literal matches four ASCII bytes; the byte form keeps
        // multi-byte comments panic-free (a leading multi-byte character
        // never matches — its first byte is not `T`), and `rest[4..]`
        // below is boundary-safe only behind this gate.
        let Some(head) = rest.as_bytes().get(..4) else {
            return false;
        };
        if !head.eq_ignore_ascii_case(b"todo") {
            return false;
        }
        // The `\b` tail: any following word character re-joins `TODO`.
        rest[4..]
            .chars()
            .next()
            .is_none_or(|ch| !ch.is_ascii_alphanumeric() && ch != '_')
    }

    /// `_todoExpectedRegExp = //\s*TODO\([a-zA-Z0-9][-a-zA-Z0-9\.]*\): `,
    /// case-sensitive.
    fn matches_expected(content: &str) -> bool {
        let Some(rest) = content.strip_prefix("//") else {
            return false;
        };
        let ws = rest
            .find(|ch: char| !is_regex_space(ch))
            .unwrap_or(rest.len());
        let rest = &rest[ws..];
        let Some(rest) = rest.strip_prefix("TODO") else {
            return false;
        };
        let Some(rest) = rest.strip_prefix('(') else {
            return false;
        };
        let mut chars = rest.char_indices();
        if !chars
            .next()
            .is_some_and(|(_, ch)| ch.is_ascii_alphanumeric())
        {
            return false;
        }
        let end = rest
            .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '.'))
            .unwrap_or(rest.len());
        let rest = &rest[end..];
        rest.starts_with("): ")
    }

    /// The ECMA-262 `\s` set behind the two regexes.
    fn is_regex_space(ch: char) -> bool {
        matches!(
            ch,
            ' ' | '\t' | '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{A0}' | '\u{1680}' | '\u{2000}'
                ..='\u{200A}'
                    | '\u{2028}'
                    | '\u{2029}'
                    | '\u{202F}'
                    | '\u{205F}'
                    | '\u{3000}'
                    | '\u{FEFF}'
        )
    }
}

pub(crate) mod missing_code_block_language_in_doc_comment {
    use super::{Check, Comment, NodeKind, RuleSpec, is_doc_form};

    /// Official: `pkg/linter/lib/src/rules/missing_code_block_language_in_doc_comment.dart:24-58`
    /// — every FENCED doc-comment code block whose info string is missing
    /// reports at its opening fence line. The block model is the analyzer's
    /// `doc_comment_builder` (`pkg/analyzer/lib/src/fasta/
    /// doc_comment_builder.dart`): a fence line is whitespace then a tick
    /// run (the tick run counts; a run of four or more can never close,
    /// `substring(index, index + 3)` vs the longer run — mirrored); the
    /// info string is everything after the ticks, `null` when only
    /// whitespace remains; indented blocks (four-space runs after an empty
    /// line) never report and shadow fence parsing while they last. The
    /// report anchor is the opening line's content span (marker and one
    /// space stripped), whose length covers the whole line. Lines inside a
    /// markdown code region never open blocks. Message:
    /// `diagnostic.g.dart` `missingCodeBlockLanguageInDocComment`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "missing_code_block_language_in_doc_comment",
    &[NodeKind::CompilationUnit],
    check,
);

    fn check(c: &mut Check<'_>) {
        // The official face is per documentation comment (`registry
        // .addComment`): the run of consecutive doc-form comments attached
        // to one token, its code-region state spanning every line.
        let comments = c.comments.all();
        let mut idx = 0;
        while idx < comments.len() {
            if !is_doc_form(c, &comments[idx]) {
                idx += 1;
                continue;
            }
            let run_token = comments[idx].token;
            let run_start = idx;
            idx += 1;
            while idx < comments.len()
                && comments[idx].token == run_token
                && is_doc_form(c, &comments[idx])
            {
                idx += 1;
            }
            scan_run(c, &comments[run_start..idx]);
        }
    }

    /// One doc-comment run's line walk: the `_parseDocComment` dispatch
    /// order (indented block only after an empty line, then fence state)
    /// with the fenced-only report face.
    fn scan_run(c: &mut Check<'_>, run: &[Comment]) {
        // A copy of the source reference — decoupled from the `&mut`
        // borrow, so line slices never hold `c` across a report.
        let source = c.unit.source;
        let mut in_block_ticks: Option<usize> = None;
        let mut in_indented = false;
        // `isPreviousLineEmpty` starts true; a `/**` opening line counts as
        // empty so an indented block may start on the next line.
        let mut prev_empty = true;
        for comment in run {
            for (offset, content) in comment_lines(source, comment) {
                let ws_end = whitespace_end(content);
                if let Some(ticks) = in_block_ticks {
                    // The closing fence needs a run of the opener's tick
                    // count; four or more never closes (see SPEC doc).
                    if ticks <= 3 && fenced_delimiter(content, ticks).is_some() {
                        in_block_ticks = None;
                    }
                    continue;
                }
                if prev_empty && ws_end >= 4 {
                    in_indented = true;
                    prev_empty = false;
                    continue;
                }
                if in_indented {
                    if ws_end >= 4 {
                        continue;
                    }
                    // The terminating line is re-dispatched as a normal
                    // line (`_parseIndentedCodeBlock` returning it).
                    in_indented = false;
                }
                if let Some(at) = fenced_delimiter(content, 3) {
                    let ticks = content[at..].bytes().take_while(|&b| b == b'`').count();
                    let info = content[at + ticks..].trim();
                    if info.is_empty() {
                        // The anchor is the opening line's whole content span
                        // (`MdCodeBlockLine.offset/length`), whitespace
                        // included — the official corpus marks `  ``` `.
                        let from = c.unit.lines.byte_to_utf16(c.unit.source, offset as u32);
                        let to = from + utf16_len(content);
                        c.report(from..to, &[]);
                    }
                    in_block_ticks = Some(ticks);
                    prev_empty = false;
                    continue;
                }
                prev_empty = content.is_empty();
            }
        }
    }

    fn utf16_len(text: &str) -> u32 {
        text.chars().map(|ch| ch.len_utf16() as u32).sum()
    }

    /// Whitespace then a ` ``` ` run — `_fencedCodeBlockDelimiter`. The
    /// tick-run comparison is always three code units wide, so a required
    /// count above three can never match (the documented never-closing
    /// face); `minimum_tick` is the opener's count.
    fn fenced_delimiter(content: &str, minimum_tick: usize) -> Option<usize> {
        if minimum_tick > 3 {
            return None;
        }
        let ws = whitespace_end(content);
        if content.len() < ws + 3 {
            return None;
        }
        content.as_bytes()[ws..].starts_with(b"```").then_some(ws)
    }

    fn whitespace_end(content: &str) -> usize {
        content
            .find(|ch: char| !matches!(ch, ' ' | '\t' | '\r' | '\n'))
            .unwrap_or(content.len())
    }

    /// The `(content offset, content)` lines of one comment — the
    /// `_CharacterSequence` faces. Line comments strip `///` and one
    /// optional space; block comments strip leading whitespace, the `*`/
    /// `* ` markers, the `/**`/`*/` delimiters, and drop lines that are
    /// only the end-of-comment marker. The callers' doc-form gate anchors
    /// every fixed marker offset in ASCII.
    fn comment_lines<'a>(source: &'a str, comment: &Comment) -> Vec<(usize, &'a str)> {
        let base = comment.range.start as usize;
        let lexeme = &source[base..comment.range.end as usize];
        match comment.kind {
            crate::comments::CommentKind::Line => {
                let lexeme = lexeme.strip_suffix('\r').unwrap_or(lexeme);
                let prefix = usize::from(lexeme.starts_with("/// ")) + 3;
                vec![(base + prefix, &lexeme[prefix..])]
            }
            crate::comments::CommentKind::Block => {
                let mut lines = Vec::new();
                let raw_lines: Vec<&str> = lexeme.split('\n').collect();
                let last = raw_lines.len() - 1;
                let mut byte_at = base;
                for (nth, raw) in raw_lines.iter().enumerate() {
                    let mut line = if nth == last {
                        raw.strip_suffix('\r').unwrap_or(raw)
                    } else {
                        raw
                    };
                    let mut start = byte_at;
                    if nth == 0 {
                        line = line.strip_prefix("/**").unwrap_or(line);
                        start += 3;
                    }
                    if nth == last
                        && let Some(stripped) = line.strip_suffix("*/")
                    {
                        line = stripped;
                    }
                    // Leading whitespace, then the `* `/`*` marker — the
                    // star-space walk of `_CharacterSequenceFromMultiLine
                    // Comment`.
                    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
                    let line = &line[indent..];
                    let marker = if let Some(rest) = line.strip_prefix("* ") {
                        start += indent + 2;
                        rest
                    } else if line == "*" {
                        start += indent + 1;
                        ""
                    } else {
                        start += indent;
                        line
                    };
                    let empty_marker_only = marker.is_empty();
                    if !(empty_marker_only && (nth == 0 || nth == last)) {
                        lines.push((start, marker));
                    }
                    byte_at += raw.len() + 1;
                }
                lines
            }
        }
    }
}

pub(crate) mod public_member_api_docs {
    use super::{Check, NodeKind, RuleSpec, is_doc_form};
    use crate::rules::{MemberQuery, NONE_TOKEN, body_members, get_set_token, initialized_identifier_name, member_annotations, member_matches, member_name, same_file_ancestors, utf16_token_span};

    /// Official: `pkg/linter/lib/src/rules/public_member_api_docs.dart` — a
    /// public declaration or member without a preceding doc comment reports
    /// at its name (`check`, :63-72; the anchor is `getNodeToAnnotate`,
    /// `ast.dart:49-93` — the written name, falling to the constructor's
    /// type name for unnamed constructors). The registration gate is the
    /// package face (:37-40 — a package whose API cannot be public, and any
    /// file outside the package's `lib/`, never registers): the path face
    /// reads the `lib` component. Skips, per face: private
    /// (`_`-prefixed) names everywhere; the top-level `main` (:143); enum
    /// constructors (:181); effectively-private constructor owners (:182 —
    /// `@internal`, `sealed`, `abstract final` or `abstract interface`,
    /// `extensions.dart:127-141`); `@internal` declarations (:64,
    /// `extensions.dart:143-162` — the annotation on the enclosing
    /// top-level member, on the variable declaration itself for top-level
    /// variables); instance members overriding an inherited member (:66,
    /// :115-116) — the same-file extends/with/on/implements chain of the
    /// override-family face, with an explicit `@override` annotation
    /// standing in for the out-of-file half; invalid extension-type
    /// instance fields (:222). Factory constructors share the constructor
    /// face (:177-185 — upstream visits a `ConstructorDeclaration` for
    /// them too): the row heads at the `factory` keyword and the declared
    /// name follows it. Getter/setter pairing (:74-112): a setter mounts
    /// only on a same-name getter that actually reported — not
    /// `@internal`, not overriding, undocumented (class-body level for
    /// members, unit level for top-level accessors). Message:
    /// `diagnostic.g.dart` `publicMemberApiDocs`.
    ///
    /// Residual (documented): the `@internal` and override faces are
    /// syntactic — a member overriding an out-of-file declaration
    /// over-reports unless it is written with an explicit `@override`
    /// (the stand-in exemption), while the cross-file resolution faces
    /// (`canHavePublicApi`) never suppress. Report order follows the
    /// traversal, so an accessor pair written setter-first reports
    /// setter-then-getter (the official buckets getters before setters per
    /// container, :96-111). A doc comment exempts every variable of its
    /// `Fields`/`TopLevelFields` row (the substrate attaches the doc group
    /// at the row's head token). Primary constructors are outside the
    /// substrate (the [`DOC_KINDS`] note), so their two faces (:55-56,
    /// :251-291) stay unported.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "public_member_api_docs",
    &[
            NodeKind::ClassDeclaration,
            NodeKind::MixinDeclaration,
            NodeKind::NamedMixinApplication,
            NodeKind::ExtensionDeclaration,
            NodeKind::ExtensionTypeDeclaration,
            NodeKind::EnumDeclaration,
            NodeKind::EnumElement,
            NodeKind::Typedef,
            NodeKind::Constructor,
            NodeKind::Factory,
            NodeKind::TopLevelMethod,
            NodeKind::Method,
            NodeKind::Fields,
            NodeKind::TopLevelFields,
        ],
    check,
);

    fn check(c: &mut Check<'_>) {
        // The registration gate (:37-40): the official visitors mount only
        // inside a package's `lib/` directory.
        if !in_lib_dir(c) {
            return;
        }
        let node = c.node();
        match c.kind(node) {
            // The `_visitMembers` self faces (:119-122, :195-200, :203-209,
            // :212-215, :246-248); the members dispatch on their own rows.
            NodeKind::ClassDeclaration
            | NodeKind::MixinDeclaration
            | NodeKind::ExtensionTypeDeclaration
            | NodeKind::EnumDeclaration => visit_class_like(c, node),
            // (:203-209) an unnamed extension is private.
            NodeKind::ExtensionDeclaration => {
                if container_name_token(c, node).is_some() {
                    visit_class_like(c, node);
                }
            }
            // (:125-129, :232-243) the alias faces: the private-name gate
            // alone.
            NodeKind::NamedMixinApplication | NodeKind::Typedef => {
                if let Some(name) = container_name_token(c, node) {
                    check_declaration(c, node, name);
                }
            }
            // (:187-192)
            NodeKind::EnumElement => visit_enum_constant(c),
            // (:177-185) — a factory constructor is a
            // `ConstructorDeclaration` upstream too.
            NodeKind::Constructor | NodeKind::Factory => visit_constructor(c),
            // (:132-175) the unit-level function faces.
            NodeKind::TopLevelMethod => visit_top_level_function(c),
            // The `checkMethods` member faces (:82-91).
            NodeKind::Method => visit_method(c),
            // (:217-229, :293-300) the per-variable faces.
            NodeKind::Fields | NodeKind::TopLevelFields => visit_variables(c),
            _ => {}
        }
    }

    fn visit_class_like(c: &mut Check<'_>, node: u32) {
        let Some(name) = container_name_token(c, node) else {
            return;
        };
        // `_visitMembers` (:303): a private name skips the face — the
        // members' own dispatch re-derives it per row.
        if c.lexeme(name).starts_with('_') {
            return;
        }
        check_declaration(c, node, name);
    }

    fn visit_enum_constant(c: &mut Check<'_>) {
        let node = c.node();
        // (:189) `!node.inPrivateMember && !node.name.isPrivate`.
        if in_private_member(c) {
            return;
        }
        // The element row is childless: the bridge `handle_enum_element`
        // anchors the row at the name token and stores it as payload word 0
        // (the constant's name, `EnumConstantDeclaration.name`).
        let Some(name) = c.payload(node).first().copied() else {
            return;
        };
        if name == NONE_TOKEN || c.lexeme(name).starts_with('_') {
            return;
        }
        check_declaration(c, node, name);
    }

    fn visit_constructor(c: &mut Check<'_>) {
        let node = c.node();
        // (:179) `node.inPrivateMember || node.name.isPrivate` — `name` is
        // the declared name after the dot (`_` in `C._()`), absent for an
        // unnamed constructor. The row anchors at the type name (bridge
        // `begin_constructor` — the event's `name` is the first identifier),
        // so the named-constructor token sits two tokens right of the head;
        // a factory row heads at the `factory` keyword, its declared name
        // follows it.
        if in_private_member(c) {
            return;
        }
        let head = c.head_token(node);
        let anchor = if c.kind(node) == NodeKind::Factory {
            factory_name_token(c, head).unwrap_or(head)
        } else {
            declared_ctor_name(c, head).unwrap_or(head)
        };
        if c.lexeme(anchor).starts_with('_') {
            return;
        }
        // (:180-182) enum constructors never report; the owner's
        // effectively-private face skips. Mixins carry no constructors —
        // no owner, no report.
        let Some(&owner) = c.path.iter().rev().find(|&&n| {
            matches!(
                c.kind(n),
                NodeKind::ClassDeclaration | NodeKind::ExtensionTypeDeclaration
            )
        }) else {
            return;
        };
        if effectively_private_class(c, owner) {
            return;
        }
        if is_internal(c) {
            return;
        }
        let first = match declaration_star(c, node) {
            Some(star) => c.head_token(star),
            None => constructor_first_token(c, anchor),
        };
        if doc_attached(c, first) {
            return;
        }
        let range = utf16_token_span(c, anchor);
        c.report(range, &[]);
    }

    fn visit_top_level_function(c: &mut Check<'_>) {
        let node = c.node();
        let Some(name) = member_name_token(c, node) else {
            return;
        };
        let lexeme = c.lexeme(name);
        // (:143) `!name.isPrivate && name.lexeme != 'main'`.
        if lexeme.starts_with('_') || lexeme == "main" {
            return;
        }
        if is_internal(c) {
            return;
        }
        // (:164-169): a setter mounts only on a same-name getter that
        // actually reported.
        if get_set_token(c, node) == Some("set") {
            let getter_missing = unit_functions(c).into_iter().any(|function| {
                c.kind(function) == NodeKind::TopLevelMethod
                    && get_set_token(c, function) == Some("get")
                    && member_name_token(c, function).is_some_and(|token| {
                        c.lexeme(token) == lexeme
                            // (:94-108) the getter's own `check` answered
                            // first — an `@internal` getter never entered
                            // `missingDocs`. (Top-level functions never
                            // override.)
                            && !getter_internal(c, function)
                            && !doc_attached(c, doc_token(c, function))
                    })
            });
            if !getter_missing {
                return;
            }
        }
        if doc_attached(c, doc_token(c, node)) {
            return;
        }
        // Top-level functions never override — no override face (:66).
        let range = utf16_token_span(c, name);
        c.report(range, &[]);
    }

    fn visit_method(c: &mut Check<'_>) {
        let node = c.node();
        let Some(name) = member_name_token(c, node) else {
            return;
        };
        // (:83) `!member.name.isPrivate`.
        if c.lexeme(name).starts_with('_') {
            return;
        }
        // The `_visitMembers` owner gate (:303-304) — `inPrivateMember`
        // (`extensions.dart:95-107`).
        if in_private_member(c) {
            return;
        }
        if is_internal(c) {
            return;
        }
        let get_set = get_set_token(c, node);
        // (:103-108): a setter mounts only on a same-name getter that
        // actually reported.
        if get_set == Some("set") {
            let Some(&owner) = c.path.iter().rev().find(|&&n| is_class_like(c.kind(n))) else {
                return;
            };
            let getter_missing = body_members(c, owner).into_iter().any(|member| {
                c.kind(member) == NodeKind::Method
                    && get_set_token(c, member) == Some("get")
                    && member_name_token(c, member).is_some_and(|token| {
                        c.lexeme(token) == c.lexeme(name)
                            // The getter's own `check` (:94-108) answered
                            // first: an `@internal` getter never reaches
                            // here (both accessors share the owner the
                            // `is_internal` face cleared), and an
                            // overriding or `@override`-annotated getter
                            // stays silent.
                            && !annotates_override(c, member)
                            && !doc_attached(c, doc_token(c, member))
                            && (is_static(c, member)
                                || !overrides_member(c, MemberQuery::Getter, c.lexeme(token)))
                    })
            });
            if !getter_missing {
                return;
            }
        }
        if doc_attached(c, doc_token(c, node)) {
            return;
        }
        // (:66, :115-116) `!isOverridingMember(node)` — a static member
        // never overrides (the header ModSet bit). The `@override`
        // spelling stands in for the out-of-file half of the resolution:
        // an annotated instance member claims its supertype and stays
        // silent.
        if !is_static(c, node) {
            if annotates_override(c, node) {
                return;
            }
            let query = match get_set {
                Some("get") => MemberQuery::Getter,
                Some("set") => MemberQuery::Setter,
                _ => MemberQuery::Method,
            };
            if overrides_member(c, query, c.lexeme(name)) {
                return;
            }
        }
        let range = utf16_token_span(c, name);
        c.report(range, &[]);
    }

    fn visit_variables(c: &mut Check<'_>) {
        let node = c.node();
        let is_top_level = c.kind(node) == NodeKind::TopLevelFields;
        if !is_top_level {
            // (:221) a private owner skips.
            if in_private_member(c) {
                return;
            }
            // (:222) invalid extension-type fields — the instance shape;
            // the static face reports (the extension-type tests' static
            // field case).
            if c.payload(node).first() == Some(&(dartq_parser::MemberKind::NonStaticField as u32))
                && c.path
                    .iter()
                    .any(|&n| c.kind(n) == NodeKind::ExtensionTypeDeclaration)
            {
                return;
            }
        }
        if is_internal(c) {
            return;
        }
        // The doc comment face reads the declaration row's doc group — the
        // substrate attaches it at the row's head token, shared by every
        // variable of the row.
        let documented = doc_attached(c, doc_token(c, node));
        let instance = !is_top_level
            && c.payload(node).first() == Some(&(dartq_parser::MemberKind::NonStaticField as u32));
        for var in variables(c, node) {
            let Some(name) = initialized_identifier_name(c, var) else {
                continue;
            };
            // (:224-228, :295-299) the per-variable private gate.
            if c.lexeme(name).starts_with('_') {
                continue;
            }
            if documented {
                continue;
            }
            // A field overrides through its getter namespace (:66); the
            // `@override` spelling is the out-of-file stand-in.
            if instance
                && (annotates_override(c, node)
                    || overrides_member(c, MemberQuery::Getter, c.lexeme(name)))
            {
                continue;
            }
            let range = utf16_token_span(c, name);
            c.report(range, &[]);
        }
    }

    /// The shared `check` (:63-72): an `@internal` (:64) or documented
    /// (:66) declaration passes; the rest reports at the name.
    fn check_declaration(c: &mut Check<'_>, node: u32, name: u32) {
        if is_internal(c) {
            return;
        }
        if doc_attached(c, doc_token(c, node)) {
            return;
        }
        let range = utf16_token_span(c, name);
        c.report(range, &[]);
    }

    /// The registration gate (`rule_context.dart` `isInLibDir`): the unit
    /// path carries a `lib` component.
    fn in_lib_dir(c: &Check<'_>) -> bool {
        c.unit
            .path
            .is_some_and(|path| path.components().any(|part| part.as_os_str() == "lib"))
    }

    /// `node.isInternal` (`extensions.dart:143-162`): an `@internal`
    /// annotation on the enclosing top-level member — for a top-level
    /// variable that is the variable declaration itself (:146-155), for
    /// every other node the `CompilationUnitMember` (:157-161). The walk
    /// answers both: the nearest wrapper-anchored row of a variable IS its
    /// declaration. The star rides beside the row in its container or
    /// beside the wrapper in the unit's children — both placements answer.
    fn is_internal(c: &Check<'_>) -> bool {
        for &ancestor in c.path[1..].iter().rev() {
            let Some(container) = c.unit.ast.parent(ancestor) else {
                continue;
            };
            if !matches!(
                c.kind(container),
                NodeKind::TopLevelDeclaration | NodeKind::CompilationUnit
            ) {
                continue;
            }
            if annotates_internal(c, container, ancestor) {
                return true;
            }
            if c.kind(container) == NodeKind::TopLevelDeclaration
                && let Some(unit) = c.unit.ast.parent(container)
                && annotates_internal(c, unit, container)
            {
                return true;
            }
            return false;
        }
        false
    }

    /// The `@internal` spelling across a member's metadata rows (the
    /// `metadata.hasInternal` face; the annotation reads by its written
    /// name — an import alias stays unseen).
    fn annotates_internal(c: &Check<'_>, container: u32, member: u32) -> bool {
        member_annotations(c, container, member)
            .iter()
            .any(|&meta| {
                c.collect_direct(meta).into_iter().any(|piece| {
                    c.kind(piece) == NodeKind::Identifier
                        && c.lexeme(c.head_token(piece)) == "internal"
                })
            })
    }

    /// The explicit `@override` spelling on a member row — the syntactic
    /// stand-in for `isOverridingMember`'s out-of-file half (the
    /// `MetadataStar` left sibling names the annotations, as in
    /// [`annotates_internal`]).
    fn annotates_override(c: &Check<'_>, member: u32) -> bool {
        left_star(c, member).is_some_and(|star| {
            c.collect_direct(star).into_iter().any(|meta| {
                c.collect_direct(meta).into_iter().any(|piece| {
                    c.kind(piece) == NodeKind::Identifier
                        && c.lexeme(c.head_token(piece)) == "override"
                })
            })
        })
    }

    /// The row-generic `isInternal` of a top-level function row
    /// (`extensions.dart:157-161`): the `@internal` annotation on the
    /// function's own `CompilationUnitMember` metadata — the star rides
    /// beside the row or its wrapper, as in the [`is_internal`] walk.
    fn getter_internal(c: &Check<'_>, row: u32) -> bool {
        let Some(parent) = c.unit.ast.parent(row) else {
            return false;
        };
        if c.kind(parent) == NodeKind::TopLevelDeclaration {
            if annotates_internal(c, parent, row) {
                return true;
            }
            return c
                .unit
                .ast
                .parent(parent)
                .is_some_and(|unit| annotates_internal(c, unit, parent));
        }
        if c.kind(parent) == NodeKind::CompilationUnit {
            return annotates_internal(c, parent, row);
        }
        false
    }

    /// `node.inPrivateMember` (`extensions.dart:95-107`): the nearest
    /// enclosing class-like names private — an unnamed extension counts
    /// private (:104).
    fn in_private_member(c: &Check<'_>) -> bool {
        let Some(&owner) = c.path.iter().rev().find(|&&n| is_class_like(c.kind(n))) else {
            return false;
        };
        let name = container_name_token(c, owner);
        match name {
            None => c.kind(owner) == NodeKind::ExtensionDeclaration,
            Some(token) => c.lexeme(token).starts_with('_'),
        }
    }

    /// `isEffectivelyPrivate` (`extensions.dart:127-141`) for the
    /// constructor owner: `@internal`, `sealed`, or `abstract final` /
    /// `abstract interface` (the class `ModSet` bits — bridge `mod_bits`).
    fn effectively_private_class(c: &Check<'_>, owner: u32) -> bool {
        const ABSTRACT: u16 = 1 << 2;
        const SEALED: u16 = 1 << 3;
        const INTERFACE: u16 = 1 << 5;
        const FINAL: u16 = 1 << 6;
        let bits = dartq_ast::mod_flags(c.unit.ast, owner);
        bits & SEALED != 0
            || (bits & ABSTRACT != 0 && bits & (FINAL | INTERFACE) != 0)
            || is_internal(c)
    }

    /// `isOverridingMember` (:115-116): the same-file ancestor chain
    /// answers the inherited-member face (cross-file supertypes resolve to
    /// `None` — the override-family face).
    fn overrides_member(c: &Check<'_>, query: MemberQuery, name: &str) -> bool {
        let Some(&owner) = c.path.iter().rev().find(|&&n| is_class_like(c.kind(n))) else {
            return false;
        };
        same_file_ancestors(c, owner, false)
            .into_iter()
            .any(|ancestor| {
                body_members(c, ancestor)
                    .into_iter()
                    .any(|member| !member_matches(c, member, query, name).is_empty())
            })
    }

    fn is_class_like(kind: NodeKind) -> bool {
        matches!(
            kind,
            NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::ExtensionDeclaration
                | NodeKind::ExtensionTypeDeclaration
        )
    }

    fn is_static(c: &Check<'_>, node: u32) -> bool {
        const STATIC_BIT: u16 = 1 << 3;
        dartq_ast::mod_flags(c.unit.ast, node) & STATIC_BIT != 0
    }

    /// The name token of a member row — the `node.name` face
    /// (`getNodeToAnnotate` anchors at the written name). The production row
    /// carries two `Identifier` children when a named return type precedes
    /// the member (`int get g`: `int` is a `TypeReference`, `g` is the
    /// `MethodDeclaration` name): the declared-name contexts answer first,
    /// so the type name never shadows the member name. An `OperatorName`
    /// names through its payload token (the head would hit the `operator`
    /// keyword); anything else falls back to [`member_name`]'s first
    /// identifier — the hand-built-tree face.
    fn member_name_token(c: &Check<'_>, member: u32) -> Option<u32> {
        let is_declared_name = |kid: u32| {
            matches!(c.kind(kid), NodeKind::OperatorName)
                || (c.kind(kid) == NodeKind::Identifier
                    && matches!(
                        c.payload(kid).first().copied(),
                        Some(ctx)
                            if ctx == dartq_parser::IdentifierContext::MethodDeclaration as u32
                                || ctx == dartq_parser::IdentifierContext::TopLevelFunctionDeclaration as u32
                                || ctx == dartq_parser::IdentifierContext::FieldDeclaration as u32
                    ))
        };
        let name = c
            .collect_direct(member)
            .into_iter()
            .find(|&kid| is_declared_name(kid))
            .or_else(|| member_name(c, member))?;
        Some(if c.kind(name) == NodeKind::OperatorName {
            c.payload(name)
                .first()
                .copied()
                .unwrap_or_else(|| c.head_token(name))
        } else {
            c.head_token(name)
        })
    }

    /// The written name token of a class-like declaration row — the
    /// `getNodeToAnnotate`/`name.isPrivate` face (file-level helper, shared
    /// with `comment_references`' scope anchor).
    fn container_name_token(c: &Check<'_>, row: u32) -> Option<u32> {
        super::container_name_token(c, row)
    }

    /// The unit's top-level function rows: each top-level wrapper's
    /// function row (the wrapper is the production shape; hand-built trees
    /// keep the row directly under the unit — both faces scan one level).
    fn unit_functions(c: &Check<'_>) -> Vec<u32> {
        let unit = c.path[0];
        let mut rows = Vec::new();
        for wrapper in c.collect_direct(unit) {
            if c.kind(wrapper) == NodeKind::TopLevelMethod {
                rows.push(wrapper);
            }
            rows.extend(
                c.collect_direct(wrapper)
                    .into_iter()
                    .filter(|&row| c.kind(row) == NodeKind::TopLevelMethod),
            );
        }
        rows
    }

    /// The variables of a `Fields`/`TopLevelFields` row (the
    /// `node.fields.variables` walk).
    fn variables(c: &Check<'_>, fields: u32) -> Vec<u32> {
        c.collect_direct(fields)
            .into_iter()
            .filter(|&decl| c.kind(decl) == NodeKind::VariablesDeclaration)
            .flat_map(|decl| c.collect_direct(decl))
            .filter(|&var| c.kind(var) == NodeKind::InitializedIdentifier)
            .collect()
    }

    /// Whether a doc-form comment attaches to `token` — the
    /// `documentationComment != null` face at the declaration's first
    /// token.
    fn doc_attached(c: &Check<'_>, token: u32) -> bool {
        c.comments
            .attached(token as usize)
            .iter()
            .any(|comment| is_doc_form(c, comment))
    }

    /// The declaration's doc-comment token: the scanner attaches the doc to
    /// the declaration's first token — the `MetadataStar` head when
    /// metadata precedes, else the row head.
    fn doc_token(c: &Check<'_>, row: u32) -> u32 {
        match declaration_star(c, row) {
            Some(star) => c.head_token(star),
            None => c.head_token(row),
        }
    }

    /// The `MetadataStar` of a declaration row: beside the row in its
    /// container, or beside the wrapper when the row anchors one level
    /// under a top-level wrapper. (File-level helper, shared with
    /// `comment_references`' doc-group face.)
    pub(crate) fn declaration_star(c: &Check<'_>, row: u32) -> Option<u32> {
        left_star(c, row).or_else(|| {
            let parent = c.unit.ast.parent(row)?;
            (c.kind(parent) == NodeKind::TopLevelDeclaration)
                .then(|| left_star(c, parent))
                .flatten()
        })
    }

    /// The `MetadataStar` immediately left of `row` in its container's
    /// child list (the metadata idiom of `member_annotations`). (File-level
    /// helper, shared with `comment_references`' doc-group face.)
    pub(crate) fn left_star(c: &Check<'_>, row: u32) -> Option<u32> {
        let parent = c.unit.ast.parent(row)?;
        let siblings = c.collect_direct(parent);
        let pos = siblings.iter().position(|&kid| kid == row)?;
        let star = siblings.get(pos.checked_sub(1)?)?;
        (c.kind(*star) == NodeKind::MetadataStar).then_some(*star)
    }

    /// The constructor's declaration-start token: the row anchors at the
    /// declared name (bridge `begin_constructor` — the documented deviation
    /// from the first-modifier-is-head contract), so the doc comment
    /// attaches left of the head — across the `.name` dot, then any of the
    /// four head modifiers (the `declarations.rs` `modifier_keyword` walk).
    /// (File-level helper, shared with `comment_references`' doc-group
    /// face.)
    pub(crate) fn constructor_first_token(c: &Check<'_>, head: u32) -> u32 {
        let mut token = head;
        if let Some(prev) = token.checked_sub(1)
            && c.lexeme(prev) == "."
        {
            token = prev - 1;
        }
        for _ in 0..4 {
            let Some(prev) = token.checked_sub(1) else {
                break;
            };
            match c.lexeme(prev) {
                "augment" | "external" | "const" | "factory" => token = prev,
                _ => break,
            }
        }
        token
    }

    /// The declared name of a named constructor — the token after the
    /// `.name` dot when one follows the type-name head (`Constructor
    /// Declaration.name`). `None` for the unnamed form and past the token
    /// table (a degenerate recovered row); the caller falls back to the
    /// head — `node.name ?? node.typeName` (`ast.dart:55-58`).
    fn declared_ctor_name(c: &Check<'_>, head: u32) -> Option<u32> {
        let dot = head.checked_add(1)?;
        (c.lexeme(dot) == ".").then(|| dot.checked_add(1)).flatten()
    }

    /// The declared-name token of a factory row: the row heads at the
    /// `factory` keyword and the name follows — the constructor name after
    /// the `.n` dot, else the type name (the forward twin of the
    /// [`constructor_first_token`] walk). `None` past the token table (a
    /// degenerate recovered row); the caller falls back to the head.
    fn factory_name_token(c: &Check<'_>, head: u32) -> Option<u32> {
        let count = u32::try_from(c.unit.tokens.len()).ok()?;
        let mut name = head.checked_add(1)?;
        if name >= count {
            return None;
        }
        let dot = name.checked_add(1)?;
        if dot < count && c.lexeme(dot) == "." {
            name = dot.checked_add(1)?;
            if name >= count {
                return None;
            }
        }
        Some(name)
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

/// The doc comment of a declaration ROW, keyed by its first token — the
/// `doc_token` face of `public_member_api_docs`: the scanner attaches the
/// doc to the declaration's first token, which the member `MetadataStar`'s
/// head carries (the bridge mints one per member), across the constructor's
/// `.name` dot and head modifiers when no star precedes. [`doc_group`]
/// keys on the row head instead, which member rows anchor at the declared
/// NAME (bridge `begin_method`/`begin_constructor`) — the doc would be
/// missed. Same trailing-run rule as [`doc_group`].
pub(crate) fn doc_group_of<'a>(c: &Check<'a>, row: u32) -> &'a [Comment] {
    let token = match public_member_api_docs::declaration_star(c, row) {
        Some(star) => c.head_token(star),
        None if c.kind(row) == NodeKind::Constructor => {
            public_member_api_docs::constructor_first_token(c, c.head_token(row))
        }
        None => c.head_token(row),
    };
    let attached = c.comments.attached(token as usize);
    let start = attached
        .iter()
        .rposition(|comment| !is_doc_form(c, comment))
        .map_or(0, |last| last + 1);
    &attached[start..]
}

/// The written name token of a class-like declaration row — the
/// `getNodeToAnnotate`/`name.isPrivate` face. The two extension shapes
/// never mint a name row: the parser reads the name straight off the
/// cursor and the bridge stores it as a payload word
/// (`begin_extension_declaration` word 1, `begin_extension_type_declaration`
/// word 2), `NONE_TOKEN` when absent — an unnamed extension answers `None`
/// (private by `extensions.dart:104`). The rest resolve through
/// [`container_name`]'s `Identifier` row, whose span head is the token.
pub(crate) fn container_name_token(c: &Check<'_>, row: u32) -> Option<u32> {
    match c.kind(row) {
        NodeKind::ExtensionDeclaration => {
            let token = c.payload(row).get(1).copied()?;
            (token != NONE_TOKEN).then_some(token)
        }
        NodeKind::ExtensionTypeDeclaration => {
            let token = c.payload(row).get(2).copied()?;
            (token != NONE_TOKEN).then_some(token)
        }
        _ => container_name(c, row).map(|name| c.head_token(name)),
    }
}

/// The name row of a wrapper-anchored declaration: the production wrapper
/// keeps the name `Identifier` as the row's left sibling, hand-built trees
/// nest it (the `declarations.rs` `container_name_node` shape).
fn container_name(c: &Check<'_>, row: u32) -> Option<u32> {
    let is_name = |kid: u32| {
        c.kind(kid) == NodeKind::Identifier
            && matches!(
                c.payload(kid).first().copied(),
                Some(ctx)
                    if ctx
                        == dartq_parser::IdentifierContext::ClassOrMixinOrExtensionDeclaration
                            as u32
                        || ctx == dartq_parser::IdentifierContext::EnumDeclaration as u32
                        || ctx == dartq_parser::IdentifierContext::TypedefDeclaration as u32
            )
    };
    if let Some(name) = c.collect_direct(row).into_iter().find(|&kid| is_name(kid)) {
        return Some(name);
    }
    let parent = c.unit.ast.parent(row)?;
    let siblings = c.collect_direct(parent);
    let pos = siblings.iter().position(|&kid| kid == row)?;
    siblings[..pos]
        .iter()
        .rev()
        .copied()
        .find(|&kid| is_name(kid))
}

pub(crate) mod comment_references {
    use super::{Check, Comment, NodeKind, RuleSpec, comment_lexeme, container_name_token, doc_group_of, is_doc_form};
    use crate::comments::CommentKind;
    use dartq_parser::IdentifierContext;
    use dartq_source::StringId;

    /// The dispatch universe: the unit face (the keyword scan runs once)
    /// plus every declaration whose doc comment attaches — the shared
    /// doc-carrying registry (`DOC_KINDS`, the official
    /// `slash_for_doc_comments.dart:59-82` list) with the two shapes the
    /// resolution face needs beyond it: a factory constructor is a
    /// `ConstructorDeclaration` upstream too, and an extension type carries
    /// docs and an instance face like any container.
    static REFERENCE_KINDS: &[NodeKind] = &[
        NodeKind::CompilationUnit,
        NodeKind::ClassDeclaration,
        NodeKind::MixinDeclaration,
        NodeKind::NamedMixinApplication,
        NodeKind::ExtensionDeclaration,
        NodeKind::ExtensionTypeDeclaration,
        NodeKind::EnumDeclaration,
        NodeKind::EnumElement,
        NodeKind::Typedef,
        NodeKind::Constructor,
        NodeKind::Factory,
        NodeKind::Method,
        NodeKind::TopLevelMethod,
        NodeKind::LocalFunctionDeclaration,
        NodeKind::Fields,
        NodeKind::TopLevelFields,
    ];

    /// Official: `pkg/linter/lib/src/rules/comment_references.dart` — a
    /// doc-comment bracket reference must name something visible in scope.
    /// The official splits the face in two. `visitComment` (:42-76) scans
    /// each doc-comment token's lexeme for bracket pairs, reporting the
    /// parser special cases `this`/`null`/`true`/`false` unconditionally
    /// (`_isParserSpecialCase`, :101-113 — the doc-comment parser never
    /// builds references for keywords, so the resolver can never see them)
    /// and collecting Markdown link-reference definitions (a bracket pair
    /// whose prefix is `///`-slashes + whitespace and whose `]` is followed
    /// by `:`, :53-62). `visitCommentReference` (:78-97) walks the parsed
    /// `CommentReference` nodes the analyzer's doc-comment builder produced
    /// (`doc_comment_builder.dart` `_parseReferences`: bracket pairs only —
    /// `](`-inline, `][`-reference, and line-start `]:`-definition shapes
    /// are links, not references; the text must parse as `ident`, `a.b`, or
    /// `a.b.c` of real identifiers, a keyword head or an operator tail never
    /// parses) and reports every reference the element model leaves
    /// unresolved, exempting the collected link-reference names by their
    /// full dotted spelling.
    ///
    /// This port mounts both faces. The keyword scan is the historical one
    /// (the same bracket walk with the documented nested-pair advance); the
    /// resolution face extracts references through the builder's decision
    /// order and resolves through the semantic query face —
    /// `Analysis::doc_reference_resolves`, the `lookup_scoped` +
    /// container-chain query. Every report is exact: a face the query
    /// cannot decide (unindexed supertype, alias unwrap, ambiguous import,
    /// unkeyable unnamed-extension member) stays silent, and a plain run
    /// (`semantics: None`) keeps the historical under-report posture.
    /// Residual under-reports (the approximation register, each pinned by
    /// an upstream channel this substrate cannot see): formal parameters
    /// named like a nested closure's (`default`-value bodies), a reference
    /// through an imported alias (`int2.isEven`), prefix-reachable
    /// superclass members past the anchor, extension members of imported
    /// libraries not re-exported through the direct-import closure,
    /// static-supertype names (`member_lookup` ignores the static bit),
    /// operator-form references (`[operator ==]`), and doc comments on
    /// directives (the dispatch universe is declarations). The keyword scan
    /// keeps its upstream-verbatim shapes: code blocks are not skipped
    /// (the official leaves its own skip as an open note at :50-52) and
    /// link definitions do not exempt keywords. Message: `diagnostic.g.dart`
    /// `commentReferences`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "comment_references",
    REFERENCE_KINDS,
    check,
);

    fn check(c: &mut Check<'_>) {
        if c.kind(c.node()) == NodeKind::CompilationUnit {
            keyword_face(c);
        } else {
            resolution_face(c);
        }
    }

    /// `visitComment` (:44-76): the parser special cases, scanned over
    /// every doc comment of the unit.
    fn keyword_face(c: &mut Check<'_>) {
        // The official face is per documentation comment (the `Comment` AST
        // node, `registry.addComment`), scanned one token at a time
        // (`node.tokens` — each `///` line is one token). The comment
        // extraction keeps the same granularity: one entry per `///` line,
        // one per `/** ... */` block.
        for comment in c.comments.all() {
            if !is_doc_form(c, comment) {
                continue;
            }
            let lexeme = comment_lexeme(c, comment);
            let bytes = lexeme.as_bytes();
            let mut p = 0usize;
            while let Some(left) = lexeme[p..].find('[').map(|rel| p + rel) {
                let Some(right_rel) = bytes[left + 1..].iter().position(|&b| b == b']') else {
                    break;
                };
                let right = left + 1 + right_rel;
                let reference = &lexeme[left + 1..right];
                if matches!(reference, "this" | "null" | "true" | "false") {
                    // `reportAtOffset(token.offset + leftIndex + 1,
                    // reference.length)` (:105-107) — the reference text's
                    // own span; the keywords are ASCII, so byte and UTF-16
                    // lengths agree.
                    let from = c
                        .unit
                        .lines
                        .byte_to_utf16(c.unit.source, comment.range.start + left as u32 + 1);
                    let to = c
                        .unit
                        .lines
                        .byte_to_utf16(c.unit.source, comment.range.start + right as u32);
                    c.report(from..to, &[]);
                }
                // Advance past the left bracket: a nested pair still gets
                // its own pass (the documented divergence in the SPEC).
                p = left + 1;
            }
        }
    }

    /// `visitCommentReference` (:78-97): extract the doc comment's
    /// references through the builder's decision order, exempt the
    /// group's collected link-reference names, and report every reference
    /// the element model leaves unresolved — `rule.reportAtNode` anchors
    /// the reference's own span.
    fn resolution_face(c: &mut Check<'_>) {
        let Some(analysis) = c.semantics() else {
            // The plain-run posture: without the query face the resolution
            // cannot answer, and under-reporting is the standing fallback.
            return;
        };
        let strings = analysis.strings();
        let node = c.node();
        let docs = doc_group_of(c, node);
        if docs.is_empty() {
            return;
        }
        // The scope anchor: the nearest container in the dispatch path —
        // the node itself for a container's own doc comment (the official
        // wraps the class/enum/extension header in its instance scope too,
        // `scope_context.dart` `visitClassDeclaration` :65-67), its
        // enclosing container for a member's.
        let container_node = c
            .path
            .iter()
            .rev()
            .copied()
            .find(|&id| is_container_kind(c.kind(id)));
        let container = container_node.and_then(|id| {
            // An unnamed extension has no container face to key on — the
            // member scope is unanswerable, the face stays silent.
            let token = container_name_token(c, id)?;
            Some(strings.intern(c.lexeme(token)))
        });
        if container_node.is_some() && container.is_none() {
            return;
        }
        let formals = attach_formals(c, node, strings);
        let Some(formals) = formals else {
            // An executable whose parameter list never parsed: the formal
            // scope is unanswerable — silence, never a false report.
            return;
        };
        // The group's Markdown link-reference definitions (:52-62): the
        // official collects them per `Comment` node and exempts matching
        // reference names by full dotted spelling (:82, :93).
        let defs = link_definitions(c, docs);
        for comment in docs {
            for extracted in reference_spans(c, comment) {
                if defs.iter().any(|def| *def == extracted.dotted()) {
                    continue;
                }
                let ids: Vec<StringId> = extracted
                    .parts
                    .iter()
                    .map(|part| strings.intern(part))
                    .collect();
                if ids.len() == 1 && formals.contains(&ids[0]) {
                    continue;
                }
                if analysis.doc_reference_resolves(container, &ids) == Some(false) {
                    // `reportAtNode(expression)` — the reference text's own
                    // span, whole `a.b.c` included.
                    let from = c
                        .unit
                        .lines
                        .byte_to_utf16(c.unit.source, comment.range.start + extracted.start);
                    let to = c
                        .unit
                        .lines
                        .byte_to_utf16(c.unit.source, comment.range.start + extracted.end);
                    c.report(from..to, &[]);
                }
            }
        }
    }

    fn is_container_kind(kind: NodeKind) -> bool {
        matches!(
            kind,
            NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::NamedMixinApplication
                | NodeKind::ExtensionDeclaration
                | NodeKind::ExtensionTypeDeclaration
                | NodeKind::EnumDeclaration
        )
    }

    /// The attaching declaration's formal names (`FormalParameterDeclaration`
    /// spellings, plus the `this.x`/`super.x` initializing and super formals
    /// the parser names under `FieldInitializer`) — the executable's own
    /// scope the official wraps its doc comment in
    /// (`scope_context.dart` `withFormalParameterScope`, :373-377). `None`:
    /// an executable whose parameter list never surfaced — the scope is
    /// unanswerable and the caller stays silent.
    fn attach_formals(
        c: &Check<'_>,
        node: u32,
        strings: &dartq_source::StringTable,
    ) -> Option<Vec<StringId>> {
        let executable = matches!(
            c.kind(node),
            NodeKind::Method
                | NodeKind::Constructor
                | NodeKind::Factory
                | NodeKind::TopLevelMethod
                | NodeKind::LocalFunctionDeclaration
        );
        if !executable {
            return Some(Vec::new());
        }
        let parameters = c.collect_direct(node).into_iter().find(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::FormalParameters
                    | NodeKind::NoFormalParameters
                    | NodeKind::ImplicitFormalParameters
            )
        })?;
        let mut names = Vec::new();
        for id in c.children(parameters) {
            if c.kind(id) != NodeKind::Identifier {
                continue;
            }
            let context = c.payload(id).first().copied();
            if context == Some(IdentifierContext::FormalParameterDeclaration as u32)
                || context == Some(IdentifierContext::FieldInitializer as u32)
            {
                names.push(strings.intern(c.lexeme(c.head_token(id))));
            }
        }
        Some(names)
    }

    /// The group's Markdown link-reference definitions, read off the raw
    /// token lexemes (:52-62): the FIRST bracket pair of a `///` token
    /// whose prefix is `///`-slashes + whitespace and whose `]` is followed
    /// by `:` — exactly `referenceIndices(0)` + `_commentStartPattern`
    /// (`^///+\s*$`), so block comments never collect.
    fn link_definitions<'a>(c: &Check<'a>, docs: &[Comment]) -> Vec<&'a str> {
        let mut defs = Vec::new();
        for comment in docs {
            let lexeme = comment_lexeme(c, comment);
            let Some(left) = lexeme.find('[') else {
                continue;
            };
            let Some(right_rel) = lexeme[left + 1..].find(']') else {
                continue;
            };
            let right = left + 1 + right_rel;
            // `left` is the `find` above — a char boundary.
            let prefix = &lexeme[..left];
            let slashes = prefix.bytes().take_while(|&byte| byte == b'/').count();
            if slashes < 3
                || !prefix[slashes..]
                    .bytes()
                    .all(|byte| byte.is_ascii_whitespace())
            {
                continue;
            }
            if lexeme.as_bytes().get(right + 1) == Some(&b':') {
                defs.push(&lexeme[left + 1..right]);
            }
        }
        defs
    }

    /// One extracted reference: the byte span of its text inside the
    /// comment lexeme and its dot-joined identifier parts (1..=3).
    struct Extracted<'a> {
        start: u32,
        end: u32,
        parts: Vec<&'a str>,
    }

    impl Extracted<'_> {
        /// The full dotted spelling — the official `expression.name`
        /// comparison face for the link-reference exemption.
        fn dotted(&self) -> String {
            self.parts.join(".")
        }
    }

    /// The builder's reference extraction (`doc_comment_builder.dart`
    /// `_parseReferences`), per content line: `](`-inline, `][`-reference,
    /// and (line-initial) `]:`-definition brackets are Markdown links, not
    /// references; `[:...:]` and backtick spans are code; an unclosed `[`
    /// never becomes a reference (the official's synthetic recovery is
    /// skipped by `visitCommentReference`'s synthetic gate).
    fn reference_spans<'a>(c: &Check<'a>, comment: &Comment) -> Vec<Extracted<'a>> {
        let lexeme = comment_lexeme(c, comment);
        let mut out = Vec::new();
        for (start, line) in content_lines(lexeme, comment.kind == CommentKind::Line) {
            scan_reference_line(line, start as u32, &mut out);
        }
        out
    }

    /// The per-line content the builder parses: a `///` token strips its
    /// marker (plus one space); a `/** ... */` block strips the opener, the
    /// closer, and each inner line's `*` bullet after its leading
    /// whitespace (`_CharacterSequenceFromMultiLineComment`, :975-1038).
    /// An unterminated block (scanner recovery runs the lexeme to the gap
    /// end) has no closer — the body ends at the lexeme's end, never at
    /// `len - 2`, which can land mid-UTF-8-char. The caller's doc-form
    /// gate anchors the fixed `3`-byte marker offsets in ASCII.
    /// Yields each line's byte offset inside the lexeme with its content.
    fn content_lines<'a>(lexeme: &'a str, is_line: bool) -> Vec<(usize, &'a str)> {
        if is_line {
            let mut start = lexeme
                .find(|ch: char| ch != '/')
                .unwrap_or(lexeme.len())
                .max(3);
            if lexeme.as_bytes().get(start) == Some(&b' ') {
                start += 1;
            }
            return vec![(start, &lexeme[start..])];
        }
        let body_start = 3;
        let body = lexeme.strip_suffix("*/").unwrap_or(lexeme);
        let body_end = body.len().max(body_start);
        let mut lines = Vec::new();
        let mut offset = body_start;
        for (index, line) in body[body_start..body_end].split('\n').enumerate() {
            let mut start = offset;
            let mut text = line;
            if index > 0 {
                let whitespace = text.len() - text.trim_start().len();
                start += whitespace;
                text = &text[whitespace..];
                if let Some(rest) = text.strip_prefix('*') {
                    start += 1;
                    text = rest;
                    if let Some(rest) = text.strip_prefix(' ') {
                        start += 1;
                        text = rest;
                    }
                }
            }
            lines.push((start, text));
            offset += line.len() + 1;
        }
        lines
    }

    /// One content line's bracket walk (`_parseReferences` :765-820): the
    /// `seenOnlyWhitespace` gate resets per line, a backtick skips its code
    /// span, `[:` skips the old-style code block to `:]`, and each bracket
    /// pair is either link text or a reference candidate.
    fn scan_reference_line<'a>(line: &'a str, line_start: u32, out: &mut Vec<Extracted<'a>>) {
        let bytes = line.as_bytes();
        let mut index = 0usize;
        let mut seen_only_whitespace = true;
        while index < bytes.len() {
            match bytes[index] {
                b'`' => {
                    // Skip the inline code span; the scan resumes after the
                    // closing backtick.
                    if let Some(rel) = line[index + 1..].find('`') {
                        index += rel + 1;
                    }
                    seen_only_whitespace = false;
                }
                b'[' => {
                    if bytes.get(index + 1) == Some(&b':') {
                        // Old-style code block `[:int:]`: skip to `:]`.
                        match line[index + 2..].find(":]") {
                            Some(rel) => index += rel + 3,
                            None => break,
                        }
                    } else {
                        let reference_start = index + 1;
                        match line[reference_start..].find(']') {
                            None => {
                                // Synthetic recovery: no `]`, never a
                                // reference (the official marks it synthetic
                                // and `visitCommentReference` skips that).
                            }
                            Some(rel) => {
                                let close = reference_start + rel;
                                if !is_link_text(line, close, seen_only_whitespace) {
                                    let text = &line[reference_start..close];
                                    if let Some(parts) = parse_reference(text) {
                                        out.push(Extracted {
                                            start: line_start + reference_start as u32,
                                            end: line_start + close as u32,
                                            parts,
                                        });
                                    }
                                }
                                index = close;
                            }
                        }
                    }
                    seen_only_whitespace = false;
                }
                byte if byte.is_ascii_whitespace() => {}
                _ => seen_only_whitespace = false,
            }
            index += 1;
        }
    }

    /// `_isLinkText` (:56-91): a `]` followed by `(` (inline link), by `:`
    /// when the bracket opened the line (a link-reference definition), or
    /// by whitespace then `[` (a reference link) is Markdown, not a
    /// reference.
    fn is_link_text(line: &str, close: usize, can_be_link_reference: bool) -> bool {
        let bytes = line.as_bytes();
        let Some(&first) = bytes.get(close + 1) else {
            return false;
        };
        if first == b'(' {
            return true;
        }
        if can_be_link_reference && first == b':' {
            return true;
        }
        let mut index = close + 1;
        let mut ch = first;
        while ch.is_ascii_whitespace() {
            index += 1;
            let Some(&next) = bytes.get(index) else {
                return false;
            };
            ch = next;
        }
        ch == b'['
    }

    /// The Dart reserved words — a keyword never heads a comment reference
    /// (`_parseOneCommentReference`'s `token.isIdentifier` gate; the bare
    /// `this`/`null`/`true`/`false` are the keyword face's business).
    const RESERVED_WORDS: &[&str] = &[
        "assert", "break", "case", "catch", "class", "const", "continue", "default", "do", "else",
        "enum", "extends", "false", "final", "finally", "for", "if", "in", "is", "new", "null",
        "rethrow", "return", "super", "switch", "this", "throw", "true", "try", "var", "void",
        "while", "with",
    ];

    /// `_parseOneCommentReference` over the between-brackets text: one to
    /// three dot-joined identifiers (`a.new` — `new` after a dot — parses;
    /// operator tails and anything else the real scanner rejects do not,
    /// and those brackets are simply not references).
    fn parse_reference(text: &str) -> Option<Vec<&str>> {
        let parts: Vec<&str> = text.split('.').collect();
        if parts.len() > 3 {
            return None;
        }
        for (index, part) in parts.iter().enumerate() {
            if !is_identifier(part) {
                return None;
            }
            if index == 0 && RESERVED_WORDS.contains(part) {
                return None;
            }
        }
        Some(parts)
    }

    fn is_identifier(part: &str) -> bool {
        let bytes = part.as_bytes();
        let Some((&first, rest)) = bytes.split_first() else {
            return false;
        };
        (first == b'_' || first.is_ascii_alphabetic())
            && rest
                .iter()
                .all(|&byte| byte == b'_' || byte.is_ascii_alphanumeric())
    }

    /// The #1956 regression face at the extraction unit. An unterminated
    /// block doc (`/**` with no closing `*/` — the scanner's error recovery
    /// runs the lexeme to the gap end, so the trigger source
    /// `/**中文\nclass C {}` yields exactly the `/**中文\n` lexeme here) used
    /// to slice at `len - 2`, which lands mid-UTF-8-char and panics. The
    /// full-rule run cannot host this fixture: `Fixture::run_semantic`
    /// gates on scanner-clean sources, and the unterminated block reports a
    /// scanner diagnostic by design.
    #[cfg(test)]
    mod tests {
        use super::{content_lines, scan_reference_line};

        #[test]
        fn unterminated_block_doc_multibyte_body_slices_char_safely() {
            // The exact scanner-recovery lexeme of `/**中文\nclass C {}` —
            // no `*/` closer, multi-byte first line.
            let lines = content_lines("/**中文\n", false);
            assert_eq!(lines, vec![(3, "中文"), (10, "")]);

            // The reference walk over the recovered body finds nothing
            // reportable (`reference_spans`'s loop verbatim).
            let mut refs = Vec::new();
            for (start, line) in &lines {
                scan_reference_line(line, *start as u32, &mut refs);
            }
            assert!(refs.is_empty());

            // Degenerate short lexemes keep their historical shape (the
            // saturating guard): the body is empty, no panic.
            assert_eq!(content_lines("/**", false), vec![(3, "")]);
        }

        #[test]
        fn terminated_block_doc_body_pairs_are_unchanged() {
            // The no-behavior-change invariant at the unit level: closed
            // comments keep their exact (offset, line) pairs.
            assert_eq!(content_lines("/**中文*/", false), vec![(3, "中文")]);
            assert_eq!(content_lines("/** a * b */", false), vec![(3, " a * b ")]);
        }
    }
}

pub(crate) mod unnecessary_ignore {
    use crate::registry::RuleSpec;
    use crate::rules::not_dispatched;

    /// Official: `pkg/linter/lib/src/rules/unnecessary_ignore.dart` — an
    /// ignore comment citing a diagnostic the analyzer never produces at
    /// that location reports. The official rule body carries NO visiting or
    /// analysis (:34-37 — the comment says so explicitly): the verdict is
    /// the analyzer's `IgnoreValidator`, run by the `library_analyzer` after
    /// all other diagnostics exist, because the lint needs the complete
    /// diagnostic set of the location and a lint cannot hook the analysis
    /// life-cycle with that awareness.
    ///
    /// `kinds: &[]` — the same posture here: the dartq engine emits lints
    /// during one traversal, so no per-node check can know whether the cited
    /// code is produced elsewhere in the run. Mounted as the engine's run
    /// epilogue (`unnecessary_ignore_rows`, engine.rs — the #1345 mount):
    /// after the dispatch walk completes, the run's suppression record joins
    /// the dedup'd citations, and every citation that dropped no diagnostic
    /// of the run reports. The four upstream codes (location/file ×
    /// citation/name) collapse into this one spec; the primary location
    /// text below carries the mirror. The late-join names
    /// (`LATE_JOIN_RULES` — `remove_deprecations_in_breaking_versions`)
    /// answer through the shell's re-join
    /// (`unnecessary_ignore_late_rows`, engine.rs — the #1904 tail
    /// window): the mounting shell records which rows its own walk's
    /// ignore gate dropped and re-runs the join with that record.
    /// Residual posture: a plain run (no mounting shell — test harnesses,
    /// embedded consumers) keeps the #1353 silence on those names, the
    /// verdict needs the shell half. Batch-close note: at the #1153
    /// promotion this name joins `STRUCTURALLY_SILENT`
    /// (`tests/registry_pin.rs`) beside the freeze regen — the freeze flip
    /// stays the serial-phase step gated on the differential.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_ignore",
    &[],
    not_dispatched,
);
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

    /// The fence tick counts must match (`_fencedCodeBlockDelimiter`
    /// :244-257, `_parseFencedCodeBlock` :492-547): a three-tick fence
    /// stays open across a two-tick line and closes on the matching
    /// three-tick line — the `<T>` inside both stay code, and prose
    /// resumes after the close.
    #[test]
    fn html_fence_tick_count_gates_the_toggle() {
        // The `` line does not close the ``` fence: still code.
        let f = class_fixture("/// ```dart\n/// <T>\n/// ``\n/// ```\nclass A {}");
        assert!(
            f.run(&one_rule("unintended_html_in_doc_comment"))
                .is_empty()
        );
        // After the matching closer, prose reports again.
        let f = class_fixture("/// ```\n/// ```\n/// Uses a <T> here.\nclass A {}");
        let lints = f.run(&one_rule("unintended_html_in_doc_comment"));
        assert_eq!(lints.len(), 1);
    }

    /// A four-tick opener never closes (`_fencedCodeBlockDelimiter`
    /// compares a fixed three-character substring against the opener's
    /// tick run, :250-253): every following line stays code.
    #[test]
    fn html_four_tick_fence_stays_open() {
        let f = class_fixture("/// ````dart\n/// <T>\n/// ```\n/// <T>\nclass A {}");
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

    /// Plain `//` comments above the doc comment never join the group —
    /// the span anchors at the first `///` line (the corpus `docs.dart`
    /// shape: a `//` header block above the dangling doc).
    #[test]
    fn dangling_plain_comments_above_doc_excluded_from_span() {
        let f = var_fixture("// h\n/// doc\n\nvar x = 1;");
        let lints = f.run(&one_rule("dangling_library_doc_comments"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range, 5..12);
    }

    /// A multi-line doc block reports at exactly the doc lines — the span
    /// starts at the first `///` and ends at the last.
    #[test]
    fn dangling_multi_line_doc_span_covers_doc_lines_only() {
        let f = var_fixture("/// a\n/// b\n\nvar x = 1;");
        let lints = f.run(&one_rule("dangling_library_doc_comments"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range, 0..11);
    }

    /// A blank line then a plain `//` after the doc makes it dangling —
    /// the span stays on the doc tokens; the trailing `//` rides the
    /// attached chain without joining the group.
    #[test]
    fn dangling_blank_then_plain_comment_after_doc_span() {
        let f = var_fixture("/// doc\n\n// c\nvar x = 1;");
        let lints = f.run(&one_rule("dangling_library_doc_comments"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range, 0..7);
    }

    /// A plain `//` between doc lines does not bridge the doc tokens: the
    /// within-doc face pairs doc tokens only, and the line gap between
    /// consecutive doc tokens decides — the report anchors at the earlier
    /// doc line, not at the group span.
    #[test]
    fn dangling_plain_comment_between_doc_lines_does_not_bridge() {
        let f = var_fixture("/// a\n// c\n/// b\n\nvar x = 1;");
        let lints = f.run(&one_rule("dangling_library_doc_comments"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range, 0..0);
    }

    /// A later `/**` re-anchors the doc group (`findDartDoc`): the `///`
    /// above it leaves the group and the span anchors at the block doc.
    #[test]
    fn dangling_block_doc_reanchors_group() {
        let f = var_fixture("/// a\n/** b */\n\nvar x = 1;");
        let lints = f.run(&one_rule("dangling_library_doc_comments"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range, 6..14);
    }

    /// The corpus F6 shape over multi-byte bytes: plain `//` comments with
    /// CJK text above the doc never join the span — the anchor is the
    /// `///` line and the offsets go through the UTF-16 mapping.
    #[test]
    fn dangling_cjk_header_above_doc_excluded_from_span() {
        let f = var_fixture("// 中文头注释\n/// dangling 文档\n\nvar x = 1;");
        let lints = f.run(&one_rule("dangling_library_doc_comments"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range, 9..24);
    }

    /// The corpus face (`tests/corpus/lints/lib/docs.dart`, oracle anchor
    /// (10, 1) length 59) through the production parse: nine plain `//`
    /// header lines above the dangling `///` stay outside the 59-byte
    /// doc-line span.
    #[test]
    fn dangling_corpus_header_block_reports_doc_line_only() {
        let source = concat!(
            "// l1\n// l2\n// l3\n// l4\n// l5\n// l6\n// l7\n// l8\n// l9\n",
            "/// A library doc comment left dangling above a blank line.\n",
            "\nclass DanglingDoced {}\n",
        );
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("dangling_library_doc_comments"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        // Nine 6-byte header lines put the 59-byte doc line at 54..113.
        assert_eq!(lints[0].range, 54..113);
    }

    /// The directive face re-anchors too (`findDartDoc`): a `///` after a
    /// `/**` block wins, and the report anchors at the `///` line.
    #[test]
    fn dangling_directive_doc_reanchors_after_block() {
        let f = Fixture::build(
            "/** a */\n/// b\nimport 'x.dart';",
            unit_of(|b, _t| {
                b.begin(NodeKind::Import, 0);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.leaf_span(NodeKind::StringLiteral, 1, 2);
                b.end(NodeKind::Import, 3);
            }),
        );
        let lints = f.run(&one_rule("dangling_library_doc_comments"));
        assert_eq!(lints.len(), 1);
        // `/** a */\n` spans 0..9; the `/// b` line starts at 9.
        assert_eq!(lints[0].range, 9..9);
    }
}

/// The W5 (#1026) production-bridge pins over real parser trees, against
/// the upstream corpora (`pkg/linter/test/rules/<name>_test.dart`).
#[cfg(test)]
mod w5_pins {
    use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

    /// A fixture mounted inside the package's `lib/` — the registration
    /// gate face of the lib-gated rules.
    fn lib(source: &str) -> Fixture {
        let mut f = Fixture::parse(source);
        f.path = "lib/a.dart".into();
        f
    }

    // —— document_ignores ——————————————————————————————————————————————

    /// `test_precedingDeclaration_notDocumented` — an undocumented ignore
    /// comment reports at itself.
    #[test]
    fn document_ignores_undocumented_reports() {
        let f = Fixture::parse("int x = 0;\n\n// ignore: unused_element\nint _y = 0;\n");
        let lints = f.run(&one_rule("document_ignores"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Missing documentation explaining why the diagnostic is ignored."
        );
        // The comment lexeme spans 12..37 (`int x = 0;` + two newlines).
        assert_eq!(lints[0].range, 12..37);
    }

    /// `test_precedingLine_documented` — a `//` comment on the line above
    /// documents the ignore.
    #[test]
    fn document_ignores_documented_above_passes() {
        let f = Fixture::parse("// Text.\n// ignore: unused_element\nint _y = 0;\n");
        assert!(f.run(&one_rule("document_ignores")).is_empty());
    }

    /// `test_sameComment_comma_documented` and
    /// `test_sameComment_whitespace_documented` — trailing free text in
    /// the ignore comment itself documents it (`IgnoredDiagnosticComment`
    /// as the last element).
    #[test]
    fn document_ignores_trailing_text_documents() {
        let f = Fixture::parse("// ignore: unused_element, I need this\nint _y = 0;\n");
        assert!(f.run(&one_rule("document_ignores")).is_empty());

        let f = Fixture::parse("// ignore: unused_element http://linter-bug\nint _y = 0;\n");
        assert!(f.run(&one_rule("document_ignores")).is_empty());
    }

    /// `test_precedingLine_multiple_notDocumented` with the
    /// `ignore_for_file:` shape — the file-level form needs the same
    /// documentation, and an empty citation never reports.
    #[test]
    fn document_ignores_for_file_and_empty_citation_faces() {
        let f = Fixture::parse(
            "// Text.\n// ignore_for_file: unused_element\n\n// ignore_for_file: unnecessary_cast\nint _y = 0 as int;\n",
        );
        let lints = f.run(&one_rule("document_ignores"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range.start, 45);

        // `// ignore:` citing nothing has no elements to document.
        let f = Fixture::parse("int x = 0;\n\n// ignore:\nint _y = 0;\n");
        assert!(f.run(&one_rule("document_ignores")).is_empty());
    }

    /// A CJK processing instruction (`<?中文 x?>`) skips as a PI and the
    /// following invalid tag still reports — byte positions survive
    /// multi-byte content between the angle constructs.
    #[test]
    fn unintended_html_multibyte_processing_instruction_face() {
        let f = Fixture::parse("/// <?中文 x?> <b1>\nvar x = 1;\n");
        let lints = f.run(&one_rule("unintended_html_in_doc_comment"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range, 13..17);
    }

    // —— flutter_style_todos ————————————————————————————————————————————

    /// `test_badPatterns` (the `// TODO something` line) — a malformed
    /// to-do comment reports at the whole comment.
    #[test]
    fn flutter_style_todos_malformed_reports() {
        let f = Fixture::parse("// TODO something\nvar x = 1;\n");
        let lints = f.run(&one_rule("flutter_style_todos"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "To-do comment doesn't follow the Flutter style."
        );
        // The comment lexeme opens the unit at 0..17.
        assert_eq!(lints[0].range, 0..17);
    }

    /// `test_properFormat_simpleUsername` — the `// TODO(name): message`
    /// shape passes, and a non-to-do comment never fires.
    #[test]
    fn flutter_style_todos_proper_format_passes() {
        let f = Fixture::parse("// TODO(username): bla\nvar x = 1;\n");
        assert!(f.run(&one_rule("flutter_style_todos")).is_empty());

        let f = Fixture::parse("// comment TODO(user): bla\nvar x = 1;\n");
        assert!(f.run(&one_rule("flutter_style_todos")).is_empty());
    }

    /// `test_docComment`, `test_properFormat_multipleSeriesOfSlashes`, and
    /// `test_slashStar` — the trigger anchors at index 0: `/// TODO(…)`
    /// reports (the expected `//` cannot cross the third slash), a later
    /// `// TODO` after other slashes stays silent, block comments never
    /// trigger, and the trigger is case-insensitive while the expected
    /// shape is not.
    #[test]
    fn flutter_style_todos_anchor_and_case_faces() {
        let f = Fixture::parse("/// TODO(user): bla\nvar x = 1;\n");
        assert_eq!(f.run(&one_rule("flutter_style_todos")).len(), 1);

        let f = Fixture::parse("// // TODO(somebody): something\nvar x = 1;\n");
        assert!(f.run(&one_rule("flutter_style_todos")).is_empty());

        let f = Fixture::parse("/* TODO bla */\nvar x = 1;\n");
        assert!(f.run(&one_rule("flutter_style_todos")).is_empty());

        let f = Fixture::parse("// Todo(somebody): something\nvar x = 1;\n");
        assert_eq!(f.run(&one_rule("flutter_style_todos")).len(), 1);
    }

    /// Multi-byte comments are byte-safe in the trigger walk (the fixed
    /// `rest[..4]` str slice once panicked on a non-char boundary): a
    /// leading em-dash or CJK run never matches the four ASCII `TODO`
    /// bytes (silent — the regex has no match either), while an emoji in
    /// the expected-face username slot still reports.
    #[test]
    fn flutter_style_todos_multibyte_faces() {
        // The panic shapes: the first post-whitespace character is
        // multi-byte, so the old fixed `..4` slice crossed a boundary.
        let f = Fixture::parse("// ——TODO(x): soon\nvar x = 1;\n");
        assert!(f.run(&one_rule("flutter_style_todos")).is_empty());

        let f = Fixture::parse("// 中文TODO(x): soon\nvar x = 1;\n");
        assert!(f.run(&one_rule("flutter_style_todos")).is_empty());

        // The trigger fires (four ASCII `TODO` bytes; the tail byte is
        // `(`) and the expected shape rejects the emoji — the comment
        // reports at its whole lexeme (16 UTF-16 units: 18 bytes with a
        // 2-unit emoji).
        let f = Fixture::parse("// TODO(🔥): fix\nvar x = 1;\n");
        let lints = f.run(&one_rule("flutter_style_todos"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range, 0..16);
    }

    // —— missing_code_block_language_in_doc_comment —————————————————————

    /// `test_missingLanguage` — an un-language fenced block reports at its
    /// opening fence line.
    #[test]
    fn missing_code_block_language_reports_the_opening_fence() {
        let f = Fixture::parse("/// ```\n/// test\n/// ```\nclass A {}\n");
        let lints = f.run(&one_rule("missing_code_block_language_in_doc_comment"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The code block is missing a specified language."
        );
        assert_eq!(lints[0].range, 4..7);
    }

    /// `test_hasLanguage` and `test_indentedCodeBlock` — an info string
    /// (even with leading fence whitespace) and an indented block pass.
    #[test]
    fn missing_code_block_language_compliant_passes() {
        let f = Fixture::parse("/// ```dart\n/// test\n/// ```\nclass A {}\n");
        assert!(
            f.run(&one_rule("missing_code_block_language_in_doc_comment"))
                .is_empty()
        );

        let f = Fixture::parse(
            "/// Example:\n///\n///     var printer = Printer();\n///     printer.printToStdout();\n///\nclass A {}\n",
        );
        assert!(
            f.run(&one_rule("missing_code_block_language_in_doc_comment"))
                .is_empty()
        );
    }

    /// `test_missingLanguage_noEndingFence` and
    /// `test_missingLanguage_leadingWhitespace` — an unterminated block
    /// reports once, and the anchor is the fence line's content span
    /// (marker plus one separator space stripped, indentation included —
    /// the upstream markup marks `[!  ```!]`).
    #[test]
    fn missing_code_block_language_unterminated_and_indent_faces() {
        let f = Fixture::parse("/// ```\n/// test\n/// more test\nclass A {}\n");
        let lints = f.run(&one_rule("missing_code_block_language_in_doc_comment"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range, 4..7);

        let f = Fixture::parse("///   ```\n/// test\n/// ```\nclass A {}\n");
        let lints = f.run(&one_rule("missing_code_block_language_in_doc_comment"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range, 4..9);
    }

    /// Multi-byte info strings and fenced content stay byte-safe: a
    /// CJK-language fence passes, and the later un-language fence still
    /// reports at its opening line's content span (UTF-16 mapped over the
    /// multi-byte lines above).
    #[test]
    fn missing_code_block_language_multibyte_fence_faces() {
        let f = Fixture::parse(
            "/// ```中文\n/// 内容\n/// ```\n///\n/// ```\n/// x\n/// ```\nvar x = 1;\n",
        );
        let lints = f.run(&one_rule("missing_code_block_language_in_doc_comment"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range, 33..36);
    }

    // —— public_member_api_docs ——————————————————————————————————————————

    /// `test/rules/public_member_api_docs_test.dart` — an undocumented
    /// public class reports at its name; the message is
    /// `publicMemberApiDocs`.
    #[test]
    fn public_member_api_docs_reports_undocumented_class() {
        let f = lib("class A {}\n");
        let lints = f.run(&one_rule("public_member_api_docs"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Missing documentation for a public member."
        );
        let start = utf16_at(&f, tok(&f.source, &f.tokens, "A", 0));
        assert_eq!(lints[0].range, start..start + 1);
    }

    /// A documented class passes, a private one is out of the API face.
    #[test]
    fn public_member_api_docs_class_gates_pass() {
        let f = lib("/// Doc.\nclass A {}\n");
        assert!(f.run(&one_rule("public_member_api_docs")).is_empty());

        let f = lib("class _A {}\n");
        assert!(f.run(&one_rule("public_member_api_docs")).is_empty());
    }

    /// Undocumented public members report at their names in source order;
    /// documented and private members pass.
    #[test]
    fn public_member_api_docs_member_faces() {
        let f = lib("/// Doc.\nclass A {\n  int f;\n  void m() {}\n  int get g => 0;\n}\n");
        let lints = f.run(&one_rule("public_member_api_docs"));
        assert_eq!(lints.len(), 3, "{lints:?}");
        for (lint, name) in lints.iter().zip(["f", "m", "g"]) {
            assert_eq!(lint.message, "Missing documentation for a public member.");
            assert_eq!(
                lint.range.start,
                utf16_at(&f, tok(&f.source, &f.tokens, name, 0))
            );
        }

        let f = lib(
            "/// Doc.\nclass A {\n  /// Doc.\n  int f;\n  /// Doc.\n  void m() {}\n  /// Doc.\n  int get g => 0;\n}\n",
        );
        assert!(f.run(&one_rule("public_member_api_docs")).is_empty());

        let f = lib("/// Doc.\nclass A {\n  int _f;\n  void _m() {}\n}\n");
        assert!(f.run(&one_rule("public_member_api_docs")).is_empty());
    }

    /// The getter/setter pairing (:74-112): a setter reports only when its
    /// same-name getter is also missing docs.
    #[test]
    fn public_member_api_docs_setter_pairing() {
        let f = lib("/// Doc.\nclass A {\n  /// Doc.\n  int get g => 0;\n  set g(int v) {}\n}\n");
        assert!(f.run(&one_rule("public_member_api_docs")).is_empty());

        let f = lib("/// Doc.\nclass A {\n  int get g => 0;\n  set g(int v) {}\n}\n");
        let lints = f.run(&one_rule("public_member_api_docs"));
        assert_eq!(lints.len(), 2, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "g", 0))
        );
        assert_eq!(
            lints[1].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "g", 1))
        );

        let f = lib("/// Doc.\nclass A {\n  int get g => 0;\n  /// Doc.\n  set g(int v) {}\n}\n");
        let lints = f.run(&one_rule("public_member_api_docs"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "g", 0))
        );
    }

    /// `test_mixin_overridingMethod_OK` and the same-file override face
    /// (:66, :115-116): a member overriding an inherited member passes.
    #[test]
    fn public_member_api_docs_overriding_member_passes() {
        let f = lib(
            "/// Doc.\nclass A {\n  void m() {}\n}\n/// Doc.\nclass B extends A {\n  void m() {}\n}\n",
        );
        let lints = f.run(&one_rule("public_member_api_docs"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "m", 0))
        );

        let f = lib("/// Doc.\nmixin M {\n  @override\n  String toString() => '';\n}\n");
        assert!(f.run(&one_rule("public_member_api_docs")).is_empty());
    }

    /// `test_topLevelMembers` — a top-level variable, typedef, and getter
    /// report at their names; the private spellings and `main` pass.
    #[test]
    fn public_member_api_docs_top_level_faces() {
        let f = lib("int g = 1;\ntypedef T = void Function();\nint get z => 0;\n");
        let lints = f.run(&one_rule("public_member_api_docs"));
        assert_eq!(lints.len(), 3, "{lints:?}");
        for (lint, name) in lints.iter().zip(["g", "T", "z"]) {
            assert_eq!(
                lint.range.start,
                utf16_at(&f, tok(&f.source, &f.tokens, name, 0))
            );
        }

        let f = lib("int _h = 1;\nvoid main() {}\n");
        assert!(f.run(&one_rule("public_member_api_docs")).is_empty());
    }

    /// The unit-level accessor pairing (:164-169): a top-level setter
    /// reports only when its getter is missing docs.
    #[test]
    fn public_member_api_docs_top_level_setter_pairing() {
        let f = lib("/// Doc.\nint get z => 0;\nset z(int v) {}\n");
        assert!(f.run(&one_rule("public_member_api_docs")).is_empty());

        let f = lib("int get z => 0;\nset z(int v) {}\n");
        let lints = f.run(&one_rule("public_member_api_docs"));
        assert_eq!(lints.len(), 2, "{lints:?}");
    }

    /// The registration gate (:40 `isInLibDir`): outside the package's
    /// `lib/` the rule never fires.
    #[test]
    fn public_member_api_docs_gates_outside_lib() {
        let mut f = Fixture::parse("class A {}\nint g = 1;\n");
        f.path = "tool/gen.dart".into();
        assert!(f.run(&one_rule("public_member_api_docs")).is_empty());
    }

    /// `test_enum` — the enum, its constants, and its members report at
    /// their names; `test_annotatedEnumValue` — a documented constant
    /// passes.
    #[test]
    fn public_member_api_docs_enum_faces() {
        let f = lib("enum A {\n  a, b;\n  int x() => 0;\n}\n");
        let lints = f.run(&one_rule("public_member_api_docs"));
        assert_eq!(lints.len(), 4, "{lints:?}");
        for (lint, name) in lints.iter().zip(["A", "a", "b", "x"]) {
            assert_eq!(
                lint.range.start,
                utf16_at(&f, tok(&f.source, &f.tokens, name, 0))
            );
        }

        let f = lib("/// Documented.\nenum A {\n  /// Doc.\n  a,\n}\n");
        assert!(f.run(&one_rule("public_member_api_docs")).is_empty());
    }

    /// `test_extension` — a named extension and its members report; an
    /// unnamed extension is private (`extensions.dart:104`).
    #[test]
    fn public_member_api_docs_extension_faces() {
        let f = lib("extension E on Object {\n  void f() {}\n}\n");
        let lints = f.run(&one_rule("public_member_api_docs"));
        assert_eq!(lints.len(), 2, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "E", 0))
        );

        let f = lib("extension on Object {\n  void f() {}\n}\n");
        assert!(f.run(&one_rule("public_member_api_docs")).is_empty());
    }

    /// The constructor faces (`getNodeToAnnotate`, `ast.dart:55-58`): a
    /// named constructor anchors at its name, an unnamed one at the type
    /// name, a factory at the written type name; a doc comment left of the
    /// head modifiers documents the constructor.
    #[test]
    fn public_member_api_docs_constructor_faces() {
        let f = lib("/// Doc.\nclass C {\n  C._();\n  C.named();\n}\n");
        let lints = f.run(&one_rule("public_member_api_docs"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "named", 0))
        );

        let f = lib("/// Doc.\nclass C {\n  C();\n}\n");
        let lints = f.run(&one_rule("public_member_api_docs"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "C", 1))
        );

        let f = lib("class C {\n  /// Doc.\n  const C.named();\n  C();\n}\n");
        let lints = f.run(&one_rule("public_member_api_docs"));
        assert_eq!(lints.len(), 2, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "C", 0))
        );
        assert_eq!(
            lints[1].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "C", 2))
        );

        let f = lib("/// Doc.\nclass C {\n  C._();\n  factory C() => C._();\n}\n");
        let lints = f.run(&one_rule("public_member_api_docs"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "C", 2))
        );
    }

    /// `test_sealedConstructor` / `test_abstractFinalConstructor` — a
    /// `sealed` or `abstract final` owner is effectively private
    /// (`extensions.dart:127-141`), its constructors pass; a plain
    /// `abstract` owner is not.
    #[test]
    fn public_member_api_docs_effectively_private_owner_faces() {
        let f = lib("sealed class S {\n  S();\n}\n");
        let lints = f.run(&one_rule("public_member_api_docs"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "S", 0))
        );

        let f = lib("abstract final class S {\n  S();\n}\n");
        let lints = f.run(&one_rule("public_member_api_docs"));
        assert_eq!(lints.len(), 1, "{lints:?}");

        let f = lib("abstract class S {\n  S();\n}\n");
        let lints = f.run(&one_rule("public_member_api_docs"));
        assert_eq!(lints.len(), 2, "{lints:?}");
    }

    /// `test_internalClass` / `test_internalTopLevelVariable` /
    /// `test_internalFunction` — an `@internal` spelling on the enclosing
    /// top-level member (the variable declaration itself for variables)
    /// skips the face.
    #[test]
    fn public_member_api_docs_internal_annotation_faces() {
        let f = lib("@internal\nclass C {\n  int? f;\n  void m() {}\n}\n");
        assert!(f.run(&one_rule("public_member_api_docs")).is_empty());

        let f = lib("@internal\nvar f = 1;\n");
        assert!(f.run(&one_rule("public_member_api_docs")).is_empty());

        let f = lib("@internal\nvoid f() {}\n");
        assert!(f.run(&one_rule("public_member_api_docs")).is_empty());
    }

    /// `test_extensionTypeDeclaration` / `test_field_static` — the
    /// extension type reports at its name; its static field reports under
    /// the class doc (the declaration doc does not document the field).
    #[test]
    fn public_member_api_docs_extension_type_faces() {
        let f = lib("extension type E(int i) {}\n");
        let lints = f.run(&one_rule("public_member_api_docs"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "E", 0))
        );

        let f = lib("/// Doc.\nextension type E(int i) {\n  static int? f;\n}\n");
        let lints = f.run(&one_rule("public_member_api_docs"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "f", 0))
        );
    }

    /// The alias faces (:125-129, :232-243): a named mixin application and
    /// a documented one.
    #[test]
    fn public_member_api_docs_named_mixin_application_faces() {
        let f = lib("class A = Object with Comparable;\n");
        let lints = f.run(&one_rule("public_member_api_docs"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "A", 0))
        );

        let f = lib("/// Doc.\nclass A = Object with Comparable;\n");
        assert!(f.run(&one_rule("public_member_api_docs")).is_empty());
    }

    /// The per-variable field faces (:224-228): separate declaration rows
    /// document independently; one row's doc group covers every variable of
    /// that row (the documented substrate approximation).
    #[test]
    fn public_member_api_docs_field_variable_faces() {
        let f = lib("/// Doc.\nclass A {\n  int a;\n  int b;\n}\n");
        let lints = f.run(&one_rule("public_member_api_docs"));
        assert_eq!(lints.len(), 2, "{lints:?}");

        let f = lib("/// Doc.\nclass A {\n  /// Doc.\n  int a, b;\n}\n");
        assert!(f.run(&one_rule("public_member_api_docs")).is_empty());
    }

    /// `test_constructor_unnamedFactory` — a factory constructor reports at
    /// its declared name (the constructor name after the dot, else the
    /// type name); a documented factory and a privately-named one stay
    /// silent.
    #[test]
    fn public_member_api_docs_factory_constructor_faces() {
        let f = lib("/// Doc.\nclass C {\n  C._();\n  factory C() => C._();\n}\n");
        let lints = f.run(&one_rule("public_member_api_docs"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "C", 2))
        );

        let f = lib("/// Doc.\nclass C {\n  C._();\n  factory C.n() => C._();\n}\n");
        let lints = f.run(&one_rule("public_member_api_docs"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "n", 0))
        );

        let f = lib("/// Doc.\nclass C {\n  C._();\n  factory C._p() => C._();\n}\n");
        assert!(f.run(&one_rule("public_member_api_docs")).is_empty());
    }

    /// `test_mixin_overridingMethod_OK` — an `@override` member whose
    /// supertype resolves out of file stays silent (the annotation stands
    /// in for the out-of-file resolution); without the annotation the
    /// documented residual over-report remains.
    #[test]
    fn public_member_api_docs_override_annotation_faces() {
        let f = lib("/// Doc.\nmixin M {\n  @override\n  String toString() => 'M';\n}\n");
        assert!(f.run(&one_rule("public_member_api_docs")).is_empty());

        let f = lib("/// Doc.\nclass C {\n  @override\n  int get hashCode => 0;\n}\n");
        assert!(f.run(&one_rule("public_member_api_docs")).is_empty());

        let f = lib("/// Doc.\nmixin M {\n  String toString() => 'M';\n}\n");
        assert_eq!(f.run(&one_rule("public_member_api_docs")).len(), 1);
    }

    /// The setter-pair gate (:94-108): a setter mounts only on a getter
    /// that actually reported — an `@internal` or overriding getter never
    /// enters `missingDocs`, so its undocumented setter stays silent too.
    #[test]
    fn public_member_api_docs_setter_pair_gate() {
        let f = lib("@internal\nint get x => 0;\nvoid set x(int v) {}\n");
        assert!(f.run(&one_rule("public_member_api_docs")).is_empty());

        let f = lib(
            "/// Doc.\nclass C extends D {\n  int get x => 0;\n  void set x(int v) {}\n}\n\
             /// Doc.\nclass D {\n  /// Doc.\n  int get x => 0;\n}\n",
        );
        assert!(f.run(&one_rule("public_member_api_docs")).is_empty());
    }
}

/// The W5 (#1124) approximation pins: the rules whose M1 face is a
/// documented under/over-report register over real parser trees.
#[cfg(test)]
mod w5_approx_pins {
    use super::unnecessary_ignore::SPEC;
    use crate::engine::{run_breaking_version_annotations, unnecessary_ignore_late_rows};
    use crate::registry::{EnableSet, Group};
    use crate::testsupport::{Fixture, one_rule};

    /// `unnecessary_ignore` plus the cited rule — the two-set shape the
    /// matched-citation pins run under.
    fn with_unnecessary_ignore(cited: &str) -> EnableSet {
        EnableSet::resolve(["unnecessary_ignore", cited]).0
    }

    // —— comment_references ————————————————————————————————————————————————

    /// `visitComment`'s parser special cases: `[this]` reports at the
    /// reference text.
    #[test]
    fn comment_references_keyword_reports() {
        let f = Fixture::parse("/// See [this].\nclass A {}\n");
        let lints = f.run(&one_rule("comment_references"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The referenced name isn't visible in scope."
        );
        // `reportAtOffset(token.offset + leftIndex + 1, reference.length)`
        // (comment_references.dart:72-73) — the reference text inside the
        // brackets: `this` at 9..13.
        assert_eq!(lints[0].range, 9..13);
    }

    /// The upstream corpus `test_false` — the nested `[[false]]` markup
    /// (code `/// [[false]]`) reports at the inner text (the scan advance
    /// divergence, documented in the SPEC).
    #[test]
    fn comment_references_nested_keyword_reports_at_inner_text() {
        let f = Fixture::parse("/// [[false]]\nclass A {}\n");
        let lints = f.run(&one_rule("comment_references"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range, 6..11);
    }

    /// A Java-style block doc comment carries the same face.
    #[test]
    fn comment_references_block_doc_reports() {
        let f = Fixture::parse("/** [null] */\nclass A {}\n");
        let lints = f.run(&one_rule("comment_references"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        // Same reference-text anchor — `null` at 5..9.
        assert_eq!(lints[0].range, 5..9);
    }

    /// The plain-run fallback: without the semantic query face every
    /// non-keyword reference stays silent (the historical under-report
    /// posture — the resolution face needs `Check::semantics`), and a
    /// plain `//` comment is not a doc comment.
    #[test]
    fn comment_references_resolution_face_stays_silent() {
        let f = Fixture::parse("/// See [Foo] and [String].\nclass A {}\n");
        assert!(f.run(&one_rule("comment_references")).is_empty());

        let f = Fixture::parse("/// Text [y].\nclass A {}\n");
        assert!(f.run(&one_rule("comment_references")).is_empty());

        let f = Fixture::parse("// [this]\nclass A {}\n");
        assert!(f.run(&one_rule("comment_references")).is_empty());
    }

    /// The upstream corpus's unresolvable POSITIVE shapes (lints-new
    /// `comment_references.dart:33/38/43/48/53`), each one row at the
    /// reference text: an unknown bare name, a dotted name whose head
    /// resolves to nothing, a twice-dotted name, a `[y]:` line-initial
    /// colon shape (the colon is only a link definition when the bracket
    /// opens the line), and the constructor doc whose `[x]` cannot see the
    /// private named parameter (`this._x` — the formal's own name is
    /// `_x`, the field's `_x`; `x` resolves nowhere).
    #[test]
    fn comment_references_unresolvable_sites_report() {
        let f = Fixture::parse("/// Text [y].\nclass CUnknown {}\n");
        let lints = f.run_semantic(&one_rule("comment_references"));
        assert_eq!(lints.len(), 1, "{lints:?}");

        let f = Fixture::parse("/// Parameter [y.z].\nclass CUnknownDotted {}\n");
        assert_eq!(f.run_semantic(&one_rule("comment_references")).len(), 1);

        let f = Fixture::parse("/// Parameter [x.y.z].\nclass CUnknownTwiceDotted {}\n");
        assert_eq!(f.run_semantic(&one_rule("comment_references")).len(), 1);

        let f = Fixture::parse("/// Parameter [y]: z.\nvoid fUnknownFollowedByColon(int x) {}\n");
        assert_eq!(f.run_semantic(&one_rule("comment_references")).len(), 1);

        let f = Fixture::parse(
            "class CPrivateNamedInvalid {\n  final int _x;\n\n  /// [x]\n  \
             CPrivateNamedInvalid({required this._x});\n}\n",
        );
        assert_eq!(f.run_semantic(&one_rule("comment_references")).len(), 1);
    }

    /// The resolvable mirrors stay silent: a core type name, a core
    /// named-constructor reference, the enclosing field, the method's own
    /// parameter, the constructor's `this.x` initializing formal, and the
    /// private named form (`[_x]` over `this._x`).
    #[test]
    fn comment_references_resolvable_sites_stay_silent() {
        let sources = [
            "/// [String] is OK.\nclass CImported {}\n",
            "/// Text [Future.delayed].\nclass CPrefixedTearoff {}\n",
            "class CField {\n  int x = 0;\n  /// Assigns to [x].\n  void m() {}\n}\n",
            "class CParameter {\n  /// Reads [x].\n  void m(int x) {}\n}\n",
            "class CParameterConstructorField {\n  final int x;\n\n  /// [x]\n  \
             CParameterConstructorField(this.x);\n}\n",
            "class CPrivateNamedValid {\n  final int _x;\n\n  /// [_x]\n  \
             CPrivateNamedValid({required this._x});\n}\n",
        ];
        for source in sources {
            let f = Fixture::parse(source);
            let lints = f.run_semantic(&one_rule("comment_references"));
            assert!(lints.is_empty(), "{source:?} -> {lints:?}");
        }
    }

    // —— unnecessary_ignore ————————————————————————————————————————————————

    /// The upstream primary code's texts, mirrored verbatim
    /// (`diagnostic.g.dart` `unnecessaryIgnore`).
    #[test]
    fn unnecessary_ignore_texts_mirror_the_official() {
        assert_eq!(
            SPEC.problem,
            "The diagnostic '{0}' isn't produced at this location so it doesn't need \
             to be ignored."
        );
        assert_eq!(SPEC.correction, "Try removing the ignore comment.");
    }

    /// The registration shape: `kinds: &[]` — the verdict is the analyzer's
    /// `IgnoreValidator` post-pass (unnecessary_ignore.dart:34-48), which no
    /// per-node check can reproduce; the rule emits through the engine's
    /// run-epilogue mount (`unnecessary_ignore_rows`, engine.rs). The #1153
    /// closeout pins this name into `STRUCTURALLY_SILENT` with the freeze
    /// regen (the emittable derivation flips at the serial-phase re-freeze).
    #[test]
    fn unnecessary_ignore_is_structurally_silent() {
        assert_eq!(SPEC.name, "unnecessary_ignore");
        assert_eq!(SPEC.group, Group::Standalone);
        assert!(SPEC.kinds.is_empty(), "{:?}", SPEC.kinds);
    }

    /// The core face end to end: a citation the run never matches reports at
    /// the cited name token, the corpus shape of `unnecessary_ignore`'s
    /// published example (`// ignore: not_a_real_code` over a file that
    /// produces nothing by that name).
    #[test]
    fn unused_citation_reports_at_the_name_token() {
        let f = Fixture::parse("// ignore: avoid_empty_else\nvar x = 1;\n");
        let lints = f.run(&one_rule("unnecessary_ignore"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range, 11..27, "the cited name token");
        assert_eq!(
            lints[0].message,
            "The diagnostic 'avoid_empty_else' isn't produced at this location \
             so it doesn't need to be ignored."
        );
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try removing the ignore comment.")
        );
    }

    /// A citation that dropped a real row stays silent: the ignore
    /// suppression happens at the emit face, the epilogue sees the drop in
    /// the run's record, and the matched citation never reports — the
    /// official match pass (`ignore_validator.dart:137-144`).
    #[test]
    fn matched_citation_stays_silent() {
        let f = Fixture::parse("void f() {\n  if (b) ; else ; // ignore: avoid_empty_else\n}\n");
        assert!(
            f.run(&with_unnecessary_ignore("avoid_empty_else"))
                .is_empty()
        );
    }

    /// Line scope: the name firing elsewhere never feeds another line's
    /// citation — the official match pass removes from the diagnostic's own
    /// line list only (`ignoredOnLine?.removeByName`); the unmatched line's
    /// citation reports.
    #[test]
    fn line_scoped_citations_match_their_own_line_only() {
        let f = Fixture::parse(concat!(
            "void f() {\n",
            "  // ignore: avoid_empty_else\n",
            "  if (b) ; else ;\n",
            "}\n",
            "// ignore: avoid_empty_else\n",
            "var y = 2;\n",
        ));
        let lints = f.run(&with_unnecessary_ignore("avoid_empty_else"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        // The citation comment sits at byte 61 (`// ignore: ` = 11 columns),
        // so the name token is 72..88 — the same anchor
        // `unused_citation_reports_at_the_name_token` pins.
        assert_eq!(lints[0].range, 72..88, "the second citation's name token");
    }

    /// File-wide scope: a `ignore_for_file:` citation is fed by a drop
    /// anywhere in the run (`ignoredForFile.removeByName`) — matched stays
    /// silent, unmatched reports.
    #[test]
    fn file_wide_citations_match_anywhere_in_the_run() {
        let set = with_unnecessary_ignore("avoid_empty_else");
        let f = Fixture::parse(
            "// ignore_for_file: avoid_empty_else\nvoid f() {\n  if (b) ; else ;\n}\n",
        );
        assert!(f.run(&set).is_empty());
        let f = Fixture::parse("// ignore_for_file: avoid_empty_else\nvar x = 1;\n");
        let lints = f.run(&set);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range, 20..36);
    }

    /// An exact duplicate leaves the dedup'd citation walk (the official
    /// duplicate pass removes it before the unnecessary walk,
    /// ignore_validator.dart:97-106) — the surviving first occurrence
    /// reports alone.
    #[test]
    fn duplicate_citation_leaves_one_row() {
        let f = Fixture::parse("// ignore: avoid_empty_else, avoid_empty_else\nvar x = 1;\n");
        let lints = f.run(&one_rule("unnecessary_ignore"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range, 11..27, "the first occurrence");
    }

    /// Names outside the registry never report — the official plugin-code
    /// bail (`ignore_validator.dart:211-218`; analyzer error codes this
    /// lint-only run never produces sit on the same face), and a removed
    /// rule's name belongs to the `removed_lint_use` face (the official
    /// removed-state `continue`, :219-235).
    #[test]
    fn unknown_and_removed_names_stay_silent() {
        let f = Fixture::parse("// ignore: totally_unknown_code\nvar x = 1;\n");
        assert!(f.run(&one_rule("unnecessary_ignore")).is_empty());
        let f = Fixture::parse("// ignore: super_goes_last\nvar x = 1;\n");
        assert!(f.run(&one_rule("unnecessary_ignore")).is_empty());
    }

    /// The epilogue's own rows pass the suppression gate like every lint
    /// (the official validator runs before the pipeline's ignore filter,
    /// `library_analyzer.dart:135`). The self-referencing shape: the
    /// `unnecessary_ignore` citation (line 1, targeting line 2) suppresses
    /// the verdict on the avoid_empty_else citation below, but produces one
    /// of its own — a citation nothing suppressed is itself unnecessary, and
    /// no entry targets the self-citation's own line.
    #[test]
    fn the_verdict_itself_is_ignorable() {
        let f = Fixture::parse(
            "// ignore: unnecessary_ignore\n// ignore: avoid_empty_else\nvar x = 1;\n",
        );
        let lints = f.run(&one_rule("unnecessary_ignore"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range, 11..29, "the self-citation's own row");
    }

    // —— unnecessary_ignore late-join tail (#1904) ———————————————————————

    /// The shell shape end to end: a line-scoped citation (its scope is
    /// the comment's own line plus the next) dropping the breaking-version
    /// walk's row records the drop and stays silent in the re-join — the
    /// official match pass removes the name from the diagnostic's line
    /// list (`ignore_validator.dart:137-144`), the same suppression gate
    /// every row passes before the pipeline's ignore filter
    /// (`library_analyzer.dart`:135).
    #[test]
    fn late_join_citation_that_dropped_a_row_stays_silent() {
        let f = Fixture::parse(concat!(
            "// ignore: remove_deprecations_in_breaking_versions\n",
            "@Deprecated('3.0')\n",
            "var x = 1;\n",
        ));
        let set = with_unnecessary_ignore("remove_deprecations_in_breaking_versions");
        let unit = f.unit();
        let unignorable = rustc_hash::FxHashSet::default();
        let (late_rows, late_drops) =
            run_breaking_version_annotations(&unit, &set, &unignorable, "2.0.0");
        assert!(late_rows.is_empty(), "{late_rows:?}");
        assert_eq!(late_drops.len(), 1, "{late_drops:?}");
        assert!(unnecessary_ignore_late_rows(&unit, &set, &unignorable, &late_drops).is_empty());
        // The plain-run face never owns the late name — the run half stays
        // silent (the #1353 posture).
        assert!(f.run(&set).is_empty());
    }

    /// The tail window: the citation's scope covers its own line and the
    /// next — the annotation sits past both, the walk's row emits
    /// unconstrained, and the citation dropped nothing. The re-join
    /// reports it at the cited name token: the official validator walks
    /// the unfiltered final list where no row of this name was removed
    /// (`library_analyzer.dart`:327-341).
    #[test]
    fn late_join_citation_that_dropped_nothing_reports() {
        let f = Fixture::parse(concat!(
            "// ignore: remove_deprecations_in_breaking_versions\n",
            "var x = 1;\n",
            "@Deprecated('3.0')\n",
            "var y = 2;\n",
        ));
        let set = with_unnecessary_ignore("remove_deprecations_in_breaking_versions");
        let unit = f.unit();
        let unignorable = rustc_hash::FxHashSet::default();
        let (late_rows, late_drops) =
            run_breaking_version_annotations(&unit, &set, &unignorable, "2.0.0");
        assert_eq!(late_rows.len(), 1, "{late_rows:?}");
        assert!(late_drops.is_empty(), "{late_drops:?}");
        let verdicts = unnecessary_ignore_late_rows(&unit, &set, &unignorable, &late_drops);
        assert_eq!(verdicts.len(), 1, "{verdicts:?}");
        assert_eq!(verdicts[0].range, 11..51, "the cited name token");
        assert_eq!(
            verdicts[0].message,
            "The diagnostic 'remove_deprecations_in_breaking_versions' isn't produced \
             at this location so it doesn't need to be ignored."
        );
    }

    /// The gate off — the walk never ran, the drop record is empty. The
    /// official validator answers the same: `getRule` finds the known rule
    /// regardless of registration and falls through the state checks
    /// (`ignore_validator.dart`:222-248), the match pass removes nothing,
    /// and the citation reports.
    #[test]
    fn late_join_citation_without_the_breaking_gate_reports() {
        let f = Fixture::parse("// ignore: remove_deprecations_in_breaking_versions\nvar x = 1;\n");
        let set = with_unnecessary_ignore("remove_deprecations_in_breaking_versions");
        let unit = f.unit();
        let verdicts =
            unnecessary_ignore_late_rows(&unit, &set, &rustc_hash::FxHashSet::default(), &[]);
        assert_eq!(verdicts.len(), 1, "{verdicts:?}");
        assert_eq!(verdicts[0].range, 11..51, "the cited name token");
    }

    /// The plain-run fallback: without the mounting shell, a bare run
    /// keeps the #1353 silence on the late-join names even on a citation
    /// that dropped nothing — the verdict belongs to the shell's re-join
    /// (`unnecessary_ignore_late_rows`).
    #[test]
    fn plain_run_keeps_the_late_join_posture() {
        let f = Fixture::parse("// ignore: remove_deprecations_in_breaking_versions\nvar x = 1;\n");
        assert!(
            f.run(&with_unnecessary_ignore(
                "remove_deprecations_in_breaking_versions"
            ))
            .is_empty()
        );
    }
}

/// The GAP-8 close (#1903): the resolution face resolves comment
/// references through the semantic query face — every upstream corpus
/// face pinned at both poles, with the undecidable residuals staying
/// silent.
#[cfg(test)]
mod comment_references_resolution_pins {
    use crate::registry::EnableSet;
    use crate::testsupport::{Fixture, one_rule};

    fn run_semantic(source: &str) -> Vec<crate::engine::Lint> {
        Fixture::parse(source).run_semantic(&one_rule("comment_references"))
    }

    /// The scanner-dirty sibling — recovery fixtures that carry a scanner
    /// diagnostic by design (e.g. an unterminated block doc).
    fn run_semantic_scanner_dirty(source: &str) -> Vec<crate::engine::Lint> {
        Fixture::parse(source).run_semantic_scanner_dirty(&one_rule("comment_references"))
    }

    /// The reference text's own span, `reportAtNode(expression)`.
    fn span_of(source: &str, needle: &str) -> core::ops::Range<u32> {
        let start = source.find(needle).expect("needle in source") as u32 + 1;
        start..start + needle.len() as u32 - 2
    }

    /// `test_unknownElement` — an unresolved bare name reports.
    #[test]
    fn unresolved_bare_name_reports() {
        let source = "/// Text [y].\nclass C {}\n";
        let lints = run_semantic(source);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range, span_of(source, "[y]"));
        assert_eq!(
            lints[0].message,
            "The referenced name isn't visible in scope."
        );
    }

    /// `test_unknownElement_dottedName` — a fully unresolved `a.b` reports
    /// at the whole reference text.
    #[test]
    fn unresolved_dotted_name_reports() {
        let source = "/// Parameter [y.z].\nclass C {}\n";
        let lints = run_semantic(source);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range, span_of(source, "[y.z]"));
    }

    /// `test_unknownElement_twiceDottedName` — the three-part face reports
    /// the full text (the official's early return leaves the third
    /// component unresolved whenever the head is not an import prefix).
    #[test]
    fn unresolved_twice_dotted_name_reports() {
        let source = "/// Parameter [x.y.z].\nclass C {}\n";
        let lints = run_semantic(source);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range, span_of(source, "[x.y.z]"));
    }

    /// `test_unknownElement_followedByColon` — a mid-comment `]:` is not a
    /// link-reference definition; the reference reports.
    #[test]
    fn colon_tail_still_reports() {
        let source = "/// Parameter [y]: z.\nvoid f(int x) {}\n";
        let lints = run_semantic(source);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range, span_of(source, "[y]"));
    }

    /// `test_field` — the enclosing container's members refute.
    #[test]
    fn resolved_member_stays_silent() {
        let source = "class C {\n  int x = 0;\n  /// Assigns to [x].\n  void m() {}\n}\n";
        assert!(run_semantic(source).is_empty(), "{source:?}");
    }

    /// `test_parameter` — the attaching declaration's formals refute.
    #[test]
    fn resolved_formal_stays_silent() {
        let source = "class C {\n  /// Reads [x].\n  void m(int x) {}\n}\n";
        assert!(run_semantic(source).is_empty(), "{source:?}");
    }

    /// The constructor-formal trio: the initializing formal refutes under
    /// its parameter spelling (`this._x` names `_x`), so the private-named
    /// valid face stays silent and the invalid `[x]` reports (upstream
    /// issue 62768 pair).
    #[test]
    fn constructor_initializing_formal_faces() {
        let valid = "class A {\n  final int x;\n\n  /// [x]\n  A(this.x);\n}\n";
        assert!(run_semantic(valid).is_empty(), "{valid:?}");

        let private_valid =
            "class A {\n  final int _x;\n\n  /// [_x]\n  A({required this._x});\n}\n";
        assert!(run_semantic(private_valid).is_empty(), "{private_valid:?}");

        let invalid = "class A {\n  final int _x;\n\n  /// [x]\n  A({required this._x});\n}\n";
        let lints = run_semantic(invalid);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range, span_of(invalid, "[x]"));
    }

    /// `test_parameter_constructor_super` — the super formal's parameter
    /// spelling refutes.
    #[test]
    fn super_parameter_stays_silent() {
        let source =
            "class A {\n  A(int x);\n}\n\nclass B extends A {\n  /// [x]\n  B(super.x);\n}\n";
        assert!(run_semantic(source).is_empty(), "{source:?}");
    }

    /// `test_typeAlias` — a library-scope name (a local typedef) refutes.
    #[test]
    fn resolved_typedef_stays_silent() {
        let source = "/// Text [Td].\nclass C {}\n\ntypedef Td = C;\n";
        assert!(run_semantic(source).is_empty(), "{source:?}");
    }

    /// The two-part container face at both poles: a declared member is
    /// silent, a missing one reports (the chain is fully walked).
    #[test]
    fn container_tail_faces() {
        let silent =
            "class D {\n  void wait() {}\n}\n\nclass C {\n  /// Text [D.wait].\n  void m() {}\n}\n";
        assert!(run_semantic(silent).is_empty(), "{silent:?}");

        let report = "class D {}\n\nclass C {\n  /// Text [D.zzz].\n  void m() {}\n}\n";
        let lints = run_semantic(report);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range, span_of(report, "[D.zzz]"));
    }

    /// An alias head never reaches the tail lookup (the unwrap is outside
    /// the model) — the silent direction.
    #[test]
    fn alias_head_stays_silent() {
        let source = "class B {\n  int get x => 7;\n}\ntypedef B2 = B;\n\nclass C {\n  /// Text [B2.x].\n  void m() {}\n}\n";
        assert!(run_semantic(source).is_empty(), "{source:?}");
    }

    /// The extension channel: a member provided by an extension face of
    /// the library in scope refutes the bare face's report.
    #[test]
    fn extension_member_stays_silent() {
        let source =
            "extension E on C {\n  int get y => 1;\n}\n\nclass C {\n  /// [y]\n  void m() {}\n}\n";
        let lints = run_semantic(source);
        assert!(lints.is_empty(), "{source:?}");
    }

    /// An unindexed superclass leaves the chain unknowable — silence, the
    /// never-fabricate direction.
    #[test]
    fn unindexed_superclass_stays_silent() {
        let source = "class C extends Nope {\n  /// [y]\n  void m() {}\n}\n";
        assert!(run_semantic(source).is_empty(), "{source:?}");
    }

    /// A setter-only member refutes under its plain doc spelling (the
    /// `name=` probe).
    #[test]
    fn setter_only_member_stays_silent() {
        let source = "class C {\n  set x(int v) {}\n\n  /// [x]\n  void m() {}\n}\n";
        assert!(run_semantic(source).is_empty(), "{source:?}");
    }

    /// An enum's own doc comment sees its constants (the header sits in
    /// the instance scope, `scope_context.dart` :201-203).
    #[test]
    fn enum_constant_stays_silent() {
        let source = "/// [a]\nenum E { a, b }\n";
        assert!(run_semantic(source).is_empty(), "{source:?}");
    }

    /// `test_markdown_inlineLink` (+ the one-period variant) — a `](`-tail
    /// is an inline link, not a reference.
    #[test]
    fn markdown_inline_link_stays_silent() {
        let source = "/// A link to [Sha256](http://tools.ietf.org/html/rfc6234) hash function.\nclass C {}\n";
        assert!(run_semantic(source).is_empty(), "{source:?}");

        let dotted = "/// A link to [Sha256.Sha256](http://x) hash function.\nclass C {}\n";
        assert!(run_semantic(dotted).is_empty(), "{dotted:?}");
    }

    /// `test_markdown_referenceLink` — the `][` shape skips the first
    /// bracket, the second becomes a reference, and the group's collected
    /// `[bar]: url` definition exempts it.
    #[test]
    fn reference_link_definition_exempts() {
        let source =
            "/// A link to [Foo][bar] hash function.\n///\n/// [bar]: http://x\nclass C {}\n";
        assert!(run_semantic(source).is_empty(), "{source:?}");
    }

    /// A line-initial `]:` definition is not a reference on its own line.
    #[test]
    fn link_definition_line_stays_silent() {
        let source = "/// [bar]: http://x\nclass C {}\n";
        assert!(run_semantic(source).is_empty(), "{source:?}");
    }

    /// A backtick code span hides its brackets from the resolution face
    /// (the builder skips spans; the keyword face keeps its upstream-verbatim
    /// no-skip posture).
    #[test]
    fn code_span_brackets_stay_silent() {
        let source = "/// A `[y]` span.\nclass C {}\n";
        assert!(run_semantic(source).is_empty(), "{source:?}");
    }

    /// `test_unclosedSquareBracket` — no `]`, no reference, no crash.
    #[test]
    fn unclosed_bracket_stays_silent() {
        let source = "/// [\n/// ^--- Should not crash (#819).\nclass C {}\n";
        assert!(run_semantic(source).is_empty(), "{source:?}");
    }

    /// A keyword never heads a resolution reference — the four special
    /// cases belong to the keyword face alone, so the faces never double-
    /// report one bracket.
    #[test]
    fn keyword_brackets_never_double_report() {
        let source = "/// Text [null] and [y].\nclass C {}\n";
        let lints = run_semantic(source);
        // Two faces, two reports — the upstream shape exactly: `null` from
        // the keyword scan (the doc-comment parser never builds references
        // for keywords, so the resolver can never exempt it) and the
        // unresolved `y` from the resolution face. Same reference span
        // anchor, no dedup across faces (upstream reports both through one
        // `visitComment` + one `visitCommentReference` pass).
        assert_eq!(lints.len(), 2, "{lints:?}");
        assert_eq!(lints[0].range, span_of(source, "[null]"));
        assert_eq!(lints[1].range, span_of(source, "[y]"));
    }

    /// The registration shape: the rule now dispatches on the doc-carrying
    /// declarations (the resolution face's per-node scope anchor) with the
    /// unit face kept for the keyword scan.
    #[test]
    fn dispatch_universe_covers_declarations() {
        use super::comment_references::SPEC;
        use dartq_ast::NodeKind;
        assert_eq!(SPEC.name, "comment_references");
        assert!(SPEC.kinds.contains(&NodeKind::CompilationUnit));
        assert!(SPEC.kinds.contains(&NodeKind::ClassDeclaration));
        assert!(SPEC.kinds.contains(&NodeKind::Method));
        assert!(SPEC.kinds.contains(&NodeKind::ExtensionTypeDeclaration));
    }

    /// Silence on an unrelated enable set — the face rides the semantic
    /// query, and a plain run never reports it (the fallback contract).
    #[test]
    fn semantic_run_is_opt_in() {
        let source = "/// Text [y].\nclass C {}\n";
        let plain = EnableSet::resolve(["comment_references"]).0;
        assert!(Fixture::parse(source).run(&plain).is_empty());
    }

    /// A terminated multi-byte block doc behaves as before — no reports
    /// (the #1956 fix changes no (offset, line) pair for closed comments).
    #[test]
    fn terminated_block_doc_multibyte_stays_silent() {
        let source = "/**中文*/\nclass C {}\n";
        assert!(run_semantic(source).is_empty(), "{source:?}");
    }

    /// The #1956 crash fix pinned end to end: the unterminated block doc
    /// with a multi-byte first line rides the real scan → extract →
    /// resolve pipeline and stays silent (the extraction-unit test covers
    /// the slice; this covers the whole run).
    #[test]
    fn unterminated_block_doc_multibyte_end_to_end_stays_silent() {
        let source = "/**中文\nclass C {}";
        assert!(run_semantic_scanner_dirty(source).is_empty(), "{source:?}");
    }

    /// The #1952 Object-root face, bare: the four implicit-interface
    /// members resolve on any container (upstream `_resolveUnqualified`
    /// ends in `inheritance.getMember`, whose chains root at `Object`).
    #[test]
    fn object_member_bare_stays_silent() {
        let source =
            "/// Text [toString], [hashCode], [runtimeType], [noSuchMethod].\nclass C {}\n";
        assert!(run_semantic(source).is_empty(), "{source:?}");
    }

    /// The Object-root face, qualified: `C.toString` / `D.hashCode` reach
    /// the inheritance root through the `_resolveInstanceMember` arm.
    #[test]
    fn object_member_qualified_stays_silent() {
        let source = "class D {}\n\nclass C {\n  /// Text [C.toString] and [D.hashCode].\n  void m() {}\n}\n";
        assert!(run_semantic(source).is_empty(), "{source:?}");
    }

    /// The Object root sits under an indexed written chain too — a class
    /// with no instance members anywhere still resolves the four names.
    #[test]
    fn object_member_over_empty_chain_stays_silent() {
        let source =
            "class D {}\n\nclass C extends D {\n  /// Text [toString].\n  void m() {}\n}\n";
        assert!(run_semantic(source).is_empty(), "{source:?}");
    }

    /// The Object root in an extension's own doc: the bare face reaches it
    /// through the receiver (`on`) type upstream. Residual: an extension
    /// whose `on` type is a function type reports upstream — this pin is
    /// the interface shape only.
    #[test]
    fn object_member_in_extension_doc_stays_silent() {
        let source = "extension E on int {\n  /// Text [toString].\n  void m() {}\n}\n";
        assert!(run_semantic(source).is_empty(), "{source:?}");
    }

    /// The fix is name-scoped: an unknown member on a resolved container
    /// head still reports (no blanket silencing of the container face).
    #[test]
    fn object_member_scoping_control_reports() {
        let source = "/// Text [C.zzz].\nclass C {}\n";
        let lints = run_semantic(source);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range, span_of(source, "[C.zzz]"));
    }

    /// The extension-head channel (`test_extension_instanceGetter_qualified`):
    /// a named extension dual-registers as a container, so its own
    /// instance getter resolves through the two-part face.
    #[test]
    fn extension_head_member_stays_silent() {
        let source = "extension E on int {\n  int get foo => 0;\n}\n\n/// [E.foo]\nvoid f() {}\n";
        assert!(run_semantic(source).is_empty(), "{source:?}");
    }

    /// The extension-head control: a miss on the same face reports (the
    /// chain is fully walked — this also proves the channel is really
    /// resolving, not skipping the doc).
    #[test]
    fn extension_head_miss_reports() {
        let source = "extension E on int {\n  int get foo => 0;\n}\n\n/// [E.zzz]\nvoid f() {}\n";
        let lints = run_semantic(source);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range, span_of(source, "[E.zzz]"));
    }

    /// The kind-gate discriminator: upstream reports `[E.toString]` — an
    /// extension is not an `InterfaceElement`, so its head never reaches
    /// the inheritance root in the qualified arm. If this goes red, the
    /// Object-root gate leaked `Extension` into a qualified face.
    #[test]
    fn extension_head_object_member_reports() {
        let source =
            "extension E on int {\n  int get foo => 0;\n}\n\n/// [E.toString]\nvoid f() {}\n";
        let lints = run_semantic(source);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range, span_of(source, "[E.toString]"));
    }

    /// A resolved non-container head (`_resolveTwoComponents` matches the
    /// `PrefixElement`/`ContainerElement` arms only): a top-level function
    /// or variable head leaves the tail unresolved and the reference
    /// reports; an alias head stays silent (the unwrap is outside the
    /// model — both sides silent there).
    #[test]
    fn non_container_head_faces() {
        let function_head = "void f() {}\n\n/// Text [f.foo].\nclass C {}\n";
        let lints = run_semantic(function_head);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range, span_of(function_head, "[f.foo]"));

        let variable_head = "final v = 0;\n\n/// Text [v.foo].\nclass C {}\n";
        let lints = run_semantic(variable_head);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range, span_of(variable_head, "[v.foo]"));

        let alias_head = "typedef A = int;\n\n/// Text [A.foo].\nclass C {}\n";
        assert!(run_semantic(alias_head).is_empty(), "{alias_head:?}");
    }
}
