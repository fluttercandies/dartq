//! Golden tests pinning `write_machine`/`write_json` to measured oracle
//! output (the oracle machine-format contract §1/§2). File contents are
//! reconstructed so the fixtures reproduce the oracle's measured positions.

use std::path::Path;

use dartq_diagnostics::{
    AnalysisOutput, CodeInfo, CodeRegistry, Diagnostic, DiagnosticType, FileId, FileLayoutProvider,
    Severity, SourceRange, write_json, write_machine,
};
use dartq_source::{FileTable, LineIndex};

const SYNTAX_ERROR_PATH: &str = "/private/tmp/dq_oracle_scratch/pkg_min/lib/syntax_error.dart";
const WARN_PATH: &str = "/private/tmp/dq_oracle_scratch/pkg_min/lib/warn.dart";
const LINT_PATH: &str = "/private/tmp/dq_oracle_scratch/pkg_min/lib/lint.dart";
const E8_DARTVER_PATH: &str = "/private/tmp/dq_oracle_scratch/errcodes/lib/e8_dartver.dart";
const PIPE_TEST_PATH: &str = "/private/tmp/dq_oracle_scratch/pkg_min/lib/pipe_test.dart";
const E4_UNTERMINATED_PATH: &str =
    "/private/tmp/dq_oracle_scratch/errcodes/lib/e4_unterminated_string.dart";
const ARG_TYPE_PATH: &str = "/private/tmp/dq_oracle_scratch/corpus_a/lib/argument_type_not_assignable_nullability_error.dart";

/// Minimal `FileLayoutProvider` over in-memory files; resolves 1-based
/// line/column through the production path — [`LineIndex`] for line starts
/// plus its indexed byte↔UTF-16 conversion methods, mirroring what the CLI
/// wires up. `FileId`s are real ids issued by a [`FileTable`] (the type has
/// no other constructor).
#[derive(Default)]
struct Fixture {
    table: FileTable,
    contents: Vec<String>,
    line_indexes: Vec<LineIndex>,
}

impl Fixture {
    fn add(&mut self, path: &'static str, content: impl Into<String>) -> FileId {
        let content = content.into();
        self.line_indexes.push(LineIndex::new(&content));
        self.contents.push(content);
        self.table.intern(Path::new(path))
    }

    fn output<'a>(&'a self, diagnostics: &'a [Diagnostic]) -> AnalysisOutput<'a> {
        AnalysisOutput {
            diagnostics,
            files: self,
        }
    }
}

impl FileLayoutProvider for Fixture {
    fn file_path(&self, file: FileId) -> &Path {
        self.table.path(file).expect("fixture interned this file")
    }

    fn line_col(&self, file: FileId, utf16_offset: u32) -> (u32, u32) {
        let index = file.index() as usize;
        // Diagnostic offsets are UTF-16; the fused conversion reports the
        // official `LineInfo.getLocation` (line, UTF-16 column) directly.
        self.line_indexes[index].line_col_utf16(&self.contents[index], utf16_offset)
    }
}

fn diagnostic(
    severity: Severity,
    code: CodeInfo,
    message: &'static str,
    file: FileId,
    range: SourceRange,
) -> Diagnostic {
    Diagnostic::new(severity, code, message, file, range, None)
}

fn machine(output: &AnalysisOutput<'_>) -> String {
    let mut buf = Vec::new();
    write_machine(&mut buf, output).unwrap();
    String::from_utf8(buf).unwrap()
}

fn json(output: &AnalysisOutput<'_>) -> String {
    let mut buf = Vec::new();
    write_json(&mut buf, output).unwrap();
    String::from_utf8(buf).unwrap()
}

#[test]
fn machine_empty_diagnostics_write_nothing() {
    let files = Fixture::default();
    assert_eq!(machine(&files.output(&[])), "");
}

#[test]
fn json_empty_diagnostics_write_bare_envelope() {
    // Oracle facts §1.2 (2026-10-08 audit correction): --format=json always
    // prints the envelope, even with zero diagnostics — exactly 31 bytes
    // including the trailing newline (`od -c` measured). Only --format=machine
    // writes nothing.
    let files = Fixture::default();
    assert_eq!(
        json(&files.output(&[])),
        "{\"version\":1,\"diagnostics\":[]}\n"
    );
}

#[test]
fn machine_golden_expected_token() {
    let mut files = Fixture::default();
    let f = files.add(SYNTAX_ERROR_PATH, "void broken() {\n  int x = 1\n}\n");
    let d = diagnostic(
        Severity::Error,
        dartq_diagnostics::EXPECTED_TOKEN,
        "Expected to find ';'.",
        f,
        SourceRange {
            offset: 26,
            length: 1,
        },
    );
    assert_eq!(
        machine(&files.output(&[d])),
        "ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|\
         /private/tmp/dq_oracle_scratch/pkg_min/lib/syntax_error.dart|2|11|1|Expected to find ';'.\n"
    );
}

#[test]
fn machine_golden_unused_local_variable() {
    let mut files = Fixture::default();
    let f = files.add(WARN_PATH, "void f() {\n  var unused = 1;\n}\n");
    let d = diagnostic(
        Severity::Warning,
        dartq_diagnostics::UNUSED_LOCAL_VARIABLE,
        "The value of the local variable 'unused' isn't used.",
        f,
        SourceRange {
            offset: 17,
            length: 6,
        },
    );
    assert_eq!(
        machine(&files.output(&[d])),
        "WARNING|STATIC_WARNING|UNUSED_LOCAL_VARIABLE|\
         /private/tmp/dq_oracle_scratch/pkg_min/lib/warn.dart|2|7|6|\
         The value of the local variable 'unused' isn't used.\n"
    );
}

#[test]
fn machine_golden_lint_info() {
    let mut files = Fixture::default();
    let f = files.add(LINT_PATH, "var greeting = \"hello\";\n");
    let d = diagnostic(
        Severity::Info,
        dartq_diagnostics::PREFER_SINGLE_QUOTES,
        "Unnecessary use of double quotes.",
        f,
        SourceRange {
            offset: 15,
            length: 7,
        },
    );
    assert_eq!(
        machine(&files.output(&[d])),
        "INFO|LINT|PREFER_SINGLE_QUOTES|\
         /private/tmp/dq_oracle_scratch/pkg_min/lib/lint.dart|1|16|7|\
         Unnecessary use of double quotes.\n"
    );
}

#[test]
fn machine_golden_invalid_language_version_override() {
    let mut files = Fixture::default();
    let f = files.add(E8_DARTVER_PATH, "// @dart = 4.0\n");
    let d = diagnostic(
        Severity::Warning,
        dartq_diagnostics::INVALID_LANGUAGE_VERSION_OVERRIDE,
        "The language version override can't specify a version greater than \
         the latest known language version: 3.13.",
        f,
        SourceRange {
            offset: 0,
            length: 14,
        },
    );
    assert_eq!(
        machine(&files.output(&[d])),
        "WARNING|STATIC_WARNING|INVALID_LANGUAGE_VERSION_OVERRIDE|\
         /private/tmp/dq_oracle_scratch/errcodes/lib/e8_dartver.dart|1|1|14|\
         The language version override can't specify a version greater than \
         the latest known language version: 3.13.\n"
    );
}

#[test]
fn machine_golden_unterminated_string_literal() {
    let mut files = Fixture::default();
    let f = files.add(E4_UNTERMINATED_PATH, "main() { var s = 'abc\n");
    let d = diagnostic(
        Severity::Error,
        dartq_diagnostics::UNTERMINATED_STRING_LITERAL,
        "Unterminated string literal.",
        f,
        SourceRange {
            offset: 14,
            length: 1,
        },
    );
    assert_eq!(
        machine(&files.output(&[d])),
        "ERROR|SYNTACTIC_ERROR|UNTERMINATED_STRING_LITERAL|\
         /private/tmp/dq_oracle_scratch/errcodes/lib/e4_unterminated_string.dart|\
         1|15|1|Unterminated string literal.\n"
    );
}

#[test]
fn machine_golden_pipe_message_is_escaped() {
    // Oracle facts §1.1: `import "a|b.dart";` → message field carries 'a\|b.dart'.
    let mut files = Fixture::default();
    let f = files.add(PIPE_TEST_PATH, "import \"a|b.dart\";\n");
    let d = diagnostic(
        Severity::Error,
        dartq_diagnostics::URI_DOES_NOT_EXIST,
        "Target of URI doesn't exist: 'a|b.dart'.",
        f,
        SourceRange {
            offset: 7,
            length: 10,
        },
    );
    assert_eq!(
        machine(&files.output(&[d])),
        r"ERROR|COMPILE_TIME_ERROR|URI_DOES_NOT_EXIST|/private/tmp/dq_oracle_scratch/pkg_min/lib/pipe_test.dart|1|8|10|Target of URI doesn't exist: 'a\|b.dart'.".to_owned()
            + "\n",
    );
}

#[test]
fn machine_escapes_backslash() {
    let mut files = Fixture::default();
    let f = files.add(WARN_PATH, "void f() {}\n");
    let d = diagnostic(
        Severity::Error,
        dartq_diagnostics::EXPECTED_TOKEN,
        r"path like C:\dir",
        f,
        SourceRange {
            offset: 0,
            length: 1,
        },
    );
    // Every backslash doubled in the message field.
    assert_eq!(
        machine(&files.output(&[d])),
        r"ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|/private/tmp/dq_oracle_scratch/pkg_min/lib/warn.dart|1|1|1|path like C:\\dir".to_owned()
            + "\n",
    );
}

#[test]
fn machine_escapes_backslash_in_owned_message() {
    // The `Cow::Owned` arm of `Diagnostic.message` (interpolated `String`
    // rendered at the emit site) must escape identically to borrowed
    // templates.
    let mut files = Fixture::default();
    let f = files.add(WARN_PATH, "void f() {}\n");
    let d = Diagnostic::new(
        Severity::Error,
        dartq_diagnostics::EXPECTED_TOKEN,
        format!("back\\slash {}", 1),
        f,
        SourceRange {
            offset: 0,
            length: 1,
        },
        None,
    );
    assert_eq!(
        machine(&files.output(&[d])),
        r"ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|/private/tmp/dq_oracle_scratch/pkg_min/lib/warn.dart|1|1|1|back\\slash 1".to_owned()
            + "\n",
    );
}

#[test]
fn machine_escapes_newline_and_carriage_return() {
    let mut files = Fixture::default();
    let f = files.add(WARN_PATH, "void f() {}\n");
    let d = diagnostic(
        Severity::Error,
        dartq_diagnostics::EXPECTED_TOKEN,
        "line1\nline2\rend",
        f,
        SourceRange {
            offset: 0,
            length: 1,
        },
    );
    // Newline and CR become the literal two-character sequences `\n` and `\r`;
    // the diagnostic stays on a single output line.
    let out = machine(&files.output(&[d]));
    assert_eq!(out.lines().count(), 1);
    assert!(out.ends_with("line1\\nline2\\rend\n"), "{out}");
}

#[test]
fn machine_escapes_file_path_field() {
    // Fixture path spells /tmp but `FileTable::intern` canonicalizes the
    // existing ancestor to /private/tmp (macOS symlink, same spelling the
    // other fixtures use); the assertion pins the ESCAPING of `|` and `\`.
    let mut files = Fixture::default();
    let f = files.add(r"/tmp/a|b\c.dart", "x\n");
    let d = diagnostic(
        Severity::Error,
        dartq_diagnostics::EXPECTED_TOKEN,
        "boom",
        f,
        SourceRange {
            offset: 0,
            length: 1,
        },
    );
    let out = machine(&files.output(&[d]));
    assert!(out.contains(r"|/private/tmp/a\|b\\c.dart|"), "{out}");
}

#[test]
fn machine_multiple_diagnostics_keep_field_order() {
    let mut files = Fixture::default();
    let syntax = files.add(SYNTAX_ERROR_PATH, "void broken() {\n  int x = 1\n}\n");
    let warn = files.add(WARN_PATH, "void f() {\n  var unused = 1;\n}\n");
    // 16 one-char lines, then line 17: oracle facts §1.2 last sample row
    // (17|5|100) — note the trailing space in the measured message.
    let arg_type = files.add(ARG_TYPE_PATH, "x\n".repeat(16) + "int f(int? x) => g(x);\n");
    let diagnostics = [
        diagnostic(
            Severity::Error,
            dartq_diagnostics::EXPECTED_TOKEN,
            "Expected to find ';'.",
            syntax,
            SourceRange {
                offset: 26,
                length: 1,
            },
        ),
        diagnostic(
            Severity::Warning,
            dartq_diagnostics::UNUSED_LOCAL_VARIABLE,
            "The value of the local variable 'unused' isn't used.",
            warn,
            SourceRange {
                offset: 17,
                length: 6,
            },
        ),
        diagnostic(
            Severity::Error,
            dartq_diagnostics::ARGUMENT_TYPE_NOT_ASSIGNABLE,
            "The argument type 'int?' can't be assigned to the parameter type 'int'. ",
            arg_type,
            SourceRange {
                offset: 36,
                length: 100,
            },
        ),
    ];
    assert_eq!(
        machine(&files.output(&diagnostics)),
        "ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|\
         /private/tmp/dq_oracle_scratch/pkg_min/lib/syntax_error.dart|2|11|1|Expected to find ';'.\n\
         WARNING|STATIC_WARNING|UNUSED_LOCAL_VARIABLE|\
         /private/tmp/dq_oracle_scratch/pkg_min/lib/warn.dart|2|7|6|\
         The value of the local variable 'unused' isn't used.\n\
         ERROR|COMPILE_TIME_ERROR|ARGUMENT_TYPE_NOT_ASSIGNABLE|\
         /private/tmp/dq_oracle_scratch/corpus_a/lib/argument_type_not_assignable_nullability_error.dart|\
         17|5|100|The argument type 'int?' can't be assigned to the parameter type 'int'. \n"
    );
}

#[test]
fn machine_golden_bare_cr_is_a_line_break() {
    // Official `LineInfo` semantics: a lone `\r` ends a line, so the second
    // diagnostic lands on line 2 column 5 — not line 1 column 16.
    let mut files = Fixture::default();
    let f = files.add(WARN_PATH, "var a = 1;\rvar b = 2;\r");
    let diagnostics = [
        diagnostic(
            Severity::Error,
            dartq_diagnostics::EXPECTED_TOKEN,
            "Expected to find ';'.",
            f,
            SourceRange {
                offset: 4,
                length: 1,
            },
        ),
        diagnostic(
            Severity::Error,
            dartq_diagnostics::EXPECTED_TOKEN,
            "Expected to find ';'.",
            f,
            SourceRange {
                offset: 15,
                length: 1,
            },
        ),
    ];
    assert_eq!(
        machine(&files.output(&diagnostics)),
        "ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|\
         /private/tmp/dq_oracle_scratch/pkg_min/lib/warn.dart|1|5|1|Expected to find ';'.\n\
         ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|\
         /private/tmp/dq_oracle_scratch/pkg_min/lib/warn.dart|2|5|1|Expected to find ';'.\n"
    );
}

#[test]
fn machine_golden_column_counts_utf16_units_after_emoji() {
    // `// 😀 ok\n`: the emoji is one char but two UTF-16 code units, so the
    // column of `ok` is 7 (UTF-16), not 9 (bytes).
    let mut files = Fixture::default();
    let f = files.add(WARN_PATH, "// \u{1F600} ok\n");
    let d = diagnostic(
        Severity::Error,
        dartq_diagnostics::EXPECTED_TOKEN,
        "Expected to find ';'.",
        f,
        SourceRange {
            offset: 6,
            length: 2,
        },
    );
    assert_eq!(
        machine(&files.output(&[d])),
        "ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|\
         /private/tmp/dq_oracle_scratch/pkg_min/lib/warn.dart|1|7|2|Expected to find ';'.\n"
    );
}

#[test]
fn json_golden_range_end_column_after_emoji() {
    // The JSON range resolves both endpoints through the same path; `target`
    // spans UTF-16 offsets 6..12 → line 1, columns 7 and 13.
    let mut files = Fixture::default();
    let f = files.add(WARN_PATH, "// \u{1F600} target\n");
    let d = diagnostic(
        Severity::Error,
        dartq_diagnostics::EXPECTED_TOKEN,
        "Expected to find ';'.",
        f,
        SourceRange {
            offset: 6,
            length: 6,
        },
    );
    let out = json(&files.output(&[d]));
    assert!(
        out.contains(
            r#""range":{"start":{"offset":6,"line":1,"column":7},"end":{"offset":12,"line":1,"column":13}}"#
        ),
        "{out}"
    );
}

#[test]
fn json_golden_unused_local_variable() {
    // Byte-exact oracle facts §2 sample, wrapped in the version envelope.
    // Every oracle json output ends with a newline (measured `]}\n`).
    let mut files = Fixture::default();
    let f = files.add(WARN_PATH, "void f() {\n  var unused = 1;\n}\n");
    let d = Diagnostic::new(
        Severity::Warning,
        dartq_diagnostics::UNUSED_LOCAL_VARIABLE,
        "The value of the local variable 'unused' isn't used.",
        f,
        SourceRange {
            offset: 17,
            length: 6,
        },
        Some(String::from("Try removing the variable or using it.")),
    );
    assert_eq!(
        json(&files.output(&[d])),
        concat!(
            r#"{"version":1,"diagnostics":[{"code":"unused_local_variable","severity":"WARNING","#,
            r#""type":"STATIC_WARNING","location":{"file":"/private/tmp/dq_oracle_scratch/pkg_min/lib/warn.dart","#,
            r#""range":{"start":{"offset":17,"line":2,"column":7},"end":{"offset":23,"line":2,"column":13}}},"#,
            r#""problemMessage":"The value of the local variable 'unused' isn't used.","#,
            r#""correctionMessage":"Try removing the variable or using it.","#,
            r#""documentation":"https://dart.dev/diagnostics/unused_local_variable"}]}"#,
            "\n",
        )
    );
}

#[test]
fn json_uses_lowercase_code_and_uppercase_severity_type() {
    let mut files = Fixture::default();
    let f = files.add(SYNTAX_ERROR_PATH, "void broken() {\n  int x = 1\n}\n");
    let d = diagnostic(
        Severity::Error,
        dartq_diagnostics::EXPECTED_TOKEN,
        "Expected to find ';'.",
        f,
        SourceRange {
            offset: 26,
            length: 1,
        },
    );
    let out = json(&files.output(&[d]));
    assert!(out.contains(r#""code":"expected_token""#), "{out}");
    assert!(out.contains(r#""severity":"ERROR""#), "{out}");
    assert!(out.contains(r#""type":"SYNTACTIC_ERROR""#), "{out}");
    assert!(!out.contains("EXPECTED_TOKEN"), "{out}");
}

#[test]
fn json_omits_correction_when_absent() {
    let mut files = Fixture::default();
    let f = files.add(SYNTAX_ERROR_PATH, "void broken() {\n");
    let d = diagnostic(
        Severity::Error,
        dartq_diagnostics::EXPECTED_TOKEN,
        "Expected to find ';'.",
        f,
        // Anchored at EOF: the source is 16 UTF-16 units, so `end()` must
        // stay at 16 — positions past the source are engine bugs that the
        // dartq-source conversions surface instead of clamping.
        SourceRange {
            offset: 16,
            length: 0,
        },
    );
    let out = json(&files.output(&[d]));
    assert!(out.contains(r#""problemMessage""#), "{out}");
    assert!(!out.contains("correctionMessage"), "{out}");
}

#[test]
fn json_omits_documentation_for_dartq_codes() {
    let mut files = Fixture::default();
    let f = files.add(WARN_PATH, "x\n");
    let d = diagnostic(
        Severity::Error,
        dartq_diagnostics::DARTQ_INTERNAL_ERROR,
        "worker panicked: stack overflow",
        f,
        SourceRange {
            offset: 0,
            length: 1,
        },
    );
    let out = json(&files.output(&[d]));
    assert!(out.contains(r#""code":"dartq/internal_error""#), "{out}");
    assert!(out.contains(r#""type":"TOOL""#), "{out}");
    assert!(!out.contains("documentation"), "{out}");
}

#[test]
fn json_omits_documentation_for_unpublished_official_code() {
    // Audit F1: the oracle emits `documentation` only for codes with
    // `hasPublishedDocs: true`; `expected_token` is unpublished (no
    // `hasPublishedDocs` in the generated registry; measured oracle run
    // shows no `documentation` key), so the key must be absent entirely.
    let mut files = Fixture::default();
    let f = files.add(SYNTAX_ERROR_PATH, "void broken() {\n  int x = 1\n}\n");
    let d = diagnostic(
        Severity::Error,
        dartq_diagnostics::EXPECTED_TOKEN,
        "Expected to find ';'.",
        f,
        SourceRange {
            offset: 26,
            length: 1,
        },
    );
    let out = json(&files.output(&[d]));
    assert!(out.contains(r#""code":"expected_token""#), "{out}");
    assert!(
        !out.contains("documentation"),
        "unpublished code must not carry a documentation key: {out}"
    );
    assert!(
        !out.contains("dart.dev"),
        "no dart.dev URL may leak for unpublished codes: {out}"
    );
}

#[test]
fn json_escapes_special_characters_in_message() {
    let mut files = Fixture::default();
    let f = files.add(WARN_PATH, "x\n");
    let d = diagnostic(
        Severity::Error,
        dartq_diagnostics::EXPECTED_TOKEN,
        "quote \" backslash \\ newline \n tab \t pipe | end",
        f,
        SourceRange {
            offset: 0,
            length: 1,
        },
    );
    let out = json(&files.output(&[d]));
    assert!(
        out.contains(r#""problemMessage":"quote \" backslash \\ newline \n tab \t pipe | end""#),
        "{out}"
    );
}

#[test]
fn severity_orders_info_then_warning_then_error() {
    let mut diagnostics = [Severity::Error, Severity::Info, Severity::Warning];
    diagnostics.sort();
    assert_eq!(
        diagnostics,
        [Severity::Info, Severity::Warning, Severity::Error]
    );
    assert!(Severity::Info < Severity::Warning && Severity::Warning < Severity::Error);
}

#[test]
fn severity_display_is_capitalized_machine_form_is_uppercase() {
    assert_eq!(Severity::Error.to_string(), "Error");
    assert_eq!(Severity::Warning.to_string(), "Warning");
    assert_eq!(Severity::Info.to_string(), "Info");
    assert_eq!(Severity::Error.as_machine_str(), "ERROR");
    assert_eq!(Severity::Warning.as_machine_str(), "WARNING");
    assert_eq!(Severity::Info.as_machine_str(), "INFO");
}

#[test]
fn diagnostic_type_display_is_uppercase_underscore() {
    assert_eq!(
        DiagnosticType::SyntacticError.to_string(),
        "SYNTACTIC_ERROR"
    );
    assert_eq!(
        DiagnosticType::CompileTimeError.to_string(),
        "COMPILE_TIME_ERROR"
    );
    assert_eq!(DiagnosticType::StaticWarning.to_string(), "STATIC_WARNING");
    assert_eq!(DiagnosticType::Lint.to_string(), "LINT");
    assert_eq!(DiagnosticType::Tool.to_string(), "TOOL");
}

#[test]
fn code_registry_looks_up_official_and_dartq_codes() {
    let registry = CodeRegistry::builtin();
    assert_eq!(
        registry.get("expected_token"),
        Some(dartq_diagnostics::EXPECTED_TOKEN)
    );
    assert_eq!(
        registry.get("dartq/internal_error"),
        Some(dartq_diagnostics::DARTQ_INTERNAL_ERROR)
    );
    // Lookup is by snake_case name only; the machine form is not a key.
    assert_eq!(registry.get("EXPECTED_TOKEN"), None);
    assert_eq!(registry.get("no_such_code"), None);
    // Every builtin code is reachable, and machine form is always the
    // uppercase of the name.
    let mut count = 0;
    for code in registry.iter() {
        assert_eq!(registry.get(code.name), Some(code));
        assert_eq!(code.machine, code.name.to_uppercase());
        count += 1;
    }
    // 259 official table codes (7 measured-dead rows pruned, #222) + 15
    // hand-written codes.
    assert_eq!(count, 274);
}
