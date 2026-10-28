//! Doc-example extraction from `messages.yaml`: each `#### Example` dart
//! block becomes a case expected to report its code at the `[!..!]` range;
//! each `#### Common fixes` dart block becomes a case expected not to report
//! it. The parsing rules are the port of the official validator
//! (`verify_diagnostics_test.dart` `DocumentationValidator` and
//! `error_code_documentation_info.dart`), including its `unverifiedDocs`
//! skip list, `%language=` / `%uri=` / `%experiments=` / `%ignore=`
//! directives, and the auxiliary-file accumulation semantics.

use crate::case::Case;
use crate::messages_yaml::{self, MessageEntry};
use crate::setup;

/// The official `unverifiedDocs` skip list (verbatim; these docs cannot be
/// verified mechanically and are recorded as skipped, not extracted).
pub const UNVERIFIED_DOCS: &[&str] = &[
    "analysis_option_deprecated",
    "asset_does_not_exist",
    "asset_directory_does_not_exist",
    "asset_field_not_list",
    "asset_missing_path",
    "asset_not_string",
    "asset_not_string_or_map",
    "asset_path_not_string",
    "dependencies_field_not_map",
    "deprecated_field",
    "deprecated_lint",
    "duplicate_rule",
    "flutter_field_not_map",
    "included_file_parse_error",
    "included_file_warning",
    "include_file_not_found",
    "incompatible_lint",
    "invalid_dependency",
    "invalid_option",
    "invalid_platforms_field",
    "invalid_section_format",
    "missing_name",
    "missing_dependency",
    "name_not_string",
    "package_names",
    "path_does_not_exist",
    "path_not_posix",
    "path_pubspec_does_not_exist",
    "parse_error",
    "platform_value_disallowed",
    "plugins_in_inner_options",
    "recursive_include_file",
    "removed_lint",
    "removed_lint_use",
    "secure_pubspec_urls",
    "sort_pub_dependencies",
    "unknown_platform",
    "undefined_lint",
    "unnecessary_dev_dependency",
    "unrecognized_error_code",
    "unsupported_option_with_legal_value",
    "unsupported_value",
    "workspace_field_not_list",
    "workspace_value_not_string",
    "workspace_value_not_subdirectory",
    "ambiguous_import",
    "extension_type_implements_itself",
    "extension_type_representation_depends_on_itself",
    "recursive_compile_time_constant",
    "recursive_constructor_redirect",
    "recursive_interface_inheritance",
    "text_direction_code_point_in_comment",
    "text_direction_code_point_in_literal",
    "top_level_cycle",
    "type_alias_cannot_reference_itself",
    "type_parameter_supertype_of_its_bound",
    "const_constructor_param_type_mismatch",
    "invalid_implementation_override",
    "invalid_uri",
    "yield_each_in_non_generator",
    "deprecated_colon_for_default_value",
    "deprecated_member_use",
    "avoid_relative_lib_imports",
    "avoid_web_libraries_in_flutter",
    "depend_on_referenced_packages",
    "file_names",
    "prefer_inlined_adds_single",
    "library_annotations",
    "package_prefixed_library_names",
    "prefer_relative_imports",
    "public_member_api_docs",
    "recursive_getters",
    "extension_declares_instance_field",
    "deprecated_extends_function",
];

/// One skipped doc snippet and why.
#[derive(Debug, PartialEq, Eq)]
pub struct DocSkip {
    pub id: String,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct DocExtraction {
    pub cases: Vec<Case>,
    pub skips: Vec<DocSkip>,
    /// Codes whose entry has documentation but no `#### Example` block (the
    /// fixes snippets are still extracted).
    pub no_example_codes: usize,
    /// Examples carrying more than one `[!..!]` range (the official
    /// validator reports them; extraction keeps the first range's semantics).
    pub multi_range: usize,
}

/// One documentation code block with its directives.
struct Block {
    text: String,
    file_type: String,
    uri: Option<String>,
    experiments: Vec<String>,
    ignores: Vec<String>,
    language_version: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Examples,
    CommonFixes,
}

/// The sections a documentation line maps to (the official
/// `computeCurrentBlockSection`).
fn section_of(header: &str) -> Option<Section> {
    match header {
        "#### Example" | "#### Examples" => Some(Section::Examples),
        "#### Common fixes" => Some(Section::CommonFixes),
        _ => None,
    }
}

/// Parses the documentation text into (section, block) pairs (the official
/// `_ErrorCodeDocumentationParser`, including the `TODO` stop and the
/// `%`-directive handling).
fn parse_documentation(documentation: &str) -> Vec<(Section, Block)> {
    let mut parts = Vec::new();
    let mut section: Option<Section> = None;
    let mut lines = documentation.lines().peekable();
    while let Some(line) = lines.next() {
        if line.starts_with("TODO") {
            break;
        }
        if line.starts_with("```") {
            let Some(current) = section else {
                skip_block(&mut lines);
                continue;
            };
            let file_type = line[3..].to_string();
            let mut text: Vec<String> = Vec::new();
            let mut uri = None;
            let mut experiments = Vec::new();
            let mut ignores = Vec::new();
            let mut language_version = None;
            let mut broken = false;
            loop {
                match lines.next() {
                    None => {
                        broken = true; // unterminated code block
                        break;
                    }
                    Some(l) if l.starts_with("```") => break,
                    Some(l) if l.starts_with('%') => {
                        if let Some(v) = l.strip_prefix("%language=") {
                            language_version = Some(v.to_string());
                        } else if let Some(v) = l.strip_prefix("%uri=\"") {
                            uri = v.strip_suffix('"').map(str::to_string);
                        } else if let Some(v) = l.strip_prefix("%ignore=") {
                            ignores = v.split(',').map(|s| s.trim().to_string()).collect();
                        } else if let Some(v) = l.strip_prefix("%experiments=") {
                            experiments = v.split(',').map(|s| s.trim().to_string()).collect();
                        }
                    }
                    Some(l) => text.push(l.to_string()),
                }
            }
            let _ = broken;
            if !file_type.is_empty() {
                parts.push((
                    current,
                    Block {
                        text: text.join("\n"),
                        file_type,
                        uri,
                        experiments,
                        ignores,
                        language_version,
                    },
                ));
            }
            continue;
        }
        if line.starts_with('#') && !line.starts_with("#####") {
            if let Some(next) = section_of(line) {
                section = Some(next);
            }
        }
    }
    parts
}

fn skip_block(lines: &mut std::iter::Peekable<std::str::Lines<'_>>) {
    while let Some(l) = lines.next() {
        if l.starts_with("```") {
            return;
        }
    }
}

/// Extracts the snippets of one section per the official `_extractSnippets`:
/// `%uri=` blocks accumulate as auxiliary files into the next dart block,
/// and the accumulator resets after each dart block.
fn snippets_of(parts: &[(Section, Block)], want: Section) -> Vec<Snippet> {
    let mut out = Vec::new();
    let mut aux: Vec<(String, String)> = Vec::new();
    for (section, block) in parts {
        if *section != want {
            continue;
        }
        match &block.uri {
            Some(uri) => aux.push((uri.clone(), block.text.clone())),
            None => {
                if block.file_type == "dart" {
                    out.push(Snippet {
                        text: block.text.clone(),
                        aux: std::mem::take(&mut aux),
                        experiments: block.experiments.clone(),
                        ignores: block.ignores.clone(),
                        language_version: block.language_version.clone(),
                    });
                } else {
                    aux.clear();
                }
            }
        }
    }
    out
}

struct Snippet {
    text: String,
    aux: Vec<(String, String)>,
    experiments: Vec<String>,
    ignores: Vec<String>,
    language_version: Option<String>,
}

/// Extracts all doc-example cases from one parsed messages.yaml.
pub fn extract_messages(entries: &[MessageEntry], origin: &str) -> DocExtraction {
    let mut out = DocExtraction::default();
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for entry in entries {
        let Some(documentation) = &entry.documentation else {
            continue;
        };
        if entry.removed_in.is_some() {
            out.skips.push(DocSkip {
                id: format!("diagnostics/{}/", variable_name(entry)),
                reason: "removed".into(),
            });
            continue;
        }
        if UNVERIFIED_DOCS.contains(&variable_name(entry).as_str()) {
            out.skips.push(DocSkip {
                id: format!("diagnostics/{}/", variable_name(entry)),
                reason: "unverified_doc".into(),
            });
            continue;
        }
        let code = code_name(entry);
        let parts = parse_documentation(documentation);
        let examples = snippets_of(&parts, Section::Examples);
        let fixes = snippets_of(&parts, Section::CommonFixes);
        if examples.is_empty() {
            out.no_example_codes += 1;
        }
        let first_aux = examples.first().map(|s| s.aux.clone()).unwrap_or_default();
        for (i, snippet) in examples.iter().enumerate() {
            push_snippet(
                &mut out,
                &mut seen_ids,
                &code,
                &format!("doc_{i}"),
                snippet,
                origin,
                entry,
            );
        }
        for (i, snippet) in fixes.iter().enumerate() {
            let mut owned = Snippet {
                text: snippet.text.clone(),
                aux: snippet.aux.clone(),
                experiments: snippet.experiments.clone(),
                ignores: Vec::new(),
                language_version: snippet.language_version.clone(),
            };
            for (uri, text) in &first_aux {
                if !owned.aux.iter().any(|(u, _)| u == uri) {
                    owned.aux.push((uri.clone(), text.clone()));
                }
            }
            push_snippet(
                &mut out,
                &mut seen_ids,
                &code,
                &format!("fix_{i}"),
                &owned,
                origin,
                entry,
            );
        }
    }
    out
}

fn variable_name(entry: &MessageEntry) -> String {
    snake_case(&entry.analyzer_code)
}

fn code_name(entry: &MessageEntry) -> String {
    match &entry.shared_name {
        Some(shared) => snake_case(shared),
        None => snake_case(&entry.analyzer_code),
    }
}

/// The official `toSnakeCase`: word starts at every capital; existing
/// underscores are preserved.
fn snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn push_snippet(
    out: &mut DocExtraction,
    seen: &mut std::collections::HashSet<String>,
    code: &str,
    name: &str,
    snippet: &Snippet,
    origin: &str,
    entry: &MessageEntry,
) {
    let mut id = format!("diagnostics/{code}/{name}");
    if seen.contains(&id) {
        // Two entries sharing a code (rare sharedName collisions).
        id = format!("diagnostics/{code}/{}_{name}", variable_name(entry));
    }
    seen.insert(id.clone());
    let mut second_range = false;
    {
        let mut from = 0usize;
        let mut first = true;
        while let Some(at) = snippet.text[from..].find("[!") {
            if !first {
                second_range = true;
                break;
            }
            first = false;
            from += at + 2;
        }
    }
    if second_range {
        out.multi_range += 1;
    }
    // The case content keeps the raw `[!..!]` marker as the human-readable
    // spec (the oracle's output on the stripped form is the expectation).
    let mut files = vec![("lib/test.dart".to_string(), ensure_newline(&snippet.text))];
    for (uri, text) in &snippet.aux {
        let path = match uri.strip_prefix("package:") {
            Some(rest) => format!("package:{rest}"),
            None => uri.clone(),
        };
        files.push((path, ensure_newline(text)));
    }
    let mut aux_packages: Vec<String> = files
        .iter()
        .filter_map(|(p, _)| p.strip_prefix("package:"))
        .filter_map(|p| p.split('/').next())
        .map(str::to_string)
        .collect();
    aux_packages.sort();
    aux_packages.dedup();
    out.cases.push(Case {
        id,
        source: format!("{origin}#{}", entry.analyzer_code),
        language: snippet.language_version.clone(),
        packages: setup::MOCK_PACKAGES.iter().map(|s| s.to_string()).collect(),
        aux_packages,
        options: Some(setup::analysis_options_content(
            &snippet.experiments,
            &[],
            false,
            false,
            false,
            &[],
            &[],
        )),
        ignores: snippet.ignores.clone(),
        files,
    });
}

/// Case file bytes end with exactly one newline.
fn ensure_newline(text: &str) -> String {
    let mut text = text.to_string();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// Convenience: parses and extracts in one call.
pub fn extract_messages_text(yaml: &str, origin: &str) -> DocExtraction {
    extract_messages(&messages_yaml::parse(yaml), origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    const YAML: &str = "CompileTimeErrorCode:\n  fooBar:\n    type: compileTimeError\n    parameters: none\n    problemMessage: \"x\"\n    hasPublishedDocs: true\n    documentation: |-\n      #### Description\n\n      Bad.\n\n      #### Example\n\n      ```dart\n      var [!x!] = 1;\n      ```\n\n      #### Common fixes\n\n      ```dart\n      var x = 1;\n      ```\n";

    #[test]
    fn extracts_example_and_fix() {
        let out = extract_messages_text(YAML, "messages.yaml");
        assert_eq!(out.cases.len(), 2);
        assert_eq!(out.cases[0].id, "diagnostics/foo_bar/doc_0");
        // The raw `[!..!]` marker stays as the human-readable spec.
        assert_eq!(out.cases[0].files[0].1, "var [!x!] = 1;\n");
        assert_eq!(out.cases[1].id, "diagnostics/foo_bar/fix_0");
        assert_eq!(
            out.cases[0].packages,
            vec!["meta", "ffi", "flutter", "angular_meta"]
        );
    }

    #[test]
    fn uri_auxiliary_files_become_package_files() {
        let yaml = "E:\n  documentation: |-\n    #### Example\n\n    ```yaml\n    %uri=\"shared.yaml\"\n    x: 1\n    ```\n\n    ```dart\n    var [!x!] = 1;\n    ```\n\n    ```dart\n    %uri=\"package:p/p.dart\"\n    var p = 1;\n    ```\n\n    #### Common fixes\n\n    ```dart\n    var x = 1;\n    ```\n";
        let out = extract_messages_text(yaml, "m");
        // example 0 has aux shared.yaml; the dart example follows.
        let example = out
            .cases
            .iter()
            .find(|c| c.id.ends_with("doc_0"))
            .expect("example");
        assert_eq!(example.files.len(), 2);
        assert_eq!(example.files[1].0, "shared.yaml");
        // fix inherits the first example's aux files.
        let fix = out
            .cases
            .iter()
            .find(|c| c.id.ends_with("fix_0"))
            .expect("fix");
        assert!(fix.files.iter().any(|(p, _)| p == "shared.yaml"));
    }

    #[test]
    fn removed_and_unverified_skip() {
        let yaml = "A:\n  removedIn: \"3.0\"\n  documentation: |-\n    #### Example\n    ```dart\n    x\n    ```\nB:\n  documentation: |-\n    #### Example\n    ```dart\n    x\n    ```\n";
        // B's analyzerCode snake must be in the skip list to test it.
        let mut entries = messages_yaml::parse(yaml);
        entries[1].analyzer_code = "parseError".into();
        let out = extract_messages(&entries, "m");
        assert_eq!(out.skips.len(), 2);
        assert_eq!(out.skips[0].reason, "removed");
        assert_eq!(out.skips[1].reason, "unverified_doc");
    }

    #[test]
    fn example_without_range_is_extracted_as_no_error() {
        let yaml =
            "E:\n  documentation: |-\n    #### Example\n\n    ```dart\n    var x = 1;\n    ```\n";
        let out = extract_messages_text(yaml, "m");
        assert_eq!(out.cases.len(), 1);
        assert_eq!(out.cases[0].files[0].1, "var x = 1;\n");
    }

    #[test]
    fn shared_name_gives_code_directory() {
        let yaml = "CompileTimeErrorCode:\n  fooBarOld:\n    sharedName: fooBar\n    documentation: |-\n      #### Example\n\n      ```dart\n      var [!x!] = 1;\n      ```\n  fooBarNew:\n    sharedName: fooBar\n    documentation: |-\n      #### Example\n\n      ```dart\n      var [!y!] = 2;\n      ```\n";
        let out = extract_messages_text(yaml, "m");
        assert_ne!(out.cases[0].id, out.cases[1].id);
        assert!(out.cases[0].id.starts_with("diagnostics/foo_bar/"));
        assert!(
            out.cases[1].id.starts_with("diagnostics/foo_bar/"),
            "{}",
            out.cases[1].id
        );
    }
}
