//! Interpretation of the official test-harness setup APIs into case
//! directives: the `analysisOptionsContent` YAML builder and the
//! `writeTestPackageConfig` package wiring (upstream
//! `pkg/analyzer_testing/lib/src/utilities/utilities.dart` and
//! `context_collection_resolution.dart` semantics).

/// The experiments the official harness enables for pub-package tests by
/// default (`experimentsForTests`, in order).
pub const DEFAULT_EXPERIMENTS: [&str; 5] = [
    "augmentations",
    "enhanced-parts",
    "static-extensions",
    "this-promotion",
    "variance",
];

/// Mock packages materialized under `conformance/mocks/` (the
/// `writeTestPackageConfig` flag names).
pub const MOCK_PACKAGES: [&str; 4] = ["meta", "ffi", "flutter", "angular_meta"];

/// Renders the exact YAML the official `analysisOptionsContent` produces.
pub fn analysis_options_content(
    experiments: &[String],
    rules: &[String],
    strict_casts: bool,
    strict_inference: bool,
    strict_raw_types: bool,
    unignorable_names: &[String],
    errors: &[(String, String)],
) -> String {
    let mut out = String::new();
    let language = strict_casts || strict_inference || strict_raw_types;
    // `propagateLinterExceptions` defaults to true at every setup site we
    // translate, so the `analyzer:` section is always present.
    out.push_str("analyzer:\n");
    if !experiments.is_empty() {
        out.push_str("  enable-experiment:\n");
        for e in experiments {
            out.push_str(&format!("    - {e}\n"));
        }
    }
    if !errors.is_empty() {
        out.push_str("  errors:\n");
        for (k, v) in errors {
            out.push_str(&format!("    {k}: {v}\n"));
        }
    }
    if language {
        out.push_str("  language:\n");
        if strict_casts {
            out.push_str("    strict-casts: true\n");
        }
        if strict_inference {
            out.push_str("    strict-inference: true\n");
        }
        if strict_raw_types {
            out.push_str("    strict-raw-types: true\n");
        }
    }
    if !unignorable_names.is_empty() {
        out.push_str("  cannot-ignore:\n");
        for n in unignorable_names {
            out.push_str(&format!("    - {n}\n"));
        }
    }
    out.push_str("  optional-checks:\n");
    out.push_str("    propagate-linter-exceptions: true\n");
    if !rules.is_empty() {
        out.push_str("linter:\n  rules:\n");
        for r in rules {
            out.push_str(&format!("    - {r}\n"));
        }
    }
    out
}

/// The default options every pub-package test starts with.
pub fn default_options(experiments_override: Option<&[String]>) -> String {
    let experiments: Vec<String> = match experiments_override {
        Some(list) => list.to_vec(),
        None => DEFAULT_EXPERIMENTS.iter().map(|s| s.to_string()).collect(),
    };
    analysis_options_content(&experiments, &[], false, false, false, &[], &[])
}

/// Parses the named arguments of an `analysisOptionsContent(...)` call into
/// the same YAML, or `None` when an argument cannot be modeled.
pub fn analysis_options_from_call(args: &[crate::dart::Arg]) -> Option<String> {
    let mut experiments: Vec<String> = Vec::new();
    let mut rules: Vec<String> = Vec::new();
    let mut strict_casts = false;
    let mut strict_inference = false;
    let mut strict_raw_types = false;
    let mut unignorable: Vec<String> = Vec::new();
    let mut errors: Vec<(String, String)> = Vec::new();
    for arg in args {
        let crate::dart::Arg { label, expr } = arg;
        match label.as_deref() {
            Some("experiments") => experiments = string_list(expr)?,
            Some("rules") => rules = string_list(expr)?,
            Some("unignorableNames") => unignorable = string_list(expr)?,
            Some("errors") => errors = error_map(expr)?,
            Some("strictCasts") => strict_casts = expr == "true",
            Some("strictInference") => strict_inference = expr == "true",
            Some("strictRawTypes") => strict_raw_types = expr == "true",
            Some("propagateLinterExceptions") => {}
            Some("includes") => {
                if string_list(expr)?.iter().any(|i| !i.is_empty()) {
                    return None;
                }
            }
            Some("legacyPlugins") => {
                if !string_list(expr)?.is_empty() {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(analysis_options_content(
        &experiments,
        &rules,
        strict_casts,
        strict_inference,
        strict_raw_types,
        &unignorable,
        &errors,
    ))
}

/// Parses a string-list argument; the bare `experiments` identifier stands
/// for the official `experimentsForTests` getter the tests pass through.
pub fn string_list(expr: &str) -> Option<Vec<String>> {
    let expr = expr.trim();
    if expr == "experiments" {
        return Some(DEFAULT_EXPERIMENTS.iter().map(|s| s.to_string()).collect());
    }
    if expr == "const []" || expr == "[]" {
        return Some(Vec::new());
    }
    let expr = expr.strip_prefix("const ").unwrap_or(expr).trim_start();
    let inner = expr.strip_prefix('[')?.strip_suffix(']')?;
    split_strings(inner)
}

fn error_map(expr: &str) -> Option<Vec<(String, String)>> {
    let expr = expr.trim();
    if expr == "const {}" || expr == "{}" {
        return Some(Vec::new());
    }
    let inner = expr.strip_prefix("const {")?.strip_suffix('}')?;
    let inner = inner.strip_prefix('{').unwrap_or(inner);
    let mut out = Vec::new();
    for pair in crate::dart::split_args(inner) {
        let (k, v) = pair.split_once(':')?;
        out.push((unquote(k.trim())?, v.trim().to_string()));
    }
    Some(out)
}

fn unquote(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'\'' && *bytes.last()? == b'\'' {
        return Some(s[1..s.len() - 1].to_string());
    }
    None
}

/// Splits the inside of a string list literal into its items.
pub fn split_strings(inner: &str) -> Option<Vec<String>> {
    let mut items = Vec::new();
    for part in crate::dart::split_args(inner) {
        let mut pos = 0usize;
        let d = crate::dart::parse_string_expression(&part, &mut pos)?;
        if d.interpolated || pos != part.len() {
            return None;
        }
        items.push(d.value);
    }
    Some(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_options_match_official_shape() {
        assert_eq!(
            default_options(None),
            "analyzer:\n  enable-experiment:\n    - augmentations\n    - enhanced-parts\n    - static-extensions\n    - this-promotion\n    - variance\n  optional-checks:\n    propagate-linter-exceptions: true\n"
        );
    }

    #[test]
    fn strict_casts_block() {
        let yaml = analysis_options_content(&[], &[], true, false, false, &[], &[]);
        assert_eq!(
            yaml,
            "analyzer:\n  language:\n    strict-casts: true\n  optional-checks:\n    propagate-linter-exceptions: true\n"
        );
    }

    #[test]
    fn rules_section_last() {
        let yaml = analysis_options_content(
            &["enhanced-parts".into()],
            &["avoid_print".into()],
            false,
            true,
            false,
            &[],
            &[],
        );
        assert!(yaml.ends_with("linter:\n  rules:\n    - avoid_print\n"));
        assert!(yaml.contains("    - enhanced-parts\n"));
        assert!(yaml.contains("    strict-inference: true\n"));
    }

    #[test]
    fn parses_analysis_options_call() {
        use crate::dart::Arg;
        let args = vec![
            Arg {
                label: None,
                expr: "x".into(),
            }, // noise filtered below via label None
        ];
        // positional argument cannot be modeled
        assert!(analysis_options_from_call(&args).is_none());
        let args = vec![
            Arg {
                label: Some("experiments".into()),
                expr: "['enhanced-parts']".into(),
            },
            Arg {
                label: Some("strictCasts".into()),
                expr: "true".into(),
            },
        ];
        let yaml = analysis_options_from_call(&args).expect("models");
        assert!(yaml.contains("    - enhanced-parts\n"));
        assert!(yaml.contains("    strict-casts: true\n"));
    }

    #[test]
    fn empty_list_literals() {
        assert_eq!(string_list("const []"), Some(vec![]));
        assert_eq!(
            string_list("['a', 'b']"),
            Some(vec!["a".into(), "b".into()])
        );
        assert_eq!(string_list("[]"), Some(vec![]));
    }
}
