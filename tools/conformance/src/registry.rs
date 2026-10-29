//! The three generated registries that define the status-board row set:
//! diagnostic codes (`analyzer-codes.tsv`), lint rules
//! (`rule_meta.rs` consts), and language features
//! (`experimental_features.yaml`). Parsed read-only; never hand-edited.

use std::collections::BTreeMap;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub enum Kind {
    Diagnostic,
    Lint,
    Feature,
}

impl Kind {
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Diagnostic => "diagnostic",
            Kind::Lint => "lint",
            Kind::Feature => "feature",
        }
    }
}

/// Every registry entry, keyed `(kind, id)` in sorted order.
pub struct Registry {
    pub entries: BTreeMap<(Kind, String), Entry>,
}

#[derive(Debug, Default, Clone)]
pub struct Entry {
    pub cases: u32,
    pub pass: u32,
    pub fail: u32,
    pub crash: u32,
    pub spec_drift: u32,
}

impl Registry {
    pub fn load(
        codes_tsv: &str,
        rule_meta_rs: &str,
        features_yaml: &str,
    ) -> Result<Registry, String> {
        let mut entries = BTreeMap::new();
        for id in parse_codes(codes_tsv) {
            entries.insert((Kind::Diagnostic, id), Entry::default());
        }
        for id in parse_rule_names(rule_meta_rs) {
            entries.insert((Kind::Lint, id), Entry::default());
        }
        for id in parse_features(features_yaml)? {
            entries.insert((Kind::Feature, id), Entry::default());
        }
        Ok(Registry { entries })
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Column 2 of the non-comment data rows is the lowercase code id.
fn parse_codes(text: &str) -> impl Iterator<Item = String> + '_ {
    text.lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split('\t').nth(1).map(str::to_string))
}

/// `RuleMeta { name: "...", ... }` row literals; the string-literal shape
/// cannot collide with the struct definition.
fn parse_rule_names(text: &str) -> impl Iterator<Item = String> + '_ {
    text.lines().filter_map(|line| {
        let start = line.find("name: \"")? + "name: \"".len();
        let rest = &line[start..];
        let end = rest.find('"')?;
        Some(rest[..end].to_string())
    })
}

/// Feature flags are the 2-space-indented keys under the top-level
/// `features:` mapping; entry attributes are indented deeper.
fn parse_features(text: &str) -> Result<Vec<String>, String> {
    let mut flags = Vec::new();
    let mut in_features = false;
    for (no, line) in text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if !in_features {
            in_features = line.trim_end() == "features:";
            continue;
        }
        if !line.starts_with(' ') {
            break; // next top-level key: the features section ended
        }
        let rest = line.trim_start();
        if line.len() - rest.len() == 2 {
            // Flag lines may carry a trailing comment (`  flag: # note`).
            let rest = rest
                .split_once(" #")
                .map_or(rest, |(prefix, _)| prefix)
                .trim_end();
            match rest.strip_suffix(':') {
                Some(flag) if !flag.is_empty() => flags.push(flag.to_string()),
                _ => {
                    return Err(format!(
                        "experimental_features.yaml line {}: expected `  <flag>:`",
                        no + 1
                    ));
                }
            }
        }
    }
    if !in_features {
        return Err("experimental_features.yaml: no `features:` section".to_string());
    }
    Ok(flags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_tsv_skips_comments_and_takes_column_two() {
        let text = "# header\nFfiCode\tabi_specific_integer_invalid\tABI_SPECIFIC_INTEGER_INVALID\tCOMPILE_TIME_ERROR\tERROR\ttrue\tmsg\tfix\n\nCompileTimeErrorCode\tabstract_field_initializer\tX\tY\tZ\ttrue\tm\tf\n";
        let ids: Vec<String> = parse_codes(text).collect();
        assert_eq!(
            ids,
            vec!["abi_specific_integer_invalid", "abstract_field_initializer"]
        );
    }

    #[test]
    fn rule_meta_names_parse_without_struct_noise() {
        let text = "pub struct RuleMeta {\n    pub name: &'static str,\n}\nRuleMeta { name: \"always_declare_return_types\", group: Group::Standalone },\nRuleMeta { name: \"avoid_print\", group: Group::Standalone },\n";
        let names: Vec<String> = parse_rule_names(text).collect();
        assert_eq!(names, vec!["always_declare_return_types", "avoid_print"]);
    }

    #[test]
    fn features_yaml_two_space_keys_only() {
        let text = "current-version: '3.13.0'\n\nfeatures:\n  variance:\n    help: \"Sound variance\"\n\n  enhanced-parts:\n    experimentalReleaseVersion: '3.6.0'\n    channels: [ \"main\" ]\n";
        assert_eq!(
            parse_features(text).expect("parses"),
            vec!["variance", "enhanced-parts"]
        );
        let commented = "features:\n  inference-update-3: # see the language repo\n    help: h\n";
        assert_eq!(
            parse_features(commented).expect("parses"),
            vec!["inference-update-3"]
        );
        assert!(parse_features("no features here").is_err());
    }

    #[test]
    fn load_merges_all_three_kinds_sorted() {
        let reg = Registry::load(
            "C\tzz_code\tZ\tY\tX\ttrue\tm\tf\n",
            "RuleMeta { name: \"aa_lint\", group: Group::Standalone },\n",
            "features:\n  mm_feature:\n    help: h\n",
        )
        .expect("loads");
        let keys: Vec<_> = reg
            .entries
            .keys()
            .map(|(k, id)| (k.name(), id.as_str()))
            .collect();
        assert_eq!(
            keys,
            vec![
                ("diagnostic", "zz_code"),
                ("lint", "aa_lint"),
                ("feature", "mm_feature")
            ]
        );
    }
}
