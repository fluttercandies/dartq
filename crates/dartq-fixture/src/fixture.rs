//! The fixture DSL: one file, many cases, each case one or more Dart files
//! with behavioral expectations marked inline.
//!
//! ```text
//! //- lint: <rule machine name>     file-level, required once: the expected
//!                                    code for inline ranges, which carry none
//! //- case: <case name>            starts a case
//! //- file: /lib/test.dart         starts a file block; the FIRST block of
//!                                    a case is the analyzed unit, later
//!                                    blocks are supporting files (parsed
//!                                    and retained; cross-file resolution
//!                                    faces are future work — a package
//!                                    meta header is reserved, unimplemented)
//! ```
//!
//! A case with no `//- file:` header collects its content into the
//! default block ([`DEFAULT_FILE`]). File content is Dart source in the
//! upstream `TestCode` markup (see [`crate::markup`]): every
//! `[!…!]` / `/*[N*/…/*N]*/` range is one expected diagnostic of the
//! case's lint at the stripped range, in UTF-16 coordinates. A case with
//! no ranges expects no diagnostics (the upstream `assertNoDiagnostics`
//! posture). The case-level `//- lint:` override stands in for the
//! upstream `code:` parameter, and the case-level `//- diverge: <reason>`
//! header quarantines a ported case whose pipeline run is known to
//! diverge from the upstream expectation — the runner skips it, the
//! reason stays pinned in the data file, and the rule lane unpins it by
//! deleting the header once the behavior matches (regeneration preserves
//! these headers; see the converter's merge step).
//!
//! Line-anchored markers — the second annotation form, for hand-written
//! cases — are line comments on their own line, extracted BEFORE the
//! inline strip (a bare `^` is itself markup, so the two forms cannot
//! share a line):
//!
//! ```text
//! //   ^^^ rule_name                previous line, caret column/length
//! //^^ ^^^ rule_name                 two lines up (the glued caret count)
//! //   ^^^ rule_name: message text   the message compares when present
//! ```
//!
//! Caret columns and lengths count UTF-16 code units of the target line's
//! stripped text — the target line must itself be free of inline markers
//! (one annotation form per line). The marker comment line is removed
//! from the analyzed source; stacked markers may point at the same target.
//! The expected rule name is required after the caret run. Header lines
//! (any line starting with `//-`) never appear in ported content — the
//! converter rejects a case whose Dart source contains one.

use core::ops::Range;

use crate::markup::{self, MarkedRange};

/// The default analyzed-file path (the upstream harness writes the markup
/// body to `test.dart` at the test package's `lib/` root).
pub const DEFAULT_FILE: &str = "/lib/test.dart";

/// One expected diagnostic: the rule's machine name, the UTF-16 range in
/// the analyzed file, and the rendered message when the expectation pins
/// it (a `None` message compares code + range only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expectation {
    pub code: String,
    pub range: Range<u32>,
    pub message: Option<String>,
}

/// One extracted line-anchored marker: the resolved target — the 0-based
/// line in the file's stripped code, the UTF-16 column, the caret run —
/// and the named rule (message optional).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineMarker {
    pub line: usize,
    pub column: u32,
    pub run: u32,
    pub code: String,
    pub message: Option<String>,
}

/// One file of a case, markup-stripped: the unmarked code, the inline
/// ranges, and the extracted line-anchored markers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileBlock {
    pub path: String,
    pub code: String,
    pub ranges: Vec<MarkedRange>,
    pub markers: Vec<LineMarker>,
}

/// One case: a name, an optional lint override (the upstream `code:`
/// parameter), an optional divergence quarantine (a known pipeline
/// divergence, with its reason), and its files — `files[0]` is the
/// analyzed unit.
#[derive(Clone, Debug)]
pub struct Case {
    pub name: String,
    pub lint: Option<String>,
    pub diverge: Option<String>,
    pub files: Vec<FileBlock>,
}

/// A parsed fixture file.
#[derive(Clone, Debug)]
pub struct FixtureDoc {
    pub lint: String,
    pub cases: Vec<Case>,
}

/// Why a fixture file could not be parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// 1-based line the grammar broke on (0 when the finding is about a
    /// whole block rather than a line).
    pub line: usize,
    pub reason: String,
}

/// A case under construction: its named blocks of marked content, each
/// line with its 1-based file line number (for error reporting).
struct Draft {
    name: String,
    lint: Option<String>,
    diverge: Option<String>,
    blocks: Vec<(String, Vec<(usize, String)>)>,
}

impl Draft {
    fn finish(self, line: usize) -> Result<Case, ParseError> {
        if self.blocks.is_empty() {
            return Err(ParseError {
                line,
                reason: format!("case `{}` has no content", self.name),
            });
        }
        let mut files = Vec::new();
        for (path, lines) in self.blocks {
            if lines.iter().all(|(_, l)| l.trim().is_empty()) {
                return Err(ParseError {
                    line,
                    reason: format!("case `{}` file {path} has no content", self.name),
                });
            }
            files.push(build_block(&path, &lines)?);
        }
        Ok(Case {
            name: self.name,
            lint: self.lint,
            diverge: self.diverge,
            files,
        })
    }
}

/// Builds one file block: extracts line-anchored marker lines, strips the
/// inline markup from the remainder, and resolves each marker's target
/// line against the stripped text.
fn build_block(path: &str, lines: &[(usize, String)]) -> Result<FileBlock, ParseError> {
    let err = |line: usize, reason: String| ParseError { line, reason: format!("{path}: {reason}") };
    // Partition by original line position: marker lines come out, the
    // rest is the inline-marked body (`filtered` maps an original
    // position to its place in the body).
    let mut body: Vec<&str> = Vec::new();
    let mut filtered = vec![0usize; lines.len()];
    let mut extracted: Vec<(usize, MarkerShape, usize)> = Vec::new();
    let mut is_marker = vec![false; lines.len()];
    for (position, (line_no, text)) in lines.iter().enumerate() {
        match marker_shape(text) {
            Ok(Some(shape)) => {
                extracted.push((position, shape, *line_no));
                is_marker[position] = true;
            }
            Ok(None) => {
                filtered[position] = body.len();
                body.push(text);
            }
            Err(reason) => return Err(err(*line_no, reason)),
        }
    }
    let (code, ranges) = markup::strip(&body.join("\n")).map_err(|e| err(0, format!("{e:?}")))?;
    // `lines_up` counts physical lines above the marker's own line; a
    // marker may not target another marker line (stacked markers share a
    // target instead).
    let markers_before = |position: usize| is_marker.iter().take(position).filter(|m| **m).count();
    let mut markers = Vec::new();
    for (position, shape, line_no) in extracted {
        let Some(target) = position.checked_sub(shape.lines_up) else {
            return Err(err(line_no, "marker points above the first line".into()));
        };
        if is_marker.get(target).copied().unwrap_or(true) {
            return Err(err(line_no, "marker targets another marker line".into()));
        }
        markers.push(LineMarker {
            line: target - markers_before(target),
            column: shape.column,
            run: shape.run,
            code: shape.code,
            message: shape.message,
        });
    }
    Ok(FileBlock {
        path: path.to_owned(),
        code,
        ranges,
        markers,
    })
}

/// The raw shape of one marker line, before target resolution.
struct MarkerShape {
    lines_up: usize,
    column: u32,
    run: u32,
    code: String,
    message: Option<String>,
}

/// Recognizes a line-anchored marker line: `Ok(Some(shape))` for one,
/// `Ok(None)` for any other line. Between the `//` and the caret run only
/// whitespace may stand — any other text makes the line an ordinary
/// comment. A glued caret count (`//^^`) declares marker intent: anything
/// malformed after it is an error, not a comment.
fn marker_shape(line: &str) -> Result<Option<MarkerShape>, String> {
    let trimmed = line.trim_start();
    let Some(after_slashes) = trimmed.strip_prefix("//") else {
        return Ok(None);
    };
    let lead = line.len() - trimmed.len();
    let glued = after_slashes.chars().take_while(|c| *c == '^').count();
    let (lines_up, tail) = if glued > 0 {
        let rest = &after_slashes[glued..];
        if !rest.starts_with([' ', '\t']) {
            return Err("glued marker needs whitespace then a separated caret run".into());
        }
        (glued, rest)
    } else if after_slashes.starts_with([' ', '\t']) {
        (1, after_slashes)
    } else {
        return Ok(None); // an ordinary comment
    };
    let Some(run_at) = tail.find('^') else {
        if glued > 0 {
            return Err("glued marker has no separated caret run".into());
        }
        return Ok(None); // an ordinary comment
    };
    if !tail[..run_at].chars().all(char::is_whitespace) {
        return Ok(None); // text before the carets: an ordinary comment
    }
    let run_byte = lead + 2 + glued + run_at;
    let rest = &line[run_byte..];
    let run = rest.chars().take_while(|c| *c == '^').count();
    let name_tail = rest[run..].trim();
    if name_tail.is_empty() {
        return Err("marker must name the expected rule".into());
    }
    let (code, message) = match name_tail.split_once(':') {
        Some((name, message)) => (name.trim().to_owned(), Some(message.trim().to_owned())),
        None => (name_tail.to_owned(), None),
    };
    Ok(Some(MarkerShape {
        lines_up,
        column: line[..run_byte].chars().map(char::len_utf16).sum::<usize>() as u32,
        run: run as u32,
        code,
        message,
    }))
}

/// Parses `text` into a [`FixtureDoc`].
///
/// # Errors
///
/// A missing or duplicate `lint` header, an unknown header key, a case
/// without content, a duplicate case name, or markup failing the upstream
/// validity rules — each with the 1-based line where the grammar broke.
pub fn parse(text: &str) -> Result<FixtureDoc, ParseError> {
    let mut lint: Option<String> = None;
    let mut cases: Vec<Case> = Vec::new();
    let mut draft: Option<Draft> = None;
    let mut named: Vec<String> = Vec::new();
    for (index, raw) in text.split('\n').enumerate() {
        let line_no = index + 1;
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let Some(header) = line.strip_prefix("//-") else {
            // Content line: it belongs to the newest block of the open
            // draft, opening the default block for a bare case start.
            if line.trim().is_empty() && draft.is_none() {
                continue;
            }
            let Some(open) = draft.as_mut() else {
                if line.trim().is_empty() || line.starts_with("//") {
                    continue;
                }
                return Err(ParseError {
                    line: line_no,
                    reason: format!("content outside a case: `{line}`"),
                });
            };
            if open.blocks.is_empty() {
                open.blocks.push((DEFAULT_FILE.to_owned(), Vec::new()));
            }
            open.blocks
                .last_mut()
                .expect("just pushed")
                .1
                .push((line_no, line.to_owned()));
            continue;
        };
        let (key, value) = header.split_once(':').ok_or_else(|| ParseError {
            line: line_no,
            reason: format!("malformed header `{line}`"),
        })?;
        let value = value.trim().to_owned();
        if value.is_empty() {
            return Err(ParseError {
                line: line_no,
                reason: format!("empty `{}` value", key.trim()),
            });
        }
        match key.trim() {
            // Case-level overrides; before the first case, `lint` is the
            // file-level default.
            "lint" => {
                if let Some(open) = draft.as_mut() {
                    open.lint = Some(value);
                } else if lint.replace(value).is_some() {
                    return Err(ParseError {
                        line: line_no,
                        reason: "duplicate lint header".into(),
                    });
                }
            }
            "diverge" => {
                let Some(open) = draft.as_mut() else {
                    return Err(ParseError {
                        line: line_no,
                        reason: "diverge header outside a case".into(),
                    });
                };
                open.diverge = Some(value);
            }
            "case" => {
                if named.contains(&value) {
                    return Err(ParseError {
                        line: line_no,
                        reason: format!("duplicate case name `{value}`"),
                    });
                }
                named.push(value.clone());
                if let Some(open) = draft.take() {
                    cases.push(open.finish(line_no)?);
                }
                draft = Some(Draft {
                    name: value,
                    lint: None,
                    diverge: None,
                    blocks: Vec::new(),
                });
            }
            "file" => {
                if draft.is_none() {
                    return Err(ParseError {
                        line: line_no,
                        reason: "file header outside a case".into(),
                    });
                }
                if !value.starts_with('/') {
                    return Err(ParseError {
                        line: line_no,
                        reason: format!("file path `{value}` must be workspace-rooted"),
                    });
                }
                let open = draft.as_mut().expect("checked above");
                if open
                    .blocks
                    .last()
                    .is_some_and(|(_, lines)| lines.iter().all(|(_, l)| l.trim().is_empty()))
                {
                    return Err(ParseError {
                        line: line_no,
                        reason: format!(
                            "case `{}` file {} has no content",
                            open.name,
                            open.blocks.last().expect("checked").0
                        ),
                    });
                }
                open.blocks.push((value, Vec::new()));
            }
            other => {
                return Err(ParseError {
                    line: line_no,
                    reason: format!("unknown header key `{other}`"),
                });
            }
        }
    }
    if let Some(open) = draft.take() {
        cases.push(open.finish(text.split('\n').count())?);
    }
    let lint = lint.ok_or_else(|| ParseError {
        line: 1,
        reason: "no `//- lint:` header before the first case".into(),
    })?;
    if cases.is_empty() {
        return Err(ParseError { line: 1, reason: "no cases".into() });
    }
    Ok(FixtureDoc { lint, cases })
}

impl Case {
    /// The case's effective lint (its override, else the doc's).
    #[must_use]
    pub fn lint<'a>(&'a self, doc_lint: &'a str) -> &'a str {
        self.lint.as_deref().unwrap_or(doc_lint)
    }

    /// The analyzed file (the first block) with every expectation folded:
    /// inline ranges — expecting the case's lint — plus line-anchored
    /// markers with their named codes, anchored over the stripped text.
    ///
    /// # Errors
    ///
    /// A line marker whose target line does not exist in the stripped code.
    pub fn analyzed(&self, doc_lint: &str) -> Result<AnalyzedFile, ParseError> {
        let target = &self.files[0];
        let lint = self.lint(doc_lint).to_owned();
        let mut expectations: Vec<Expectation> = target
            .ranges
            .iter()
            .map(|r| Expectation {
                code: lint.clone(),
                range: r.range.clone(),
                message: None,
            })
            .collect();
        // UTF-16 offset of every line start of the stripped code.
        let mut line_start: Vec<u32> = Vec::new();
        let mut utf16 = 0u32;
        for line in target.code.split('\n') {
            line_start.push(utf16);
            utf16 += line.chars().map(char::len_utf16).sum::<usize>() as u32 + 1;
        }
        for marker in &target.markers {
            let Some(&start_of_line) = line_start.get(marker.line) else {
                return Err(ParseError {
                    line: 0,
                    reason: format!(
                        "{}: marker targets line {} past the stripped code",
                        target.path, marker.line
                    ),
                });
            };
            let start = start_of_line + marker.column;
            expectations.push(Expectation {
                code: marker.code.clone(),
                range: start..start + marker.run,
                message: marker.message.clone(),
            });
        }
        Ok(AnalyzedFile {
            path: target.path.clone(),
            code: target.code.clone(),
            expectations,
        })
    }
}

/// The analyzed file with every expectation folded.
#[derive(Clone, Debug)]
pub struct AnalyzedFile {
    pub path: String,
    pub code: String,
    pub expectations: Vec<Expectation>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The minimal two-case shape: an implicit default file and an
    /// explicit one, inline ranges folded against the doc lint.
    #[test]
    fn parses_cases_and_inline_ranges() {
        let doc = parse(concat!(
            "//- lint: avoid_empty_else\n",
            "\n",
            "//- case: reports\n",
            "void f() { if (c) ; else [!;!] }\n",
            "\n",
            "//- case: clean\n",
            "//- file: /lib/other.dart\n",
            "void f() { if (c) ; else { } }\n",
        ))
        .expect("parses");
        assert_eq!(doc.lint, "avoid_empty_else");
        assert_eq!(doc.cases.len(), 2);
        assert_eq!(doc.cases[0].files.len(), 1);
        assert_eq!(doc.cases[0].files[0].path, DEFAULT_FILE);
        // The blank separator line before the next header rides along as
        // the block's trailing newline.
        assert_eq!(doc.cases[0].files[0].code, "void f() { if (c) ; else ; }\n");
        let analyzed = doc.cases[0].analyzed("avoid_empty_else").expect("analyzed");
        assert_eq!(analyzed.expectations.len(), 1);
        assert_eq!(analyzed.expectations[0].code, "avoid_empty_else");
        assert_eq!(analyzed.expectations[0].range, 25..26, "the `;` after `else`");
        assert!(doc.cases[1].analyzed("avoid_empty_else").expect("analyzed").expectations.is_empty());
    }

    /// Case-level lint override and a supporting file block.
    #[test]
    fn case_lint_override_and_support_files() {
        let doc = parse(concat!(
            "//- lint: a\n",
            "//- case: two_files\n",
            "//- file: /lib/test.dart\n",
            "int x = 1;\n",
            "//- file: /lib/part.dart\n",
            "int y = 2;\n",
        ))
        .expect("parses");
        assert_eq!(doc.cases[0].files.len(), 2);
        assert_eq!(doc.cases[0].files[1].path, "/lib/part.dart");
        let doc =
            parse("//- lint: a\n//- case: overridden\n//- lint: b\nint x = [!1!];\n").expect("parses");
        assert_eq!(doc.cases[0].lint("a"), "b");
        assert_eq!(
            doc.cases[0].analyzed("a").expect("analyzed").expectations[0].code,
            "b"
        );
    }

    /// Grammar failures: duplicate lint, unknown key, empty case, content
    /// outside a case, missing lint.
    #[test]
    fn grammar_failures_carry_lines() {
        assert_eq!(parse("//- lint: a\n//- lint: b\n").unwrap_err().line, 2);
        assert_eq!(
            parse("//- lint: a\n//- nope: x\n//- case: c\nx;\n").unwrap_err().line,
            2
        );
        assert!(parse("//- lint: a\n//- case: c\n").is_err(), "empty case");
        assert!(parse("int x;\n").is_err(), "content outside a case");
        assert!(parse("//- case: c\nx;\n").is_err(), "no lint header");
    }

    /// Line-anchored markers: extracted before the inline strip (their
    /// carets would otherwise be eaten as position markup), previous-line
    /// and glued forms, stacked markers sharing a target, optional
    /// message, ordinary comments untouched.
    #[test]
    fn line_anchored_markers() {
        let doc = parse(concat!(
            "//- lint: a\n",
            "//- case: m\n",
            "int xy = 1;\n",
            "//  ^^ some_rule\n",
            "//^^ ^ some_other: watch out\n",
            "// ordinary comment without carets\n",
        ))
        .expect("parses");
        let analyzed = doc.cases[0].analyzed("a").expect("analyzed");
        assert_eq!(analyzed.code, "int xy = 1;\n// ordinary comment without carets\n");
        assert_eq!(analyzed.expectations.len(), 2, "{:?}", analyzed.expectations);
        assert_eq!(analyzed.expectations[0].code, "some_rule");
        assert_eq!(analyzed.expectations[0].range, 4..6, "the `^^` sits under `xy`");
        assert_eq!(analyzed.expectations[1].code, "some_other");
        assert_eq!(analyzed.expectations[1].message.as_deref(), Some("watch out"));
        assert_eq!(
            analyzed.expectations[1].range,
            5..6,
            "glued `^^` targets two lines up, the `^` under the `y`"
        );
    }

    /// Markers over non-ASCII targets count UTF-16 columns.
    #[test]
    fn line_markers_count_utf16_columns() {
        let doc = parse(concat!(
            "//- lint: a\n",
            "//- case: wide\n",
            "var s = '😀x';\n",
            "//         ^ r\n",
        ))
        .expect("parses");
        let analyzed = doc.cases[0].analyzed("a").expect("analyzed");
        // The emoji is two UTF-16 units, so the column-11 caret (the `x`)
        // anchors at UTF-16 offset 11.
        assert_eq!(analyzed.expectations[0].range, 11..12);
    }

    /// A marker above the first line is a parse error.
    #[test]
    fn marker_above_first_line_fails() {
        let doc = parse("//- lint: a\n//- case: top\n// ^ r\nint x;\n").unwrap_err();
        assert!(doc.reason.contains("above the first line"), "{doc:?}");
    }
}
