//! Extraction of lint-rule cases from the pinned SDK's linter package:
//! every `assertDiagnostics` / `assertNoDiagnostics` / markup-variant call in
//! `pkg/linter/test/rules/*_test.dart` becomes one case whose setup mirrors
//! the official harness (`LintRuleTest`: the rule enabled via
//! `analysis_options.yaml`, the `add*PackageDep` mock getters, companion
//! `newFile` / `newPackage` files), and every BAD/GOOD doc snippet in
//! `pkg/linter/messages.yaml` (the `documentation` blocks and the
//! `**BAD:**` / `**GOOD:**` fences of `deprecatedDetails`) becomes one
//! `doc_bad_*` / `doc_good_*` case.
//!
//! Expected output always comes from the oracle; inline markers are kept out
//! of the case bytes: markup (`[!` / `!]`) is stripped before storage, the
//! same way the official `TestCode.parse` produces the analyzed text.

use crate::case::Case;
use crate::dart::{
    find_call, is_ident_start, match_paren, parse_args, parse_string_expression,
};
use crate::markers::camel_to_snake;
use crate::setup;
use crate::structure::{scan_declarations, Declaration};

/// The official test package layout (upstream `workspaceRootPath` etc.).
const WORKSPACE_ROOT: &str = "/home";
const TEST_ROOT: &str = "/home/test";
const TEST_LIB: &str = "/home/test/lib";

/// The mock packages every doc case declares (superset resolution: an
/// unused mock never changes the analysis result).
const DOC_PACKAGES: [&str; 4] = ["meta", "ffi", "flutter", "angular_meta"];

/// The lint test's assertion entry points, with the main file they analyze
/// and whether their source argument carries test-code markup.
/// `Named` takes the file-name argument first, `FromFile` resolves a path
/// argument against a preceding `newFile`.
#[derive(Clone, Copy, PartialEq)]
enum Sink {
    TestLib,
    TestDir,
    Bin,
    HookName,
    LibName,
    FromPath,
    FromUnits,
    Pubspec,
}

fn target(name: &str) -> Option<(Sink, bool)> {
    let sink = match name {
        "assertDiagnostics" | "assertNoDiagnostics" => Sink::TestLib,
        "assertDiagnosticsFromMarkup" => Sink::TestLib,
        "assertNoDiagnosticsInTestDir" | "assertDiagnosticsInTestDirFromMarkup" => Sink::TestDir,
        "assertDiagnosticsInBinFromMarkup" => Sink::Bin,
        "assertDiagnosticsInHookFromMarkup" | "assertNoDiagnosticsInHook" => Sink::HookName,
        "assertDiagnosticsInFileNameFromMarkup" | "assertNoDiagnosticsInFileName" => Sink::LibName,
        "assertDiagnosticsInFile" | "assertNoDiagnosticsInFile" => Sink::FromPath,
        "assertDiagnosticsInUnits" => Sink::FromUnits,
        "assertPubspecDiagnostics" | "assertNoPubspecDiagnostics" => Sink::Pubspec,
        _ => return None,
    };
    let strip = matches!(
        name,
        "assertDiagnosticsFromMarkup"
            | "assertDiagnosticsInTestDirFromMarkup"
            | "assertDiagnosticsInBinFromMarkup"
            | "assertDiagnosticsInHookFromMarkup"
            | "assertDiagnosticsInFileNameFromMarkup"
    );
    Some((sink, strip))
}

const TARGET_NAMES: [&str; 15] = [
    "assertDiagnostics",
    "assertNoDiagnostics",
    "assertDiagnosticsFromMarkup",
    "assertNoDiagnosticsInTestDir",
    "assertDiagnosticsInTestDirFromMarkup",
    "assertDiagnosticsInBinFromMarkup",
    "assertDiagnosticsInHookFromMarkup",
    "assertNoDiagnosticsInHook",
    "assertDiagnosticsInFileNameFromMarkup",
    "assertNoDiagnosticsInFileName",
    "assertDiagnosticsInFile",
    "assertNoDiagnosticsInFile",
    "assertDiagnosticsInUnits",
    "assertPubspecDiagnostics",
    "assertNoPubspecDiagnostics",
];

/// Setup calls whose effects are modeled.
const SETUP_CALLS: [&str; 5] = [
    "newFile",
    "newPubspecYamlFile",
    "newAnalysisOptionsYamlFile",
    "writeTestPackageConfig",
    "newPackage",
];

/// Fixture packages a test registers through `PackageConfigFileBuilder` that
/// the mocks tree backs (`use_test_throws_matchers_test.dart` registers
/// `test_api`; the mock carries the two files the official setUp writes).
/// Other registered names (`analyzer`, `nonAnalyzer`, ...) have no mock and
/// stay unmodeled.
const CONFIG_REGISTERED_MOCKS: [&str; 1] = ["test_api"];

/// A skipped call site and why.
#[derive(Debug, PartialEq, Eq)]
pub struct Skip {
    pub id: String,
    pub reason: String,
}

/// Everything extracted from one source.
#[derive(Debug, Default)]
pub struct LintExtraction {
    pub cases: Vec<Case>,
    pub skips: Vec<Skip>,
}

/// Extracts all cases from one lint rule test file. `rel` is the path
/// relative to `pkg/linter/test/rules`; the file stem is the rule name.
pub fn extract_file(rel: &str, content: &str) -> LintExtraction {
    let mut out = LintExtraction::default();
    let file_name = rel.rsplit('/').next().unwrap_or(rel);
    let Some(rule) = file_name.strip_suffix("_test.dart") else {
        return out;
    };
    for decl in scan_declarations(content) {
        let holds_targets = decl.members.iter().any(|m| {
            m.body.is_some_and(|(s, e)| {
                find_call(&content[s..e], 0, &|n| TARGET_NAMES.contains(&n)).is_some()
            })
        });
        if holds_targets {
            extract_declaration(rule, rel, content, &decl, &mut out);
        }
    }
    out
}

/// The resolved class environment: harness defaults + class getters + setUp.
struct Env {
    experiments: Vec<String>,
    language: Option<String>,
    packages: Vec<String>,
    aux_packages: Vec<String>,
    /// package name -> concrete root path, for `package:` section mapping.
    package_roots: Vec<(String, String)>,
    /// identifier -> concrete path, for `$identifier` path substitution.
    path_env: Vec<(String, String)>,
    /// identifier -> section path, for `newFile` results assigned to local
    /// variables (`var b = newFile(...)` then `assertInFile(b.path, ...)`).
    bindings: Vec<(String, String)>,
    /// Companion files (section path, content), in write order.
    files: Vec<(String, String)>,
}

fn base_path_env() -> Vec<(String, String)> {
    vec![
        ("workspaceRootPath".to_string(), WORKSPACE_ROOT.to_string()),
        ("testPackageRootPath".to_string(), TEST_ROOT.to_string()),
        ("testPackageLibPath".to_string(), TEST_LIB.to_string()),
        (
            "testPackagePubspecPath".to_string(),
            format!("{TEST_ROOT}/pubspec.yaml"),
        ),
        (
            "testFilePath".to_string(),
            format!("{TEST_LIB}/test.dart"),
        ),
    ]
}

fn class_env(content: &str, decl: &Declaration) -> Env {
    let mut language = if decl.mixins.iter().any(|m| m == "LanguageVersion219Mixin") {
        Some("2.19".to_string())
    } else {
        None
    };
    let mut experiments: Option<Vec<String>> = None;
    let mut packages: Vec<String> = Vec::new();
    let mut path_env = base_path_env();
    for member in &decl.members {
        let Some((start, end)) = member.initializer else {
            continue;
        };
        let expr = content[start..end].trim();
        match member.name.as_str() {
            "experiments" if experiments.is_none() => {
                experiments = setup::string_list(expr);
            }
            "addMetaPackageDep" | "addFlutterPackageDep" | "addFixnumPackageDep"
            | "addTestReflectiveLoaderPackageDep"
                if expr == "true" =>
            {
                let name = member
                    .name
                    .strip_prefix("add")
                    .and_then(|n| n.strip_suffix("PackageDep"))
                    .expect("getter shape")
                    .to_string();
                let name = match name.as_str() {
                    "TestReflectiveLoader" => "test_reflective_loader".to_string(),
                    other => other.to_lowercase(),
                };
                if !packages.contains(&name) {
                    packages.push(name);
                }
            }
            "testPackageLanguageVersion" => {
                let mut pos = 0usize;
                if let Some(d) = parse_string_expression(expr, &mut pos) {
                    if !d.interpolated {
                        language = Some(d.value);
                    }
                }
            }
            _ => {
                // Path getters (`String get libFile => '$testPackageLibPath/
                // file.dart';`) become named paths of the environment.
                if let Some(path) = resolve_path_text(expr, &path_env) {
                    if !path_env.iter().any(|(n, _)| n == &member.name) {
                        path_env.push((member.name.clone(), path));
                    }
                }
            }
        }
    }
    Env {
        // The official `AnalysisRuleTest.setUp` passes its `experiments`
        // getter, whose default is `experimentsForTests`; no lint test class
        // at this pin overrides it.
        experiments: experiments.unwrap_or_else(|| {
            setup::DEFAULT_EXPERIMENTS
                .iter()
                .map(|s| s.to_string())
                .collect()
        }),
        language,
        packages,
        aux_packages: Vec::new(),
        package_roots: Vec::new(),
        path_env,
        bindings: Vec::new(),
        files: Vec::new(),
    }
}

fn options_for(env: &Env, rule: &str) -> String {
    setup::analysis_options_content(&env.experiments, &[rule.to_string()], false, false, false, &[], &[])
}

fn extract_declaration(rule: &str, rel: &str, content: &str, decl: &Declaration, out: &mut LintExtraction) {
    let mut env = class_env(content, decl);
    if let Some(set_up) = decl.members.iter().find(|m| m.name == "setUp") {
        if let Some((start, end)) = set_up.body {
            interpret_calls(
                &content[start..end],
                &mut env,
                &mut |reason| out.skips.push(Skip {
                    id: format!("lints/{rule}/{}", decl.name),
                    reason,
                }),
            );
        }
    }
    for member in &decl.members {
        let Some((start, end)) = member.body else {
            continue;
        };
        if member.name == "setUp" {
            continue;
        }
        extract_method(rule, rel, &decl.name, &member.name, &content[start..end], &env, out);
    }
}

/// Resolves a path expression (one literal or an adjacent run) substituting a
/// leading `$identifier` / `${identifier}` from `paths`.
fn resolve_path_text(text: &str, paths: &[(String, String)]) -> Option<String> {
    let text = text.trim();
    let mut pos = 0usize;
    let first = parse_string_expression(text, &mut pos)?;
    let mut out = substitute(&first.value, paths)?;
    while pos < text.len() {
        let mut consumed = 0usize;
        let next = parse_string_expression(&text[pos..], &mut consumed)?;
        out.push_str(&substitute(&next.value, paths)?);
        if consumed == 0 {
            break;
        }
        pos += consumed;
    }
    Some(out)
}

fn substitute(value: &str, paths: &[(String, String)]) -> Option<String> {
    if !value.starts_with('$') {
        return Some(value.to_string());
    }
    let rest = &value[1..];
    let (name, tail) = if let Some(rest) = rest.strip_prefix('{') {
        let end = rest.find('}')?;
        (&rest[..end], &rest[end + 1..])
    } else {
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        (&rest[..end], &rest[end..])
    };
    let base = paths
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.clone())?;
    Some(format!("{base}{tail}"))
}

/// Maps a concrete absolute path to its case-file section path.
fn section_for(path: &str, env: &Env) -> Option<String> {
    if let Some(rest) = path.strip_prefix(TEST_LIB) {
        return Some(format!("lib{rest}"));
    }
    if let Some(rest) = path.strip_prefix(TEST_ROOT) {
        return Some(rest.trim_start_matches('/').to_string());
    }
    env.package_roots
        .iter()
        .find(|(_, root)| path.starts_with(&format!("{root}/")))
        .map(|(name, root)| format!("package:{name}/{}", &path[root.len() + 1..]))
}

/// Resolves a path-ish argument to its case-file section path. Handles
/// string-literal expressions, bare identifiers naming path getters or
/// `newFile` result variables (with an optional `.path` / `.parent.path`
/// member access).
fn resolve_section(expr: &str, env: &Env) -> Option<String> {
    let expr = expr.trim();
    let ident = expr
        .strip_suffix(".parent.path")
        .or_else(|| expr.strip_suffix(".path"))
        .unwrap_or(expr);
    if is_bare_ident(ident) {
        if let Some((_, section)) = env.bindings.iter().find(|(n, _)| n == ident) {
            return Some(section.clone());
        }
        if let Some((_, path)) = env.path_env.iter().find(|(n, _)| n == ident) {
            return section_for(path, env);
        }
        return None;
    }
    let path = resolve_path_text(expr, &env.path_env)?;
    section_for(&path, env)
}

fn is_bare_ident(expr: &str) -> bool {
    !expr.is_empty()
        && is_ident_start(expr.as_bytes()[0])
        && expr.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The identifier of a `var x = ...` / `x = ...` assignment immediately
/// ending at the call being interpreted (its prefix text is `prefix`).
fn assigned_ident(prefix: &str) -> Option<String> {
    let tail = prefix.trim_end();
    let start = tail
        .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .map(|i| i + 1)
        .unwrap_or(0);
    let ident = &tail[start..];
    if ident.is_empty() || !is_bare_ident(ident) {
        return None;
    }
    let before = tail[..start].trim_end();
    if before.ends_with("var") || before.ends_with('=') {
        Some(ident.to_string())
    } else {
        None
    }
}

fn literal_arg(arg: &str) -> Option<String> {
    let mut pos = 0usize;
    let d = parse_string_expression(arg, &mut pos)?;
    (pos == arg.len()).then(|| d.value)
}

/// Interprets the modeled setup calls of one body in offset order, updating
/// `env`. Unmodelable effects are counted via `note` and never abort.
fn interpret_calls(body: &str, env: &mut Env, note: &mut dyn FnMut(String)) {
    let mut pos = 0usize;
    while let Some((name, open, _)) =
        find_call(body, pos, &|n| SETUP_CALLS.contains(&n) || n == "add")
    {
        let Some(close) = match_paren(body, open) else {
            break;
        };
        let args = parse_args(&body[open + 1..close]);
        match name.as_str() {
            "newFile" | "newPubspecYamlFile" => {
                interpret_new_file(&body[..open], env, &args, note)
            }
            "newPackage" => interpret_new_package(body, open, close, env, note),
            "add" => interpret_config_add(&body[..open], env, &args),
            "writeTestPackageConfig" => {
                // The builders used here carry no mock packages beyond the
                // getters; a non-empty cascade cannot be modeled.
                if let Some(arg) = args.iter().find(|a| a.label.is_none()) {
                    if arg.expr.contains("..add(") {
                        note("setup_package_config".into());
                    }
                }
            }
            "newAnalysisOptionsYamlFile" => {
                // The harness itself writes options after setUp; a test-level
                // rewrite with the same shape is the default already.
                let custom = args
                    .iter()
                    .any(|a| a.expr.contains("analysisOptionsContent(") && !same_as_default(a));
                if custom {
                    note("options_custom".into());
                }
            }
            _ => {}
        }
        pos = close + 1;
    }
}

/// Whether an `analysisOptionsContent(...)` argument text only repeats the
/// harness default (`experiments` / `rules: [lintRule]` shapes).
fn same_as_default(arg: &crate::dart::Arg) -> bool {
    let expr = arg.expr.trim();
    let Some(inner) = expr.strip_prefix("analysisOptionsContent(") else {
        return false;
    };
    let Some(close) = match_paren(inner, 0) else {
        return false;
    };
    inner[close..].trim().is_empty()
        && parse_args(&inner[..close]).into_iter().all(|a| match a.label.as_deref() {
            Some("rules") => matches!(a.expr.trim(), "lintRule" | "rule" | "analysisRule"),
            Some("experiments") => a.expr.trim() == "experiments",
            _ => false,
        })
}

/// `<builder>.add(name: <pkg>, rootFolder: ...)` on a
/// `PackageConfigFileBuilder`: registers `<pkg>` when the mocks tree backs it
/// (the mock carries the files the official setUp writes alongside). The
/// companion `newFile` calls for those files stay unmodeled — their
/// `$fixtureRootPath` local has no modeled value, so they record their
/// advisory skips and produce no sections.
fn interpret_config_add(prefix: &str, env: &mut Env, args: &[crate::dart::Arg]) {
    if !prefix.contains("PackageConfigFileBuilder") {
        return;
    }
    let Some(name) = args
        .iter()
        .find(|a| a.label.as_deref() == Some("name"))
        .and_then(|a| literal_arg(&a.expr))
    else {
        return;
    };
    if !CONFIG_REGISTERED_MOCKS.contains(&name.as_str()) || env.aux_packages.contains(&name) {
        return;
    }
    env.aux_packages.push(name);
}

fn interpret_new_file(
    prefix: &str,
    env: &mut Env,
    args: &[crate::dart::Arg],
    note: &mut dyn FnMut(String),
) {
    let (Some(path_arg), Some(content_arg)) = (args.first(), args.get(1)) else {
        return;
    };
    let Some(section) = resolve_section(&path_arg.expr, env) else {
        note("companion_path".into());
        return;
    };
    let Some(content) = literal_arg(&content_arg.expr) else {
        note("companion_interpolated".into());
        return;
    };
    let content = if !content.is_empty() && !content.ends_with('\n') {
        format!("{content}\n")
    } else {
        content
    };
    if let Some(ident) = assigned_ident(prefix) {
        if !env.bindings.iter().any(|(n, _)| n == &ident) {
            env.bindings.push((ident, section.clone()));
        }
    }
    env.files.push((section, content));
}

/// `newPackage('name').addFile(local, content).addFile(...)` cascades.
fn interpret_new_package(
    body: &str,
    open: usize,
    close: usize,
    env: &mut Env,
    note: &mut dyn FnMut(String),
) {
    let args = parse_args(&body[open + 1..close]);
    let Some(name_arg) = args.first() else {
        return;
    };
    let Some(name) = literal_arg(&name_arg.expr) else {
        note("companion_path".into());
        return;
    };
    let root = format!("/package/{name}");
    if !env.aux_packages.contains(&name) {
        env.aux_packages.push(name.clone());
    }
    if !env.package_roots.iter().any(|(n, _)| n == &name) {
        env.package_roots.push((name.clone(), root));
    }
    let mut rest = &body[close + 1..];
    while let Some(idx) = rest.find(".addFile(") {
        let open = idx + ".addFile(".len() - 1;
        let Some(close) = match_paren(rest, open) else {
            break;
        };
        let args = parse_args(&rest[open + 1..close]);
        let content = args.get(1).and_then(|a| literal_arg(&a.expr));
        match (args.first(), content) {
            (Some(local), Some(content)) => {
                if let Some(local) = literal_arg(&local.expr) {
                    env.files
                        .push((format!("package:{name}/{local}"), content));
                } else {
                    note("companion_path".into());
                }
            }
            _ => note("companion_interpolated".into()),
        }
        rest = &rest[close + 1..];
    }
}

fn extract_method(
    rule: &str,
    rel: &str,
    class_name: &str,
    method_name: &str,
    body: &str,
    class_env: &Env,
    out: &mut LintExtraction,
) {
    let mut index = 0usize;
    let mut p = 0usize;
    while let Some((name, open, _)) = find_call(body, p, &|n| TARGET_NAMES.contains(&n)) {
        p = open + 1;
        let id = format!("lints/{rule}/{class_name}/{method_name}#{index}");
        index += 1;
        let Some((sink, strip)) = target(&name) else {
            out.skips.push(Skip {
                id,
                reason: "unmodeled_assert".into(),
            });
            continue;
        };
        let Some(close) = match_paren(body, open) else {
            continue;
        };
        let args = parse_args(&body[open + 1..close]);

        let mut env = Env {
            experiments: class_env.experiments.clone(),
            language: class_env.language.clone(),
            packages: class_env.packages.clone(),
            aux_packages: class_env.aux_packages.clone(),
            package_roots: class_env.package_roots.clone(),
            path_env: class_env.path_env.clone(),
            bindings: class_env.bindings.clone(),
            files: class_env.files.clone(),
        };
        interpret_calls(&body[..open], &mut env, &mut |reason| {
            out.skips.push(Skip {
                id: id.clone(),
                reason,
            });
        });

        let advisory = |out: &mut LintExtraction, id: &str, reason: &str| {
            out.skips.push(Skip {
                id: id.to_string(),
                reason: reason.to_string(),
            });
        };
        let source_of = |arg: &crate::dart::Arg| -> Option<String> {
            let text = literal_arg(&arg.expr)?;
            Some(if strip { strip_markup(&text) } else { text })
        };
        let main_and_files = |out: &mut LintExtraction,
                              id: &str|
         -> Option<(String, String, Vec<(String, String)>)> {
            // Returns (main path, main content when pre-written, other files).
            match sink {
                Sink::TestLib => Some(("lib/test.dart".to_string(), String::new(), Vec::new())),
                Sink::TestDir => Some(("test/test.dart".to_string(), String::new(), Vec::new())),
                Sink::Bin => Some(("bin/bin.dart".to_string(), String::new(), Vec::new())),
                Sink::Pubspec => {
                    advisory(out, id, "pubspec_case");
                    Some(("pubspec.yaml".to_string(), String::new(), Vec::new()))
                }
                Sink::HookName | Sink::LibName => {
                    let name = args.first().and_then(|a| literal_arg(&a.expr))?;
                    let prefix = if sink == Sink::HookName { "hook" } else { "lib" };
                    Some((format!("{prefix}/{name}"), String::new(), Vec::new()))
                }
                Sink::FromPath => {
                    let path_expr = args.first()?.expr.as_str();
                    let section = resolve_section(path_expr, &env)?;
                    let at = env.files.iter().position(|(s, _)| s == &section)?;
                    let mut files = env.files.clone();
                    let (path, content) = files.remove(at);
                    Some((path, content, files))
                }
                Sink::FromUnits => {
                    advisory(out, id, "assert_in_units");
                    let mut paths = strings_in(&args[0].expr);
                    let first = paths.remove(0);
                    let path = resolve_path_text(&format!("'{first}'"), &env.path_env)?;
                    let section = section_for(&path, &env)?;
                    let at = env.files.iter().position(|(s, _)| s == &section)?;
                    let mut files = env.files.clone();
                    let (path, content) = files.remove(at);
                    Some((path, content, files))
                }
            }
        };

        let Some((main_path, prewritten, others)) = main_and_files(out, &id) else {
            out.skips.push(Skip {
                id,
                reason: "unresolvable_target".into(),
            });
            continue;
        };
        let source_arg = match sink {
            Sink::HookName | Sink::LibName => args.get(1),
            Sink::FromPath | Sink::FromUnits => None,
            _ => args.first(),
        };
        let mut files: Vec<(String, String)> = Vec::new();
        if let Some(arg) = source_arg {
            let Some(source) = source_of(arg) else {
                out.skips.push(Skip {
                    id,
                    reason: "interpolated_source".into(),
                });
                continue;
            };
            files.push((main_path, source));
        } else {
            // In-file asserts: the target file content was written earlier.
            files.push((main_path, prewritten));
        }
        files.extend(others);
        for (path, content) in &env.files {
            if !files.iter().any(|(p, _)| p == path) {
                files.push((path.clone(), content.clone()));
            }
        }
        let options = options_for(&env, rule);
        out.cases.push(Case {
            id,
            source: format!("pkg/linter/test/rules/{rel}#{class_name}.{method_name}"),
            language: env.language,
            packages: env.packages,
            aux_packages: env.aux_packages,
            options: Some(options),
            ignores: Vec::new(),
            files,
        });
    }
}

/// Collects the string-literal texts of an expression in source order (used
/// for the unit-list argument of `assertDiagnosticsInUnits`).
fn strings_in(expr: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < expr.len() {
        let rest = &expr[pos..];
        match rest.find(|c: char| c == '\'' || c == '"') {
            Some(at) => {
                pos += at;
                let mut consumed = 0usize;
                if let Some(d) = parse_string_expression(&expr[pos..], &mut consumed) {
                    if consumed > 0 {
                        out.push(d.value);
                        pos += consumed;
                        continue;
                    }
                }
                pos += 1;
            }
            None => break,
        }
    }
    out
}

/// Strips the official test-code markup exactly like the official
/// `TestCode.parse` scanner: a character-wise scan over the raw text that
/// does not skip string literals or comments, removing `^` (position), `[!` /
/// `!]` (range), `/*N*/` (position), `/*[N*/` / `/*N]*/` (range) and `/**/`
/// (zero-width) markers.
pub fn strip_markup(marked: &str) -> String {
    let mut out = String::with_capacity(marked.len());
    let mut i = 0usize;
    while i < marked.len() {
        let rest = &marked[i..];
        if rest.starts_with('^') {
            i += 1;
            continue;
        }
        if let Some(consumed) = scan_numbered(rest, "/*", "*/") {
            i += consumed;
            continue;
        }
        if rest.starts_with("[!") {
            i += 2;
            continue;
        }
        if rest.starts_with("!]") {
            i += 2;
            continue;
        }
        if let Some(consumed) = scan_numbered(rest, "/*[", "*/") {
            i += consumed;
            continue;
        }
        if let Some(consumed) = scan_numbered(rest, "/*", "]*/") {
            i += consumed;
            continue;
        }
        if rest.starts_with("/**/") {
            i += 4;
            continue;
        }
        let ch = rest.chars().next().expect("non-empty rest");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Matches `open digits close` at the head of `rest`, returning the consumed
/// byte length (`/*N*/`, `/*[N*/`, `/*N]*/` markers).
fn scan_numbered(rest: &str, open: &str, close: &str) -> Option<usize> {
    let after_open = rest.strip_prefix(open)?;
    let digits = after_open
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(after_open.len());
    if digits == 0 {
        return None;
    }
    let after_digits = &after_open[digits..];
    after_digits.starts_with(close).then(|| open.len() + digits + close.len())
}

// ---------------------------------------------------------------------------
// Doc snippets from pkg/linter/messages.yaml.
// ---------------------------------------------------------------------------

/// One `LinterLintCode` entry with the two documentation blocks.
struct RuleEntry {
    key: String,
    shared_name: Option<String>,
    removed_in: Option<String>,
    documentation: Option<String>,
    deprecated_details: Option<String>,
}

/// Reads the entries of the linter messages.yaml shape (a single indent-0
/// class header, entry keys at indent 2, fields at indent 4).
fn parse_rule_entries(yaml: &str) -> Vec<RuleEntry> {
    let lines: Vec<&str> = yaml.lines().collect();
    let mut entries = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        let indent = line.len() - line.trim_start_matches(' ').len();
        let trimmed = line.trim_start();
        if indent != 2 || trimmed.starts_with('#') || !trimmed.ends_with(':') {
            i += 1;
            continue;
        }
        let key = trimmed.trim_end_matches(':').to_string();
        let mut entry = RuleEntry {
            key,
            shared_name: None,
            removed_in: None,
            documentation: None,
            deprecated_details: None,
        };
        i += 1;
        while i < lines.len() {
            let field = lines[i];
            let fi = field.len() - field.trim_start_matches(' ').len();
            if field.trim().is_empty() {
                i += 1;
                continue;
            }
            if fi <= 2 {
                break;
            }
            if fi == 4 {
                let (name, value) = field.trim_start().split_once(':').unwrap_or((field.trim_start(), ""));
                let value = value.trim();
                match name {
                    "sharedName" => entry.shared_name = Some(unquote(value)),
                    "removedIn" => entry.removed_in = Some(unquote(value)),
                    "documentation" | "deprecatedDetails" if value == "|-" || value == "|" || value == "|+" => {
                        let (block, next) = read_block(&lines, i + 1, 4);
                        if name == "documentation" {
                            entry.documentation = Some(block);
                        } else {
                            entry.deprecated_details = Some(block);
                        }
                        i = next;
                        continue;
                    }
                    _ => {}
                }
            }
            i += 1;
        }
        entries.push(entry);
    }
    entries
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        value[1..value.len() - 1].replace("\\\"", "\"").replace("\\\\", "\\")
    } else if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        value[1..value.len() - 1].replace("''", "'")
    } else {
        value.to_string()
    }
}

/// Reads a block scalar's lines (indent > `key_indent`), stripping the
/// block's own indentation; trailing empty lines are dropped.
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
        let indent = line.len() - line.trim_start_matches(' ').len();
        if indent <= key_indent {
            break;
        }
        let bi = *block_indent.get_or_insert(indent);
        collected.push(line[indent.min(bi)..].to_string());
        i += 1;
    }
    while collected.last().is_some_and(|l| l.is_empty()) {
        collected.pop();
    }
    (collected.join("\n"), i)
}

/// One documentation dart snippet with its directives and auxiliary files.
struct Snippet {
    text: String,
    aux: Vec<(String, String)>,
    experiments: Vec<String>,
    ignores: Vec<String>,
    language_version: Option<String>,
}

/// One documentation dart block as written (before aux accumulation).
struct DocBlock {
    text: String,
    uri: Option<String>,
    experiments: Vec<String>,
    ignores: Vec<String>,
    language_version: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum DocSection {
    Bad,
    Good,
}

/// Splits a `documentation` block into (section, block) pairs: `#### Example`
/// dart blocks are the BAD snippets, `#### Common fixes` dart blocks the GOOD
/// ones; `%`-directives inside a block carry its environment.
fn parse_documentation_blocks(documentation: &str) -> Vec<(DocSection, DocBlock)> {
    let mut parts = Vec::new();
    let mut section: Option<DocSection> = None;
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
            let file_type = line[3..].trim().to_string();
            let mut text: Vec<String> = Vec::new();
            let mut block = DocBlock {
                text: String::new(),
                uri: None,
                experiments: Vec::new(),
                ignores: Vec::new(),
                language_version: None,
            };
            loop {
                match lines.next() {
                    None | Some("```") => break,
                    Some(l) if l.starts_with('%') => {
                        if let Some(v) = l.strip_prefix("%language=") {
                            block.language_version = Some(v.to_string());
                        } else if let Some(v) = l.strip_prefix("%uri=\"") {
                            block.uri = v.strip_suffix('"').map(str::to_string);
                        } else if let Some(v) = l.strip_prefix("%ignore=") {
                            block.ignores = v.split(',').map(|s| s.trim().to_string()).collect();
                        } else if let Some(v) = l.strip_prefix("%experiments=") {
                            block.experiments =
                                v.split(',').map(|s| s.trim().to_string()).collect();
                        }
                    }
                    Some(l) => text.push(l.to_string()),
                }
            }
            if file_type == "dart" {
                block.text = text.join("\n");
                parts.push((current, block));
            } else if !file_type.is_empty() {
                // Non-dart blocks without a %uri are dropped; with one they
                // are auxiliary files handled below.
                if block.uri.is_some() {
                    block.text = text.join("\n");
                    parts.push((current, block));
                }
            }
            continue;
        }
        if line.starts_with('#') && !line.starts_with("#####") {
            match line.trim() {
                "#### Example" | "#### Examples" => section = Some(DocSection::Bad),
                "#### Common fixes" => section = Some(DocSection::Good),
                _ => {}
            }
        }
    }
    parts
}

/// Extracts the snippets of one section: `%uri=` blocks accumulate as
/// auxiliary files into the next dart block, and the accumulator resets
/// after each dart block.
fn snippets_of(parts: &[(DocSection, DocBlock)], want: DocSection) -> Vec<Snippet> {
    let mut out = Vec::new();
    let mut aux: Vec<(String, String)> = Vec::new();
    for (section, block) in parts {
        if *section != want {
            continue;
        }
        match &block.uri {
            Some(uri) => aux.push((uri.clone(), block.text.clone())),
            None => {
                out.push(Snippet {
                    text: block.text.clone(),
                    aux: std::mem::take(&mut aux),
                    experiments: block.experiments.clone(),
                    ignores: block.ignores.clone(),
                    language_version: block.language_version.clone(),
                });
            }
        }
    }
    out
}

fn skip_block(lines: &mut std::iter::Peekable<std::str::Lines<'_>>) {
    while let Some(l) = lines.next() {
        if l.starts_with("```") {
            return;
        }
    }
}

/// Splits a `deprecatedDetails` block into BAD/GOOD dart fences: the text
/// after a `**BAD:**` or `**GOOD:**` label owns the following fences.
fn parse_details(details: &str) -> Vec<(DocSection, DocBlock)> {
    let mut parts = Vec::new();
    let mut section: Option<DocSection> = None;
    let mut lines = details.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("**BAD:**") {
            section = Some(DocSection::Bad);
            continue;
        }
        if trimmed.starts_with("**GOOD:**") {
            section = Some(DocSection::Good);
            continue;
        }
        if line.starts_with("```") {
            let Some(current) = section else {
                skip_block(&mut lines);
                continue;
            };
            if line[3..].trim() != "dart" {
                skip_block(&mut lines);
                continue;
            }
            let mut text: Vec<String> = Vec::new();
            loop {
                match lines.next() {
                    None | Some("```") => break,
                    Some(l) => text.push(l.to_string()),
                }
            }
            parts.push((
                current,
                DocBlock {
                    text: text.join("\n"),
                    uri: None,
                    experiments: Vec::new(),
                    ignores: Vec::new(),
                    language_version: None,
                },
            ));
        }
    }
    parts
}

/// Extracts all doc cases from the linter messages.yaml text. BAD snippets
/// report the rule, GOOD snippets report nothing for it; both carry the
/// harness options with the rule enabled.
pub fn extract_docs(yaml: &str, origin: &str) -> LintExtraction {
    let mut out = LintExtraction::default();
    // Several messages.yaml entries share one sharedName (fix variants such as
    // `alwaysDeclareReturnTypesOfFunctions` and `...OfMethods`); they document
    // a single rule, so their snippets merge and number under that rule.
    let mut rules: Vec<(String, Vec<(String, Snippet)>, Vec<(String, Snippet)>)> = Vec::new();
    for entry in parse_rule_entries(yaml) {
        let rule = entry
            .shared_name
            .as_deref()
            .map(camel_to_snake)
            .unwrap_or_else(|| camel_to_snake(&entry.key));
        if entry.removed_in.is_some() {
            out.skips.push(Skip {
                id: format!("lints/{rule}/"),
                reason: "removed".into(),
            });
            continue;
        }
        let mut bads: Vec<Snippet> = Vec::new();
        let mut goods: Vec<Snippet> = Vec::new();
        if let Some(documentation) = &entry.documentation {
            let blocks = parse_documentation_blocks(documentation);
            bads = snippets_of(&blocks, DocSection::Bad);
            goods = snippets_of(&blocks, DocSection::Good);
        }
        if let Some(details) = &entry.deprecated_details {
            for (section, block) in parse_details(details) {
                let snippet = Snippet {
                    text: block.text,
                    aux: Vec::new(),
                    experiments: block.experiments,
                    ignores: block.ignores,
                    language_version: block.language_version,
                };
                match section {
                    DocSection::Bad => bads.push(snippet),
                    DocSection::Good => goods.push(snippet),
                }
            }
        }
        let slot = rules
            .iter_mut()
            .find(|(name, _, _)| name == &rule)
            .map(|(_, b, g)| (b, g));
        match slot {
            Some((b, g)) => {
                b.extend(bads.into_iter().map(|s| (entry.key.clone(), s)));
                g.extend(goods.into_iter().map(|s| (entry.key.clone(), s)));
            }
            None => rules.push((
                rule.clone(),
                bads.into_iter().map(|s| (entry.key.clone(), s)).collect(),
                goods.into_iter().map(|s| (entry.key.clone(), s)).collect(),
            )),
        }
    }
    for (rule, bads, goods) in &rules {
        for (kind, snippets) in [("doc_bad", bads), ("doc_good", goods)] {
            for (i, (key, snippet)) in snippets.iter().enumerate() {
                let id = format!("lints/{rule}/{kind}_{i}");
                let mut files: Vec<(String, String)> = vec![(
                    "lib/test.dart".to_string(),
                    ensure_newline(&strip_markup(&snippet.text)),
                )];
                for (uri, text) in &snippet.aux {
                    let path = match uri.strip_prefix("package:") {
                        Some(rest) => format!("package:{rest}"),
                        None => uri.clone(),
                    };
                    files.push((path, ensure_newline(text)));
                }
                out.cases.push(Case {
                    id,
                    source: format!("{origin}#{key}"),
                    language: snippet.language_version.clone(),
                    packages: DOC_PACKAGES.iter().map(|s| s.to_string()).collect(),
                    aux_packages: Vec::new(),
                    options: Some(setup::analysis_options_content(
                        &snippet.experiments,
                        &[rule.to_string()],
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
        }
    }
    out
}

fn ensure_newline(text: &str) -> String {
    let mut text = text.to_string();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The upstream `use_test_throws_matchers_test.dart` shape: a
    /// `PackageConfigFileBuilder` registration plus companion files whose
    /// path local the extractor does not model.
    const TEST_API_FILE: &str = r#"
@reflectiveTest
class UseTestThrowsMatchersTest extends LintRuleTest {
  @override
  String get lintRule => LintNames.use_test_throws_matchers;

  @override
  void setUp() {
    super.setUp();
    var testApiPath = '$workspaceRootPath/test_api';
    var packageConfigBuilder = PackageConfigFileBuilder();
    packageConfigBuilder.add(
      name: 'test_api',
      rootFolder: getFolder(testApiPath),
    );
    writeTestPackageConfig2(config: packageConfigBuilder);
    newFile('$testApiPath/lib/src/frontend/expect.dart', r'''
void expect(dynamic actual, dynamic matcher) {}

Never fail(String message) {}
''');
    newFile('$testApiPath/lib/test_api.dart', r'''
export 'src/frontend/expect.dart';
''');
  }

  test_failInTry() async {
    await assertDiagnosticsFromMarkup(r'''
import 'package:test_api/test_api.dart';

void f() {
  try {
    f();
    [!fail('fail');!]
  } catch (e) {}
}
''');
  }
}
"#;

    #[test]
    fn config_registered_test_api_backs_the_case() {
        let out = extract_file("use_test_throws_matchers_test.dart", TEST_API_FILE);
        assert_eq!(out.cases.len(), 1, "{:?}", out.skips);
        let case = &out.cases[0];
        assert_eq!(case.id, "lints/use_test_throws_matchers/UseTestThrowsMatchersTest/test_failInTry#0");
        assert_eq!(case.aux_packages, vec!["test_api"]);
        // The fixture files come from the mocks tree; the unmodeled companion
        // paths record advisory skips and produce no sections.
        assert_eq!(case.files.len(), 1);
        assert_eq!(case.files[0].0, "lib/test.dart");
        assert!(
            out.skips
                .iter()
                .all(|s| s.reason == "companion_path"),
            "{:?}",
            out.skips
        );
    }

    #[test]
    fn markup_stripping_matches_official_scanner() {
        assert_eq!(strip_markup("var [!x!] = 1;"), "var x = 1;");
        assert_eq!(strip_markup("f(^)"), "f()");
        assert_eq!(strip_markup("/*[0*/a/*0]*/ b /*1*/ c /*1]*/"), "a b  c ");
        // The zero-width marker vanishes; `/**/` never reads as a position.
        assert_eq!(strip_markup("[/**/!"), "[!");
        assert_eq!(strip_markup("!/**/]"), "!]");
        // Numbers above 9 and multi-digit.
        assert_eq!(strip_markup("/*[12*/xy/*12]*/"), "xy");
        // The scan does not respect strings (official behavior).
        assert_eq!(strip_markup("var s = '^';"), "var s = '';") ;
    }

    #[test]
    fn extracts_cases_with_getters_and_companions() {
        let src = r#"
import '../rule_test_support.dart';

@reflectiveTest
class AvoidPrintTest extends LintRuleTest with LanguageVersion219Mixin {
  @override
  bool get addFlutterPackageDep => true;

  @override
  String get lintRule => LintNames.avoid_print;

  test_directCall() async {
    await assertDiagnosticsFromMarkup(r'''
void f() {
  [!print!]('ha');
}
''');
  }

  test_companion() async {
    await newFile('$testPackageLibPath/a.dart', 'class A {}');
    await assertNoDiagnostics(r'''
import 'a.dart';
void f() {}
''');
  }
}
"#;
        let out = extract_file("avoid_print_test.dart", src);
        assert_eq!(out.cases.len(), 2, "{:?}", out.skips);
        let first = &out.cases[0];
        assert_eq!(first.id, "lints/avoid_print/AvoidPrintTest/test_directCall#0");
        assert_eq!(first.language.as_deref(), Some("2.19"));
        assert_eq!(first.packages, vec!["flutter"]);
        assert_eq!(first.files[0].0, "lib/test.dart");
        // The raw-string literal keeps its leading newline, as analyzed.
        assert_eq!(first.files[0].1, "\nvoid f() {\n  print('ha');\n}\n");
        assert!(first
            .options
            .as_ref()
            .expect("options")
            .contains("    - avoid_print\n"));

        let second = &out.cases[1];
        assert_eq!(second.files.len(), 2);
        assert_eq!(second.files[1].0, "lib/a.dart");
    }

    #[test]
    fn default_experiments_match_official_harness() {
        let src = r#"
class T extends LintRuleTest {
  String get lintRule => 'avoid_print';

  test_x() async {
    await assertNoDiagnostics('var x = 1;');
  }
}
"#;
        let out = extract_file("avoid_print_test.dart", src);
        assert_eq!(out.cases.len(), 1, "{:?}", out.skips);
        let options = out.cases[0].options.as_ref().expect("options");
        for flag in setup::DEFAULT_EXPERIMENTS {
            assert!(options.contains(&format!("    - {flag}\n")), "{options}");
        }
        assert!(options.contains("    - avoid_print\n"));
    }

    #[test]
    fn setup_and_in_file_asserts() {
        let src = r#"
class T extends LintRuleTest {
  String get lintRule => 'avoid_catches_without_on_clauses';

  @override
  void setUp() {
    super.setUp();
    newPackage('protobuf').addFile('lib/src/pb.dart', 'class ProtobufEnum {}');
  }

  test_inFile() async {
    await newFile('$testPackageLibPath/a.dart', r'''
void f() {}
''');
    await assertNoDiagnosticsInFile('$testPackageLibPath/a.dart');
  }

  test_interpolated() async {
    var code = 'var x = 1;';
    await assertNoDiagnostics(code);
  }
}
"#;
        let out = extract_file("some_rule_test.dart", src);
        assert_eq!(out.cases.len(), 1, "{:?}", out.skips);
        let case = &out.cases[0];
        assert_eq!(case.files.len(), 2);
        assert_eq!(case.files[0].0, "lib/a.dart");
        assert_eq!(case.files[1].0, "package:protobuf/lib/src/pb.dart");
        assert_eq!(case.aux_packages, vec!["protobuf"]);
        assert!(out
            .skips
            .iter()
            .any(|s| s.reason == "interpolated_source" && s.id.ends_with("test_interpolated#0")));
    }

    #[test]
    fn test_dir_and_hook_sinks() {
        let src = r#"
class T extends LintRuleTest {
  String get lintRule => 'avoid_print';

  test_bin() async {
    await assertDiagnosticsInBinFromMarkup(r'''
void main() { [!print!]('x'); }
''');
  }

  test_named() async {
    await assertNoDiagnosticsInHook('build.dart', 'void f() {}');
  }
}
"#;
        let out = extract_file("avoid_print_test.dart", src);
        assert_eq!(out.cases.len(), 2, "{:?}", out.skips);
        assert_eq!(out.cases[0].files[0].0, "bin/bin.dart");
        assert_eq!(out.cases[1].files[0].0, "hook/build.dart");
    }

    const YAML: &str = "LinterLintCode:\n  alwaysDeclareReturnTypesOfFunctions:\n    type: lint\n    sharedName: alwaysDeclareReturnTypes\n    problemMessage: \"x\"\n    documentation: |-\n      #### Example\n\n      ```dart\n      [!f!]() {}\n      ```\n\n      #### Common fixes\n\n      ```dart\n      void f() {}\n      ```\n    deprecatedDetails: |-\n      **DO** declare return types.\n\n      **BAD:**\n      ```dart\n      main() { }\n      ```\n\n      **GOOD:**\n      ```dart\n      int main() { }\n      ```\n";

    #[test]
    fn doc_bad_and_good_from_both_blocks() {
        let out = extract_docs(YAML, "pkg/linter/messages.yaml");
        assert_eq!(out.cases.len(), 4, "{:?}", out.skips);
        // All BAD snippets first (documentation, then deprecatedDetails),
        // then all GOOD snippets in the same order.
        assert_eq!(out.cases[0].id, "lints/always_declare_return_types/doc_bad_0");
        assert_eq!(out.cases[0].files[0].1, "f() {}\n");
        assert_eq!(out.cases[1].id, "lints/always_declare_return_types/doc_bad_1");
        assert_eq!(out.cases[1].files[0].1, "main() { }\n");
        assert_eq!(out.cases[2].id, "lints/always_declare_return_types/doc_good_0");
        assert_eq!(out.cases[3].id, "lints/always_declare_return_types/doc_good_1");
        for case in &out.cases {
            assert!(case
                .options
                .as_ref()
                .expect("options")
                .contains("    - always_declare_return_types\n"));
        }
    }

    #[test]
    fn doc_directives_flow_into_case() {
        let yaml = "LinterLintCode:\n  someRule:\n    documentation: |-\n      #### Example\n\n      ```dart\n      %language=2.12\n      %ignore=unused_import\n      var [!x!] = 1;\n      ```\n";
        let out = extract_docs(yaml, "m");
        assert_eq!(out.cases.len(), 1);
        assert_eq!(out.cases[0].language.as_deref(), Some("2.12"));
        assert_eq!(out.cases[0].ignores, vec!["unused_import"]);
    }

    #[test]
    fn removed_rule_skips() {
        let yaml = "LinterLintCode:\n  oldRule:\n    removedIn: \"3.0\"\n    documentation: |-\n      #### Example\n      ```dart\n      x\n      ```\n";
        let out = extract_docs(yaml, "m");
        assert!(out.cases.is_empty());
        assert_eq!(out.skips[0].reason, "removed");
    }

    #[test]
    fn featureless_entry_key_becomes_rule_name() {
        let yaml = "LinterLintCode:\n  camelCaseRule:\n    deprecatedDetails: |-\n      **BAD:**\n      ```dart\n      x\n      ```\n";
        let out = extract_docs(yaml, "m");
        assert_eq!(out.cases[0].id, "lints/camel_case_rule/doc_bad_0");
    }

    #[test]
    fn strings_in_collects_literals() {
        assert_eq!(
            strings_in("[('a.dart', []), ('b.dart', [])]"),
            vec!["a.dart", "b.dart"]
        );
    }

    #[test]
    fn doc_uri_aux_files_accumulate() {
        let yaml = "LinterLintCode:\n  someRule:\n    documentation: |-\n      #### Example\n\n      ```dart\n      %uri=\"analysis_options.yaml\"\n      linter:\n        rules:\n          - some_rule\n      ```\n\n      ```dart\n      var [!x!] = 1;\n      ```\n";
        let out = extract_docs(yaml, "m");
        assert_eq!(out.cases.len(), 1);
        assert_eq!(out.cases[0].files.len(), 2);
        assert_eq!(out.cases[0].files[1].0, "analysis_options.yaml");
    }
}
