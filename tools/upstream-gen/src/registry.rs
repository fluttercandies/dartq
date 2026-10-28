//! The upstream registry reader: parses the analyzer's consolidated
//! generated registry and the messages.yaml class groups, folds same-name
//! variants, and renders the checked-in TSV.
//!
//! Single source: `pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart` —
//! the file upstream generates from `pkg/analyzer/messages.yaml` (class
//! groups) plus `pkg/_fe_analyzer_shared/messages.yaml` (shared parser
//! codes surfaced through their `analyzerCode:` field). The yaml files
//! are read only for the class column and the per-class cross-check
//! tallies; every behavioral column (name, type, published docs,
//! messages) comes from the g.dart entries themselves.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// One `const Diagnostic...` entry of the generated registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The Dart const identifier (camelCase, e.g. `abiSpecificIntegerInvalid`).
    pub const_name: String,
    /// Official code name (`name:` field, `snake_case`).
    pub name: String,
    /// `problemMessage:` with adjacent literals concatenated and escapes
    /// resolved.
    pub problem: String,
    /// `correctionMessage:`, when the entry carries one.
    pub correction: Option<String>,
    /// `hasPublishedDocs:` (absent upstream means `false`: both impl
    /// constructors default the field to `false`).
    pub published: bool,
    /// `DiagnosticType.<X>` spelling, e.g. `COMPILE_TIME_ERROR`.
    pub dtype: String,
}

/// A same-name-folded registry row; the table is name-sorted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Registry class: a `pkg/analyzer/messages.yaml` class group, or
    /// `_fe_analyzer_shared` for the shared parser codes.
    pub class: String,
    pub name: String,
    pub dtype: String,
    pub published: bool,
    pub problem: String,
    pub correction: Option<String>,
    /// How many registry consts fold into this name (`sharedName`
    /// families; 1 for the plain case).
    pub variants: u32,
}

/// The severity of a `DiagnosticType` — decided upstream exactly as
/// `DiagnosticCodeImpl.severity => type.severity`, with every type
/// constant declared with its severity (upstream Dart SDK source,
/// `pkg/_fe_analyzer_shared/lib/src/base/errors.dart`):
/// `SYNTACTIC_ERROR`/`COMPILE_TIME_ERROR`/`CHECKED_MODE_COMPILE_TIME_ERROR`
/// carry ERROR, `STATIC_WARNING` WARNING, `HINT`/`TODO`/`LINT` INFO.
/// `None` marks a type unknown to the upstream enum — a parse error.
#[must_use]
pub fn severity_of(dtype: &str) -> Option<&'static str> {
    match dtype {
        "SYNTACTIC_ERROR" | "COMPILE_TIME_ERROR" | "CHECKED_MODE_COMPILE_TIME_ERROR" => {
            Some("ERROR")
        }
        "STATIC_WARNING" => Some("WARNING"),
        "HINT" | "TODO" | "LINT" => Some("INFO"),
        _ => None,
    }
}

/// Parses the generated registry into entries, in file order. Errors
/// carry `line: message`; the parse is total — one unrecognized shape
/// aborts the run rather than emitting a partial table.
pub fn parse_registry(src: &str) -> Result<Vec<Entry>, String> {
    let line_of = |offset: usize| 1 + src[..offset].matches('\n').count();
    let mut entries = Vec::new();
    let mut starts = src.match_indices("\nconst ").peekable();
    if starts.peek().is_none() {
        return Err("1: no `const Diagnostic` entries found".to_owned());
    }
    while let Some((at, _)) = starts.next() {
        let body_start = at + 1; // past the newline
        let body_end = starts.peek().map_or(src.len(), |(next, _)| *next);
        entries.push(parse_entry(
            &src[body_start..body_end],
            line_of(body_start),
        )?);
    }
    Ok(entries)
}

/// Parses one `const ...` declaration (the block from `const` up to the
/// next top-level const; trailing helper functions after the entry's
/// closing `);` are ignored).
fn parse_entry(block: &str, line: usize) -> Result<Entry, String> {
    let err = |what: &str| format!("{line}: {what}");
    let after_const = block
        .strip_prefix("const ")
        .ok_or_else(|| err("block does not start with `const `"))?;
    // Declared type: `DiagnosticWithArguments<...>` (generic may span
    // lines), `DiagnosticWithoutArguments`, or `DiagnosticCode`.
    let after_type = if let Some(rest) = after_const.strip_prefix("DiagnosticWithArguments") {
        let generic_end = rest
            .find('>')
            .ok_or_else(|| err("unterminated generic argument list"))?;
        &rest[generic_end + 1..]
    } else if let Some(rest) = after_const.strip_prefix("DiagnosticWithoutArguments") {
        rest
    } else if let Some(rest) = after_const.strip_prefix("DiagnosticCode") {
        rest
    } else {
        return Err(err("unrecognized declared type"));
    };
    let after_type = after_type.trim_start();
    let ident_len = after_type
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|&end| end > 0)
        .ok_or_else(|| err("missing const identifier"))?;
    let const_name = after_type[..ident_len].to_owned();

    let body = after_type[..after_type.find("\n);").unwrap_or(after_type.len())].to_owned();
    let field = |name: &str| -> Option<String> {
        let at = body.find(name)?;
        let (value, _) = dart_string_chain(&body[at + name.len()..]).ok()?;
        Some(value)
    };
    let name = field("name:").ok_or_else(|| err("missing name field"))?;
    let problem = field("problemMessage:").ok_or_else(|| err("missing problemMessage field"))?;
    let correction = field("correctionMessage:");
    let published = body.find("hasPublishedDocs:").is_some_and(|at| {
        body[at + "hasPublishedDocs:".len()..]
            .trim_start()
            .starts_with('t')
    });
    let dtype = body
        .find("type: DiagnosticType.")
        .map(|at| {
            body[at + "type: DiagnosticType.".len()..]
                .chars()
                .take_while(|c| c.is_ascii_uppercase() || *c == '_')
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .ok_or_else(|| err("missing type field"))?;
    if severity_of(&dtype).is_none() {
        return Err(err(&format!("unknown DiagnosticType `{dtype}`")));
    }
    Ok(Entry {
        const_name,
        name,
        problem,
        correction,
        published,
        dtype,
    })
}

/// Parses a chain of adjacent Dart string literals at the start of `s`;
/// returns the concatenated unescaped value and the offset past the
/// chain. Handles literals across newlines, single and double quotes,
/// and the escapes the registry uses (`\\`, `\'`, `\"`, `\$`, `\n`, `\t`,
/// `\r`); any other escape is an error naming the line.
fn dart_string_chain(s: &str) -> Result<(String, usize), String> {
    let mut out = String::new();
    let mut i = 0;
    let bytes = s.as_bytes();
    loop {
        while i < bytes.len() && matches!(bytes[i], b' ' | b'\n' | b'\r') {
            i += 1;
        }
        if !matches!(bytes.get(i), Some(b'"' | b'\'')) {
            break;
        }
        let quote = bytes[i];
        i += 1;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => {
                    let esc = bytes.get(i + 1).copied().unwrap_or(b'?');
                    let decoded = match esc {
                        b'\\' | b'\'' | b'"' | b'$' => char::from(esc),
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        _ => {
                            let at = 1 + s[..i].matches('\n').count();
                            return Err(format!("{at}: unsupported escape \\{}", char::from(esc)));
                        }
                    };
                    out.push(decoded);
                    i += 2;
                }
                b if b == quote => {
                    i += 1;
                    break;
                }
                _ => {
                    let ch = s[i..].chars().next().expect("byte index on char boundary");
                    out.push(ch);
                    i += ch.len_utf8();
                }
            }
        }
    }
    Ok((out, i))
}

/// `camelCase const identifier -> class` from the messages.yaml sources.
///
/// `analyzer_yaml` is class-grouped (outer key = class, inner keys =
/// const names); `fe_yaml` is the CFE-style flat map whose entries carry
/// an `analyzerCode:` field naming the const they surface into the
/// registry. Analyzer groups win on overlap; a registry const resolving
/// to neither source fails the [`fold`] cross-check.
pub fn class_map(analyzer_yaml: &str, fe_yaml: &str) -> Result<BTreeMap<String, String>, String> {
    let mut map = BTreeMap::new();
    let analyzer: serde_yaml::Value =
        serde_yaml::from_str(analyzer_yaml).map_err(|e| format!("analyzer messages.yaml: {e}"))?;
    let analyzer = analyzer
        .as_mapping()
        .ok_or("analyzer messages.yaml: root is not a mapping")?;
    for (class, entries) in analyzer {
        let class = class
            .as_str()
            .ok_or("analyzer messages.yaml: non-string class key")?;
        let Some(entries) = entries.as_mapping() else {
            continue;
        };
        for const_name in entries.keys() {
            let const_name = const_name
                .as_str()
                .ok_or("analyzer messages.yaml: non-string const key")?;
            map.insert(const_name.to_owned(), class.to_owned());
        }
    }
    let fe: serde_yaml::Value = serde_yaml::from_str(fe_yaml)
        .map_err(|e| format!("_fe_analyzer_shared messages.yaml: {e}"))?;
    let fe = fe
        .as_mapping()
        .ok_or("_fe_analyzer_shared messages.yaml: root is not a mapping")?;
    for (_key, entry) in fe {
        let Some(entry) = entry.as_mapping() else {
            continue;
        };
        for (field, value) in entry {
            if field.as_str() == Some("analyzerCode")
                && let Some(code) = value.as_str()
            {
                map.entry(code.to_owned())
                    .or_insert_with(|| "_fe_analyzer_shared".to_owned());
            }
        }
    }
    Ok(map)
}

/// Folds entries into name-keyed rows (the first registry entry wins the
/// per-variant columns) and cross-checks every const against the class
/// map. Rows come out sorted by name.
pub fn fold(entries: &[Entry], classes: &BTreeMap<String, String>) -> Result<Vec<Row>, String> {
    let mut rows: Vec<Row> = Vec::new();
    let mut index: BTreeMap<&str, usize> = BTreeMap::new();
    for entry in entries {
        let class = classes.get(&entry.const_name).ok_or_else(|| {
            format!(
                "registry const `{}` resolves to no messages.yaml class — \
                 a new upstream source the class map must learn",
                entry.const_name
            )
        })?;
        if let Some(&at) = index.get(entry.name.as_str()) {
            let row = &mut rows[at];
            row.variants += 1;
            // The any-published fold must stay exact: a name with
            // mixed published flags has no single correct URL decision.
            if row.published != entry.published {
                return Err(format!(
                    "`{}` has variants with mixed hasPublishedDocs",
                    entry.name
                ));
            }
        } else {
            index.insert(entry.name.as_str(), rows.len());
            rows.push(Row {
                class: class.clone(),
                name: entry.name.clone(),
                dtype: entry.dtype.clone(),
                published: entry.published,
                problem: entry.problem.clone(),
                correction: entry.correction.clone(),
                variants: 1,
            });
        }
    }
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(rows)
}

/// Renders the full TSV document (header comments + rows) — a pure
/// function of `rows`, the SDK version stamp, and the registry entry
/// count, so regeneration is byte-reproducible for identical inputs.
#[must_use]
pub fn render_tsv(rows: &[Row], registry_entries: usize, sdk_version: &str) -> String {
    let mut out = String::with_capacity(4096 + rows.len() * 128);
    let _ = writeln!(
        out,
        "# dartq official analyzer diagnostic codes — generated data table."
    );
    out.push_str("# DO NOT EDIT: regenerate with\n");
    out.push_str(
        "#   cargo run -p upstream-gen --bin analyzer-codes-gen -- --sdk-src <dart-sdk-tree>\n",
    );
    out.push_str("# Content pin: crates/dartq-diagnostics/tests/analyzer_codes_tsv.rs (blake3).\n");
    out.push_str("# Source: pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart of the pinned\n");
    let _ = writeln!(
        out,
        "#   SDK checkout ({sdk_version}), generated upstream from"
    );
    out.push_str("#   pkg/analyzer/messages.yaml (class groups) plus\n");
    out.push_str("#   pkg/_fe_analyzer_shared/messages.yaml (shared parser codes; their\n");
    out.push_str("#   class column spells `_fe_analyzer_shared`).\n");
    out.push_str("# Columns: <class> TAB <name> TAB <MACHINE> TAB <TYPE> TAB <SEV> TAB\n");
    out.push_str("#   <hasPublishedDocs> TAB <problem_message> [TAB <correction_message>];\n");
    out.push_str("#   the correction column is absent when upstream has none. Message\n");
    out.push_str("#   columns escape backslash/tab/newline/carriage-return as \\\\ \\t \\n \\r.\n");
    out.push_str("# MACHINE = uppercase(name). SEV derives from TYPE by the upstream\n");
    out.push_str("#   DiagnosticType constants: SYNTACTIC_ERROR/COMPILE_TIME_ERROR -> ERROR,\n");
    out.push_str("#   STATIC_WARNING -> WARNING, HINT/TODO -> INFO.\n");
    let _ = writeln!(
        out,
        "# {registry_entries} registry consts fold to {} name rows (sharedName families",
        rows.len()
    );
    out.push_str("#   keep the first registry entry; hasPublishedDocs folds any-variant —\n");
    out.push_str("#   upstream has no name with mixed flags, verified on every run).\n");
    out.push_str("# Rows are sorted by name. The syntax tier (data/syntax-codes.tsv) is\n");
    out.push_str("#   oracle-locked to dart 3.13.1 and OVERRIDES these rows for its 259\n");
    out.push_str("#   names in build.rs: 3.15 recast field_initializer_outside_constructor\n");
    out.push_str("#   to COMPILE_TIME_ERROR and moved two templates — the pinned table\n");
    out.push_str("#   wins for the acceptance set.\n");
    for row in rows {
        let _ = write!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            row.class,
            row.name,
            row.name.to_uppercase(),
            row.dtype,
            severity_of(&row.dtype).unwrap_or_else(|| {
                unreachable!("parse rejects unknown types, fold does not add any")
            }),
            row.published,
            tsv_escape(&row.problem),
        );
        if let Some(correction) = &row.correction {
            let _ = write!(out, "\t{}", tsv_escape(correction));
        }
        out.push('\n');
    }
    out
}

/// TSV field escaping for the message columns (tab is the separator, so
/// embedded separators must not split a column).
fn tsv_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A miniature of the real registry covering every entry shape: the
    /// no-arguments impl, the multi-line generic `DiagnosticWithArguments`,
    /// adjacent literal concatenation, quote/backslash escapes, an absent
    /// `hasPublishedDocs`, and a sharedName pair folding into one row.
    const SAMPLE: &str = r#"part of "package:analyzer/src/diagnostic/diagnostic.dart";

const DiagnosticWithoutArguments sampleOne = DiagnosticWithoutArgumentsImpl(
  name: 'sample_one',
  problemMessage: "Members of classes can't be declared 'abstract', and the "
      "message continues across adjacent literals.",
  correctionMessage:
      "Try removing the 'abstract' keyword, or escaping \"quotes\" and a '\\'.",
  hasPublishedDocs: true,
  type: DiagnosticType.SYNTACTIC_ERROR,
  uniqueName: 'sample_one',
  expectedTypes: [],
);

const DiagnosticWithArguments<
  LocatableDiagnostic Function({required String mappingName})
>
sampleTwo = DiagnosticWithArguments(
  name: 'sample_two',
  problemMessage:
      "Invalid mapping to '{0}'; only 'Int8' is \"supported\".",
  type: DiagnosticType.COMPILE_TIME_ERROR,
  uniqueName: 'sample_two',
  withArguments: _withArgumentsSampleTwo,
  expectedTypes: [ExpectedType.string],
);

const DiagnosticCode sampleTwoVariant = DiagnosticCodeWithExpectedTypes(
  name: 'sample_two',
  problemMessage: "A second variant folds into the shared name.",
  type: DiagnosticType.STATIC_WARNING,
  uniqueName: 'sample_two_variant',
);

const DiagnosticWithoutArguments sampleHint = DiagnosticWithoutArgumentsImpl(
  name: 'sample_hint',
  problemMessage: "A hint.",
  type: DiagnosticType.HINT,
);

LocatableDiagnostic _withArgumentsSampleTwo({
  required String mappingName,
}) {
  return LocatableDiagnosticImpl(diag.sampleTwo, [mappingName]);
}
"#;

    #[test]
    fn parses_every_entry_shape() {
        let entries = parse_registry(SAMPLE).expect("sample parses");
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].const_name, "sampleOne");
        assert_eq!(entries[0].name, "sample_one");
        assert_eq!(
            entries[0].problem,
            "Members of classes can't be declared 'abstract', and the message continues across adjacent literals."
        );
        assert_eq!(
            entries[0].correction.as_deref(),
            Some("Try removing the 'abstract' keyword, or escaping \"quotes\" and a '\\'.")
        );
        assert!(entries[0].published);
        assert_eq!(entries[0].dtype, "SYNTACTIC_ERROR");
        // Generic-declared entry: const name sits after the closing `>`.
        assert_eq!(entries[1].const_name, "sampleTwo");
        assert_eq!(
            entries[1].problem,
            "Invalid mapping to '{0}'; only 'Int8' is \"supported\"."
        );
        // `hasPublishedDocs` absent -> false.
        assert!(!entries[1].published);
        assert_eq!(entries[1].correction, None);
        // `DiagnosticCode`-declared entry parses like the rest.
        assert_eq!(entries[2].const_name, "sampleTwoVariant");
        // The trailing helper function is not a const.
        assert_eq!(entries[3].name, "sample_hint");
        assert_eq!(entries[3].dtype, "HINT");
    }

    #[test]
    fn severity_follows_the_upstream_type_constants() {
        assert_eq!(severity_of("SYNTACTIC_ERROR"), Some("ERROR"));
        assert_eq!(severity_of("COMPILE_TIME_ERROR"), Some("ERROR"));
        assert_eq!(
            severity_of("CHECKED_MODE_COMPILE_TIME_ERROR"),
            Some("ERROR")
        );
        assert_eq!(severity_of("STATIC_WARNING"), Some("WARNING"));
        assert_eq!(severity_of("HINT"), Some("INFO"));
        assert_eq!(severity_of("TODO"), Some("INFO"));
        assert_eq!(severity_of("LINT"), Some("INFO"));
        assert_eq!(severity_of("SOMETHING_NEW"), None);
    }

    #[test]
    fn class_map_merges_analyzer_groups_and_fe_codes() {
        let analyzer = "CompileTimeErrorCode:\n  sampleOne:\n    problemMessage: \"x\"\nHintCode:\n  sampleHint:\n    problemMessage: \"y\"\n";
        let fe = "sharedParserCode:\n  problemMessage: \"z\"\n  analyzerCode: sampleTwoVariant\n";
        let map = class_map(analyzer, fe).expect("yaml parses");
        assert_eq!(map["sampleOne"], "CompileTimeErrorCode");
        assert_eq!(map["sampleHint"], "HintCode");
        assert_eq!(map["sampleTwoVariant"], "_fe_analyzer_shared");
    }

    #[test]
    fn fold_keeps_first_variant_and_counts_shared_names() {
        let entries = parse_registry(SAMPLE).expect("sample parses");
        let classes = BTreeMap::from([
            ("sampleOne".to_owned(), "ParserErrorCode".to_owned()),
            ("sampleTwo".to_owned(), "ParserErrorCode".to_owned()),
            ("sampleTwoVariant".to_owned(), "ParserErrorCode".to_owned()),
            ("sampleHint".to_owned(), "HintCode".to_owned()),
        ]);
        let rows = fold(&entries, &classes).expect("fold succeeds");
        assert_eq!(rows.len(), 3);
        // Name-sorted: sample_hint, sample_one, sample_two.
        assert_eq!(rows[0].name, "sample_hint");
        assert_eq!(rows[1].name, "sample_one");
        let sample_two = &rows[2];
        assert_eq!(sample_two.variants, 2);
        assert_eq!(sample_two.dtype, "COMPILE_TIME_ERROR");
        assert_eq!(
            sample_two.problem,
            "Invalid mapping to '{0}'; only 'Int8' is \"supported\"."
        );
    }

    #[test]
    fn fold_rejects_mixed_published_flags() {
        let entries = vec![
            Entry {
                const_name: "a".to_owned(),
                name: "shared".to_owned(),
                problem: String::new(),
                correction: None,
                published: true,
                dtype: "HINT".to_owned(),
            },
            Entry {
                const_name: "b".to_owned(),
                name: "shared".to_owned(),
                problem: String::new(),
                correction: None,
                published: false,
                dtype: "HINT".to_owned(),
            },
        ];
        assert!(fold(&entries, &BTreeMap::new()).is_err());
    }

    #[test]
    fn fold_rejects_unmapped_consts() {
        let entries = vec![Entry {
            const_name: "nowhereToBeFound".to_owned(),
            name: "x".to_owned(),
            problem: String::new(),
            correction: None,
            published: false,
            dtype: "HINT".to_owned(),
        }];
        assert!(fold(&entries, &BTreeMap::new()).is_err());
    }

    #[test]
    fn tsv_escapes_separators_and_renders_rows() {
        let rows = vec![Row {
            class: "ParserErrorCode".to_owned(),
            name: "sample_one".to_owned(),
            dtype: "SYNTACTIC_ERROR".to_owned(),
            published: true,
            problem: "tab\there\nnewline back\\slash".to_owned(),
            correction: Some("fix\tit".to_owned()),
            variants: 1,
        }];
        let doc = render_tsv(&rows, 1, "dart 3.15.0 (main channel)");
        let data_line = doc.lines().find(|l| !l.starts_with('#')).expect("one row");
        assert_eq!(
            data_line,
            "ParserErrorCode\tsample_one\tSAMPLE_ONE\tSYNTACTIC_ERROR\tERROR\ttrue\ttab\\there\\nnewline back\\\\slash\tfix\\tit"
        );
        assert!(doc.contains("1 registry consts fold to 1 name rows"));
        assert!(doc.contains("dart 3.15.0 (main channel)"));
    }

    #[test]
    fn unsupported_escape_is_an_error() {
        assert!(dart_string_chain(r#""\x plus""#).is_err());
    }
}
