//! Canonical diagnostic rows and the `expected.tsv` line format.
//!
//! A row is `severity|code|file|line:col|length|message` — the full unit of
//! comparison between the oracle and dartq. The key
//! `severity|code|file|line:col` is the position-level unit used to report
//! message-only diffs separately.

/// One canonical diagnostic row, positioned relative to a case's package
/// root (`file` = e.g. `lib/test.dart`).
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Row {
    pub severity: String,
    pub code: String,
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub length: u64,
    pub message: String,
}

impl Row {
    /// The only construction point for a comparable row: the message is
    /// canonicalized here so that every path — the chunked runner's
    /// `attribute`, the realworld runner's `attribute`, and `parse_text` —
    /// stores the same form the oracle side was written in. Skipping this
    /// (e.g. a struct literal with a raw multi-line message) is what made
    /// every multi-line diagnostic count once as missing and once as extra.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        severity: String,
        code: String,
        file: String,
        line: u32,
        column: u32,
        length: u64,
        message: String,
    ) -> Row {
        Row {
            severity,
            code,
            file,
            line,
            column,
            length,
            message: sanitize(&message),
        }
    }

    pub fn key(&self) -> (String, String, String, u32, u32) {
        (
            self.severity.clone(),
            self.code.clone(),
            self.file.clone(),
            self.line,
            self.column,
        )
    }

    /// The full canonical row text; the message is last so it may contain
    /// `|` (parsed back with `splitn`).
    pub fn text(&self) -> String {
        format!(
            "{}|{}|{}|{}:{}|{}|{}",
            self.severity,
            self.code,
            self.file,
            self.line,
            self.column,
            self.length,
            sanitize(&self.message)
        )
    }

    pub fn parse_text(text: &str) -> Option<Row> {
        let parts: Vec<&str> = text.splitn(6, '|').collect();
        if parts.len() != 6 {
            return None;
        }
        let (line, column) = parts[3].split_once(':')?;
        Some(Row::new(
            parts[0].to_string(),
            parts[1].to_string(),
            parts[2].to_string(),
            line.parse().ok()?,
            column.parse().ok()?,
            parts[4].parse().ok()?,
            parts[5].to_string(),
        ))
    }
}

/// TSV-safe message: newlines and tabs collapse to spaces (messages never
/// gain meaning from them; positions carry the structure).
fn sanitize(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    for c in message.chars() {
        match c {
            '\n' | '\r' | '\t' => out.push(' '),
            _ => out.push(c),
        }
    }
    out
}

/// Sort order of canonical rows (full-tuple, so equal rows are adjacent).
pub fn sort_rows(rows: &mut Vec<Row>) {
    rows.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then(a.code.cmp(&b.code))
            .then(a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
            .then(a.column.cmp(&b.column))
            .then(a.length.cmp(&b.length))
            .then(a.message.cmp(&b.message))
    });
}

/// Multiset diff of two sorted row lists: rows present only in `expected`
/// (missing) and only in `actual` (extra), duplicate-count aware.
pub fn diff_sorted<'a>(expected: &'a [Row], actual: &'a [Row]) -> (Vec<&'a Row>, Vec<&'a Row>) {
    let mut missing = Vec::new();
    let mut extra = Vec::new();
    let mut i = 0usize;
    let mut j = 0usize;
    while i < expected.len() && j < actual.len() {
        match expected[i].cmp_text(&actual[j]) {
            std::cmp::Ordering::Less => {
                missing.push(&expected[i]);
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                extra.push(&actual[j]);
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                i += 1;
                j += 1;
            }
        }
    }
    missing.extend(expected[i..].iter());
    extra.extend(actual[j..].iter());
    (missing, extra)
}

impl Row {
    fn cmp_text(&self, other: &Row) -> std::cmp::Ordering {
        self.severity
            .cmp(&other.severity)
            .then(self.code.cmp(&other.code))
            .then(self.file.cmp(&other.file))
            .then(self.line.cmp(&other.line))
            .then(self.column.cmp(&other.column))
            .then(self.length.cmp(&other.length))
            .then(self.message.cmp(&other.message))
    }
}

/// A case's oracle-determined expectation: its canonical rows, or the
/// `oracle_crash` marker (the official analyzer itself failed on this case;
/// excluded from both sides).
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Expected {
    Rows(Vec<Row>),
    Crash,
}

impl Expected {
    pub fn render(&self) -> String {
        match self {
            Expected::Rows(rows) if rows.is_empty() => "-".to_string(),
            Expected::Rows(rows) => rows.iter().map(Row::text).collect::<Vec<_>>().join("\t"),
            Expected::Crash => "oracle_crash".to_string(),
        }
    }

    pub fn parse(cell: &str) -> Option<Expected> {
        match cell {
            "-" => Some(Expected::Rows(Vec::new())),
            "oracle_crash" => Some(Expected::Crash),
            _ => {
                let mut rows = Vec::new();
                for text in cell.split('\t') {
                    rows.push(Row::parse_text(text)?);
                }
                Some(Expected::Rows(rows))
            }
        }
    }

    pub fn rows(&self) -> &[Row] {
        match self {
            Expected::Rows(rows) => rows,
            Expected::Crash => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(sev: &str, code: &str, file: &str, line: u32, col: u32, len: u64, msg: &str) -> Row {
        Row::new(
            sev.into(),
            code.into(),
            file.into(),
            line,
            col,
            len,
            msg.into(),
        )
    }

    #[test]
    fn row_text_roundtrip() {
        let r = row(
            "ERROR",
            "undefined_identifier",
            "lib/test.dart",
            3,
            9,
            10,
            "Undefined name 'x'.",
        );
        assert_eq!(Row::parse_text(&r.text()), Some(r.clone()));
        assert_eq!(
            r.text(),
            "ERROR|undefined_identifier|lib/test.dart|3:9|10|Undefined name 'x'."
        );
    }

    #[test]
    fn message_may_contain_pipes_but_not_newlines() {
        let r = row("INFO", "c", "f.dart", 1, 1, 0, "a|b\tc\nd");
        let parsed = Row::parse_text(&r.text()).expect("parses");
        assert_eq!(parsed.message, "a|b c d");
    }

    /// Regression for the missing+extra double count: the oracle side stores
    /// messages with `\n`/`\r`/`\t` already collapsed to spaces, so a dartq
    /// row carrying the raw multi-line form must canonicalize to the same
    /// message at construction — otherwise `diff_sorted` reports the row
    /// once as missing and once as extra.
    #[test]
    fn multiline_message_matches_its_canonicalized_expected_row() {
        let expected = row(
            "ERROR",
            "could_not_infer",
            "lib/test.dart",
            3,
            9,
            10,
            "Couldn't infer type parameter 'T'. Tried to infer 'X' for 'T' which doesn't work:  Type parameter 'T' is declared to extend 'Y'.",
        );
        let actual = row(
            "ERROR",
            "could_not_infer",
            "lib/test.dart",
            3,
            9,
            10,
            "Couldn't infer type parameter 'T'.\nTried to infer 'X' for 'T' which doesn't work:\r\nType parameter 'T' is declared to extend\t'Y'.",
        );
        let mut expected = vec![expected];
        let mut actual = vec![actual];
        sort_rows(&mut expected);
        sort_rows(&mut actual);
        let (missing, extra) = diff_sorted(&expected, &actual);
        assert!(missing.is_empty(), "missing: {missing:?}");
        assert!(extra.is_empty(), "extra: {extra:?}");
    }

    #[test]
    fn sort_and_diff_are_duplicate_aware() {
        let a = row("ERROR", "a", "f.dart", 1, 1, 1, "m");
        let a2 = row("ERROR", "a", "f.dart", 1, 1, 1, "m");
        let b = row("WARNING", "b", "f.dart", 2, 1, 1, "m");
        let mut expected = vec![b.clone(), a.clone(), a.clone()];
        sort_rows(&mut expected);
        assert_eq!(expected, vec![a.clone(), a2.clone(), b]);
        let actual = vec![a.clone()];
        let (missing, extra) = diff_sorted(&expected, &actual);
        assert_eq!(missing.len(), 2); // one `a` plus `b`
        assert!(extra.is_empty());
        let (missing, extra) = diff_sorted(&actual, &expected);
        assert!(missing.is_empty());
        assert_eq!(extra.len(), 2);
    }

    #[test]
    fn expected_cell_vocabulary() {
        assert_eq!(Expected::Rows(Vec::new()).render(), "-");
        assert_eq!(Expected::Crash.render(), "oracle_crash");
        let rows = vec![
            row("ERROR", "a", "f.dart", 1, 1, 1, "m"),
            row("INFO", "b", "g.dart", 2, 3, 4, "n"),
        ];
        let text = Expected::Rows(rows.clone()).render();
        let parsed = Expected::parse(&text).expect("parses");
        assert_eq!(parsed.rows(), rows.as_slice());
        assert!(Expected::parse("garbage without tabs").is_none());
        assert!(Expected::parse("no|enough|fields").is_none());
    }
}
