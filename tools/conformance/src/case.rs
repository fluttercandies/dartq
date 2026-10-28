//! The `.case` format: one standalone conformance case per file. A case
//! carries provenance headers, environment directives
//! (options / packages / language version) and a sequence of file sections;
//! the last header before a file section is that section's path.
//!
//! Canonical byte shape (what the writer produces and `--check` requires):
//!
//! ```text
//! //- id: <id>
//! //- source: <provenance, human-readable>
//! //- language: 2.19              (optional, 0/1)
//! //- packages: meta, flutter     (optional, 0/1, comma list)
//! //- package: <name>             (optional, 0..n, aux packages to register)
//! //- ignore: code_a, code_b      (optional, 0/1, codes the official harness
//! //!                               allows this case to report; `%ignore=`)
//! //- options:                    (optional, 0/1; `//+ ` continuation lines)
//! //+ analyzer:
//! //+   strict-casts: true
//! //- file: lib/test.dart         (first section = the case's main file)
//! <file bytes, terminated by a newline when a section follows>
//! //- file: lib/a.dart
//! ...
//! ```
//!
//! Continuation lines encode multi-line values (options YAML, file bytes):
//! `//+ ` + the line, or bare `//+` for an empty line. File bytes are
//! canonicalized to end with exactly one newline.

/// Why a `.case` payload could not be parsed.
#[derive(Debug, PartialEq, Eq)]
#[allow(dead_code)] // format reader for the consuming lanes; exercised by tests
pub struct CaseError(pub String);

#[derive(Debug, PartialEq, Eq, Clone, Default)]
pub struct Case {
    pub id: String,
    pub source: String,
    pub language: Option<String>,
    pub packages: Vec<String>,
    /// Extra packages registered in the package config (beyond `packages`).
    pub aux_packages: Vec<String>,
    pub options: Option<String>,
    /// Diagnostic codes the official harness allows this case to report
    /// without failing (`%ignore=` in doc examples).
    pub ignores: Vec<String>,
    /// File sections in order; the first is the case's main file.
    pub files: Vec<(String, String)>,
}

impl Case {
    /// The main file's content (`files[0]`); every case has one.
    #[allow(dead_code)] // format reader for the consuming lanes; exercised by tests
    pub fn main_file(&self) -> Option<&str> {
        self.files.first().map(|(_, c)| c.as_str())
    }
}

/// Renders a case into its canonical byte form.
pub fn write(case: &Case) -> String {
    let mut out = String::new();
    out.push_str(&format!("//- id: {}\n", case.id));
    if !case.source.is_empty() {
        out.push_str(&format!("//- source: {}\n", case.source));
    }
    if let Some(language) = &case.language {
        out.push_str(&format!("//- language: {language}\n"));
    }
    if !case.packages.is_empty() {
        out.push_str(&format!("//- packages: {}\n", case.packages.join(", ")));
    }
    for name in &case.aux_packages {
        out.push_str(&format!("//- package: {name}\n"));
    }
    if !case.ignores.is_empty() {
        out.push_str(&format!("//- ignore: {}\n", case.ignores.join(", ")));
    }
    if let Some(options) = &case.options {
        out.push_str("//- options:\n");
        for line in options.split('\n') {
            if line.is_empty() {
                out.push_str("//+\n");
            } else {
                out.push_str("//+ ");
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    for (path, content) in &case.files {
        out.push_str(&format!("//- file: {path}\n"));
        // Canonical file bytes end with exactly one newline.
        let mut norm = content.clone();
        if !norm.is_empty() && !norm.ends_with('\n') {
            norm.push('\n');
        }
        if norm.is_empty() {
            continue;
        }
        for line in norm.strip_suffix('\n').expect("ends with newline").split('\n') {
            if line.is_empty() {
                out.push_str("//+\n");
            } else {
                out.push_str("//+ ");
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

/// Parses canonical case bytes. Accepts exactly the canonical shape (the
/// manifest's blake3 comparison depends on writer/reader agreement).
#[allow(dead_code)] // format reader for the consuming lanes; exercised by tests
pub fn read(text: &str) -> Result<Case, CaseError> {
    let mut case = Case::default();
    let mut current: Option<(String, Vec<String>)> = None; // open file section
    let mut section: Section = Section::Header;
    let mut options: Vec<String> = Vec::new();

    let flush_file = |current: &mut Option<(String, Vec<String>)>,
                      files: &mut Vec<(String, String)>| {
        if let Some((path, lines)) = current.take() {
            files.push((path, join_continuations(&lines)));
        }
    };
    fn flush_options(
        section: Section,
        options: &mut Vec<String>,
        target: &mut Option<String>,
    ) {
        if section == Section::Options {
            *target = Some(options.join("\n"));
            options.clear();
        }
    }

    for (no, raw) in text.lines().enumerate() {
        if let Some(rest) = raw.strip_prefix("//- ") {
            flush_file(&mut current, &mut case.files);
            flush_options(section, &mut options, &mut case.options);
            section = Section::Header;
            let (key, value) = rest.split_once(": ").unwrap_or((rest, ""));
            let key = key.strip_suffix(':').unwrap_or(key);
            match key {
                "id" if value.is_empty() => return err(no, "empty id"),
                "id" => case.id = value.to_string(),
                "source" => case.source = value.to_string(),
                "language" => case.language = Some(value.to_string()),
                "packages" => {
                    case.packages = value
                        .split(", ")
                        .filter(|p| !p.is_empty())
                        .map(str::to_string)
                        .collect();
                }
                "package" => case.aux_packages.push(value.to_string()),
                "ignore" => {
                    case.ignores = value
                        .split(", ")
                        .filter(|p| !p.is_empty())
                        .map(str::to_string)
                        .collect();
                }
                "options" if value.is_empty() => {
                    section = Section::Options;
                }
                "options" => return err(no, "options directive carries inline text"),
                "file" => {
                    if value.is_empty() {
                        return err(no, "empty file path");
                    }
                    current = Some((value.to_string(), Vec::new()));
                    section = Section::File;
                }
                other => return err(no, &format!("unknown directive `{other}`")),
            }
        } else if raw == "//-" {
            return err(no, "bare directive line");
        } else if raw == "//+" {
            match section {
                Section::Options => options.push(String::new()),
                Section::File => {
                    current.as_mut().expect("continuation outside a section").1.push(String::new());
                }
                Section::Header => return err(no, "continuation outside a section"),
            }
        } else if let Some(rest) = raw.strip_prefix("//+ ") {
            match section {
                Section::Options => options.push(rest.to_string()),
                Section::File => {
                    current.as_mut().expect("continuation outside a section").1.push(rest.to_string());
                }
                Section::Header => return err(no, "continuation outside a section"),
            }
        } else {
            return err(no, &format!("non-directive line `{}`", truncate(raw)));
        }
    }
    flush_options(section, &mut options, &mut case.options);
    flush_file(&mut current, &mut case.files);
    if case.id.is_empty() {
        return err(text.lines().count(), "missing id directive");
    }
    if case.files.is_empty() {
        return err(text.lines().count(), "missing file section");
    }
    Ok(case)
}

#[allow(dead_code)] // format reader for the consuming lanes; exercised by tests
fn join_continuations(lines: &[String]) -> String {
    let mut joined = lines.join("\n");
    if !joined.is_empty() {
        joined.push('\n');
    }
    joined
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[allow(dead_code)] // format reader for the consuming lanes; exercised by tests
enum Section {
    Header,
    Options,
    File,
}

#[allow(dead_code)] // format reader for the consuming lanes; exercised by tests
fn err(no: usize, why: &str) -> Result<Case, CaseError> {
    Err(CaseError(format!("line {no}: {why}")))
}

#[allow(dead_code)] // format reader for the consuming lanes; exercised by tests
fn truncate(s: &str) -> &str {
    let end = s
        .char_indices()
        .nth(40)
        .map_or(s.len(), |(i, _)| i);
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(case: Case) {
        let text = write(&case);
        let back = read(&text).expect("parses");
        assert_eq!(back, case);
        assert_eq!(write(&back), text, "canonical form is stable");
    }

    fn sample() -> Case {
        Case {
            id: "diagnostics/abstract_field_initializer/x_test.dart/ClassTest#0".into(),
            source: "pkg/analyzer/test/src/diagnostics/x_test.dart#ClassTest.test".into(),
            language: Some("2.19".into()),
            packages: vec!["meta".into(), "flutter".into()],
            aux_packages: vec!["aaa".into()],
            options: Some("analyzer:\n  strict-casts: true\n".into()),
            ignores: vec!["unused_import".into()],
            files: vec![
                ("lib/test.dart".into(), "class A {}\nvar x = 1;\n".into()),
                ("lib/a.dart".into(), "part of 'test.dart';\n".into()),
            ],
        }
    }

    #[test]
    fn roundtrip_full() {
        roundtrip(sample());
    }

    #[test]
    fn roundtrip_minimal() {
        roundtrip(Case {
            id: "diagnostics/x/doc_0".into(),
            source: String::new(),
            language: None,
            packages: vec![],
            aux_packages: vec![],
            options: None,
            ignores: vec![],
            files: vec![("lib/test.dart".into(), "var x = 1;\n".into())],
        });
    }

    #[test]
    fn roundtrip_empty_file_and_blank_lines() {
        roundtrip(Case {
            id: "x".into(),
            source: String::new(),
            language: None,
            packages: vec![],
            aux_packages: vec![],
            options: Some(String::new()),
            ignores: vec![],
            files: vec![("lib/test.dart".into(), "\n\n".into())],
        });
    }

    #[test]
    fn content_with_directive_like_lines_roundtrips() {
        roundtrip(Case {
            id: "x".into(),
            source: String::new(),
            language: None,
            packages: vec![],
            aux_packages: vec![],
            options: None,
            ignores: vec![],
            files: vec![(
                "lib/test.dart".into(),
                "//- not a directive\n//+  also not\n".into(),
            )],
        });
    }

    #[test]
    fn empty_content_has_no_continuations() {
        let case = Case {
            id: "x".into(),
            source: String::new(),
            language: None,
            packages: vec![],
            aux_packages: vec![],
            options: None,
            ignores: vec![],
            files: vec![("lib/test.dart".into(), String::new())],
        };
        let text = write(&case);
        assert!(text.trim_end().ends_with("//- file: lib/test.dart"), "{text}");
        assert_eq!(read(&text).expect("parses"), case);
    }

    #[test]
    fn missing_trailing_newline_is_normalized() {
        let mut case = Case {
            id: "x".into(),
            source: String::new(),
            language: None,
            packages: vec![],
            aux_packages: vec![],
            options: None,
            ignores: vec![],
            files: vec![("lib/test.dart".into(), "var x = 1;".into())],
        };
        let text = write(&case);
        assert_eq!(read(&text).expect("parses").files[0].1, "var x = 1;\n");
        case.files[0].1 = "var x = 1;\n".into();
        assert_eq!(write(&case), text, "normalization is canonical");
    }

    #[test]
    fn rejects_non_directive_body() {
        assert!(read("var x = 1;\n").is_err());
        assert!(read("//- id: x\n").is_err());
        assert!(read("//- id: x\n//- file: a.dart\nraw\n").is_err());
    }
}
