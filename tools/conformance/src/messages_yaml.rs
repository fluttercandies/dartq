//! A reader for the two messages.yaml shapes in the pinned SDK source:
//!
//! - two-level (analyzer): class key at indent 0, entry key at indent 2,
//!   fields at indent 4;
//! - one-level (shared): entry key at indent 0, fields at indent 2.
//!
//! Only the fields the doc-example extractor needs are surfaced; everything
//! else (nested parameter maps, `script:` blocks, `previousName`, ...) is
//! skipped structurally. This is NOT a general YAML parser — messages.yaml is
//! machine-generated and regular.

/// One parsed message entry.
#[derive(Debug, Default, Clone)]
pub struct MessageEntry {
    /// The analyzer-side code name (camelCase), i.e. the `analyzerCode` field
    /// when present, else the entry key.
    pub analyzer_code: String,
    /// The `sharedName` field, when present.
    pub shared_name: Option<String>,
    /// The `removedIn` field, when present (the code is no longer reported).
    pub removed_in: Option<String>,
    /// Whether the entry carries documentation to extract.
    pub documentation: Option<String>,
}

/// The kind of file layout detected.
#[derive(Debug, PartialEq, Eq)]
enum Layout {
    TwoLevel,
    OneLevel,
}

fn is_header(line: &str) -> bool {
    let indent = indent_of(line);
    if indent > 2 {
        return false;
    }
    let body = line.trim_start();
    !body.starts_with('#') && body.ends_with(':')
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// Two-level files put the first entry at indent 2 directly under the indent-0
/// class header; one-level files put the first field there (a `key: value`
/// line is never a header).
fn detect_layout(lines: &[&str]) -> Layout {
    for (i, line) in lines.iter().enumerate() {
        if is_header(line) && indent_of(line) == 0 {
            for next in &lines[i + 1..] {
                if next.trim().is_empty() || next.trim_start().starts_with('#') {
                    continue;
                }
                return if is_header(next) && indent_of(next) == 2 {
                    Layout::TwoLevel
                } else {
                    Layout::OneLevel
                };
            }
        }
    }
    Layout::OneLevel
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        let inner = &value[1..value.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
                    None => out.push('\\'),
                }
            } else {
                out.push(c);
            }
        }
        return out;
    }
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        return value[1..value.len() - 1].replace("''", "'");
    }
    value.to_string()
}

/// Parses messages.yaml into entries, preserving file order.
pub fn parse(text: &str) -> Vec<MessageEntry> {
    let lines: Vec<&str> = text.lines().collect();
    let layout = detect_layout(&lines);
    let entry_indent = match layout {
        Layout::TwoLevel => 2,
        Layout::OneLevel => 0,
    };
    let field_indent = entry_indent + 2;

    let mut entries = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        if !is_header(line) || indent_of(line) != entry_indent {
            i += 1;
            continue;
        }
        let key = line.trim_end_matches(':').trim_start().to_string();
        let mut entry = MessageEntry { analyzer_code: key.clone(), ..Default::default() };
        i += 1;
        while i < lines.len() {
            let field = lines[i];
            let indent = indent_of(field);
            if field.trim().is_empty() {
                i += 1;
                continue;
            }
            if indent <= entry_indent {
                break; // next entry (or file section)
            }
            if indent == field_indent {
                let (name, value) = field.split_once(':').unwrap_or((field, ""));
                let name = name.trim();
                let value = value.trim();
                match name {
                    "sharedName" => entry.shared_name = Some(unquote(value)),
                    "analyzerCode" if layout == Layout::OneLevel => {
                        entry.analyzer_code = unquote(value);
                    }
                    "removedIn" => entry.removed_in = Some(unquote(value)),
                    _ => {}
                }
                if value == "|-" || value == "|" || value == "|+" {
                    let (block, next) = read_block(&lines, i + 1, field_indent);
                    if name == "documentation" {
                        entry.documentation = Some(block);
                    }
                    i = next;
                    continue;
                }
            }
            i += 1;
        }
        entries.push(entry);
    }
    entries
}

/// Reads a block scalar's lines (indent > `key_indent`), stripping the
/// block's own indentation, with `|-` chomping (no trailing newline).
/// Returns the block text and the index of the first line after it.
fn read_block(lines: &[&str], start: usize, key_indent: usize) -> (String, usize) {
    let mut collected: Vec<String> = Vec::new();
    let mut block_indent: Option<usize> = None;
    let mut i = start;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            collected.push(String::new());
            i += 1;
            continue;
        }
        let indent = indent_of(line);
        if indent <= key_indent {
            break;
        }
        let bi = *block_indent.get_or_insert(indent);
        let take = indent.min(bi);
        collected.push(line[take..].to_string());
        i += 1;
    }
    while collected.last().is_some_and(|l| l.is_empty()) {
        collected.pop();
    }
    (collected.join("\n"), i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_two_level_layout() {
        let src = "# comment\nCompileTimeErrorCode:\n  fooBar:\n    type: compileTimeError\n    sharedName: fooBarShared\n    problemMessage: \"x\"\n    documentation: |-\n      #### Example\n\n      ```dart\n      var [!x!] = 1;\n      ```\n  baz:\n    type: compileTimeError\n    parameters: none\n";
        let entries = parse(src);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].analyzer_code, "fooBar");
        assert_eq!(entries[0].shared_name.as_deref(), Some("fooBarShared"));
        assert_eq!(
            entries[0].documentation.as_deref(),
            Some("#### Example\n\n```dart\nvar [!x!] = 1;\n```")
        );
        assert_eq!(entries[1].analyzer_code, "baz");
        assert!(entries[1].documentation.is_none());
    }

    #[test]
    fn parses_one_level_layout() {
        let src = "someCode:\n  type: syntacticError\n  parameters:\n    String f: desc\n  analyzerCode: someAnalyzerCode\n  hasPublishedDocs: true\n  documentation: |-\n    #### Example\n    ```dart\n    x\n    ```\n  script: |\n    main() {}\nnextCode:\n  type: syntacticError\n";
        let entries = parse(src);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].analyzer_code, "someAnalyzerCode");
        assert_eq!(
            entries[0].documentation.as_deref(),
            Some("#### Example\n```dart\nx\n```")
        );
        assert_eq!(entries[1].analyzer_code, "nextCode");
    }

    #[test]
    fn removed_and_quoted_values() {
        let src = "oldCode:\n  removedIn: \"3.0\"\n  sharedName: 'old Code'\n";
        let entries = parse(src);
        assert_eq!(entries[0].removed_in.as_deref(), Some("3.0"));
        assert_eq!(entries[0].shared_name.as_deref(), Some("old Code"));
    }

    #[test]
    fn block_scalars_are_normalized_to_stripped_form() {
        let src = "e:\n  documentation: |\n    a\n    b\n  other: x\n";
        let entries = parse(src);
        assert_eq!(entries[0].documentation.as_deref(), Some("a\nb"));
    }
}
