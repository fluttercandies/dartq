//! Conservative extraction from Dart SDK sources: the flat `libraries.dart`
//! existence table and the top-level public declaration surface of one
//! compilation unit.
//!
//! The SDK `lib/` tree is machine-formatted (top-level declarations start at
//! column 0, continuations are indented), so a column-0 line scanner with
//! comment stripping extracts the surface without a Dart parser. Every rule
//! is deliberately narrow: anything the scanner cannot confidently classify
//! is surfaced as a warning, never guessed (a partial face silently joining
//! the engine map would flip used imports into false `unused_import`s).

use std::collections::BTreeSet;

/// One top-level `export` directive of a library file.
pub struct Export {
    /// The exported URI as written (`dart:async`, `dart:_internal`, …).
    pub target: String,
    /// `show a, b` list; `None` when the directive shows nothing (wildcard
    /// export modulo `hide`).
    pub show: Option<Vec<String>>,
    /// `hide a, b` list.
    pub hide: Vec<String>,
}

/// The extracted top-level surface of one compilation unit.
#[derive(Default)]
pub struct UnitScan {
    /// Public top-level names; setters under their `name=` namespace
    /// spelling. `_`-private names excluded.
    pub names: BTreeSet<String>,
    /// `part 'x.dart';` targets of a library file, source order.
    pub parts: Vec<String>,
    /// Top-level `export` directives in source order.
    pub exports: Vec<Export>,
    /// Column-0 declaration-shaped lines no rule could classify. Never
    /// silently dropped — a non-empty list fails the run (an unscanned
    /// declaration would silently shrink the face).
    pub warnings: Vec<String>,
}

/// One flat-table entry: the library name, its default source path, and the
/// dart2js implementation variant when the entry carries one
/// (`html_common`'s face lives in the variant file — the base file is the
/// platform-neutral shell the variant's exports flow through).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatEntry {
    pub name: String,
    pub path: String,
    pub dart2js_path: Option<String>,
}

impl FlatEntry {
    /// The file whose top-level surface is the library's face.
    pub fn face_path(&self) -> &str {
        self.dart2js_path.as_deref().unwrap_or(&self.path)
    }
}

/// Parses the oracle's flat library table out of
/// `lib/_internal/sdk_library_metadata/lib/libraries.dart`: every
/// `'name': const LibraryInfo('path', …)` entry of the `libraries` map,
/// source order preserved.
pub fn parse_flat_table(text: &str) -> Result<Vec<FlatEntry>, String> {
    const MARKER: &str = "libraries = const {";
    let start = text
        .find(MARKER)
        .ok_or("libraries.dart: `libraries = const {` map not found")?;
    let body = &text[start + MARKER.len()..];
    let end = body
        .find("\n};")
        .ok_or("libraries.dart: map terminator `};` not found")?;
    let body = &body[..end];

    const HEAD: &str = "const LibraryInfo(";
    let mut heads = Vec::new();
    let mut cursor = 0;
    while let Some(rel) = body[cursor..].find(HEAD) {
        heads.push(cursor + rel);
        cursor += rel + HEAD.len();
    }
    if heads.is_empty() {
        return Err("libraries.dart: no `LibraryInfo` entries parsed".to_owned());
    }
    let mut entries = Vec::new();
    for (i, &at) in heads.iter().enumerate() {
        let window_end = heads.get(i + 1).copied().unwrap_or(body.len());
        let window = &body[at + HEAD.len()..window_end];
        let name = quoted_before(&body[..at])
            .ok_or_else(|| format!("libraries.dart: no quoted name before entry at byte {at}"))?;
        let path = first_quoted(window)
            .ok_or_else(|| format!("libraries.dart: no path quoted in entry `{name}`"))?;
        let dart2js_path = window
            .find("dart2jsPath:")
            .and_then(|pos| first_quoted(&window[pos..]));
        entries.push(FlatEntry {
            name,
            path,
            dart2js_path,
        });
    }
    Ok(entries)
}

/// The quoted map-entry name preceding `const LibraryInfo(`: the segment
/// between the name's closing quote and the head must be exactly the `:`
/// separator — a doc-comment mention of `LibraryInfo` further up must not
/// read as an entry.
fn quoted_before(text: &str) -> Option<String> {
    let quote = text.rfind(['\'', '"'])?;
    let q = text.as_bytes()[quote] as char;
    let open = text[..quote].rfind(q)?;
    let name = text[open + 1..quote].to_owned();
    (text[quote + 1..].trim() == ":").then_some(name)
}

/// The first quoted string of `text`.
fn first_quoted(text: &str) -> Option<String> {
    let quote = text.find(['\'', '"'])?;
    let q = text.as_bytes()[quote] as char;
    let close = text[quote + 1..].find(q)?;
    Some(text[quote + 1..quote + 1 + close].to_owned())
}

/// Scans one compilation unit's top-level public surface.
pub fn scan_unit(text: &str) -> UnitScan {
    let mut scan = UnitScan::default();
    let mut in_block_comment = false;
    // The active multi-line (triple-quoted) string, if any: SDK sources
    // embed doc examples and `toString` bodies whose literal content lines
    // start at column 0 and must never read as declarations.
    let mut in_string: Option<&'static str> = None;
    // A multi-line `export` directive accumulates until its `;`.
    let mut export_acc: Option<String> = None;
    for raw in text.lines() {
        // Column-0 anchor: continuation lines are indented, so top-level
        // classification only ever considers unindented lines (the SDK tree
        // is formatted).
        let top_level = !raw.starts_with([' ', '\t']);
        let mut line = raw;
        if let Some(triple) = in_string {
            if let Some(pos) = line.find(triple) {
                in_string = None;
                let _ = pos;
            }
            continue;
        }
        if in_block_comment {
            match line.find("*/") {
                Some(end) => {
                    in_block_comment = false;
                    line = &line[end + 2..];
                }
                None => continue,
            }
        }
        if let Some(pos) = find_line_comment(line) {
            line = &line[..pos];
        }
        // Inline block comments are spliced out (replaced by a space) so the
        // declaration tail after them still classifies; an unclosed one
        // enters block-comment state.
        let mut line = line.to_owned();
        while let Some(pos) = line.find("/*") {
            match line[pos..].find("*/") {
                Some(end) => {
                    line.replace_range(pos..pos + end + 2, " ");
                }
                None => {
                    in_block_comment = true;
                    line.truncate(pos);
                    break;
                }
            }
        }
        let line = line.trim_end();
        for triple in ["\"\"\"", "'''"] {
            if line.matches(triple).count() % 2 == 1 {
                in_string = Some(triple);
            }
        }
        if in_string.is_some() {
            // The string opener itself declares nothing.
            continue;
        }
        if line.is_empty() {
            continue;
        }

        if let Some(acc) = &mut export_acc {
            acc.push(' ');
            acc.push_str(line);
            if line.ends_with(';') {
                match parse_export(acc) {
                    Some(export) => scan.exports.push(export),
                    None => scan.warnings.push(acc.clone()),
                }
                export_acc = None;
            }
            continue;
        }
        if !top_level {
            continue;
        }
        if line.starts_with("export ") {
            if line.ends_with(';') {
                match parse_export(line) {
                    Some(export) => scan.exports.push(export),
                    None => scan.warnings.push(line.to_owned()),
                }
            } else {
                export_acc = Some(line.to_owned());
            }
        } else if line.starts_with("part ") && !line.starts_with("part of") {
            if let Some(part) = first_quoted(line) {
                scan.parts.push(part);
            }
        } else if !directive_or_noise(line) {
            match classify_decl(line) {
                Some(name) if !name.starts_with('_') => {
                    scan.names.insert(name);
                }
                Some(_) => {}
                None => scan.warnings.push(line.to_owned()),
            }
        }
    }
    if let Some(acc) = export_acc {
        scan.warnings.push(acc);
    }
    scan
}

/// The offset of a line comment: `//` at line start or preceded by
/// whitespace. A `://` inside a URI literal is preceded by `:` and survives.
fn find_line_comment(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    if bytes.first() == Some(&b'/') && bytes.get(1) == Some(&b'/') {
        return Some(0);
    }
    for i in 1..bytes.len().saturating_sub(1) {
        if bytes[i] == b'/' && bytes[i + 1] == b'/' && bytes[i - 1].is_ascii_whitespace() {
            return Some(i);
        }
    }
    None
}

/// Lines that begin a top-level region without declaring a name.
fn directive_or_noise(line: &str) -> bool {
    // `library dart.x;` and the unnamed `library;` shape alike.
    let first = split_first_word(line).map(|(head, _)| head);
    (first == Some("library") || first == Some("library;"))
        || line.starts_with("import ")
        || line.starts_with("part of")
        || line.starts_with('@')
        // Closers of multi-line annotations, literals, and declarations
        // land at column 0.
        || line.starts_with(')')
        || line.starts_with(']')
        || line.starts_with('}')
        || line.starts_with(';')
}

/// Parses one (possibly multi-line, already space-joined) `export`
/// directive: `export 'uri' show a, b hide c;`.
fn parse_export(directive: &str) -> Option<Export> {
    let target = first_quoted(directive)?;
    let rest_start = directive.find(&target)? + target.len() + 1;
    let rest = &directive[rest_start..];
    let names = |segment: &str| -> Vec<String> {
        segment
            .split(',')
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .collect()
    };
    let mut show = None;
    let mut hide = Vec::new();
    if let Some(pos) = rest.find(" show ") {
        let tail = &rest[pos + 6..];
        let end = tail
            .find(" hide ")
            .unwrap_or_else(|| tail.find(';').unwrap_or(tail.len()));
        show = Some(names(&tail[..end]));
    }
    if let Some(pos) = rest.find(" hide ") {
        let tail = &rest[pos + 6..];
        let end = tail.find(';').unwrap_or(tail.len());
        hide = names(&tail[..end]);
    }
    Some(Export { target, show, hide })
}

/// The declared name of a column-0 top-level declaration, or `None` when no
/// rule claims the line (surfaced as a warning, never guessed).
fn classify_decl(line: &str) -> Option<String> {
    if let Some(name) = type_decl_name(line) {
        return Some(name);
    }
    if let Some(name) = accessor_name(line, "get", false) {
        return Some(name);
    }
    if let Some(name) = accessor_name(line, "set", true) {
        return Some(format!("{name}="));
    }
    if keyword_prefixed(line) {
        // A parenthesis before any `=` marks an external function
        // (`external T min<T extends num>(…);`) — its name is the identifier
        // before the parameter list, generic block walked back. Otherwise
        // the declaration is a variable (`external final int x;`).
        let function_shaped = match line.find('=') {
            Some(eq) => line[..eq].contains('('),
            None => line.contains('('),
        };
        return if function_shaped {
            function_name(line).or_else(|| var_or_external_name(line))
        } else {
            var_or_external_name(line)
        };
    }
    if let Some(name) = function_name(line) {
        return Some(name);
    }
    // A bare top-level variable with no keyword prefix: `Type name;` or
    // `Type? name = …;` — the declared name is the last identifier before
    // the first `=`/`;`.
    bare_variable_name(line)
}

/// The declared name of a type-family declaration: class (with its modifier
/// run), `mixin class`, mixin, enum, `extension [type] [const]?`, or
/// typedef (both the `Name = …` and old-style `Ret name(…)` shapes).
fn type_decl_name(line: &str) -> Option<String> {
    let (head, tail) = split_first_word(line)?;
    match head {
        "class" | "enum" => first_ident(tail).map(str::to_owned),
        "abstract" | "base" | "final" | "interface" | "sealed" => type_decl_name(tail),
        "mixin" => {
            let (next, rest) = split_first_word(tail)?;
            if next == "class" {
                first_ident(rest)
            } else {
                first_ident(tail)
            }
            .map(str::to_owned)
        }
        "extension" => {
            let (next, rest) = split_first_word(tail)?;
            match next {
                "type" => {
                    let (third, rest) = split_first_word(rest)?;
                    if third == "const" {
                        first_ident(rest)
                    } else {
                        first_ident(third)
                    }
                }
                _ => first_ident(tail),
            }
            .map(str::to_owned)
        }
        "typedef" => typedef_name(tail),
        _ => None,
    }
}

/// The typedef name: `Name<T> = …` (the token after `typedef`, cut at its
/// generic parameters) when an `=` precedes the first parenthesis, else the
/// old-style `Ret name(…)` shape (the identifier before the first `(`).
fn typedef_name(tail: &str) -> Option<String> {
    let paren = tail.find('(').unwrap_or(tail.len());
    let has_eq = tail[..paren].contains('=');
    if has_eq {
        let (word, _) = split_first_word(tail)?;
        first_ident(word).map(str::to_owned)
    } else {
        function_name(tail)
    }
}

/// A top-level accessor: `… get name` followed by a body terminator
/// (`{`, `=>`, `;`, or end of line), or `… set name(` for a setter (the
/// namespace spelling is the caller's business). A function whose name or
/// parameters contain the keyword is not misread: the word must start a
/// fresh token.
fn accessor_name(line: &str, keyword: &str, followed_by_paren: bool) -> Option<String> {
    let mut from = 0;
    while let Some(rel) = line[from..].find(keyword) {
        let at = from + rel;
        let bounded_before =
            at == 0 || !line[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_');
        let after = line[at + keyword.len()..].trim_start();
        let name_len = after
            .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
            .unwrap_or(after.len());
        let (name, rest) = after.split_at(name_len);
        let rest = rest.trim_start();
        let terminated = if followed_by_paren {
            rest.starts_with('(')
        } else {
            rest.is_empty() || rest.starts_with(['{', '=', ';'])
        };
        if bounded_before && !name.is_empty() && terminated {
            return Some(name.to_owned());
        }
        from = at + keyword.len();
    }
    None
}

/// The name of a keyword-less top-level variable declaration
/// (`Type name;`, `Type? name = …;`, `void Function(T) name = …;`): at least
/// a type token and a name token precede the first `=` or `;`, and the
/// declared name is the last identifier before that cut (function-typed
/// declaration heads carry the parenthesized type earlier in the line —
/// those lines function_name resolved already).
fn bare_variable_name(line: &str) -> Option<String> {
    let cut = line.find(['=', ';']).unwrap_or(line.len());
    let head = line[..cut].trim_end();
    if !head.ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == '$') {
        return None;
    }
    let mut tokens = head.split_whitespace();
    tokens.next()?;
    let name = tokens.next()?;
    (!name.starts_with('(')).then(|| first_ident_of_token(name))
}

/// Whether the line carries a top-level variable keyword run (`const`,
/// `final`, `var`, `late`, `external`, `covariant`).
fn keyword_prefixed(line: &str) -> bool {
    matches!(
        split_first_word(line).map(|(head, _)| head),
        Some("const" | "final" | "var" | "late" | "external" | "covariant")
    )
}

/// The name of a variable/external declaration: everything before the first
/// `=`, `(`, or `;`, its last identifier token.
fn var_or_external_name(line: &str) -> Option<String> {
    let cut = line
        .find(['=', '('])
        .unwrap_or_else(|| line.find(';').unwrap_or(line.len()));
    line[..cut]
        .rsplit(|c: char| c.is_whitespace() || c == ',')
        .find(|token| !token.is_empty())
        .map(first_ident_of_token)
        .filter(|name| !name.is_empty())
}

/// The identifier declared by a top-level function, read backward from an
/// opening paren at `at`: the word before it, walking back over a balanced
/// `<…>` generic parameter block when one sits between the name and the
/// paren (`name<T extends void Function()>(…)`). `None` when no word is
/// found (the caller's Function-group skip then keeps scanning).
fn word_before_paren(line: &str, bytes: &[u8], at: usize) -> Option<String> {
    let mut end = at;
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    let mut start = end;
    while start > 0
        && (bytes[start - 1].is_ascii_alphanumeric()
            || bytes[start - 1] == b'_'
            || bytes[start - 1] == b'$')
    {
        start -= 1;
    }
    if start < end {
        return Some(line[start..end].to_owned());
    }
    // The byte before the paren is (the end of) a generic parameter list;
    // walk back over the balanced `<…>` block to the declared name.
    let last = *bytes[..end].last()?;
    if last != b'>' {
        return None;
    }
    let mut depth = 0i32;
    let mut open = end;
    while open > 0 {
        match bytes[open - 1] {
            b'>' => depth += 1,
            b'<' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        open -= 1;
    }
    // `open` sits just past the generic block's opening `<`; the declared
    // name ends before that bracket.
    let mut name_end = open.saturating_sub(1);
    while name_end > 0 && bytes[name_end - 1].is_ascii_whitespace() {
        name_end -= 1;
    }
    let mut name_start = name_end;
    while name_start > 0
        && (bytes[name_start - 1].is_ascii_alphanumeric()
            || bytes[name_start - 1] == b'_'
            || bytes[name_start - 1] == b'$')
    {
        name_start -= 1;
    }
    (name_start < name_end).then(|| line[name_start..name_end].to_owned())
}

/// The name of a plain top-level function: the identifier before the first
/// depth-0 `(` (generic parameter lists are skipped by angle tracking and a
/// balanced `<…>` backtrack; a `Function` before the paren means the paren
/// belongs to a function-typed return type and the scan continues past its
/// group).
fn function_name(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    let mut angle = 0i32;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'<' => angle += 1,
            b'>' => angle -= 1,
            b'(' if angle <= 0 => match word_before_paren(line, bytes, i) {
                Some(word) if word != "Function" => return Some(word),
                word => {
                    if word.is_none() {
                        // No name directly before the paren: either a
                        // function-typed return type's own group (skip it)
                        // or an unparseable shape.
                        let mut depth = 1i32;
                        i += 1;
                        while i < bytes.len() && depth > 0 {
                            match bytes[i] {
                                b'(' => depth += 1,
                                b')' => depth -= 1,
                                _ => {}
                            }
                            i += 1;
                        }
                        continue;
                    }
                    // `Function` before the paren: the paren belongs to a
                    // function-typed return type — skip its group.
                    let mut depth = 1i32;
                    i += 1;
                    while i < bytes.len() && depth > 0 {
                        match bytes[i] {
                            b'(' => depth += 1,
                            b')' => depth -= 1,
                            _ => {}
                        }
                        i += 1;
                    }
                    continue;
                }
            },
            _ => {}
        }
        i += 1;
    }
    None
}

fn split_first_word(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    let end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    Some((&trimmed[..end], &trimmed[end..]))
}

/// The leading identifier of a whitespace-token: cut at the first non-
/// identifier byte (`Foo<T>` → `Foo`).
fn first_ident(token: &str) -> Option<&str> {
    let trimmed = token.trim_start();
    let end = trimmed
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .unwrap_or(trimmed.len());
    let ident = &trimmed[..end];
    (!ident.is_empty()).then_some(ident)
}

fn first_ident_of_token(token: &str) -> String {
    first_ident(token).unwrap_or_default().to_owned()
}
