//! Extraction of cases from analyzer package test files: every
//! `resolveTestCodeWithDiagnostics` / `resolveTestCode` /
//! `assertTestCodeWithStrictCastsDiagnostics` call becomes one case whose
//! setup (options, packages, language version, companion files) is derived
//! from the class-level setup and the calls preceding the target in its
//! method body, following the official harness semantics.

use crate::case::Case;
use crate::dart::{find_call, match_paren, parse_args, parse_string_expression, Arg};
use crate::setup;
use crate::structure::{scan_declarations, Declaration};

/// The test-entry methods whose first argument carries the code snippet.
const TARGET_CALLS: [&str; 3] = [
    "resolveTestCodeWithDiagnostics",
    "resolveTestCode",
    "assertTestCodeWithStrictCastsDiagnostics",
];

/// Setup calls whose effects are modeled.
const SETUP_CALLS: [&str; 7] = [
    "writeTestPackageConfig",
    "writeTestPackageConfigWithMeta",
    "writeTestPackageAnalysisOptionsFile",
    "newAnalysisOptionsYamlFile",
    "newFile",
    "newPubspecYamlFile",
    "addTestFile",
];

/// Mixins that pin the test package's language version.
const LANGUAGE_VERSION_MIXINS: [(&str, &str); 4] = [
    ("WithLanguage219Mixin", "2.19"),
    ("WithoutConstructorTearoffsMixin", "2.14"),
    ("WithoutEnhancedEnumsMixin", "2.16"),
    ("WithoutPrivateNamedParametersMixin", "3.9"),
];

/// The official test package layout (upstream `workspaceRootPath` etc.).
const WORKSPACE_ROOT: &str = "/home";
const TEST_ROOT: &str = "/home/test";
const TEST_LIB: &str = "/home/test/lib";

/// A skipped call site and why.
#[derive(Debug, PartialEq, Eq)]
pub struct Skip {
    pub id: String,
    pub reason: String,
}

/// Everything extracted from one test file.
#[derive(Debug, Default)]
pub struct FileExtraction {
    pub cases: Vec<Case>,
    pub skips: Vec<Skip>,
}

/// Extracts all cases from one test file. `rel` is the path relative to the
/// analyzer test root.
pub fn extract_file(rel: &str, content: &str) -> FileExtraction {
    let mut out = FileExtraction::default();
    let file_name = rel.rsplit('/').next().unwrap_or(rel);
    let stem = file_name.strip_suffix("_test.dart").unwrap_or(file_name);
    let stem = stem.strip_suffix(".dart").unwrap_or(stem);
    let code_prefix = if rel.starts_with("src/diagnostics/") {
        format!("diagnostics/{stem}")
    } else {
        let path = rel.strip_suffix("_test.dart").unwrap_or(rel);
        format!("diagnostics/_misc/{}", path.strip_suffix(".dart").unwrap_or(path))
    };
    for decl in scan_declarations(content) {
        let holds_targets = decl
            .members
            .iter()
            .any(|m| m.body.is_some_and(|(s, e)| has_target_call(&content[s..e])));
        if holds_targets {
            extract_declaration(&code_prefix, rel, content, &decl, &mut out);
        }
    }
    out
}

fn has_target_call(body: &str) -> bool {
    find_call(body, 0, &|n| TARGET_CALLS.contains(&n)).is_some()
}

/// The resolved class environment: defaults + mixins + the class setUp.
struct Env {
    options: String,
    language: Option<String>,
    packages: Vec<String>,
    aux_packages: Vec<String>,
    /// package name -> concrete root path, for companion resolution.
    package_roots: Vec<(String, String)>,
    /// identifier -> concrete path, for `$identifier` path substitution.
    path_env: Vec<(String, String)>,
    experiments: Vec<String>,
    /// File sections (path, content). Paths starting with `package:` belong
    /// to an auxiliary package.
    files: Vec<(String, String)>,
}

fn base_env(content: &str, decl: &Declaration) -> Env {
    let mut language: Option<String> = None;
    for mixin in &decl.mixins {
        if let Some((_, version)) = LANGUAGE_VERSION_MIXINS.iter().find(|(n, _)| n == mixin) {
            language = Some(version.to_string());
        }
    }
    let mut experiments: Option<Vec<String>> = None;
    for member in &decl.members {
        if member.name == "experiments" && experiments.is_none() {
            if let Some((start, end)) = member.initializer {
                if let Some(items) = setup::string_list(content[start..end].trim()) {
                    experiments = Some(items);
                }
            }
        }
    }
    let experiments =
        experiments.unwrap_or_else(|| setup::DEFAULT_EXPERIMENTS.iter().map(|s| s.to_string()).collect());
    let mut env = Env {
        options: setup::default_options(Some(&experiments)),
        language,
        packages: Vec::new(),
        aux_packages: Vec::new(),
        package_roots: Vec::new(),
        path_env: base_path_env(content, decl),
        experiments,
        files: Vec::new(),
    };
    if decl.mixins.iter().any(|m| m == "WithStrictCastsMixin") {
        env.options = strict_casts_options(&env.experiments);
    }
    env
}

fn strict_casts_options(experiments: &[String]) -> String {
    setup::analysis_options_content(experiments, &[], true, false, false, &[], &[])
}

fn base_path_env(content: &str, decl: &Declaration) -> Vec<(String, String)> {
    let mut env = vec![
        ("workspaceRootPath".to_string(), WORKSPACE_ROOT.to_string()),
        ("testPackageRootPath".to_string(), TEST_ROOT.to_string()),
        ("testPackageLibPath".to_string(), TEST_LIB.to_string()),
    ];
    for member in &decl.members {
        if let Some((start, end)) = member.initializer {
            if let Some(path) = resolve_path_text(&content[start..end], &env) {
                env.push((member.name.clone(), path));
            }
        }
    }
    env
}

/// Resolves a path expression (one literal or an adjacent run) to a concrete
/// string, substituting a leading `$identifier` / `${identifier}` from `env`.
fn resolve_path_text(text: &str, env: &[(String, String)]) -> Option<String> {
    let text = text.trim();
    let mut pos = 0usize;
    let first = parse_string_expression(text, &mut pos)?;
    let mut out = substitute(&first.value, env)?;
    while pos < text.len() {
        let mut consumed = 0usize;
        let next = parse_string_expression(&text[pos..], &mut consumed)?;
        out.push_str(&substitute(&next.value, env)?);
        if consumed == 0 {
            break;
        }
        pos += consumed;
    }
    Some(out)
}

/// Resolves a package-root expression: a bare identifier naming a class
/// getter (already in `path_env`), or a string-literal expression.
fn resolve_root_expr(expr: &str, env: &Env) -> Option<String> {
    let expr = expr.trim();
    let ident = !expr.is_empty()
        && crate::dart::is_ident_start(expr.as_bytes()[0])
        && expr.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ident {
        return env.path_env.iter().find(|(n, _)| n == expr).map(|(_, v)| v.clone());
    }
    resolve_path_text(expr, &env.path_env)
}

fn substitute(value: &str, env: &[(String, String)]) -> Option<String> {
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
    let base = env.iter().find(|(n, _)| n == name)?.1.clone();
    Some(format!("{base}{tail}"))
}

fn extract_declaration(
    code_prefix: &str,
    rel: &str,
    content: &str,
    decl: &Declaration,
    out: &mut FileExtraction,
) {
    let mut env = base_env(content, decl);
    if let Some(set_up) = decl.members.iter().find(|m| m.name == "setUp") {
        if let Some((start, end)) = set_up.body {
            interpret_calls(
                &content[start..end],
                &mut env,
                &mut |reason| {
                    out.skips.push(Skip {
                        id: format!("{code_prefix}/{}", decl.name),
                        reason,
                    })
                },
            );
        }
    }
    for member in &decl.members {
        let Some((start, end)) = member.body else { continue };
        if member.name == "setUp" {
            continue;
        }
        extract_method(
            code_prefix,
            rel,
            &decl.name,
            &member.name,
            &content[start..end],
            &env,
            out,
        );
    }
}

/// Interprets the modeled calls of one body in offset order, updating `env`.
/// Unmodelable setup calls fire `skip` with a `setup_*` reason.
fn interpret_calls(body: &str, env: &mut Env, skip: &mut dyn FnMut(String)) {
    let mut pos = 0usize;
    #[cfg(test)]
    eprintln!("DBG interpret_calls on: {:?}", &body[..body.len().min(120)]);
    while let Some((name, open, _)) =
        find_call(body, pos, &|n| TARGET_CALLS.contains(&n) || SETUP_CALLS.contains(&n))
    {
        #[cfg(test)]
        eprintln!("DBG call: {name}");
        let Some(close) = match_paren(body, open) else { break };
        let args = parse_args(&body[open + 1..close]);
        #[cfg(test)]
        if name == "writeTestPackageConfig" {
            eprintln!("DBG args: {:?}", args);
        }
        match name.as_str() {
            "writeTestPackageConfigWithMeta" => add_package(env, "meta"),
            "writeTestPackageConfig" => interpret_config_call(env, &args),
            "writeTestPackageAnalysisOptionsFile" => {
                interpret_options_call(env, args.first().map(|a| a.expr.as_str()), skip);
            }
            "newAnalysisOptionsYamlFile" => {
                let root = args.first().map(|a| a.expr.as_str()).unwrap_or("");
                let content_expr = args.get(1).map(|a| a.expr.as_str()).unwrap_or("");
                if root == "testPackageRootPath" {
                    interpret_options_call(env, Some(content_expr), skip);
                } else {
                    skip("setup_newAnalysisOptionsYamlFile".into());
                }
            }
            "newFile" => interpret_new_file(env, &args, skip),
            "newPubspecYamlFile" => {
                let root = args.first().map(|a| a.expr.as_str()).unwrap_or("");
                let content_arg = args.get(1);
                if root == "testPackageRootPath" {
                    match content_arg.and_then(literal_arg) {
                        Some(text) => env.files.push(("pubspec.yaml".into(), text)),
                        None => skip("companion_interpolated".into()),
                    }
                } else {
                    skip("setup_newPubspecYamlFile".into());
                }
            }
            "addTestFile" => {}
            _ => {}
        }
        pos = close + 1;
    }
}

fn add_package(env: &mut Env, name: &str) {
    if !env.packages.iter().any(|p| p == name) {
        env.packages.push(name.to_string());
    }
}

fn interpret_config_call(env: &mut Env, args: &[Arg]) {
    for arg in args {
        match arg.label.as_deref() {
            Some("meta" | "ffi" | "flutter" | "angularMeta") if arg.expr == "true" => {
                let name = if arg.label.as_deref() == Some("angularMeta") {
                    "angular_meta"
                } else {
                    arg.label.as_deref().expect("checked above")
                };
                add_package(env, name);
            }
            Some("languageVersion") => {
                let mut pos = 0usize;
                if let Some(d) = parse_string_expression(&arg.expr, &mut pos) {
                    if !d.interpolated {
                        env.language = Some(d.value);
                    }
                }
            }
            None => {
                // The positional PackageConfigFileBuilder cascade.
                #[cfg(test)]
                eprintln!("DBG cascade arm: {:?}", arg.expr);
                let expr = arg.expr.as_str();
                let mut rest = expr;
                while let Some(idx) = rest.find("..add(") {
                    let open = idx + "..add(".len();
                    #[cfg(test)]
                    eprintln!("DBG loop idx={idx} open={open}");
                    let Some(close) = match_paren(rest, open) else {
                        #[cfg(test)]
                        eprintln!("DBG match_paren None");
                        break;
                    };
                    #[cfg(test)]
                    eprintln!("DBG close={close}");
                    let inner = parse_args(&rest[open + 1..close]);
                    if let (Some(name), Some(root)) = (
                        inner.iter().find(|a| a.label.as_deref() == Some("name")),
                        inner.iter().find(|a| a.label.as_deref() == Some("rootFolder")),
                    ) {
                        let mut pos = 0usize;
                        if let Some(d) = parse_string_expression(&name.expr, &mut pos) {
                            if !d.interpolated {
                                let root_expr = root.expr.trim();
                                let root_expr = root_expr
                                    .strip_prefix("getFolder(")
                                    .and_then(|e| e.strip_suffix(')'))
                                    .unwrap_or(root_expr);
                                #[cfg(test)]
                                eprintln!("DBG root_expr: {:?}", root_expr);
                                if let Some(resolved) = resolve_root_expr(root_expr, env) {
                                    #[cfg(test)]
                                    eprintln!("DBG cascade: {} -> {:?}", d.value, resolved);
                                    if !env.aux_packages.contains(&d.value) {
                                        env.aux_packages.push(d.value.clone());
                                    }
                                    env.package_roots.push((d.value, resolved));
                                }
                            }
                        }
                    }
                    rest = &rest[close..];
                }
            }
            _ => {}
        }
    }
}

fn interpret_options_call(env: &mut Env, expr: Option<&str>, skip: &mut dyn FnMut(String)) {
    let Some(expr) = expr else { return };
    let expr = expr.trim();
    if let Some(rest) = expr.strip_prefix("analysisOptionsContent(") {
        if let Some(close) = match_paren(rest, 0) {
            let args = parse_args(&rest[..close]);
            match setup::analysis_options_from_call(&args) {
                Some(yaml) => env.options = yaml,
                None => skip("options_custom".into()),
            }
        }
        return;
    }
    let mut pos = 0usize;
    match parse_string_expression(expr, &mut pos) {
        Some(d) if !d.interpolated && pos == expr.len() => env.options = d.value,
        _ => skip("options_custom".into()),
    }
}

fn literal_arg(arg: &Arg) -> Option<String> {
    let mut pos = 0usize;
    let d = parse_string_expression(&arg.expr, &mut pos)?;
    if d.interpolated || pos != arg.expr.len() {
        return None;
    }
    Some(d.value)
}

fn interpret_new_file(env: &mut Env, args: &[Arg], skip: &mut dyn FnMut(String)) {
    let (Some(path_arg), Some(content_arg)) = (args.first(), args.get(1)) else { return };
    let Some(path) = resolve_path_text(&path_arg.expr, &env.path_env) else {
        #[cfg(test)]
        eprintln!("DBG companion_path: expr={:?} env={:?}", path_arg.expr, env.path_env);
        skip("companion_path".into());
        return;
    };
    let Some(content) = literal_arg(content_arg) else {
        skip("companion_interpolated".into());
        return;
    };
    let section = if let Some(rest) = path.strip_prefix(TEST_LIB) {
        Some(format!("lib{rest}"))
    } else if let Some(rest) = path.strip_prefix(TEST_ROOT) {
        Some(rest.trim_start_matches('/').to_string())
    } else {
        env.package_roots
            .iter()
            .find(|(_, root)| path.starts_with(&format!("{root}/")))
            .map(|(name, root)| format!("package:{name}/{}", &path[root.len() + 1..]))
    };
    match section {
        Some(rel_path) => {
            if !content.is_empty() && !content.ends_with('\n') {
                env.files.push((rel_path, format!("{content}\n")));
            } else {
                env.files.push((rel_path, content));
            }
        }
        None => skip("companion_path".into()),
    }
}

/// Extracts one method's cases: the calls before each target call shape that
/// target's environment.
fn extract_method(
    code_prefix: &str,
    rel: &str,
    class_name: &str,
    method_name: &str,
    body: &str,
    class_env: &Env,
    out: &mut FileExtraction,
) {
    let mut index = 0usize;
    let mut p = 0usize;
    while let Some((name, open, _)) = find_call(body, p, &|n| TARGET_CALLS.contains(&n)) {
        p = open + 1;
        let id = format!("{code_prefix}/{class_name}/{method_name}#{index}");
        index += 1;
        let Some(close) = match_paren(body, open) else { continue };
        let args = parse_args(&body[open + 1..close]);
        let source = args
            .first()
            .and_then(|a| {
                let mut pos = 0usize;
                parse_string_expression(&a.expr, &mut pos).filter(|d| !d.interpolated && pos == a.expr.len())
            });
        let Some(source) = source else {
            out.skips.push(Skip { id, reason: "interpolated_source".into() });
            continue;
        };

        // The calls before this target, applied to a copy of the class env.
        let mut env = Env {
            options: class_env.options.clone(),
            language: class_env.language.clone(),
            packages: class_env.packages.clone(),
            aux_packages: class_env.aux_packages.clone(),
            package_roots: class_env.package_roots.clone(),
            path_env: class_env.path_env.clone(),
            experiments: class_env.experiments.clone(),
            files: class_env.files.clone(),
        };
        interpret_calls(&body[..open], &mut env, &mut |reason| {
            out.skips.push(Skip { id: id.clone(), reason });
        });
        if name == "assertTestCodeWithStrictCastsDiagnostics" {
            env.options = strict_casts_options(&env.experiments);
        }

        let mut files = vec![("lib/test.dart".to_string(), source.value)];
        files.extend(env.files.iter().cloned());
        out.cases.push(Case {
            id,
            source: format!("{rel}#{class_name}.{method_name}"),
            language: env.language,
            packages: env.packages,
            aux_packages: env.aux_packages,
            options: Some(env.options),
            ignores: Vec::new(),
            files,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"
import 'context.dart';

@reflectiveTest
class FooTest extends PubPackageResolutionTest with WithLanguage219Mixin {
  String get aaaPackageRootPath => '$workspaceRootPath/aaa';

  @override
  void setUp() {
    super.setUp();
    writeTestPackageConfig(
      PackageConfigFileBuilder()..add(name: 'aaa', rootFolder: getFolder(aaaPackageRootPath)),
      meta: true,
    );
    newFile('$aaaPackageRootPath/lib/a.dart', 'class A {}');
  }

  test_companion() async {
    await resolveTestCodeWithDiagnostics(r'''
import 'package:aaa/a.dart';
var x = 1;
//         ^
// [diag.foo] m
''');
  }

  test_interpolated() async {
    var code = 'var x = 1;';
    await resolveTestCodeWithDiagnostics(code);
  }

  test_config() async {
    writeTestPackageConfig(
      PackageConfigFileBuilder()..add(name: 'b', rootFolder: getFolder(bPackageRootPath)),
      languageVersion: '2.17',
    );
    await resolveTestCode('var y = 2;');
  }
}
"#;

    #[test]
    fn debug_skips() {
        let out = extract_file("src/diagnostics/foo_test.dart", FILE);
        eprintln!("cases: {:?}", out.cases.iter().map(|c| (&c.id, &c.aux_packages, &c.files)).collect::<Vec<_>>());
        eprintln!("skips: {:?}", out.skips);
    }

    #[test]
    fn extracts_cases_with_setup() {
        let out = extract_file("src/diagnostics/foo_test.dart", FILE);
        assert_eq!(out.cases.len(), 2, "cases: {:?}", out.cases.iter().map(|c| &c.id).collect::<Vec<_>>());
        let first = &out.cases[0];
        assert_eq!(first.id, "diagnostics/foo/FooTest/test_companion#0");
        assert_eq!(first.language.as_deref(), Some("2.19"));
        assert_eq!(first.packages, vec!["meta"]);
        assert_eq!(first.aux_packages, vec!["aaa"]);
        assert_eq!(first.files.len(), 2);
        assert_eq!(first.files[0].0, "lib/test.dart");
        assert_eq!(first.files[1].0, "package:aaa/lib/a.dart");
        assert_eq!(first.files[1].1, "class A {}\n");
        assert!(first.options.as_ref().expect("options").contains("propagate-linter-exceptions"));

        let second = &out.cases[1];
        assert_eq!(second.language.as_deref(), Some("2.17"));
        assert!(second.options.as_ref().expect("options").contains("- enhanced-parts"));
    }

    #[test]
    fn interpolated_sources_skip() {
        let out = extract_file("src/diagnostics/foo_test.dart", FILE);
        assert_eq!(out.skips.len(), 1);
        assert_eq!(out.skips[0].reason, "interpolated_source");
        assert!(out.skips[0].id.ends_with("test_interpolated#0"));
    }

    #[test]
    fn misc_files_land_under_misc() {
        let out = extract_file("src/dart/resolution/other_test.dart", FILE);
        assert!(out.cases[0].id.starts_with("diagnostics/_misc/src/dart/resolution/other/"));
    }
}
