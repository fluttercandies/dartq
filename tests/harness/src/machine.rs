//! Parsing of `dart analyze` machine/json output and normalization of diagnostic entry keys.
//!
//! Wire-format source of truth: `.third/dart-sdk/pkg/dartdev/lib/src/commands/analyze.dart`
//! `emitMachineFormat` (stdout, uppercase codes). `analyzer_cli`'s `MachineErrorFormatter`
//! (`pkg/analyzer_cli/lib/src/error_formatter.dart`, stderr, lowercase codes) is the legacy CLI
//! path, not what this oracle prints. Format facts source:
//! docs/facts/oracle-machine-format.md §1 (machine EBNF/escapes), §2 (json fields).

use std::path::{Path, PathBuf};

use rustc_hash::FxHashMap;

/// All values of the diagnostic TYPE field in machine/json output (observed domain per facts §1/§2).
pub(crate) const DIAG_TYPES: [&str; 4] = [
    "SYNTACTIC_ERROR",
    "COMPILE_TIME_ERROR",
    "STATIC_WARNING",
    "LINT",
];

/// Diagnostic severity (first machine field, always uppercase).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "ERROR" => Some(Self::Error),
            "WARNING" => Some(Self::Warning),
            "INFO" => Some(Self::Info),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warning => "WARNING",
            Self::Info => "INFO",
        }
    }
}

/// Output format. machine is the primary differential format; json cross-checks the other documented oracle format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputFormat {
    Machine,
    Json,
}

impl OutputFormat {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "machine" => Some(Self::Machine),
            "json" => Some(Self::Json),
            _ => None,
        }
    }

    pub(crate) fn flag(self) -> &'static str {
        match self {
            Self::Machine => "--format=machine",
            Self::Json => "--format=json",
        }
    }
}

/// A single machine/json diagnostic. `code` is kept verbatim from the source side
/// (machine uppercase, json lowercase) and normalized to lowercase via [`EntryKeyBuilder::key`]
/// for comparison.
#[derive(Debug, Clone)]
#[allow(clippy::module_name_repetitions)] // mandated domain naming machine::MachineDiag
pub(crate) struct MachineDiag {
    pub(crate) severity: Severity,
    pub(crate) dtype: String,
    pub(crate) code: String,
    pub(crate) file: PathBuf,
    pub(crate) line: u32,
    pub(crate) col: u32,
    pub(crate) length: u32,
    pub(crate) message: String,
}

pub(crate) fn parse_output(text: &str, format: OutputFormat) -> Result<Vec<MachineDiag>, String> {
    match format {
        OutputFormat::Machine => parse_machine(text),
        OutputFormat::Json => parse_json(text),
    }
}

/// Parse machine output: each line has 8 `|`-separated fields (facts §1.1 EBNF); empty output =
/// no diagnostics; a blank line is malformed output and errors like every other malformed line
/// (dartdev machine mode never emits blank lines).
fn parse_machine(text: &str) -> Result<Vec<MachineDiag>, String> {
    let mut diags = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if line.is_empty() {
            return Err(format!(
                "machine line {} is blank; dartdev machine mode never emits blank lines",
                idx + 1
            ));
        }
        let fields = split_unescaped_pipe(line);
        let [sev, dtype, code, file, line_s, col_s, len_s, message] = fields.as_slice() else {
            return Err(format!(
                "machine line {} has {} fields, expected 8: {line}",
                idx + 1,
                fields.len()
            ));
        };
        let Some(severity) = Severity::parse(sev) else {
            return Err(format!(
                "machine line {}: unknown severity `{sev}`",
                idx + 1
            ));
        };
        let line_no = parse_num(line_s, "LINE", idx)?;
        let col = parse_num(col_s, "COLUMN", idx)?;
        let length = parse_num(len_s, "LENGTH", idx)?;
        diags.push(MachineDiag {
            severity,
            dtype: (*dtype).to_owned(),
            code: (*code).to_owned(),
            file: PathBuf::from(unescape_field(file)),
            line: line_no,
            col,
            length,
            message: unescape_field(message),
        });
    }
    Ok(diags)
}

/// Split on "unescaped `|`". The unescaping order is fixed here (facts §7.5): first the escape-aware split,
/// then a single unescape pass over `FILE_PATH`/`MESSAGE`; unescaping first would turn `\|` into a real separator and destroy field boundaries.
fn split_unescaped_pipe(line: &str) -> Vec<&str> {
    let mut fields = Vec::new();
    let mut start = 0;
    let mut chars = line.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == '\\' {
            // A backslash and the character after it form a pair; both are field content (`\|` is not a separator)
            chars.next();
        } else if c == '|' {
            fields.push(&line[start..i]);
            start = i + 1;
        }
    }
    fields.push(&line[start..]);
    fields
}

/// Single left-to-right unescape pass over the four escapes `\n` `\r` `\\` `\|` (oracle encoding only produces these four);
/// unknown escape sequences are kept verbatim, no guessing.
fn unescape_field(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('\\') | None => out.push('\\'),
            Some('|') => out.push('|'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    out
}

/// Parse json output (facts §2): a single-line JSON object `{version:1, diagnostics:[...]}`.
fn parse_json(text: &str) -> Result<Vec<MachineDiag>, String> {
    let root: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("json output is not valid JSON: {e}"))?;
    if root["version"].as_i64() != Some(1) {
        return Err(format!(
            "json output has unsupported version `{}`",
            root["version"]
        ));
    }
    let diagnostics = root["diagnostics"]
        .as_array()
        .ok_or_else(|| "json output has no diagnostics array".to_string())?;
    diagnostics.iter().map(parse_json_diag).collect()
}

fn parse_json_diag(obj: &serde_json::Value) -> Result<MachineDiag, String> {
    let severity = Severity::parse(json_str(obj, "severity")?)
        .ok_or_else(|| format!("json: unknown severity `{}`", obj["severity"]))?;
    let start = &obj["location"]["range"]["start"];
    let end = &obj["location"]["range"]["end"];
    let start_offset = json_num(start, "offset")?;
    let end_offset = json_num(end, "offset")?;
    if end_offset < start_offset {
        // Fail loud, matching the rest of the json parser: every other
        // malformed field (unknown severity, bad numbers, u32 overflow)
        // errors too. Clamping to 0 would fabricate a length from malformed
        // oracle output and mask real format drift.
        return Err(format!(
            "json: range end offset {end_offset} is before start offset \
             {start_offset} (code `{}`, {})",
            json_str(obj, "code")?,
            json_str(&obj["location"], "file")?
        ));
    }
    Ok(MachineDiag {
        severity,
        dtype: json_str(obj, "type")?.to_owned(),
        // json codes are always lowercase (facts §2), no conversion needed
        code: json_str(obj, "code")?.to_owned(),
        file: PathBuf::from(json_str(&obj["location"], "file")?),
        line: json_num(start, "line")?,
        col: json_num(start, "column")?,
        // json length = end.offset - start.offset (UTF-16 code units, facts §2)
        length: end_offset - start_offset,
        message: json_str(obj, "problemMessage")?.to_owned(),
    })
}

fn json_str<'a>(obj: &'a serde_json::Value, field: &str) -> Result<&'a str, String> {
    obj[field]
        .as_str()
        .ok_or_else(|| format!("json field `{field}` is not a string"))
}

fn json_num(obj: &serde_json::Value, field: &str) -> Result<u32, String> {
    let n = obj[field]
        .as_u64()
        .ok_or_else(|| format!("json field `{field}` is not a number"))?;
    u32::try_from(n).map_err(|_| format!("json field `{field}` overflows u32"))
}

fn parse_num(field: &str, what: &str, line_idx: usize) -> Result<u32, String> {
    field
        .parse::<u32>()
        .map_err(|_| format!("machine line {}: invalid {what} `{field}`", line_idx + 1))
}

/// Memoized entry-key builder for one side build. The oracle repeats the same file path across
/// many diagnostics (a large corpus emits ~10^5 instances over a few thousand distinct files)
/// and each `canonicalize` is a chain of syscalls, so raw paths are canonicalized once and the
/// result is reused. A failed canonicalize is memoized as the raw path itself, keeping the
/// fallback identical to the unmemoized behavior.
pub(crate) struct EntryKeyBuilder {
    corpus_root: PathBuf,
    canon: FxHashMap<PathBuf, PathBuf>,
}

impl EntryKeyBuilder {
    pub(crate) fn new(corpus_root: &Path) -> Self {
        Self {
            corpus_root: corpus_root.to_path_buf(),
            canon: FxHashMap::default(),
        }
    }

    /// Entry key = (lowercased code, path relative to the corpus). The oracle emits symlink-resolved
    /// absolute paths (facts §7.4); the dartq side canonicalizes before normalizing too, so
    /// `corpus_root` must already be canonicalized. Files outside the corpus root keep their
    /// canonical absolute path -- both sides normalize identically, so comparison still holds.
    pub(crate) fn key(&mut self, diag: &MachineDiag) -> (String, String) {
        let canon = self.canonical(&diag.file);
        let rel = canon.strip_prefix(&self.corpus_root).ok().map(|p| {
            p.components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        });
        let file = rel.unwrap_or_else(|| canon.display().to_string());
        (diag.code.to_lowercase(), file)
    }

    fn canonical(&mut self, raw: &Path) -> PathBuf {
        if let Some(canon) = self.canon.get(raw) {
            return canon.clone();
        }
        let canon = raw.canonicalize().unwrap_or_else(|_| raw.to_path_buf());
        self.canon.insert(raw.to_path_buf(), canon.clone());
        canon
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Observed sample from facts §1.2 (six lines, covering three severity tiers and two TYPEs).
    const GOLDEN: &str = "\
ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|/private/tmp/dq_oracle_scratch/pkg_min/lib/syntax_error.dart|2|11|1|Expected to find ';'.
WARNING|STATIC_WARNING|UNUSED_LOCAL_VARIABLE|/private/tmp/dq_oracle_scratch/pkg_min/lib/warn.dart|2|7|6|The value of the local variable 'unused' isn't used.
INFO|LINT|PREFER_SINGLE_QUOTES|/private/tmp/dq_oracle_scratch/pkg_min/lib/lint.dart|1|16|7|Unnecessary use of double quotes.
ERROR|SYNTACTIC_ERROR|UNTERMINATED_STRING_LITERAL|/private/tmp/dq_oracle_scratch/errcodes/lib/e4_unterminated_string.dart|1|15|1|Unterminated string literal.
WARNING|STATIC_WARNING|INVALID_LANGUAGE_VERSION_OVERRIDE|/private/tmp/dq_oracle_scratch/errcodes/lib/e8_dartver.dart|1|1|14|The language version override can't specify a version greater than the latest known language version: 3.13.
ERROR|COMPILE_TIME_ERROR|ARGUMENT_TYPE_NOT_ASSIGNABLE|/private/tmp/dq_oracle_scratch/corpus_a/lib/argument_type_not_assignable_nullability_error.dart|17|5|100|The argument type 'int?' can't be assigned to the parameter type 'int'.
";

    /// Observed escape sample from facts §1.1: message contains an escaped pipe.
    const ESCAPED: &str = "\
ERROR|COMPILE_TIME_ERROR|URI_DOES_NOT_EXIST|/private/tmp/dq_oracle_scratch/pkg_min/lib/pipe_test.dart|1|8|10|Target of URI doesn't exist: 'a\\|b.dart'.
";

    #[test]
    fn golden_six_lines_parse() {
        let diags = parse_machine(GOLDEN).expect("golden must parse");
        assert_eq!(diags.len(), 6);

        assert_eq!(diags[0].severity, Severity::Error);
        assert_eq!(diags[0].dtype, "SYNTACTIC_ERROR");
        assert_eq!(diags[0].code, "EXPECTED_TOKEN");
        assert_eq!(diags[0].line, 2);
        assert_eq!(diags[0].col, 11);
        assert_eq!(diags[0].length, 1);
        assert_eq!(diags[0].message, "Expected to find ';'.");

        assert_eq!(diags[1].severity, Severity::Warning);
        assert_eq!(diags[1].code, "UNUSED_LOCAL_VARIABLE");
        assert_eq!(
            diags[1].message,
            "The value of the local variable 'unused' isn't used."
        );

        assert_eq!(diags[2].severity, Severity::Info);
        assert_eq!(diags[2].dtype, "LINT");
        assert_eq!(
            diags[2].file,
            Path::new("/private/tmp/dq_oracle_scratch/pkg_min/lib/lint.dart")
        );

        assert_eq!(diags[5].length, 100);
    }

    #[test]
    fn escaped_pipe_in_message() {
        let diags = parse_machine(ESCAPED).expect("escaped line must parse");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "URI_DOES_NOT_EXIST");
        assert_eq!(diags[0].message, "Target of URI doesn't exist: 'a|b.dart'.");
    }

    #[test]
    fn escape_roundtrip_variants() {
        // `\\` = a literal backslash (at the end of the message, still counts as one escape pair)
        let line = "ERROR|SYNTACTIC_ERROR|X|/f.dart|1|1|2|a\\\\";
        let diags = parse_machine(line).expect("must parse");
        assert_eq!(diags[0].message, "a\\");

        let line = "WARNING|STATIC_WARNING|Y|/g.dart|1|1|11|line1\\nline2";
        let diags = parse_machine(line).expect("must parse");
        assert_eq!(diags[0].message, "line1\nline2");

        let line = "INFO|LINT|Z|/h\\i.dart|1|1|0|m";
        let diags = parse_machine(line).expect("must parse");
        assert_eq!(diags[0].file, Path::new("/h\\i.dart"));

        // `\\` followed by `\|`: the text `a\|b` is encoded as `a\\\|b`, producing no false separator
        let line = "ERROR|COMPILE_TIME_ERROR|W|/k.dart|1|1|3|a\\\\\\|b";
        let diags = parse_machine(line).expect("must parse");
        assert_eq!(diags[0].message, "a\\|b");
    }

    #[test]
    fn unknown_escape_kept_verbatim() {
        let line = "ERROR|SYNTACTIC_ERROR|X|/f.dart|1|1|2|\\x";
        let diags = parse_machine(line).expect("must parse");
        assert_eq!(diags[0].message, "\\x");
    }

    #[test]
    fn malformed_lines_error() {
        assert!(
            parse_machine("ERROR|SYNTACTIC_ERROR|X|/f.dart|1|1").is_err(),
            "7 fields must error"
        );
        assert!(
            parse_machine("ERROR|SYNTACTIC_ERROR|X|/f.dart|a|1|1|m").is_err(),
            "bad LINE must error"
        );
        assert!(
            parse_machine("FATAL|SYNTACTIC_ERROR|X|/f.dart|1|1|1|m").is_err(),
            "bad severity must error"
        );
        assert!(
            parse_machine(
                "ERROR|SYNTACTIC_ERROR|X|/f.dart|1|1|1|m\n\nERROR|SYNTACTIC_ERROR|Y|/g.dart|1|1|1|m"
            )
            .is_err(),
            "blank line must error"
        );
        assert!(parse_machine("\n").is_err(), "a lone blank line must error");
        assert!(
            parse_machine("").unwrap().is_empty(),
            "empty output = no diags (not a blank line)"
        );
    }

    /// Observed json sample from facts §2 (one entry, verbatim).
    #[test]
    fn json_sample_parse() {
        let line = r#"{"code":"unused_local_variable","severity":"WARNING","type":"STATIC_WARNING","location":{"file":"/private/tmp/dq_oracle_scratch/pkg_min/lib/warn.dart","range":{"start":{"offset":17,"line":2,"column":7},"end":{"offset":23,"line":2,"column":13}}},"problemMessage":"The value of the local variable 'unused' isn't used.","correctionMessage":"Try removing the variable or using it.","documentation":"https://dart.dev/diagnostics/unused_local_variable"}"#;
        let wrapped = format!(r#"{{"version":1,"diagnostics":[{line}]}}"#);
        let diags = parse_json(&wrapped).expect("json must parse");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Warning);
        assert_eq!(diags[0].dtype, "STATIC_WARNING");
        assert_eq!(diags[0].code, "unused_local_variable");
        assert_eq!(diags[0].line, 2);
        assert_eq!(diags[0].col, 7);
        assert_eq!(diags[0].length, 6);
        assert_eq!(
            diags[0].message,
            "The value of the local variable 'unused' isn't used."
        );
    }

    /// A reversed range (end offset < start offset) is malformed output: it
    /// must be a harness error citing both offsets, not a debug-panic underflow
    /// or a silently clamped length (strictness matches the rest of the parser).
    #[test]
    fn json_reversed_range_is_error() {
        let line = r#"{"code":"expected_token","severity":"ERROR","type":"SYNTACTIC_ERROR","location":{"file":"/c/lib/a.dart","range":{"start":{"offset":23,"line":1,"column":24},"end":{"offset":17,"line":1,"column":18}}},"problemMessage":"m"}"#;
        let wrapped = format!(r#"{{"version":1,"diagnostics":[{line}]}}"#);
        let err = parse_json(&wrapped).expect_err("reversed range must error");
        assert!(
            err.contains("23") && err.contains("17"),
            "cites both offsets: {err}"
        );
        assert!(err.contains("expected_token"), "cites the code: {err}");
    }

    #[test]
    fn entry_key_normalizes_case_and_path() {
        let root = std::env::temp_dir().join(format!("dq_key_{}", std::process::id()));
        let lib = root.join("lib");
        std::fs::create_dir_all(&lib).expect("mkdir");
        std::fs::write(lib.join("a.dart"), "var x = 1;").expect("write");
        // The root path itself is canonicalized (on macOS /tmp -> /private/tmp)
        let root_canon = root.canonicalize().expect("canonicalize root");
        let mut keys = EntryKeyBuilder::new(&root_canon);

        let diag = MachineDiag {
            severity: Severity::Error,
            dtype: "SYNTACTIC_ERROR".into(),
            code: "Expected_Token".into(),
            // Non-canonical form (contains ..); after canonicalize it should normalize to a path relative to the corpus
            file: lib.join("..").join("lib").join("a.dart"),
            line: 1,
            col: 1,
            length: 1,
            message: "m".into(),
        };
        let key = keys.key(&diag);
        assert_eq!(key, ("expected_token".into(), "lib/a.dart".into()));
        // Memo consistency: the same raw path through the cache must normalize
        // identically (the memo elides repeated syscalls, never changes results).
        assert_eq!(keys.key(&diag), key);

        // Files outside the corpus root keep the canonical absolute path (on macOS /etc -> /private/etc;
        // such symlink resolution is exactly canonicalize's job, and the expectation is normalized the same way)
        let outside = MachineDiag {
            file: PathBuf::from("/etc/hosts"),
            ..diag
        };
        let expected_outside = PathBuf::from("/etc/hosts").canonicalize().unwrap();
        let key = keys.key(&outside);
        assert_eq!(key.1, expected_outside.display().to_string());

        std::fs::remove_dir_all(&root).ok();
    }
}
