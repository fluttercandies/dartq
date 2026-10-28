//! The `cases.manifest`: sorted `id<TAB>blake3` rows, one per extracted
//! case, proving that the pin produced exactly these case bytes.

/// One manifest row.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Row {
    pub id: String,
    pub hash: String,
}

/// Builds rows from (id, case bytes) pairs.
pub fn build<'a>(cases: impl Iterator<Item = (&'a String, &'a [u8])>) -> Vec<Row> {
    let mut rows: Vec<Row> = cases
        .map(|(id, bytes)| Row {
            id: id.clone(),
            hash: blake3::hash(bytes).to_string(),
        })
        .collect();
    rows.sort();
    rows
}

/// Serializes rows (sorted, `id<TAB>hash`, LF-terminated).
pub fn render(rows: &[Row]) -> String {
    let mut out = String::with_capacity(rows.len() * 90);
    for row in rows {
        out.push_str(&row.id);
        out.push('\t');
        out.push_str(&row.hash);
        out.push('\n');
    }
    out
}

/// Parses manifest bytes; malformed lines are an error (the manifest is
/// machine-written).
pub fn parse(text: &str) -> Result<Vec<Row>, String> {
    let mut rows = Vec::new();
    for (no, line) in text.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let Some((id, hash)) = line.split_once('\t') else {
            return Err(format!("line {}: expected `id<TAB>hash`", no + 1));
        };
        rows.push(Row {
            id: id.to_string(),
            hash: hash.to_string(),
        });
    }
    Ok(rows)
}

/// Diffs two manifests: (missing, unexpected, changed).
pub fn diff<'a>(
    expected: &'a [Row],
    actual: &'a [Row],
) -> (Vec<&'a Row>, Vec<&'a Row>, Vec<(&'a Row, &'a Row)>) {
    let mut missing = Vec::new();
    let mut unexpected = Vec::new();
    let mut changed = Vec::new();
    let mut i = 0usize;
    let mut j = 0usize;
    while i < expected.len() && j < actual.len() {
        match expected[i].id.cmp(&actual[j].id) {
            std::cmp::Ordering::Less => {
                missing.push(&expected[i]);
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                unexpected.push(&actual[j]);
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                if expected[i].hash != actual[j].hash {
                    changed.push((&expected[i], &actual[j]));
                }
                i += 1;
                j += 1;
            }
        }
    }
    missing.extend(expected[i..].iter());
    unexpected.extend(actual[j..].iter());
    (missing, unexpected, changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_sorted_and_stable() {
        let ids = ["b".to_string(), "a".to_string()];
        let bytes: [&[u8]; 2] = [b"xb".as_slice(), b"xa".as_slice()];
        let rows = build(ids.iter().zip(bytes.iter().copied()));
        assert_eq!(rows[0].id, "a");
        assert_eq!(rows[1].id, "b");
        assert_eq!(
            render(&rows),
            format!("a\t{}\nb\t{}\n", rows[0].hash, rows[1].hash)
        );
    }

    #[test]
    fn diff_reports_all_directions() {
        let mk = |id: &str, h: &str| Row {
            id: id.into(),
            hash: h.into(),
        };
        let expected = vec![mk("a", "1"), mk("b", "2"), mk("c", "3")];
        let actual = vec![mk("a", "9"), mk("d", "4")];
        let (missing, unexpected, changed) = diff(&expected, &actual);
        assert_eq!(
            missing.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["b", "c"]
        );
        assert_eq!(
            unexpected.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["d"]
        );
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].0.id, "a");
    }

    #[test]
    fn roundtrip_parse() {
        let text = "a\tdeadbeef\nb\tcafe\n";
        let rows = parse(text).expect("parses");
        assert_eq!(rows.len(), 2);
        assert_eq!(render(&rows), text);
        assert!(parse("nonsense").is_err());
    }
}
