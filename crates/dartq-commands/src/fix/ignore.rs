use super::*;


/// The `// ignore: <code>` row for the diagnostic at `range`: spliced
/// into the previous line's ignore comment when one is there (the new
/// code first, the official insertion point), else inserted above the
/// hit's line at the line's own indentation. The first line inserts at
/// the file head.
pub fn ignore_line_fix(
    code: &'static str,
    range: core::ops::Range<u32>,
    text: &str,
    lines: &LineIndex,
) -> Option<RuleFix> {
    let (line, _) = lines.line_col_utf16(text, range.start);
    let starts = byte_line_starts(text);
    let index = line as usize - 1; // 0-based; the report line always exists
    let comment = format!("// ignore: {code}");
    let (at, insertion) = if index == 0 {
        (0, format!("{comment}\n"))
    } else if index < starts.len() {
        let prev_start = starts[index - 1];
        let line_start = starts[index];
        let prev_line = text.get(prev_start..line_start)?;
        if is_ignore_comment(prev_line.trim_start()) {
            let colon = prev_line.find(':')?;
            (prev_start + colon + 1, format!(" {code},"))
        } else {
            (
                line_start,
                format!("{}{comment}\n", line_indent(text, line_start)),
            )
        }
    } else {
        return None;
    };
    let offset = u32::try_from(utf16_count(&text[..at])).ok()?;
    Some(RuleFix {
        rule: code,
        message: format!("Ignore '{code}' for this line"),
        source: BUILTIN_IGNORE_SOURCE,
        edit: anchored_edit(text, lines, offset, 0, &insertion),
        target: None,
    })
}

/// The `// ignore_for_file: <code>` row: spliced into an existing
/// `ignore_for_file` comment in the leading region (the new code first),
/// else inserted after the last blank line of the header (or before the
/// first code line, its attached doc comments and annotations included)
/// — the official `IgnoreDiagnosticInFile` walk over the line stream.
pub fn ignore_file_fix(code: &'static str, text: &str, lines: &LineIndex) -> Option<RuleFix> {
    let starts = byte_line_starts(text);
    let comment = format!("// ignore_for_file: {code}");
    enum Spot {
        Splice(usize),
        Line {
            at: usize,
            before: bool,
            after: bool,
        },
    }
    let spot = if starts.len() == 1 {
        Spot::Line {
            at: 0,
            before: false,
            after: true,
        }
    } else {
        let mut last_blank: Option<usize> = None;
        let mut walked: Option<usize> = None;
        let mut found: Option<Spot> = None;
        for index in 0..starts.len() - 1 {
            let line_start = starts[index];
            let line_text = text.get(line_start..starts[index + 1])?;
            walked = Some(line_start);
            let trimmed = line_text.trim();
            if is_ignore_for_file_comment(trimmed) {
                let colon = line_text.find(':')?;
                found = Some(Spot::Splice(line_start + colon + 1));
                break;
            }
            if trimmed.is_empty() {
                last_blank = Some(line_start);
            } else if trimmed.starts_with("#!") || trimmed.starts_with("//") {
                // Header comments ride above the insertion point.
            } else {
                // First code line: its attached doc comments and
                // annotations precede the insertion (the official
                // `documentationComment ?? node` face).
                let mut at = line_start;
                let mut back = index;
                while back > 0 {
                    let prev = text.get(starts[back - 1]..starts[back])?.trim_start();
                    if prev.starts_with("///") || prev.starts_with("/**") || prev.starts_with('@') {
                        at = starts[back - 1];
                        back -= 1;
                    } else {
                        break;
                    }
                }
                found = Some(Spot::Line {
                    at,
                    before: false,
                    after: true,
                });
                break;
            }
        }
        match found {
            Some(spot) => spot,
            None => match last_blank {
                Some(blank) => Spot::Line {
                    at: blank,
                    before: true,
                    after: false,
                },
                None => Spot::Line {
                    at: walked.unwrap_or(0),
                    before: false,
                    after: true,
                },
            },
        }
    };
    let (at, insertion) = match spot {
        Spot::Splice(at) => (at, format!(" {code},")),
        Spot::Line { at, before, after } => {
            let mut insertion = String::new();
            if before {
                insertion.push('\n');
            }
            insertion.push_str(line_indent(text, at));
            insertion.push_str(&comment);
            insertion.push('\n');
            if after {
                insertion.push('\n');
            }
            (at, insertion)
        }
    };
    let offset = u32::try_from(utf16_count(&text[..at])).ok()?;
    Some(RuleFix {
        rule: code,
        message: format!("Ignore '{code}' for the whole file"),
        source: BUILTIN_IGNORE_SOURCE,
        edit: anchored_edit(text, lines, offset, 0, &insertion),
        target: None,
    })
}

/// The line-start byte offsets of `text` (the last line carries its start
/// whether or not a terminator closes the file).
pub(crate) fn byte_line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index + 1);
        }
    }
    starts
}

/// The whitespace run opening the line containing `at` (the official
/// `linePrefix` face — the line's own indentation, not the text before
/// `at`).
pub(crate) fn line_indent(text: &str, at: usize) -> &str {
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line = &text[line_start..];
    let end = line
        .find(|c: char| !matches!(c, ' ' | '\t'))
        .unwrap_or(line.len());
    &line[..end]
}

/// Whether the trimmed line is an ignore comment (`//` — more slashes
/// allowed — then whitespace, then `ignore:`; the official
/// `IgnoreInfo.isIgnoreComment` shape).
pub(crate) fn is_ignore_comment(trimmed: &str) -> bool {
    let rest = trimmed
        .trim_start_matches('/')
        .trim_start_matches([' ', '\t']);
    trimmed.starts_with("//") && rest.starts_with("ignore:")
}

/// Whether the trimmed line is an `ignore_for_file` comment (the official
/// `IgnoreInfo.isIgnoreForFileComment` shape — exactly two slashes).
pub(crate) fn is_ignore_for_file_comment(trimmed: &str) -> bool {
    trimmed.starts_with("//")
        && trimmed[2..]
            .trim_start_matches([' ', '\t'])
            .starts_with("ignore_for_file:")
}

/// The analysis-options face of the ignore trio: the edit (UTF-16
/// offset/length/replacement) that lands `analyzer: errors: <code>:
/// ignore` in `text`, the official `IgnoreDiagnosticInAnalysisOptionsFile`
/// shapes — a root mapping grows the missing `analyzer`/`errors` section
/// at its block end with the block's own indentation, an existing
/// `errors` mapping grows or re-spells the code key, a non-mapping
/// document is replaced whole, and anything the surgery cannot prove
/// (malformed YAML, a non-mapping `analyzer` or `errors`) drops.
pub fn ignore_options_edit(code: &str, text: &str) -> Option<(u32, u32, String)> {
    let entry = format!("{code}: ignore");
    let Ok(Yaml::Mapping(root)) = serde_yaml::from_str::<Yaml>(text) else {
        // The official replaces the non-mapping document whole.
        return Some((
            0,
            utf16_count(text),
            format!("analyzer:\n  errors:\n    {entry}\n"),
        ));
    };
    match root.get("analyzer") {
        None => {
            let mut insertion = String::new();
            if !text.is_empty() && !text.ends_with('\n') {
                insertion.push('\n');
            }
            insertion.push_str("analyzer:\n  errors:\n    ");
            insertion.push_str(&entry);
            insertion.push('\n');
            Some((utf16_count(text), 0, insertion))
        }
        Some(analyzer) if matches!(analyzer, Yaml::Mapping(_)) => {
            match analyzer.get("errors") {
                None => {
                    let (key_line, _) = yaml_key_line(text, "analyzer:", 0, text.len())?;
                    let (at, child_indent) = yaml_block_end(text, key_line, 0);
                    let indent = " ".repeat(child_indent);
                    Some((
                        u32::try_from(utf16_count(&text[..at])).ok()?,
                        0,
                        format!("{indent}errors:\n{indent}  {entry}\n"),
                    ))
                }
                Some(errors) if matches!(errors, Yaml::Mapping(_)) => {
                    let (key_line, _) = yaml_key_line(text, "analyzer:", 0, text.len())?;
                    let (_, child_indent) = yaml_block_end(text, key_line, 0);
                    let (errors_line, errors_indent) =
                        yaml_key_line(text, "errors:", child_indent, text.len())?;
                    let (errors_end, code_indent) =
                        yaml_block_end(text, errors_line, errors_indent);
                    // An existing code key re-spells its value in place; a
                    // missing one appends to the errors block.
                    let existing =
                        yaml_key_line(text, &format!("{code}:"), code_indent, errors_end)
                            .map(|(at, _)| at);
                    if let Some(at) = existing {
                        let line_end = text[at..].find('\n').map_or(text.len(), |i| at + i);
                        let colon = text[at..line_end].find(':')?;
                        return Some((
                            u32::try_from(utf16_count(&text[..at + colon + 1])).ok()?,
                            u32::try_from(utf16_count(&text[at + colon + 1..line_end])).ok()?,
                            " ignore".to_owned(),
                        ));
                    }
                    let indent = " ".repeat(code_indent);
                    Some((
                        u32::try_from(utf16_count(&text[..errors_end])).ok()?,
                        0,
                        format!("{indent}{entry}\n"),
                    ))
                }
                Some(_) => None,
            }
        }
        Some(_) => None,
    }
}

/// The byte offset and indentation of the first line under `until` whose
/// content starts with `key` at exactly `indent` columns (`None` when
/// absent).
pub(crate) fn yaml_key_line(text: &str, key: &str, indent: usize, until: usize) -> Option<(usize, usize)> {
    let mut cursor = 0usize;
    while cursor < until && cursor < text.len() {
        let line_end = text[cursor..].find('\n').map_or(text.len(), |i| cursor + i);
        let line_text = &text[cursor..line_end];
        let trimmed = line_text.trim_start();
        if line_text.len() - trimmed.len() == indent && trimmed.starts_with(key) {
            return Some((cursor, indent));
        }
        cursor = line_end + 1;
    }
    None
}

/// The insertion point of a child under the block opened at `key_line`
/// (indentation `key_indent`): the first non-blank, non-comment line at
/// or below the block's indentation ends the block; the block's existing
/// child indentation wins over the two-column default.
pub(crate) fn yaml_block_end(text: &str, key_line: usize, key_indent: usize) -> (usize, usize) {
    let mut at = text[key_line..]
        .find('\n')
        .map_or(text.len(), |i| key_line + i + 1);
    let mut child_indent = None;
    while at < text.len() {
        let line_end = text[at..].find('\n').map_or(text.len(), |i| at + i);
        let line_text = &text[at..line_end];
        let trimmed = line_text.trim_start();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            let indent = line_text.len() - trimmed.len();
            if indent <= key_indent {
                break;
            }
            child_indent = child_indent.or(Some(indent));
        }
        at = line_end + 1;
        if line_end == text.len() {
            break;
        }
    }
    (at.min(text.len()), child_indent.unwrap_or(key_indent + 2))
}
