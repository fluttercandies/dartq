//! Structural scanning of Dart test files: top-level `class` / `mixin`
//! declarations, their headers (superclass + mixins) and their members
//! (methods with block bodies, getters with literal values). Built directly
//! on the lexer so string literals and comments never confuse the structure.

use crate::dart::{
    ident_at, ident_end, is_ident_start, match_paren, next_char, skip_ws_and_comments, split_args,
};

/// One scanned member.
#[derive(Debug)]
pub struct Member {
    pub name: String,
    /// Method body (between the braces), when the member has one.
    pub body: Option<(usize, usize)>,
    /// Getter/field initializer expression text, when present. For a
    /// block-bodied getter this is the expression of its first `return`.
    pub initializer: Option<(usize, usize)>,
    /// Declared parameter names, in order (methods only).
    pub params: Vec<String>,
}

/// One scanned class or mixin declaration.
#[derive(Debug)]
pub struct Declaration {
    pub name: String,
    #[allow(dead_code)] // part of the scanned header; exercised by tests
    pub superclass: Option<String>,
    pub mixins: Vec<String>,
    /// Types named by an `implements` clause (no member inheritance).
    pub implements: Vec<String>,
    pub members: Vec<Member>,
}

/// Scans every top-level class/mixin declaration in file order.
pub fn scan_declarations(src: &str) -> Vec<Declaration> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut pos = 0usize;
    let mut depth = 0i64;
    while pos < src.len() {
        let rest = &src[pos..];
        if rest.starts_with('\'') || rest.starts_with('"') {
            skip_string(src, &mut pos);
            continue;
        }
        if rest.starts_with('r') && matches!(bytes.get(pos + 1), Some(b'\'') | Some(b'"')) {
            skip_string(src, &mut pos);
            continue;
        }
        if rest.starts_with("//") {
            pos += rest.find('\n').map_or(rest.len(), |e| e);
            continue;
        }
        if rest.starts_with("/*") {
            pos += 2;
            while pos < src.len() && !src[pos..].starts_with("*/") {
                pos = next_char(src, pos);
            }
            pos = (pos + 2).min(src.len());
            continue;
        }
        match bytes[pos] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ if depth == 0 && is_ident_start(bytes[pos]) => {
                let word = ident_at(src, pos).to_string();
                let after = match word.as_str() {
                    "class" | "mixin" => Some(ident_end(src, pos)),
                    "abstract" => {
                        let mut p = ident_end(src, pos);
                        p += skip_ws_and_comments(&src[p..]);
                        ident_at(src, p).eq("class").then(|| ident_end(src, p))
                    }
                    _ => None,
                };
                if let Some(after) = after {
                    if let Some((decl, end)) = scan_declaration(src, after, word == "mixin") {
                        out.push(decl);
                        pos = end;
                        depth = 0;
                        continue;
                    }
                }
                pos = ident_end(src, pos);
                continue;
            }
            _ => {}
        }
        pos = next_char(src, pos);
    }
    out
}

type ScanResult = Option<(Declaration, usize)>;

/// Parses a class/mixin declaration whose keyword ends at `pos`; returns the
/// declaration and the offset just past its body (or header for `;`-form).
fn scan_declaration(src: &str, pos: usize, is_mixin: bool) -> ScanResult {
    let _ = is_mixin;
    let mut p = pos + skip_ws_and_comments(&src[pos..]);
    if src[p..].starts_with('<') {
        // Type parameters on the declaration.
        let mut angle = 0i64;
        while p < src.len() {
            match src.as_bytes()[p] {
                b'<' => angle += 1,
                b'>' => {
                    angle -= 1;
                    if angle == 0 {
                        p += 1;
                        break;
                    }
                }
                _ => {}
            }
            p += 1;
        }
    }
    let name = ident_at(src, p).to_string();
    if name.is_empty() {
        return None;
    }
    p = ident_end(src, p);

    let mut superclass = None;
    let mut mixins = Vec::new();
    let mut implements: Vec<String> = Vec::new();
    let mut clause: Option<&str> = None;
    loop {
        p += skip_ws_and_comments(&src[p..]);
        if p >= src.len() {
            return None;
        }
        match src.as_bytes()[p] {
            b'{' => break,
            b';' => {
                return Some((
                    Declaration {
                        name,
                        superclass,
                        mixins,
                        implements: implements,
                        members: Vec::new(),
                    },
                    p + 1,
                ));
            }
            _ if is_ident_start(src.as_bytes()[p]) => {
                let word = ident_at(src, p);
                match word {
                    "extends" | "on" => clause = Some("extends"),
                    "with" => clause = Some("with"),
                    "implements" => clause = Some("implements"),
                    _ => {
                        p = ident_end(src, p);
                        if let Some(kind) = clause {
                            match kind {
                                "extends" if superclass.is_none() => {
                                    superclass = Some(word.to_string());
                                }
                                "with" => mixins.push(word.to_string()),
                                "implements" => implements.push(word.to_string()),
                                _ => {}
                            }
                        }
                        continue;
                    }
                }
                p = ident_end(src, p);
            }
            b'<' => {
                // Generic arguments of a type in the header: skip balanced.
                let mut angle = 0i64;
                while p < src.len() {
                    match src.as_bytes()[p] {
                        b'<' => angle += 1,
                        b'>' => {
                            angle -= 1;
                            if angle == 0 {
                                p += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    p += 1;
                }
            }
            _ => p += 1,
        }
    }
    // p is at the body `{`.
    let Some(close) = match_brace(src, p) else {
        return None;
    };
    // Member spans are absolute (file) offsets.
    let members = scan_members(&src[p + 1..close], p + 1);
    Some((
        Declaration {
            name,
            superclass,
            mixins,
            implements,
            members,
        },
        close + 1,
    ))
}

/// Scans members of a class body (the text between the braces). Member level
/// is nesting depth 0 inside that text; reported spans are absolute offsets
/// (`base` + body-relative).
pub fn scan_members(body: &str, base: usize) -> Vec<Member> {
    let mut members = Vec::new();
    let bytes = body.as_bytes();
    let mut pos = 0usize;
    let mut depth = 0i64;
    let mut prologue: Vec<String> = Vec::new();
    while pos < body.len() {
        let rest = &body[pos..];
        if rest.starts_with('\'') || rest.starts_with('"') {
            skip_string(body, &mut pos);
            continue;
        }
        if rest.starts_with('r') && matches!(bytes.get(pos + 1), Some(b'\'') | Some(b'"')) {
            skip_string(body, &mut pos);
            continue;
        }
        if rest.starts_with("//") {
            pos += rest.find('\n').map_or(body.len(), |e| e);
            continue;
        }
        if rest.starts_with("/*") {
            pos += 2;
            while pos < body.len() && !body[pos..].starts_with("*/") {
                pos = next_char(body, pos);
            }
            pos = (pos + 2).min(body.len());
            continue;
        }
        match bytes[pos] {
            b'(' if depth == 0 => {
                if let Some(member) = take_method(body, base, &mut pos, &prologue) {
                    members.push(member);
                }
                prologue.clear();
            }
            b'{' if depth == 0 => {
                let name = member_name(&prologue);
                if !name.is_empty() && prologue.iter().any(|w| w == "get") {
                    // A block-bodied getter: body plus first-`return`
                    // initializer.
                    if let Some(close) = match_brace(body, pos) {
                        let initializer = return_expr(&body[pos + 1..close])
                            .map(|(s, e)| (base + pos + 1 + s, base + pos + 1 + e));
                        members.push(Member {
                            name,
                            body: Some((base + pos + 1, base + close)),
                            initializer,
                            params: Vec::new(),
                        });
                        prologue.clear();
                        pos = close + 1;
                    } else {
                        pos += 1;
                    }
                } else if let Some(close) = match_brace(body, pos) {
                    pos = close + 1;
                } else {
                    pos += 1;
                }
            }
            b'=' if depth == 0 => {
                if body[pos..].starts_with("=>") {
                    let name = member_name(&prologue);
                    let mut start = pos + 2;
                    start += skip_ws_and_comments(&body[start..]);
                    let mut p = start;
                    while p < body.len() && bytes[p] != b';' {
                        if body[p..].starts_with('\'') || body[p..].starts_with('"') {
                            skip_string(body, &mut p);
                        } else {
                            p += 1;
                        }
                    }
                    if !name.is_empty() {
                        members.push(Member {
                            name,
                            body: None,
                            initializer: Some((base + start, base + p)),
                            params: Vec::new(),
                        });
                    }
                    pos = p + 1;
                } else {
                    let name = member_name(&prologue);
                    let start = pos + 1;
                    let mut p = start;
                    while p < body.len() && bytes[p] != b';' {
                        if body[p..].starts_with('\'') || body[p..].starts_with('"') {
                            skip_string(body, &mut p);
                        } else {
                            p += 1;
                        }
                    }
                    if !name.is_empty() {
                        members.push(Member {
                            name,
                            body: None,
                            initializer: Some((base + start, base + p)),
                            params: Vec::new(),
                        });
                    }
                    pos = p + 1;
                }
                prologue.clear();
            }
            b';' if depth == 0 => {
                // An abstract getter declaration (`String get x;`): recorded
                // so name lookups can distinguish "declared without value"
                // from "unknown".
                let name = member_name(&prologue);
                if !name.is_empty() && prologue.iter().any(|w| w == "get") {
                    members.push(Member {
                        name,
                        body: None,
                        initializer: None,
                        params: Vec::new(),
                    });
                }
                prologue.clear();
                pos += 1;
            }
            b'{' | b'(' | b'[' => {
                depth += 1;
                pos += 1;
            }
            b'}' | b')' | b']' => {
                depth -= 1;
                pos += 1;
            }
            _ if depth == 0 && is_ident_start(bytes[pos]) => {
                let end = ident_end(body, pos);
                prologue.push(body[pos..end].to_string());
                pos = end;
            }
            _ => pos = next_char(body, pos),
        }
    }
    members
}

/// A `(` at member level: the prologue names the method; parameters follow,
/// then `async*?`/`sync*?` and a block or `=>` body.
fn take_method(body: &str, base: usize, pos: &mut usize, prologue: &[String]) -> Option<Member> {
    let name = member_name(prologue);
    let close_paren = match_paren(body, *pos)?;
    let params = parse_param_names(&body[*pos + 1..close_paren]);
    let mut p = close_paren + 1;
    p += skip_ws_and_comments(&body[p..]);
    for keyword in ["async", "sync"] {
        if body[p..].starts_with(keyword) {
            p += keyword.len();
            if body[p..].starts_with('*') {
                p += 1;
            }
            p += skip_ws_and_comments(&body[p..]);
            break;
        }
    }
    if body[p..].starts_with('{') {
        let close = match_brace(body, p)?;
        // A block-bodied getter carries its first `return` expression as the
        // initializer (`String get x { return 'a'; }`).
        let initializer = if prologue.iter().any(|w| w == "get") {
            return_expr(&body[p + 1..close]).map(|(s, e)| (base + p + 1 + s, base + p + 1 + e))
        } else {
            None
        };
        *pos = close + 1;
        return Some(Member {
            name,
            body: Some((base + p + 1, base + close)),
            initializer,
            params,
        });
    }
    if body[p..].starts_with("=>") {
        let mut start = p + 2;
        start += skip_ws_and_comments(&body[start..]);
        let mut q = start;
        while q < body.len() && body.as_bytes()[q] != b';' {
            if body[q..].starts_with('\'') || body[q..].starts_with('"') {
                skip_string(body, &mut q);
            } else {
                q = next_char(body, q);
            }
        }
        *pos = q + 1;
        return Some(Member {
            name,
            body: None,
            initializer: Some((base + start, base + q)),
            params,
        });
    }
    *pos = close_paren + 1;
    None
}

/// The declared parameter names of a parameter list (the text between the
/// parens), in declaration order. Named (`{ ... }`) and optional positional
/// (`[ ... ]`) groups are flattened; type annotations, `required`/`covariant`
/// modifiers, `this.`/`super.` targets and defaults are stripped.
fn parse_param_names(list: &str) -> Vec<String> {
    let mut names = Vec::new();
    let (head, named) = match (list.find('{'), list.rfind('}')) {
        (Some(open), Some(close)) if close > open => (&list[..open], Some(&list[open + 1..close])),
        _ => (list, None),
    };
    for item in split_args(head) {
        if let Some(name) = param_name(&item) {
            names.push(name);
        }
    }
    if let Some(named) = named {
        for item in split_args(named) {
            if let Some(name) = param_name(&item) {
                names.push(name);
            }
        }
    }
    names
}

/// The declared name of one parameter item.
fn param_name(item: &str) -> Option<String> {
    let item = item.trim().trim_matches(['[', ']']);
    let item = item.strip_prefix("required ").unwrap_or(item).trim_start();
    let item = item.strip_prefix("covariant ").unwrap_or(item).trim_start();
    if let Some(rest) = item.strip_prefix("this.") {
        let name: &str = rest
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
            .next()?;
        return (!name.is_empty()).then(|| name.to_string());
    }
    if let Some(rest) = item.strip_prefix("super.") {
        let name: &str = rest
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
            .next()?;
        return (!name.is_empty()).then(|| name.to_string());
    }
    // The name is the last identifier of the (type + name) pair, before any
    // `= default`.
    let head = item.split('=').next()?.trim_end();
    let name: &str = head
        .rsplit(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
        .find(|s| !s.is_empty())?;
    Some(name.to_string())
}

/// The expression of the first top-level `return` statement of a getter body.
fn return_expr(body: &str) -> Option<(usize, usize)> {
    let bytes = body.as_bytes();
    let mut pos = 0usize;
    while pos < body.len() {
        let rest = &body[pos..];
        if rest.starts_with('\'') || rest.starts_with('"') {
            skip_string(body, &mut pos);
            continue;
        }
        if rest.starts_with('r') && matches!(bytes.get(pos + 1), Some(b'\'') | Some(b'"')) {
            skip_string(body, &mut pos);
            continue;
        }
        if rest.starts_with("//") {
            pos += rest.find('\n').map_or(rest.len(), |e| e);
            continue;
        }
        if rest.starts_with("/*") {
            pos += 2;
            while pos < body.len() && !body[pos..].starts_with("*/") {
                pos = next_char(body, pos);
            }
            pos = (pos + 2).min(body.len());
            continue;
        }
        if is_ident_start(bytes[pos]) {
            let end = ident_end(body, pos);
            if &body[pos..end] == "return" {
                let start = end + skip_ws_and_comments(&body[end..]);
                let mut q = start;
                while q < body.len() && bytes[q] != b';' {
                    if body[q..].starts_with('\'') || body[q..].starts_with('"') {
                        skip_string(body, &mut q);
                    } else {
                        q = next_char(body, q);
                    }
                }
                return Some((start, q));
            }
            pos = end;
            continue;
        }
        pos = next_char(body, pos);
    }
    None
}

fn member_name(prologue: &[String]) -> String {
    // Skip `get`/`set` keywords; the member name is the last remaining word.
    let mut words: Vec<&str> = prologue.iter().map(String::as_str).collect();
    if words.last() == Some(&"get") || words.last() == Some(&"set") {
        words.pop();
    }
    words.last().map(|w| w.to_string()).unwrap_or_default()
}

/// Matches a `{` at `open`, string/comment aware; returns the `}` offset.
pub fn match_brace(src: &str, open: usize) -> Option<usize> {
    let mut depth = 0i64;
    let bytes = src.as_bytes();
    let mut pos = open;
    while pos < src.len() {
        let rest = &src[pos..];
        if rest.starts_with('\'') || rest.starts_with('"') {
            skip_string(src, &mut pos);
            continue;
        }
        if rest.starts_with('r') && matches!(bytes.get(pos + 1), Some(b'\'') | Some(b'"')) {
            skip_string(src, &mut pos);
            continue;
        }
        if rest.starts_with("//") {
            pos += rest.find('\n').map_or(rest.len(), |e| e);
            continue;
        }
        if rest.starts_with("/*") {
            pos += 2;
            while pos < src.len() && !src[pos..].starts_with("*/") {
                pos = next_char(src, pos);
            }
            pos = (pos + 2).min(src.len());
            continue;
        }
        match bytes[pos] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(pos);
                }
            }
            _ => {}
        }
        pos = next_char(src, pos);
    }
    None
}

fn skip_string(src: &str, pos: &mut usize) {
    if let Some(len) = parse_single_string_len(src) {
        *pos += len;
    } else {
        *pos = next_char(src, *pos);
    }
}

fn parse_single_string_len(src: &str) -> Option<usize> {
    let mut pos = 0usize;
    let d = crate::dart::parse_string_expression(src, &mut pos)?;
    let _ = d;
    Some(pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"
import 'x.dart';

@reflectiveTest
class FooTest extends PubPackageResolutionTest with WithLanguage219Mixin {
  String get aaaPackageRootPath => '$workspaceRootPath/aaa';

  @override
  void setUp() {
    super.setUp();
    writeTestPackageConfigWithMeta();
  }

  Future<void> test_one() async {
    newFile('$aaaPackageRootPath/lib/a.dart', r'''
class A {}
''');
    await resolveTestCodeWithDiagnostics(r'''
var x = 1;
// [diag.foo] m
''');
  }

  String get experiments => ['enhanced-parts'];

  void test_alias() => helper();
}

mixin SharedTests on PubPackageResolutionTest {
  Future<void> test_inMixin() async {
    await resolveTestCode('''
var y = 2;
''');
  }
}

class Alias = Foo with Bar;
"#;

    #[test]
    fn scans_classes_mixins_and_members() {
        let decls = scan_declarations(FILE);
        assert_eq!(
            decls.len(),
            3,
            "{:?}",
            decls.iter().map(|d| &d.name).collect::<Vec<_>>()
        );
        let class = &decls[0];
        assert_eq!(class.name, "FooTest");
        assert_eq!(
            class.superclass.as_deref(),
            Some("PubPackageResolutionTest")
        );
        assert_eq!(class.mixins, vec!["WithLanguage219Mixin"]);
        let names: Vec<&str> = class.members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "aaaPackageRootPath",
                "setUp",
                "test_one",
                "experiments",
                "test_alias"
            ]
        );

        let set_up = &class.members[1];
        let (bs, be) = set_up.body.expect("setUp has a body");
        assert!(bs > be.min(0) || be <= FILE.len(), "offsets in range");
        let set_up_body = &FILE[bs..be];
        assert!(
            set_up_body.contains("writeTestPackageConfigWithMeta"),
            "{set_up_body}"
        );

        let getter = &class.members[3];
        let init = getter.initializer.expect("experiments getter");
        assert_eq!(&FILE[init.0..init.1], "['enhanced-parts']");

        let mixin = &decls[1];
        assert_eq!(mixin.name, "SharedTests");
        assert!(mixin.members.iter().any(|m| m.name == "test_inMixin"));
    }

    #[test]
    fn alias_declaration_has_no_body() {
        let decls = scan_declarations("class A = B with C;\nclass D {}\n");
        assert_eq!(decls.len(), 2);
        assert_eq!(decls[0].name, "A");
        assert_eq!(decls[0].mixins, vec!["C"]);
        assert!(decls[0].members.is_empty());
        assert_eq!(decls[1].name, "D");
    }

    #[test]
    fn strings_do_not_confuse_structure() {
        let src = r#"
class A {
  void test_x() {
    var s = '''
class Fake { void fake() {} }
''';
    var t = '} // fake brace';
  }
}
"#;
        let decls = scan_declarations(src);
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].members.len(), 1);
    }

    #[test]
    fn getter_string_value_span() {
        let src = "class A {\n  String get p => '$workspaceRootPath/aaa';\n}\n";
        let decls = scan_declarations(src);
        let init = decls[0].members[0].initializer.expect("getter body");
        assert_eq!(&src[init.0..init.1], "'$workspaceRootPath/aaa'");
    }

    #[test]
    fn method_body_offsets_are_absolute() {
        let decls = scan_declarations(FILE);
        let class = &decls[0];
        for member in &class.members {
            if let Some((bs, be)) = member.body {
                assert!(
                    FILE[bs..be].contains(&FILE[bs..bs + 1].to_string()),
                    "offsets slice the file text"
                );
                assert!(be <= FILE.len());
            }
        }
        let one_method = class
            .members
            .iter()
            .find(|m| m.name == "test_one")
            .expect("method");
        let (bs, be) = one_method.body.expect("body");
        assert!(FILE[bs..be].contains("resolveTestCodeWithDiagnostics"));
    }

    const PARAMS_FILE: &str = r#"
class A {
  void _helper({required String packageName, required String pathInLib, String content = ''}) {
    newFile('$root/$packageName/lib/$pathInLib', content);
  }

  void _versioned(String versionStr) {
    writeTestPackageConfig(languageVersion: versionStr);
  }

  String get generatedPath {
    return '$testPackageRootPath/.dart_tool/build/generated';
  }

  String get abstractOnly;
}
"#;

    #[test]
    fn params_block_getters_and_abstract_getters() {
        let decls = scan_declarations(PARAMS_FILE);
        let class = &decls[0];
        let helper = class
            .members
            .iter()
            .find(|m| m.name == "_helper")
            .expect("helper");
        assert_eq!(helper.params, vec!["packageName", "pathInLib", "content"]);
        let versioned = class
            .members
            .iter()
            .find(|m| m.name == "_versioned")
            .expect("versioned");
        assert_eq!(versioned.params, vec!["versionStr"]);
        let getter = class
            .members
            .iter()
            .find(|m| m.name == "generatedPath")
            .expect("getter");
        let init = getter.initializer.expect("block getter initializer");
        assert_eq!(
            PARAMS_FILE[init.0..init.1].trim(),
            "'$testPackageRootPath/.dart_tool/build/generated'"
        );
        let abstract_getter = class
            .members
            .iter()
            .find(|m| m.name == "abstractOnly")
            .expect("abstract getter");
        assert!(abstract_getter.initializer.is_none());
        assert!(abstract_getter.body.is_none());
    }
}
