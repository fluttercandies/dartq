//! The upstream converter: one linter `*_test.dart` file into fixture-DSL
//! cases — mechanical, verbatim, and loud about anything it cannot
//! represent (a case is ported whole or rejected with its reason; nothing
//! is silently dropped).
//!
//! Recognized upstream shapes (the `LintRuleTest` family,
//! `pkg/linter/test/rules/*_test.dart`, upstream Dart SDK source):
//!
//! ```text
//! String get lintRule => LintNames.<name>;   (or '<name>')
//! test_<name>() async { … }                  one case per method
//! await assertDiagnosticsFromMarkup(r'''…''');      ported: the marked body
//! await assertDiagnosticsFromMarkup(r'''…''',       skipped: the expected
//!     code: <other>);                                 code is not the lint
//! await assertNoDiagnostics(r'''…''');               ported: zero expectation
//! newFile('$testPackageLibPath/a.dart', r'''…''');   support file /lib/a.dart
//! newFile('$testPackageRootPath/bin/x.dart', …);     support file /bin/x.dart
//! ```
//!
//! Rejected per-case constructs (with reasons, in the tally): the raw
//! `assertDiagnostics(code, [error(…)])` form (analyzer parse-error rows
//! the lint DSL does not model), statements the converter does not know,
//! content whose lines collide with the fixture header grammar, and a
//! `code:` override naming anything but the class's lint. Whole-file
//! rejects: a missing `lintRule` getter and the language-version-pinned
//! class mixins (`LanguageVersion219Mixin` — the settled-version face the
//! DSL does not carry yet). `@FailingTest` methods are skipped, not
//! rejected: they assert behavior the upstream itself knows is broken.
//!
//! The rendered output keeps the Dart bodies byte-verbatim (markers and
//! all); the analyzed file block is written first, the `newFile` support
//! blocks after.

/// One ported case: the verbatim marked body of the analyzed unit plus its
/// supporting files (path → verbatim content).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortedCase {
    pub name: String,
    pub analyzed: String,
    pub support: Vec<(String, String)>,
}

/// The port result: the rule, the ported cases, and every skipped or
/// rejected test with its reason.
#[derive(Clone, Debug)]
pub struct Outcome {
    pub rule: String,
    pub ported: Vec<PortedCase>,
    pub skipped: Vec<(String, String)>,
}

impl Outcome {
    /// The per-file tally the converter reports: ported vs skipped.
    #[must_use]
    pub fn tally(&self) -> (usize, usize) {
        (self.ported.len(), self.skipped.len())
    }
}

/// Ports one upstream test file's text.
///
/// # Errors
///
/// Whole-file rejects, with the reason: a missing `lintRule` getter, a
/// language-version mixin, or a test method shape the walker cannot close.
pub fn port_file(text: &str) -> Result<Outcome, String> {
    if text.contains("LanguageVersion") && text.contains("Mixin") {
        return Err("language-version-pinned test class is unsupported".into());
    }
    let rule = lint_rule(text).ok_or("no `lintRule => …` getter found")?;
    let lines: Vec<&str> = text.split('\n').collect();
    // Byte offset of each line start, to hand the walker text from the
    // line after a `test_…` declaration.
    let mut line_start = Vec::with_capacity(lines.len() + 1);
    let mut at = 0usize;
    for line in &lines {
        line_start.push(at);
        at += line.len() + 1;
    }
    let mut ported = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        let Some(name) = test_name(line) else {
            i += 1;
            continue;
        };
        // A @FailingTest annotation sits directly above its test (its
        // argument lines between); any hit in that window marks the test.
        let failing = lines[i.saturating_sub(5)..i]
            .iter()
            .any(|l| l.contains("@FailingTest"));
        let body = &text[line_start[(i + 1).min(lines.len() - 1)]..];
        let walk = port_body(body, &rule, &name);
        i += 1 + body[..walk.consumed].matches('\n').count();
        if failing {
            skipped.push((name, "@FailingTest upstream".into()));
            continue;
        }
        match walk.result {
            Ok(Some(case)) => ported.push(case),
            Ok(None) => skipped.push((name, "no assertion in body; nothing to port".into())),
            Err(reason) => skipped.push((name, reason)),
        }
    }
    Ok(Outcome {
        rule,
        ported,
        skipped,
    })
}

/// The `lintRule => …;` value: `LintNames.<name>`, `'<name>'`, or
/// `"<name>"`.
fn lint_rule(text: &str) -> Option<String> {
    for line in text.split('\n') {
        let Some(after) = line.split("lintRule =>").nth(1) else {
            continue;
        };
        let value = after.trim().trim_end_matches(';').trim();
        if let Some(name) = value.strip_prefix("LintNames.") {
            return Some(name.to_owned());
        }
        let name = value.trim_matches(['\'', '"']);
        if !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
            return Some(name.to_owned());
        }
    }
    None
}

/// `test_<name>() async {` — the method name, when the line is one.
fn test_name(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix("test_")?;
    let end = rest.find('(')?;
    let name = &rest[..end];
    if !name.is_empty() && trimmed.ends_with("async {") {
        return Some(format!("test_{name}"));
    }
    None
}

/// One walked method body: how many bytes it consumed (statement
/// boundaries only — raw strings are consumed whole, so a `}` inside a
/// string never ends the body), and what the walk made of it.
pub struct BodyWalk {
    pub consumed: usize,
    pub result: Result<Option<PortedCase>, String>,
}

/// Walks one method body from `body` (the text starting on the line after
/// the `test_…() async {` declaration): the analyzed assertion, any
/// `newFile` support files, stopping at the method's closing `}`.
///
/// # Errors
///
/// Per-case rejects, with the reason.
fn port_body(body: &str, rule: &str, name: &str) -> BodyWalk {
    let mut analyzed: Option<(String, Option<String>)> = None; // (content, code override)
    let mut support = Vec::new();
    let mut cursor = 0usize;
    while cursor < body.len() {
        let rest = &body[cursor..];
        let trimmed = rest.trim_start();
        if trimmed.is_empty() {
            return BodyWalk {
                consumed: body.len(),
                result: Err("no closing `}` found".into()),
            };
        }
        let offset = cursor + (rest.len() - trimmed.len());
        if trimmed.starts_with("}") {
            // The method's own closing brace: body complete.
            return BodyWalk {
                consumed: offset,
                result: finish(analyzed, support, rule, name),
            };
        }
        if trimmed.starts_with("//") {
            cursor = next_line(body, offset);
            continue;
        }
        let prefix_len = |prefix: &str| offset + prefix.len();
        let (result, end) = if let Some(after) = trimmed.strip_prefix("newFile(") {
            match take_new_file(after, prefix_len("newFile(")) {
                Ok((path, content, end)) => {
                    support.push((path, content));
                    (Ok(()), end)
                }
                Err(e) => (Err(e), cursor),
            }
        } else if trimmed.starts_with("await assertDiagnostics(") {
            (
                Err("raw `assertDiagnostics` with explicit analyzer error rows is not representable".into()),
                cursor,
            )
        } else if let Some(after) = trimmed.strip_prefix("await assertDiagnosticsFromMarkup(") {
            match take_marked(after, prefix_len("await assertDiagnosticsFromMarkup(")) {
                Ok((content, code, end)) => {
                    if analyzed.replace((content, code)).is_some() {
                        (Err("multiple analyzed assertions in one body".into()), end)
                    } else {
                        (Ok(()), end)
                    }
                }
                Err(e) => (Err(e), cursor),
            }
        } else if let Some(after) = trimmed.strip_prefix("await assertNoDiagnostics(") {
            match take_marked(after, prefix_len("await assertNoDiagnostics(")) {
                Ok((content, _, end)) => {
                    if analyzed.replace((content, None)).is_some() {
                        (Err("multiple analyzed assertions in one body".into()), end)
                    } else {
                        (Ok(()), end)
                    }
                }
                Err(e) => (Err(e), cursor),
            }
        } else {
            (
                Err(format!("unrecognized statement: `{}`", first_line(trimmed))),
                cursor,
            )
        };
        match result {
            Ok(()) => cursor = past_semicolon(body, end),
            Err(reason) => {
                return BodyWalk {
                    consumed: skip_to_method_close(body, cursor),
                    result: Err(reason),
                };
            }
        }
    }
    BodyWalk {
        consumed: body.len(),
        result: Err("no closing `}` found".into()),
    }
}

/// Assembles the walked body into a case (or `None` when no analyzed
/// assertion was found).
fn finish(
    analyzed: Option<(String, Option<String>)>,
    support: Vec<(String, String)>,
    rule: &str,
    name: &str,
) -> Result<Option<PortedCase>, String> {
    let Some((content, code)) = analyzed else {
        return Ok(None);
    };
    if let Some(code) = code
        && code != rule
    {
        return Err(format!("`code:` override `{code}` is not the lint rule"));
    }
    if content.split('\n').any(|l| l.starts_with("//-")) {
        return Err("content line collides with the fixture header grammar".into());
    }
    Ok(Some(PortedCase {
        name: name.to_owned(),
        analyzed: content,
        support,
    }))
}

/// Advances past an optional `;` (with its leading whitespace) — every
/// statement in these bodies closes with one.
fn past_semicolon(text: &str, from: usize) -> usize {
    let rest = text[from..].trim_start();
    if rest.starts_with(';') {
        from + (text[from..].len() - rest.len()) + 1
    } else {
        from
    }
}

/// For a rejected body: consumes through the method's closing line so the
/// file walk resumes at the next test. Raw strings are not parsed on this
/// path (the reason is already decided); a `}` at 2-space indent closes.
fn skip_to_method_close(body: &str, from: usize) -> usize {
    let mut cursor = from;
    for line in body[from..].split('\n') {
        if line.trim_end() == "  }" || line.trim_end() == "}" {
            return (cursor + line.len() + 1).min(body.len());
        }
        cursor += line.len() + 1;
    }
    body.len()
}

/// Byte offset just past the next line break at or after `from`.
fn next_line(text: &str, from: usize) -> usize {
    text[from..]
        .find('\n')
        .map_or(text.len(), |at| from + at + 1)
}

/// The first line of a snippet, for reject messages.
fn first_line(text: &str) -> String {
    text.split('\n')
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned()
}

/// One raw-string argument (`r'''…'''` / `r"""…"""`): the verbatim
/// content and the byte offset just past the closing delimiter.
fn take_raw_string(after: &str, base: usize) -> Result<(String, usize), String> {
    let triple = if after.trim_start().starts_with("r'''") {
        "'''"
    } else if after.trim_start().starts_with("r\"\"\"") {
        "\"\"\""
    } else {
        return Err("argument is not a raw triple-quoted string".into());
    };
    let start = after.find(triple).expect("checked prefix") + triple.len();
    let content_end = after[start..]
        .find(triple)
        .ok_or("unterminated raw string")?;
    Ok((
        after[start..start + content_end].to_owned(),
        base + start + content_end + triple.len(),
    ))
}

/// `r'''…'''[, code: X])` after an `assert*Diagnostics*(` call: the
/// content, the `code:` override when present, and the offset past the
/// closing `)`.
fn take_marked(after: &str, base: usize) -> Result<(String, Option<String>, usize), String> {
    let (content, past_string) = take_raw_string(after, base)?;
    let tail = &after[past_string - base..];
    let close = tail.find(')').ok_or("unterminated call")?;
    let params = &tail[..close];
    let code = params
        .split("code:")
        .nth(1)
        .map(|value| value.trim().to_owned());
    Ok((content, code, past_string + close + 1))
}

/// `newFile('<path>', r'''…''')`: the mapped workspace path, the content,
/// and the offset past the closing `)`.
fn take_new_file(after: &str, base: usize) -> Result<(String, String, usize), String> {
    let quote = after
        .chars()
        .find(|c| *c == '\'' || *c == '"')
        .ok_or("newFile without a path string")?;
    let path_start = after.find(quote).expect("checked") + 1;
    let path_end = after[path_start..]
        .find(quote)
        .ok_or("unterminated path string")?;
    let raw_path = &after[path_start..path_start + path_end];
    let under = |root: &str| -> Option<String> {
        raw_path
            .split_once(root)
            .map(|(_, rest)| format!("/{rest}").replace("//", "/"))
    };
    let path = under("testPackageLibPath")
        .map(|rest| format!("/lib{rest}"))
        .or_else(|| under("testPackageRootPath"))
        .ok_or_else(|| format!("unsupported newFile path `{raw_path}`"))?;
    let mut string_at = path_start + path_end + 1;
    let args = &after[string_at..];
    string_at += args.len() - args.trim_start_matches([',', ' ', '\t', '\r', '\n']).len();
    let (content, past_string) = take_raw_string(&after[string_at..], base + string_at)?;
    let tail = &after[past_string - base..];
    let close = tail.find(')').ok_or("unterminated newFile")?;
    Ok((path, content, past_string + close + 1))
}

/// Renders an [`Outcome`] as a fixture-DSL file.
#[must_use]
pub fn render(out: &Outcome) -> String {
    let mut text = String::from(concat!(
        "// Ported by port_upstream from the upstream linter test file\n",
        "// (pkg/linter/test/rules, upstream Dart SDK source); regenerate\n",
        "// with the converter rather than editing. Case names are the\n",
        "// upstream test method names, verbatim. The first file block of\n",
        "// each case is the analyzed unit; later blocks are the test's\n",
        "// supporting files.\n",
    ));
    text.push_str(&format!("//- lint: {}\n", out.rule));
    for case in &out.ported {
        text.push_str(&format!("\n//- case: {}\n", case.name));
        text.push_str("//- file: /lib/test.dart\n");
        push_verbatim(&mut text, &case.analyzed);
        for (path, content) in &case.support {
            text.push_str(&format!("//- file: {path}\n"));
            push_verbatim(&mut text, content);
        }
    }
    text
}

/// Carries a previously generated file's `//- diverge:` quarantines into a
/// fresh render — regenerating must not unpick a pinned divergence. The
/// caller hands the old file's text (empty on first generation) and gets
/// the merged text plus the stale quarantine entries (cases that no longer
/// port, whose reasons would otherwise vanish silently).
#[must_use]
pub fn merge_divergences(rendered: &str, previous: &str) -> (String, Vec<String>) {
    let quarantines: Vec<(String, String)> = crate::parse(previous)
        .map(|doc| {
            doc.cases
                .iter()
                .filter_map(|c| c.diverge.clone().map(|reason| (c.name.clone(), reason)))
                .collect()
        })
        .unwrap_or_default();
    let mut merged = String::with_capacity(rendered.len());
    for line in rendered.split_inclusive('\n') {
        merged.push_str(line);
        if let Some(name) = line
            .strip_prefix("//- case: ")
            .and_then(|n| n.strip_suffix('\n'))
            && let Some((_, reason)) = quarantines.iter().find(|(cased, _)| cased == name)
        {
            merged.push_str(&format!("//- diverge: {reason}\n"));
        }
    }
    let stale = quarantines
        .iter()
        .map(|(name, _)| name)
        .filter(|name| !merged.contains(&format!("//- case: {name}\n")))
        .cloned()
        .collect();
    (merged, stale)
}

/// Appends `content` with its trailing newline normalized: the fixture
/// grammar is line-oriented, and every upstream body ends with one.
fn push_verbatim(text: &mut String, content: &str) {
    text.push_str(content);
    if !content.ends_with('\n') {
        text.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical sample: every recognized shape plus every reject and
    /// skip, with the rendered output and tally pinned.
    #[test]
    fn ports_the_construct_zoo() {
        let source = concat!(
            "class T extends LintRuleTest {\n",
            "  @override\n",
            "  String get lintRule => LintNames.some_rule;\n",
            "\n",
            "  test_reports() async {\n",
            "    await assertDiagnosticsFromMarkup(r'''\n",
            "void f() { if (c) ; else [!;!] }\n",
            "''');\n",
            "  }\n",
            "\n",
            "  test_clean() async {\n",
            "    await assertNoDiagnostics(r'''\n",
            "void f() { if (c) ; else { g(); } }\n",
            "''');\n",
            "  }\n",
            "\n",
            "  test_withSupportFile() async {\n",
            "    newFile('$testPackageLibPath/a.dart', r'''\n",
            "class A {}\n",
            "''');\n",
            "\n",
            "    await assertDiagnosticsFromMarkup(r'''\n",
            "part of 'a.dart';\n",
            "augment class A { int [!x!] = 1; }\n",
            "''');\n",
            "  }\n",
            "\n",
            "  @FailingTest(\n",
            "    issue: 'https://example.invalid/1',\n",
            "  )\n",
            "  test_knownBroken() async {\n",
            "    await assertNoDiagnostics(r'''\n",
            "var x = 1;\n",
            "''');\n",
            "  }\n",
            "\n",
            "  test_rawDiagnostics() async {\n",
            "    await assertDiagnostics(r'''\n",
            "void f() { }\n",
            "''', [error(diag.expectedToken, 1, 2)]);\n",
            "  }\n",
            "}\n",
        );
        let out = port_file(source).expect("ports");
        assert_eq!(out.rule, "some_rule");
        assert_eq!(out.tally(), (3, 2));
        assert_eq!(
            out.skipped,
            vec![
                ("test_knownBroken".to_owned(), "@FailingTest upstream".to_owned()),
                (
                    "test_rawDiagnostics".to_owned(),
                    "raw `assertDiagnostics` with explicit analyzer error rows is not representable"
                        .to_owned()
                ),
            ]
        );
        let support_case = out
            .ported
            .iter()
            .find(|c| !c.support.is_empty())
            .expect("support case ported");
        assert_eq!(support_case.support[0].0, "/lib/a.dart");
        assert_eq!(support_case.support[0].1, "\nclass A {}\n");
        assert!(support_case.analyzed.starts_with("\npart of 'a.dart';"));
        let rendered = render(&out);
        assert!(rendered.starts_with("// Ported by port_upstream"));
        assert!(rendered.contains("//- lint: some_rule\n"));
        assert!(rendered.contains("//- case: test_reports\n"));
        assert!(
            rendered.contains("//- file: /lib/test.dart\n\nvoid f() { if (c) ; else [!;!] }\n")
        );
        assert!(rendered.contains("//- file: /lib/a.dart\n\nclass A {}\n"));
        // And the rendered text parses back through the DSL.
        let doc = crate::parse(&rendered).expect("round-trips");
        assert_eq!(doc.lint, "some_rule");
        assert_eq!(doc.cases.len(), 3);
    }

    /// Whole-file rejects: no lintRule, a version-pinned class.
    #[test]
    fn whole_file_rejects() {
        assert_eq!(
            port_file("class T { test_x() async { } }").unwrap_err(),
            "no `lintRule => …` getter found"
        );
        let pinned = "class T extends LanguageVersion219Mixin on LintRuleTest {\n\
                      String get lintRule => 'x';\n}";
        assert!(
            port_file(pinned)
                .unwrap_err()
                .contains("language-version-pinned")
        );
    }

    /// The quoted-`lintRule` spelling and the `code:` override skip.
    #[test]
    fn quoted_rule_and_code_override() {
        let source = concat!(
            "class T extends LintRuleTest {\n",
            "  String get lintRule => 'quoted_rule';\n",
            "  test_override() async {\n",
            "    await assertDiagnosticsFromMarkup(r'''\n",
            "int x = [!1!];\n",
            "''', code: somethingElse);\n",
            "  }\n",
            "}\n",
        );
        let out = port_file(source).expect("ports");
        assert_eq!(out.rule, "quoted_rule");
        assert_eq!(out.tally(), (0, 1));
        assert!(
            out.skipped[0]
                .1
                .contains("`code:` override `somethingElse`")
        );
    }

    /// An unrecognized statement rejects the case, never silently.
    #[test]
    fn unknown_statement_rejects() {
        let source = concat!(
            "class T extends LintRuleTest {\n",
            "  String get lintRule => LintNames.x;\n",
            "  test_custom() async {\n",
            "    writePackageConfig();\n",
            "  }\n",
            "}\n",
        );
        let out = port_file(source).expect("ports");
        assert_eq!(out.tally(), (0, 1));
        assert!(out.skipped[0].1.contains("unrecognized statement"));
    }

    /// The double-quoted raw strings (the multiline-string rules use
    /// them), kept verbatim.
    #[test]
    fn double_quoted_raw_strings() {
        let source = concat!(
            "class T extends LintRuleTest {\n",
            "  String get lintRule => LintNames.x;\n",
            "  test_dq() async {\n",
            "    await assertNoDiagnostics(r\"\"\"\n",
            "var x = ''''';\n",
            "\"\"\");\n",
            "  }\n",
            "}\n",
        );
        let out = port_file(source).expect("ports");
        assert_eq!(out.tally(), (1, 0));
        assert_eq!(out.ported[0].analyzed, "\nvar x = ''''';\n");
    }
}
