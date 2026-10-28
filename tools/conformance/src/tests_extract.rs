//! Extraction of cases from analyzer package test files: every
//! `resolveTestCodeWithDiagnostics` / `resolveTestCode` /
//! `assertTestCodeWithStrictCastsDiagnostics` call becomes one diagnostics
//! case, every `resolveFilesWithDiagnostics` map becomes one multi-file
//! diagnostics case, and every `parseTestCodeWithDiagnostics` call becomes
//! one parse-only `parser/` case. The setup (options, packages, language
//! version, companion files) is derived from the class-level setup and the
//! calls preceding the target in its method body, following the official
//! harness semantics.
//!
//! The environment mirrors the official inheritance: path getters, string
//! constants, `setUp` effects and helper methods are resolved through the
//! superclass/mixin chain declared in the same file, plus a small table for
//! the pinned base classes that live outside `*_test.dart` files. A site
//! whose companion setup cannot be modeled faithfully either produces a
//! precisely named skip or suppresses the case entirely (a case with wrong
//! inputs would poison the differential run).

use crate::case::Case;
use crate::dart::{
    Arg, find_call, ident_end, is_ident_start, match_paren, next_char, parse_args,
    parse_string_expression, skip_ws_and_comments,
};
use crate::setup;
use crate::structure::{Declaration, match_brace, scan_declarations};
use std::path::{Path, PathBuf};

/// The test-entry methods whose first argument carries the code snippet.
/// `resolveFilesWithDiagnostics` carries a file->code map instead and is
/// routed to its own branch; the parse-only call lands in the `parser/`
/// face.
const TARGET_CALLS: [&str; 5] = [
    "resolveTestCodeWithDiagnostics",
    "resolveTestCode",
    "assertTestCodeWithStrictCastsDiagnostics",
    "resolveFilesWithDiagnostics",
    "parseTestCodeWithDiagnostics",
];

/// Target-shaped entries the corpus intentionally does not carry: their
/// official harnesses resolve outside the modeled single-context shape
/// (`resolveTestFile`) or deliberately ignore diagnostics
/// (`parseTestCodeIgnoringDiagnostics`). Recorded as named skips so the
/// coverage hole stays visible in the skip inventory.
const UNMODELED_TARGET_CALLS: [&str; 2] = [
    "resolveTestFile",
    "parseTestCodeIgnoringDiagnostics",
];

/// Package/workspace registration helpers whose effects the single
/// package-root case model cannot express. A test calling them wires its
/// import graph outside the modeled environment; the case is suppressed
/// with a per-helper named skip instead of being emitted with a broken
/// import graph.
const WORKSPACE_REGISTRATION_CALLS: [&str; 3] = [
    "newBuildGnFile",
    "_writeWorkspacePackagesFile",
    "_newTestPackageGeneratedFile",
];

/// Setup calls whose effects are modeled.
const SETUP_CALLS: [&str; 8] = [
    "writeTestPackageConfig",
    "writeTestPackageConfigWithMeta",
    "writeTestPackageAnalysisOptionsFile",
    "newAnalysisOptionsYamlFile",
    "newFile",
    "newPubspecYamlFile",
    "addTestFile",
    "getFile",
];

/// Mixins that pin the test package's language version.
const LANGUAGE_VERSION_MIXINS: [(&str, &str); 4] = [
    ("WithLanguage219Mixin", "2.19"),
    ("WithoutConstructorTearoffsMixin", "2.14"),
    ("WithoutEnhancedEnumsMixin", "2.16"),
    ("WithoutPrivateNamedParametersMixin", "3.9"),
];

/// The official test package layout (upstream `workspaceRootPath` etc.),
/// the fallback when the resolved getters are absent.
const TEST_ROOT: &str = "/home/test";
const TEST_LIB: &str = "/home/test/lib";

/// The canonical path getters in dependency order. Each resolves from its
/// source override, or from the official `PubPackageResolutionTest` default
/// below; getters without a default or an override are omitted.
const CANONICAL_PATHS: [&str; 7] = [
    "workspaceRootPath",
    "myPackageRootPath",
    "myPackageLibPath",
    "workspaceThirdPartyDartPath",
    "testPackageRootPath",
    "testPackageLibPath",
    "packagesRootPath",
];

/// The official `PubPackageResolutionTest` getter expressions.
const CANONICAL_DEFAULTS: [(&str, &str); 4] = [
    ("workspaceRootPath", "'/home'"),
    ("testPackageRootPath", "'$workspaceRootPath/test'"),
    ("testPackageLibPath", "'$testPackageRootPath/lib'"),
    ("packagesRootPath", "'/packages'"),
];

/// Path getters of pinned base classes declared outside `*_test.dart` files
/// (`context_collection_resolution.dart` / the micro `file_resolution.dart`),
/// transcribed from the pin. Applied when the class lineage names them.
const BASE_CLASS_PATHS: [(&str, &[(&str, &str)]); 2] = [
    (
        "BlazeWorkspaceResolutionTest",
        &[
            ("workspaceRootPath", "'/workspace'"),
            ("myPackageRootPath", "'$workspaceRootPath/dart/my'"),
            ("myPackageLibPath", "'$myPackageRootPath/lib'"),
            (
                "workspaceThirdPartyDartPath",
                "'$workspaceRootPath/third_party/dart'",
            ),
        ],
    ),
    (
        "FileResolutionTest",
        &[
            ("workspaceRootPath", "'/workspace'"),
            ("testPackageRootPath", "'$workspaceRootPath/dart/test'"),
            ("testPackageLibPath", "'$testPackageRootPath/lib'"),
        ],
    ),
];

/// The workspace flavor of a test class: blaze/gn/package-build workspaces
/// resolve packages through their own machinery, which the single
/// package-root model of a `.case` cannot express.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Workspace {
    Pub,
    Blaze,
    Gn,
    PackageBuild,
}

/// Why a path expression could not be resolved.
#[derive(Debug, PartialEq, Eq)]
enum ResolveFail {
    /// The name is declared (abstract getter) but has no value.
    Abstract(String),
    Unresolved,
}

/// The substitution table for path/source expressions: method locals first,
/// then class getters/constants.
struct Names<'a> {
    locals: &'a [(String, String)],
    values: &'a [(String, String)],
    abstract_names: &'a [String],
}

impl Names<'_> {
    fn get(&self, name: &str) -> Result<String, ResolveFail> {
        for (n, v) in self.locals.iter().rev() {
            if n == name {
                return Ok(v.clone());
            }
        }
        for (n, v) in self.values.iter().rev() {
            if n == name {
                return Ok(v.clone());
            }
        }
        if self.abstract_names.iter().any(|n| n == name) {
            Err(ResolveFail::Abstract(name.to_string()))
        } else {
            Err(ResolveFail::Unresolved)
        }
    }
}

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

/// Everything extracted from a whole source-only corpus directory.
#[derive(Debug, Default)]
pub struct DirExtraction {
    pub cases: Vec<Case>,
    pub skips: Vec<Skip>,
    /// Corpus files scanned (each yields one case or one skip).
    pub scanned: usize,
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
        format!(
            "diagnostics/_misc/{}",
            path.strip_suffix(".dart").unwrap_or(path)
        )
    };
    // The parse-only harness mirrors the diagnostics routing: the two
    // parse-shaped `src/diagnostics` files keep their bare stem, every other
    // file keeps its full path stem. Full path stems keep the ids unique
    // (several parse suites share file stems across directories).
    let parse_prefix = if rel.starts_with("src/diagnostics/") {
        format!("parser/{stem}")
    } else {
        let path = rel.strip_suffix("_test.dart").unwrap_or(rel);
        format!("parser/{}", path.strip_suffix(".dart").unwrap_or(path))
    };
    let decls = scan_declarations(content);
    for decl in &decls {
        let holds_targets = decl
            .members
            .iter()
            .any(|m| m.body.is_some_and(|(s, e)| has_target_call(&content[s..e])));
        if holds_targets {
            extract_declaration(
                &code_prefix,
                &parse_prefix,
                rel,
                content,
                decl,
                &decls,
                &mut out,
            );
        }
    }
    out
}

fn has_target_call(body: &str) -> bool {
    find_call(body, 0, &|n| {
        TARGET_CALLS.contains(&n) || UNMODELED_TARGET_CALLS.contains(&n)
    })
    .is_some()
}

/// The resolved class environment: defaults + mixins + the setUp chain.
#[derive(Clone)]
struct Env {
    options: String,
    language: Option<String>,
    packages: Vec<String>,
    aux_packages: Vec<String>,
    /// package name -> concrete root path, for companion resolution.
    package_roots: Vec<(String, String)>,
    /// identifier -> concrete path/string value, for `$identifier`
    /// substitution and bare-identifier lookups.
    path_env: Vec<(String, String)>,
    /// identifiers declared as abstract getters (no value).
    path_abstract: Vec<String>,
    experiments: Vec<String>,
    /// File sections (path, content). Paths starting with `package:` belong
    /// to an auxiliary package.
    files: Vec<(String, String)>,
    /// Local identifiers bound to a file's section path (`var a =
    /// getFile(...)` / `var a = newFile(...)`), for `Map<File, String>` keys.
    file_sections: Vec<(String, String)>,
    workspace: Workspace,
    /// Method-local string values (empty on the class env).
    locals: Vec<(String, String)>,
}

impl Env {
    fn names(&self) -> Names<'_> {
        Names {
            locals: &self.locals,
            values: &self.path_env,
            abstract_names: &self.path_abstract,
        }
    }

    fn lookup(&self, name: &str) -> Option<String> {
        self.names().get(name).ok()
    }

    fn test_root(&self) -> String {
        self.lookup("testPackageRootPath")
            .unwrap_or_else(|| TEST_ROOT.to_string())
    }

    fn test_lib(&self) -> String {
        self.lookup("testPackageLibPath")
            .unwrap_or_else(|| TEST_LIB.to_string())
    }
}

/// The superclass chain of `decl`, root first, plus every superclass name
/// seen (including the terminal one declared outside the file).
fn class_chain<'d>(
    decl: &'d Declaration,
    decls: &'d [Declaration],
) -> (Vec<&'d Declaration>, Vec<String>) {
    let mut chain = vec![decl];
    let mut supers = Vec::new();
    let mut current = decl;
    while let Some(name) = &current.superclass {
        supers.push(name.clone());
        match decls.iter().find(|d| &d.name == name) {
            Some(next) => {
                chain.push(next);
                current = next;
            }
            None => break,
        }
    }
    chain.reverse();
    (chain, supers)
}

/// The member sources in Dart application order: for each chain member
/// (root first) its mixins, then the member itself.
fn application_order<'d>(
    chain: &[&'d Declaration],
    decls: &'d [Declaration],
) -> Vec<&'d Declaration> {
    let mut out = Vec::new();
    for source in chain {
        for mixin in &source.mixins {
            if let Some(decl) = decls.iter().find(|d| &d.name == mixin) {
                out.push(decl);
            }
        }
        out.push(source);
    }
    out
}

fn workspace_of(lineage: &[String]) -> Workspace {
    for name in lineage {
        if name.contains("BlazeWorkspace") {
            return Workspace::Blaze;
        }
        if name.contains("GnWorkspace") {
            return Workspace::Gn;
        }
        if name.contains("PackageBuildWorkspace") {
            return Workspace::PackageBuild;
        }
    }
    Workspace::Pub
}

fn base_env(content: &str, decl: &Declaration, decls: &[Declaration]) -> Env {
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
    let experiments = experiments.unwrap_or_else(|| {
        setup::DEFAULT_EXPERIMENTS
            .iter()
            .map(|s| s.to_string())
            .collect()
    });
    let (chain, supers) = class_chain(decl, decls);
    // The extends lineage (superclasses and mixins) carries member values;
    // `implements` names declare interface members only.
    let mut lineage: Vec<String> = chain.iter().map(|d| d.name.clone()).collect();
    lineage.extend(supers);
    for source in &chain {
        lineage.extend(source.mixins.iter().cloned());
    }
    let implements_pub = decl
        .implements
        .iter()
        .chain(chain.iter().flat_map(|d| d.implements.iter()))
        .any(|n| n == "PubPackageResolutionTest");
    let (path_env, path_abstract) =
        build_path_env(content, decls, &chain, &lineage, implements_pub);
    let mut env = Env {
        options: setup::default_options(Some(&experiments)),
        language,
        packages: Vec::new(),
        aux_packages: Vec::new(),
        package_roots: Vec::new(),
        path_env,
        path_abstract,
        experiments,
        files: Vec::new(),
        file_sections: Vec::new(),
        workspace: workspace_of(&lineage),
        locals: Vec::new(),
    };
    if decl.mixins.iter().any(|m| m == "WithStrictCastsMixin") {
        env.options = strict_casts_options(&env.experiments);
    }
    env
}

fn strict_casts_options(experiments: &[String]) -> String {
    setup::analysis_options_content(experiments, &[], true, false, false, &[], &[])
}

/// Builds the class-level substitution table: canonical path getters in
/// dependency order (source override or official default), then every other
/// string-valued member of the lineage (paths, constants, block getters).
/// Abstract getters are collected separately so name lookups can report
/// "declared without value" distinctly from "unknown".
fn build_path_env(
    content: &str,
    decls: &[Declaration],
    chain: &[&Declaration],
    lineage: &[String],
    implements_pub: bool,
) -> (Vec<(String, String)>, Vec<String>) {
    let mut overrides: Vec<(String, String)> = Vec::new();
    let mut abstract_names: Vec<String> = Vec::new();
    // Pinned out-of-file base classes are the least derived sources.
    for (class_name, getters) in BASE_CLASS_PATHS {
        if lineage.iter().any(|n| n == class_name) {
            for (name, expr) in getters {
                upsert(&mut overrides, (*name).to_string(), (*expr).to_string());
            }
        }
    }
    for source in application_order(chain, decls) {
        for member in &source.members {
            if let Some((start, end)) = member.initializer {
                upsert(
                    &mut overrides,
                    member.name.clone(),
                    content[start..end].to_string(),
                );
            } else if member.body.is_none() && member.params.is_empty() {
                if !abstract_names.contains(&member.name) {
                    abstract_names.push(member.name.clone());
                }
            }
        }
    }
    let mut env: Vec<(String, String)> = Vec::new();
    // Getters resolve against the values already derived (dependency order),
    // never against raw override text.
    let has_pub_base = lineage.iter().any(|n| n == "PubPackageResolutionTest");
    fn resolve_against(
        expr: &str,
        env: &[(String, String)],
        abstract_names: &[String],
    ) -> Result<String, ResolveFail> {
        let names = Names {
            locals: &[],
            values: env,
            abstract_names,
        };
        resolve_path_expr(expr, &names)
    }
    // A declaration that only `implements` a path-provider base declares the
    // getters without values (the mixing class supplies them).
    let interface_pub = !has_pub_base && implements_pub;
    for name in CANONICAL_PATHS {
        let expr = match overrides.iter().find(|(n, _)| *n == name) {
            Some((_, expr)) => Some(expr.as_str()),
            None if has_pub_base => CANONICAL_DEFAULTS
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, expr)| *expr),
            None => None,
        };
        if expr.is_none() && interface_pub && !abstract_names.iter().any(|n| n == name) {
            abstract_names.push(name.to_string());
        }
        let Some(expr) = expr else { continue };
        if let Ok(value) = resolve_against(expr, &env, &abstract_names) {
            env.push((name.to_string(), value));
        }
    }
    for (name, expr) in &overrides {
        if env.iter().any(|(n, _)| n == name) {
            continue;
        }
        match resolve_against(expr, &env, &abstract_names) {
            Ok(value) => env.push((name.clone(), value)),
            // A getter whose value hangs off an abstract getter is itself
            // valueless; report the dependency at the lookup site.
            Err(ResolveFail::Abstract(_)) => {
                if !abstract_names.contains(name) {
                    abstract_names.push(name.clone());
                }
            }
            Err(ResolveFail::Unresolved) => {}
        }
    }
    (env, abstract_names)
}

fn upsert(list: &mut Vec<(String, String)>, name: String, value: String) {
    match list.iter_mut().find(|(n, _)| *n == name) {
        Some(entry) => entry.1 = value,
        None => list.push((name, value)),
    }
}

/// Resolves a path expression: a string-literal run (with `$name`
/// interpolation), a bare identifier naming a local/getter, or
/// `join(...)` / `convertPath(...)` over such expressions.
fn resolve_path_expr(expr: &str, names: &Names<'_>) -> Result<String, ResolveFail> {
    let expr = expr.trim();
    if let Some(inner) = call_args(expr, "join") {
        let mut joined: Option<String> = None;
        for arg in parse_args(inner) {
            let part = resolve_path_expr(&arg.expr, names)?;
            joined = Some(match joined {
                Some(prefix) => format!("{prefix}/{part}"),
                None => part,
            });
        }
        return joined.ok_or(ResolveFail::Unresolved);
    }
    // `convertPath` is the identity on the posix paths every case uses.
    if let Some(inner) = call_args(expr, "convertPath") {
        return resolve_path_expr(inner, names);
    }
    if is_bare_ident(expr) {
        return names.get(expr);
    }
    resolve_path_text(expr, names)
}

/// Resolves a path string expression (one literal or an adjacent run) to a
/// concrete string, interpolating `$identifier` / `${identifier}` from
/// `names`.
fn resolve_path_text(text: &str, names: &Names<'_>) -> Result<String, ResolveFail> {
    let text = text.trim();
    let mut pos = 0usize;
    let first = parse_string_expression(text, &mut pos).ok_or(ResolveFail::Unresolved)?;
    let mut out = string_part(&first, names)?;
    while pos < text.len() {
        let mut consumed = 0usize;
        let next =
            parse_string_expression(&text[pos..], &mut consumed).ok_or(ResolveFail::Unresolved)?;
        out.push_str(&string_part(&next, names)?);
        if consumed == 0 {
            break;
        }
        pos += consumed;
    }
    Ok(out)
}

fn string_part(d: &crate::dart::DartString, names: &Names<'_>) -> Result<String, ResolveFail> {
    if d.interpolated {
        interpolate(&d.value, names)
    } else {
        Ok(d.value.clone())
    }
}

/// Resolves every `$` interpolation of a decoded string value.
fn interpolate(value: &str, names: &Names<'_>) -> Result<String, ResolveFail> {
    let mut out = String::new();
    let mut rest = value;
    while let Some(idx) = rest.find('$') {
        out.push_str(&rest[..idx]);
        let after = &rest[idx + 1..];
        if let Some(inner) = after.strip_prefix('{') {
            let Some(end) = inner.find('}') else {
                return Err(ResolveFail::Unresolved);
            };
            out.push_str(&dollar_value(&inner[..end], names)?);
            rest = &inner[end + 1..];
        } else {
            let name_end = after
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            let (name, tail) = after.split_at(name_end);
            if name.is_empty() {
                out.push('$');
                rest = after;
            } else {
                out.push_str(&dollar_value(name, names)?);
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// The value of one interpolation body: an identifier or a plain string
/// literal (`${"  "}`).
fn dollar_value(inner: &str, names: &Names<'_>) -> Result<String, ResolveFail> {
    if is_bare_ident(inner) {
        return names.get(inner);
    }
    let mut pos = 0usize;
    let d = parse_string_expression(inner, &mut pos).ok_or(ResolveFail::Unresolved)?;
    if d.interpolated || pos != inner.len() {
        return Err(ResolveFail::Unresolved);
    }
    Ok(d.value)
}

/// `Some(inner)` when `expr` is exactly `name(<args>)`.
fn call_args<'a>(expr: &'a str, name: &str) -> Option<&'a str> {
    if !expr.starts_with(name) {
        return None;
    }
    let rest = expr[name.len()..].trim_start();
    if !rest.starts_with('(') {
        return None;
    }
    let open = expr.len() - rest.len();
    let close = match_paren(expr, open)?;
    if expr[close + 1..].trim().is_empty() {
        Some(&expr[open + 1..close])
    } else {
        None
    }
}

fn is_bare_ident(expr: &str) -> bool {
    !expr.is_empty()
        && is_ident_start(expr.as_bytes()[0])
        && expr.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A helper method declared in the class lineage, callable from the bodies
/// being interpreted.
struct Helper {
    name: String,
    body: (usize, usize),
    params: Vec<String>,
}

/// Per-declaration interpretation context.
struct InterpCtx<'a> {
    content: &'a str,
    helpers: Vec<Helper>,
}

fn build_ctx<'a>(content: &'a str, decl: &Declaration, decls: &[Declaration]) -> InterpCtx<'a> {
    let (chain, _) = class_chain(decl, decls);
    let mut helpers = Vec::new();
    for source in &chain {
        for member in &source.members {
            if let Some((start, end)) = member.body {
                if member.name != "setUp" {
                    helpers.push(Helper {
                        name: member.name.clone(),
                        body: (start, end),
                        params: member.params.clone(),
                    });
                }
            }
        }
    }
    InterpCtx { content, helpers }
}

/// The string-valued local declarations of one body (`var name = <expr>;`,
/// `final`/`const`/typed forms included), in declaration order. The value
/// must resolve through the names visible so far; anything else is ignored.
fn scan_local_strings(body: &str, env: &Env) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let bytes = body.as_bytes();
    let mut pos = 0usize;
    let mut boundary = true;
    while pos < body.len() {
        let rest = &body[pos..];
        if rest.starts_with('\'')
            || rest.starts_with('"')
            || (rest.starts_with('r') && matches!(bytes.get(pos + 1), Some(b'\'') | Some(b'"')))
        {
            skip_string(body, &mut pos);
            continue;
        }
        if rest.starts_with("//") {
            pos += rest.find('\n').map_or(rest.len(), |e| e);
            continue;
        }
        if rest.starts_with("/*") {
            pos += 2;
            while pos < body.len() && !body[pos..].starts_with("*/") {
                pos = next_char(body, pos);
            }
            pos = (pos + 2).min(body.len());
            continue;
        }
        match bytes[pos] {
            b';' | b'{' | b'}' => {
                boundary = true;
                pos += 1;
            }
            _ if boundary => match try_decl(body, pos, &mut out, env) {
                Some(end) => {
                    pos = end;
                }
                None => {
                    boundary = false;
                    pos = next_char(body, pos);
                }
            },
            _ => pos = next_char(body, pos),
        }
    }
    out
}

/// Attempts to read `prefix... name = <string expr>;` at a statement
/// boundary; on success pushes `(name, value)` and returns the offset just
/// past the `;`.
fn try_decl(body: &str, start: usize, out: &mut Vec<(String, String)>, env: &Env) -> Option<usize> {
    let bytes = body.as_bytes();
    let mut p = start;
    let mut words = 0usize;
    loop {
        p += skip_ws_and_comments(&body[p..]);
        let b = *bytes.get(p)?;
        if !is_ident_start(b) {
            return None;
        }
        let w_end = ident_end(body, p);
        let q = w_end + skip_ws_and_comments(&body[w_end..]);
        // Qualifiers, generics and lambdas are outside the simple pattern.
        if body[q..].starts_with('.') || body[q..].starts_with('<') || body[q..].starts_with("=>") {
            return None;
        }
        if body[q..].starts_with('=') {
            if body[q + 1..].starts_with('=') {
                return None; // comparison, not assignment
            }
            if words == 0 {
                return None; // `name = ...` is an assignment, not a declaration
            }
            let name = body[p..w_end].to_string();
            let expr_start = q + 1;
            let mut r = expr_start;
            let mut depth = 0i64;
            while r < body.len() {
                let rest = &body[r..];
                if rest.starts_with('\'')
                    || rest.starts_with('"')
                    || (rest.starts_with('r')
                        && matches!(bytes.get(r + 1), Some(b'\'') | Some(b'"')))
                {
                    skip_string(body, &mut r);
                    continue;
                }
                if rest.starts_with("//") {
                    r += rest.find('\n').map_or(rest.len(), |e| e);
                    continue;
                }
                if rest.starts_with("/*") {
                    r += 2;
                    while r < body.len() && !body[r..].starts_with("*/") {
                        r = next_char(body, r);
                    }
                    r = (r + 2).min(body.len());
                    continue;
                }
                match bytes[r] {
                    b'(' | b'[' | b'{' => depth += 1,
                    b')' | b']' | b'}' => depth -= 1,
                    b';' if depth == 0 => break,
                    _ => {}
                }
                r = next_char(body, r);
            }
            let expr = body[expr_start..r.min(body.len())].trim();
            let names = Names {
                locals: out,
                values: &env.path_env,
                abstract_names: &env.path_abstract,
            };
            if let Ok(value) = resolve_path_expr(expr, &names) {
                out.push((name, value));
            }
            return Some(r + 1);
        }
        words += 1;
        if words > 3 {
            return None;
        }
        p = q;
    }
}

/// Interprets the modeled calls of one body in offset order, updating `env`.
/// Unmodelable setup calls fire `skip` with a `setup_*` reason; workspace
/// registration helpers fire `unmodeled_workspace_setup:<helper>`; helper
/// methods of the class lineage are interpreted in place with their
/// arguments bound (depth-limited).
fn interpret_calls(
    body: &str,
    env: &mut Env,
    ctx: &InterpCtx<'_>,
    depth: usize,
    skip: &mut dyn FnMut(String),
) {
    let mut pos = 0usize;
    while let Some((name, open, _)) = find_call(body, pos, &|n| {
        TARGET_CALLS.contains(&n)
            || SETUP_CALLS.contains(&n)
            || WORKSPACE_REGISTRATION_CALLS.contains(&n)
            || n == "addMeta"
            || n == "addFfi"
            || ctx.helpers.iter().any(|h| h.name == n)
    }) {
        let Some(close) = match_paren(body, open) else {
            break;
        };
        let args = parse_args(&body[open + 1..close]);
        if WORKSPACE_REGISTRATION_CALLS.contains(&name.as_str()) {
            skip(format!("unmodeled_workspace_setup:{name}"));
            pos = close + 1;
            continue;
        }
        match name.as_str() {
            "writeTestPackageConfigWithMeta" => add_package(env, "meta"),
            "writeTestPackageConfig" => interpret_config_call(env, &args),
            "writeTestPackageAnalysisOptionsFile" => {
                interpret_options_call(env, args.first().map(|a| a.expr.as_str()), skip);
            }
            "newAnalysisOptionsYamlFile" => interpret_options_file_call(env, &args, skip),
            "newFile" => interpret_new_file(&body[..open], env, &args, skip),
            "newPubspecYamlFile" => interpret_pubspec_call(env, &args, skip),
            "getFile" => interpret_get_file(&body[..open], env, &args),
            // The official MockPackagesMixin members that write a mock
            // package and register it.
            "addMeta" => add_package(env, "meta"),
            "addFfi" => add_package(env, "ffi"),
            "addTestFile" => {}
            _ if depth < 3 => {
                if let Some(helper) = ctx.helpers.iter().rev().find(|h| h.name == name) {
                    let saved = env.locals.len();
                    bind_helper_args(env, helper, &args);
                    let helper_body = &ctx.content[helper.body.0..helper.body.1];
                    interpret_calls(helper_body, env, ctx, depth + 1, skip);
                    env.locals.truncate(saved);
                }
            }
            _ => {}
        }
        pos = close + 1;
    }
}

/// The identifier of a `var x = call(...)` / `x = call(...)` assignment whose
/// callee ends at the interpreted call (its prefix text, callee name
/// included, is `prefix`).
fn assigned_ident(prefix: &str) -> Option<String> {
    let tail = prefix.trim_end();
    let callee_start = tail
        .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .map_or(0, |i| i + 1);
    if callee_start == tail.len() {
        return None;
    }
    let head = tail[..callee_start].trim_end().strip_suffix('=')?.trim_end();
    let start = head
        .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .map_or(0, |i| i + 1);
    let ident = &head[start..];
    (!ident.is_empty() && is_bare_ident(ident)).then(|| ident.to_string())
}

/// `getFile(path)`: binds the assigned identifier to the path's file section
/// so `Map<File, String>` keys resolve. A failed resolution stays silent —
/// a map key that references the failed binding reports its own
/// `companion_path` skip at the target site.
fn interpret_get_file(prefix: &str, env: &mut Env, args: &[Arg]) {
    let Some(path_arg) = args.first() else {
        return;
    };
    let Ok(path) = resolve_path_expr(&path_arg.expr, &env.names()) else {
        return;
    };
    let Some(ident) = assigned_ident(prefix) else {
        return;
    };
    if let Some(section) = section_for_path(&path, env) {
        upsert(&mut env.file_sections, ident, section);
    }
}

fn add_package(env: &mut Env, name: &str) {
    if !env.packages.iter().any(|p| p == name) {
        env.packages.push(name.to_string());
    }
}

fn bind_helper_args(env: &mut Env, helper: &Helper, args: &[Arg]) {
    let mut positional = 0usize;
    for arg in args {
        let Ok(value) = resolve_path_expr(&arg.expr, &env.names()) else {
            continue;
        };
        match &arg.label {
            Some(label) => {
                if helper.params.iter().any(|p| p == label) {
                    upsert(&mut env.locals, label.clone(), value);
                }
            }
            None => {
                if let Some(param) = helper.params.get(positional) {
                    upsert(&mut env.locals, param.clone(), value);
                }
                positional += 1;
            }
        }
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
                if name == "flutter" {
                    // The official builder registers meta alongside flutter
                    // (`if (meta || flutter)`).
                    add_package(env, "meta");
                }
            }
            Some("languageVersion") => {
                if let Ok(version) = resolve_path_expr(&arg.expr, &env.names()) {
                    env.language = Some(version);
                }
            }
            None => {
                // The positional PackageConfigFileBuilder cascade.
                let expr = arg.expr.as_str();
                let mut rest = expr;
                while let Some(idx) = rest.find("..add(") {
                    // `match_paren` expects `open` at the '(' itself.
                    let open = idx + "..add(".len() - 1;
                    let Some(close) = match_paren(rest, open) else {
                        break;
                    };
                    let inner = parse_args(&rest[open + 1..close]);
                    if let (Some(name), Some(root)) = (
                        inner.iter().find(|a| a.label.as_deref() == Some("name")),
                        inner
                            .iter()
                            .find(|a| a.label.as_deref() == Some("rootFolder")),
                    ) {
                        let mut pos = 0usize;
                        if let Some(d) = parse_string_expression(&name.expr, &mut pos) {
                            if !d.interpolated {
                                let root_expr = root.expr.trim();
                                let root_expr = root_expr
                                    .strip_prefix("getFolder(")
                                    .and_then(|e| e.strip_suffix(')'))
                                    .unwrap_or(root_expr);
                                if let Ok(resolved) = resolve_path_expr(root_expr, &env.names()) {
                                    if !env.aux_packages.contains(&d.value) {
                                        env.aux_packages.push(d.value.clone());
                                    }
                                    upsert(&mut env.package_roots, d.value, resolved);
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

/// The options YAML of an `analysisOptionsContent(...)` builder call or a
/// plain YAML string literal.
fn options_content_expr(expr: &str, names: &Names<'_>) -> Option<String> {
    let expr = expr.trim();
    if let Some(rest) = expr.strip_prefix("analysisOptionsContent(") {
        let open = expr.len() - rest.len() - 1;
        let close = match_paren(expr, open)?;
        let args = parse_args(&expr[open + 1..close]);
        return setup::analysis_options_from_call(&args);
    }
    resolve_path_expr(expr, names).ok()
}

fn interpret_options_call(env: &mut Env, expr: Option<&str>, skip: &mut dyn FnMut(String)) {
    let Some(expr) = expr else { return };
    match options_content_expr(expr, &env.names()) {
        Some(yaml) => env.options = yaml,
        None => skip("options_custom".into()),
    }
}

/// `newAnalysisOptionsYamlFile(root, content)`: the test package's options
/// file becomes the case options; a registered package's own options file
/// becomes a section of that package; anything else stays unmodeled.
fn interpret_options_file_call(env: &mut Env, args: &[Arg], skip: &mut dyn FnMut(String)) {
    let (Some(root_arg), Some(content_arg)) = (args.first(), args.get(1)) else {
        return;
    };
    let root = match resolve_path_expr(&root_arg.expr, &env.names()) {
        Ok(root) => root,
        Err(_) => {
            skip("setup_newAnalysisOptionsYamlFile".into());
            return;
        }
    };
    let test_root = env.test_root();
    if root == test_root {
        interpret_options_call(env, Some(&content_arg.expr), skip);
        return;
    }
    let Some(content) = options_content_expr(&content_arg.expr, &env.names()) else {
        skip("setup_newAnalysisOptionsYamlFile".into());
        return;
    };
    let section = env.package_roots.iter().find_map(|(name, package_root)| {
        root.strip_prefix(package_root).map(|rel| {
            let rel = rel.trim_matches('/');
            if rel.is_empty() {
                format!("package:{name}")
            } else {
                format!("package:{name}/{rel}")
            }
        })
    });
    match section {
        Some(rel) => upsert(
            &mut env.files,
            format!("{rel}/analysis_options.yaml"),
            content,
        ),
        None => skip("setup_options_file_outside_test_package".into()),
    }
}

fn interpret_pubspec_call(env: &mut Env, args: &[Arg], skip: &mut dyn FnMut(String)) {
    let (Some(root_arg), Some(content_arg)) = (args.first(), args.get(1)) else {
        return;
    };
    let root = match resolve_path_expr(&root_arg.expr, &env.names()) {
        Ok(root) => root,
        Err(_) => {
            skip("setup_newPubspecYamlFile".into());
            return;
        }
    };
    if root != env.test_root() {
        skip("setup_newPubspecYamlFile".into());
        return;
    }
    match resolve_path_expr(&content_arg.expr, &env.names()) {
        Ok(content) => {
            upsert(&mut env.files, "pubspec.yaml".into(), content);
        }
        Err(_) => skip("companion_interpolated".into()),
    }
}

/// The file section of a companion path: inside the test package's lib, the
/// test package root, or a registered package root; `None` otherwise.
fn section_for_path(path: &str, env: &Env) -> Option<String> {
    let test_lib = env.test_lib();
    if let Some(rest) = path.strip_prefix(&format!("{test_lib}/")) {
        return Some(format!("lib/{rest}"));
    }
    let test_root = env.test_root();
    if let Some(rest) = path.strip_prefix(&format!("{test_root}/")) {
        return Some(rest.to_string());
    }
    env.package_roots
        .iter()
        .find(|(_, package_root)| path.starts_with(&format!("{package_root}/")))
        .map(|(name, package_root)| format!("package:{name}/{}", &path[package_root.len() + 1..]))
}

fn interpret_new_file(prefix: &str, env: &mut Env, args: &[Arg], skip: &mut dyn FnMut(String)) {
    let (Some(path_arg), Some(content_arg)) = (args.first(), args.get(1)) else {
        return;
    };
    let path = match resolve_path_expr(&path_arg.expr, &env.names()) {
        Ok(path) => path,
        Err(ResolveFail::Abstract(_)) => {
            skip("companion_abstract_path_getter".into());
            return;
        }
        Err(ResolveFail::Unresolved) => {
            skip("companion_path".into());
            return;
        }
    };
    let content = match resolve_path_expr(&content_arg.expr, &env.names()) {
        Ok(content) => content,
        Err(_) => {
            skip("companion_interpolated".into());
            return;
        }
    };
    // Package-build generated directories are only reachable through the
    // workspace's generated-file mapping, which the single-package-root
    // model of a case cannot express.
    if env.workspace == Workspace::PackageBuild {
        if let Some(generated) = env.lookup("testPackageGeneratedPath") {
            if path.starts_with(&generated) {
                skip("companion_package_build_workspace".into());
                return;
            }
        }
    }
    match section_for_path(&path, env) {
        Some(rel_path) => {
            let content = if !content.is_empty() && !content.ends_with('\n') {
                format!("{content}\n")
            } else {
                content
            };
            if let Some(ident) = assigned_ident(prefix) {
                upsert(&mut env.file_sections, ident, rel_path.clone());
            }
            upsert(&mut env.files, rel_path, content);
        }
        None => skip(
            match env.workspace {
                Workspace::Blaze => "companion_blaze_workspace",
                Workspace::Gn => "companion_gn_workspace",
                Workspace::Pub | Workspace::PackageBuild => "companion_no_package_root",
            }
            .into(),
        ),
    }
}

/// The decoded source argument: a plain literal, an interpolation of env
/// names, or a bare identifier naming a local/getter — optionally
/// post-processed by a `.split("\n").join("\r\n")` chain.
fn source_arg(expr: &str, env: &Env) -> Option<String> {
    let expr = expr.trim();
    let mut pos = 0usize;
    match parse_string_expression(expr, &mut pos) {
        Some(d) => {
            let mut value = if d.interpolated {
                interpolate(&d.value, &env.names()).ok()?
            } else {
                d.value
            };
            let rest = expr[pos..].trim_start();
            if !rest.is_empty() {
                value = apply_split_join(rest, &value)?;
            }
            Some(value)
        }
        None => {
            if is_bare_ident(expr) {
                env.lookup(expr)
            } else {
                None
            }
        }
    }
}

/// Applies a trailing `.split(<lit>).join(<lit>)` chain, replacing `from`
/// with `to` (the official harness's line-ending rewrites).
fn apply_split_join(rest: &str, value: &str) -> Option<String> {
    let s = rest.trim_start();
    if !s.starts_with(".split(") {
        return None;
    }
    let open = ".split(".len() - 1;
    let close = match_paren(s, open)?;
    let from = literal_single(&parse_args(&s[open + 1..close]))?;
    let rest2 = s[close + 1..].trim_start();
    if !rest2.starts_with(".join(") {
        return None;
    }
    let open2 = ".join(".len() - 1;
    let close2 = match_paren(rest2, open2)?;
    let to = literal_single(&parse_args(&rest2[open2 + 1..close2]))?;
    if !rest2[close2 + 1..].trim().is_empty() {
        return None;
    }
    Some(value.replace(&from, &to))
}

fn literal_single(args: &[Arg]) -> Option<String> {
    if args.len() != 1 {
        return None;
    }
    let mut pos = 0usize;
    let d = parse_string_expression(&args[0].expr, &mut pos)?;
    (pos == args[0].expr.len() && !d.interpolated).then(|| d.value)
}

fn skip_string(src: &str, pos: &mut usize) {
    let mut consumed = 0usize;
    if parse_string_expression(&src[*pos..], &mut consumed).is_some() {
        *pos += consumed;
    } else {
        *pos = next_char(src, *pos);
    }
}

/// Whether a skip recorded at a case's own call site means the case's inputs
/// are unfaithful; such cases are suppressed (recorded as skips only) — a
/// case with wrong inputs would poison the differential run.
fn suppresses_case(reason: &str) -> bool {
    reason == "interpolated_source"
        || reason == "options_custom"
        || reason == "featureset_custom"
        || reason.starts_with("companion_")
        || reason.starts_with("setup_")
        || reason.starts_with("unmodeled_workspace_setup:")
}

fn extract_declaration(
    code_prefix: &str,
    parse_prefix: &str,
    rel: &str,
    content: &str,
    decl: &Declaration,
    decls: &[Declaration],
    out: &mut FileExtraction,
) {
    let mut env = base_env(content, decl, decls);
    let ctx = build_ctx(content, decl, decls);
    let (chain, _) = class_chain(decl, decls);
    let sources = application_order(&chain, decls);
    // First pass: package registrations only. The official harness is
    // order-independent (companion files land on disk before the context
    // starts), while the call interpretation is sequential — a package's
    // options file may be written before its package-config entry.
    let mut scratch = env.clone();
    for source in &sources {
        let Some(set_up) = source.members.iter().find(|m| m.name == "setUp") else {
            continue;
        };
        let Some((start, end)) = set_up.body else {
            continue;
        };
        let body = &content[start..end];
        let saved = scratch.locals.len();
        scratch.locals = scan_local_strings(body, &scratch);
        interpret_calls(body, &mut scratch, &ctx, 0, &mut |_| {});
        scratch.locals.truncate(saved);
    }
    env.packages = scratch.packages;
    env.aux_packages = scratch.aux_packages;
    env.package_roots = scratch.package_roots;
    // The setUp chain proper, base first: the official `super.setUp()`
    // sequence, with skips recorded against the class.
    for source in &sources {
        let Some(set_up) = source.members.iter().find(|m| m.name == "setUp") else {
            continue;
        };
        let Some((start, end)) = set_up.body else {
            continue;
        };
        let body = &content[start..end];
        let saved = env.locals.len();
        env.locals = scan_local_strings(body, &env);
        interpret_calls(body, &mut env, &ctx, 0, &mut |reason| {
            out.skips.push(Skip {
                id: format!("{code_prefix}/{}", decl.name),
                reason,
            })
        });
        env.locals.truncate(saved);
    }
    for member in &decl.members {
        let Some((start, end)) = member.body else {
            continue;
        };
        if member.name == "setUp" {
            continue;
        }
        extract_method(
            code_prefix,
            parse_prefix,
            rel,
            &decl.name,
            &member.name,
            &content[start..end],
            &env,
            &ctx,
            out,
        );
    }
}

/// Extracts one method's cases: the calls before each target call shape that
/// target's environment. `resolveFilesWithDiagnostics` carries a
/// `Map<File, String>` whose entries become the case's file sections (the
/// first entry is the main file); every other target carries the snippet as
/// its first positional argument.
fn extract_method(
    code_prefix: &str,
    parse_prefix: &str,
    rel: &str,
    class_name: &str,
    method_name: &str,
    body: &str,
    class_env: &Env,
    ctx: &InterpCtx<'_>,
    out: &mut FileExtraction,
) {
    let locals = scan_local_strings(body, class_env);
    let mut index = 0usize;
    let mut p = 0usize;
    while let Some((name, open, _)) = find_call(body, p, &|n| {
        TARGET_CALLS.contains(&n) || UNMODELED_TARGET_CALLS.contains(&n)
    }) {
        p = open + 1;
        if UNMODELED_TARGET_CALLS.contains(&name.as_str()) {
            out.skips.push(Skip {
                id: format!("{code_prefix}/{class_name}/{method_name}"),
                reason: format!("unmodeled_target_call:{name}"),
            });
            continue;
        }
        let prefix = if name == "parseTestCodeWithDiagnostics" {
            parse_prefix
        } else {
            code_prefix
        };
        let id = format!("{prefix}/{class_name}/{method_name}#{index}");
        index += 1;
        let Some(close) = match_paren(body, open) else {
            continue;
        };
        let args = parse_args(&body[open + 1..close]);
        let mut env = class_env.clone();
        env.locals = locals.clone();

        if name == "resolveFilesWithDiagnostics" {
            let mut site_skips: Vec<String> = Vec::new();
            interpret_calls(&body[..open], &mut env, ctx, 0, &mut |reason| {
                site_skips.push(reason)
            });
            for reason in &site_skips {
                out.skips.push(Skip {
                    id: id.clone(),
                    reason: reason.clone(),
                });
            }
            if site_skips.iter().any(|reason| suppresses_case(reason)) {
                continue;
            }
            match files_map_sections(args.first().map(|a| a.expr.as_str()), &env) {
                Ok(sections) => {
                    let mut files: Vec<(String, String)> = sections;
                    for (path, content) in &env.files {
                        if !files.iter().any(|(p, _)| p == path) {
                            files.push((path.clone(), content.clone()));
                        }
                    }
                    add_placeholder_sections(&mut files, &env);
                    out.cases.push(Case {
                        id,
                        source: format!("{rel}#{class_name}.{method_name}"),
                        language: env.language.clone(),
                        packages: env.packages.clone(),
                        aux_packages: env.aux_packages.clone(),
                        options: Some(env.options.clone()),
                        ignores: Vec::new(),
                        files,
                    });
                }
                Err(reason) => out.skips.push(Skip {
                    id,
                    reason,
                }),
            }
            continue;
        }

        // The official parse harness defaults its `featureSet` to
        // `experimentsForTests`; an explicit override is a different
        // environment than the modeled one.
        if name == "parseTestCodeWithDiagnostics"
            && args.iter().any(|a| a.label.as_deref() == Some("featureSet"))
        {
            out.skips.push(Skip {
                id,
                reason: "featureset_custom".into(),
            });
            continue;
        }

        let Some(source) = args.first().and_then(|a| source_arg(&a.expr, &env)) else {
            out.skips.push(Skip {
                id,
                reason: "interpolated_source".into(),
            });
            continue;
        };

        let mut site_skips: Vec<String> = Vec::new();
        interpret_calls(&body[..open], &mut env, ctx, 0, &mut |reason| {
            site_skips.push(reason)
        });
        if name == "assertTestCodeWithStrictCastsDiagnostics" {
            env.options = strict_casts_options(&env.experiments);
        }
        for reason in &site_skips {
            out.skips.push(Skip {
                id: id.clone(),
                reason: reason.clone(),
            });
        }
        if site_skips.iter().any(|reason| suppresses_case(reason)) {
            continue;
        }

        let mut files = vec![("lib/test.dart".to_string(), source)];
        files.extend(env.files.iter().cloned());
        add_placeholder_sections(&mut files, &env);
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

/// The `Map<File, String>` entries of a `resolveFilesWithDiagnostics` call,
/// as case file sections in map order (the first entry becomes the main
/// file). Keys are identifiers bound by `getFile`/`newFile`; values are the
/// marked sources. An unmodeled key or value names its skip.
fn files_map_sections(expr: Option<&str>, env: &Env) -> Result<Vec<(String, String)>, String> {
    let Some(expr) = expr else {
        return Err("companion_path".into());
    };
    let expr = expr.trim();
    let Some(inner) = expr.strip_prefix('{').and_then(|_| {
        let close = match_brace(expr, 0)?;
        expr[close + 1..].trim().is_empty().then(|| &expr[1..close])
    }) else {
        return Err("companion_path".into());
    };
    let mut out = Vec::new();
    for entry in parse_args(inner) {
        let Some(key) = entry.label.as_deref() else {
            return Err("companion_path".into());
        };
        let Some((_, section)) = env.file_sections.iter().find(|(n, _)| n == key) else {
            return Err("companion_path".into());
        };
        let Some(content) = source_arg(&entry.expr, env) else {
            return Err("interpolated_source".into());
        };
        out.push((section.clone(), content));
    }
    if out.is_empty() {
        return Err("companion_path".into());
    }
    Ok(out)
}

/// A config-registered auxiliary package with no file sections still needs
/// its package-config entry at materialization; an empty marker file inside
/// the package root provides the registering section (the official harness's
/// registered roots may simply be empty folders).
fn add_placeholder_sections(files: &mut Vec<(String, String)>, env: &Env) {
    for name in &env.aux_packages {
        let prefix = format!("package:{name}/");
        if files.iter().any(|(path, _)| path.starts_with(&prefix)) {
            continue;
        }
        if setup::MOCK_PACKAGES.contains(&name.as_str()) {
            continue;
        }
        files.push((format!("{prefix}.package_root"), String::new()));
    }
}

/// Extracts the front_end `parser_testcases/` corpus: one source-only case
/// per `.dart` file, `*.crash_dart` included (the runner's crash
/// bookkeeping covers them). The official parser suite runs every file with
/// the `--enable-experiment` flags of its `folder.options` chain, merged
/// ancestor-first over the default experiment set; no markers are handled —
/// the oracle rows are the expectations.
pub fn extract_front_end_parser(root: &Path) -> DirExtraction {
    let mut out = DirExtraction::default();
    let mut files = Vec::new();
    collect_dir_files(root, "", &mut files);
    files.sort();
    for rel in &files {
        out.scanned += 1;
        let id = rel.strip_suffix(".dart").unwrap_or(rel);
        let content = match std::fs::read_to_string(root.join(rel)) {
            Ok(text) => text,
            Err(_) => {
                out.skips.push(Skip {
                    id: format!("parser-fe/{id}"),
                    reason: "unreadable".into(),
                });
                continue;
            }
        };
        let experiments = folder_experiments(root, rel);
        let options = setup::analysis_options_content(&experiments, &[], false, false, false, &[], &[]);
        out.cases.push(Case {
            id: format!("parser-fe/{id}"),
            source: format!("pkg/front_end/parser_testcases/{rel}"),
            language: None,
            packages: Vec::new(),
            aux_packages: Vec::new(),
            options: Some(options),
            ignores: Vec::new(),
            files: vec![(format!("parser-fe/{rel}"), content)],
        });
    }
    out
}

/// The `--enable-experiment` values of the `folder.options` files along a
/// file's directory chain, root first, merged ancestor-first over the
/// default experiment set.
fn folder_experiments(root: &Path, rel: &str) -> Vec<String> {
    let mut experiments: Vec<String> = setup::DEFAULT_EXPERIMENTS
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut dir = PathBuf::from("");
    let chain = rel.split('/').collect::<Vec<_>>();
    for part in chain.iter().take(chain.len().saturating_sub(1)) {
        dir = dir.join(part);
        let Ok(text) = std::fs::read_to_string(root.join(&dir).join("folder.options")) else {
            continue;
        };
        for word in text.split_whitespace() {
            let Some(list) = word.strip_prefix("--enable-experiment=") else {
                continue;
            };
            for flag in list.split(',').map(str::trim).filter(|f| !f.is_empty()) {
                if !experiments.iter().any(|e| e == flag) {
                    experiments.push(flag.to_string());
                }
            }
        }
    }
    experiments
}

fn collect_dir_files(root: &Path, rel: &str, out: &mut Vec<String>) {
    let dir = if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let child = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if entry.path().is_dir() {
            collect_dir_files(root, &child, out);
        } else if name.ends_with(".dart") || name.ends_with(".crash_dart") {
            out.push(child);
        }
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

  test_localSource() async {
    var code = 'var x = 1;';
    await resolveTestCodeWithDiagnostics(code);
  }

  test_interpolatedSource() async {
    const String kPrivateName = '_privateName';
    await resolveTestCodeWithDiagnostics('''
const String $kPrivateName = 'Name';
''');
  }

  test_unknownInterpolation() async {
    await resolveTestCodeWithDiagnostics('''
var x = $noSuchName;
''');
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
    fn extracts_cases_with_setup() {
        let out = extract_file("src/diagnostics/foo_test.dart", FILE);
        assert_eq!(
            out.cases.len(),
            4,
            "cases: {:?}",
            out.cases.iter().map(|c| &c.id).collect::<Vec<_>>()
        );
        let first = &out.cases[0];
        assert_eq!(first.id, "diagnostics/foo/FooTest/test_companion#0");
        assert_eq!(first.language.as_deref(), Some("2.19"));
        assert_eq!(first.packages, vec!["meta"]);
        assert_eq!(first.aux_packages, vec!["aaa"]);
        assert_eq!(first.files.len(), 2);
        assert_eq!(first.files[0].0, "lib/test.dart");
        assert_eq!(first.files[1].0, "package:aaa/lib/a.dart");
        assert_eq!(first.files[1].1, "class A {}\n");
        assert!(
            first
                .options
                .as_ref()
                .expect("options")
                .contains("propagate-linter-exceptions")
        );

        let config = &out.cases[3];
        assert_eq!(config.id, "diagnostics/foo/FooTest/test_config#0");
        assert_eq!(config.language.as_deref(), Some("2.17"));
        assert!(
            config
                .options
                .as_ref()
                .expect("options")
                .contains("- enhanced-parts")
        );
    }

    #[test]
    fn local_and_const_interpolation_are_substituted() {
        let out = extract_file("src/diagnostics/foo_test.dart", FILE);
        let local = &out.cases[1];
        assert_eq!(local.id, "diagnostics/foo/FooTest/test_localSource#0");
        assert_eq!(local.files[0].1, "var x = 1;");
        let interpolated = &out.cases[2];
        assert_eq!(
            interpolated.id,
            "diagnostics/foo/FooTest/test_interpolatedSource#0"
        );
        assert_eq!(
            interpolated.files[0].1,
            "\nconst String _privateName = 'Name';\n"
        );
    }

    #[test]
    fn unknown_interpolation_suppresses_the_case() {
        let out = extract_file("src/diagnostics/foo_test.dart", FILE);
        assert!(out.skips.contains(&Skip {
            id: "diagnostics/foo/FooTest/test_unknownInterpolation#0".into(),
            reason: "interpolated_source".into(),
        }));
        assert!(
            !out.cases
                .iter()
                .any(|c| c.id.ends_with("test_unknownInterpolation#0"))
        );
    }

    #[test]
    fn misc_files_land_under_misc() {
        let out = extract_file("src/dart/resolution/other_test.dart", FILE);
        assert!(
            out.cases[0]
                .id
                .starts_with("diagnostics/_misc/src/dart/resolution/other/")
        );
    }

    /// Inherited getters and inherited `setUp` effects (`_TestBase` shape).
    const INHERIT_FILE: &str = r#"
@reflectiveTest
class SubTest extends _TestBase {
  test_aaa() async {
    newFile('$aaaPackageRootPath/lib/a.dart', 'class A {}');
    await resolveTestCodeWithDiagnostics('''
import 'package:aaa/a.dart';
''');
  }
}

class _TestBase extends PubPackageResolutionTest {
  String get aaaPackageRootPath => '$workspaceRootPath/aaa';

  @override
  void setUp() {
    super.setUp();
    writeTestPackageConfig(
      PackageConfigFileBuilder()
        ..add(name: 'aaa', rootFolder: getFolder(aaaPackageRootPath)),
      meta: true,
    );
  }
}
"#;

    #[test]
    fn inherited_setups_and_getters_apply() {
        let out = extract_file("src/diagnostics/inherit_test.dart", INHERIT_FILE);
        assert!(out.skips.is_empty(), "{:?}", out.skips);
        assert_eq!(out.cases.len(), 1);
        let case = &out.cases[0];
        assert_eq!(case.packages, vec!["meta"]);
        assert_eq!(case.aux_packages, vec!["aaa"]);
        assert_eq!(case.files[1].0, "package:aaa/lib/a.dart");
    }

    /// `join` / `convertPath` / bare local identifiers in companion paths.
    const LOCALS_FILE: &str = r#"
@reflectiveTest
class LocalsTest extends PubPackageResolutionTest {
  String get bbbPackageRootPath => '$workspaceRootPath/bbb';

  @override
  void setUp() {
    super.setUp();
    writeTestPackageConfig(
      PackageConfigFileBuilder()
        ..add(name: 'bbb', rootFolder: getFolder(bbbPackageRootPath)),
    );
  }

  test_join() async {
    newFile(join(testPackageLibPath, 'private_enum.dart'), 'enum E { x }');
    await resolveTestCodeWithDiagnostics("enum E { x }\n");
  }

  test_convertPath() async {
    String pathB = convertPath('$testPackageLibPath/b.dart');
    newFile(pathB, 'var b = 1;');
    await resolveTestCodeWithDiagnostics("var b = 1;\n");
  }

  test_localPath() async {
    var aaaFilePath = convertPath('$bbbPackageRootPath/lib/a.dart');
    newFile(aaaFilePath, 'class A {}');
    await resolveTestCodeWithDiagnostics("class A {}\n");
  }
}
"#;

    #[test]
    fn join_convert_path_and_bare_locals() {
        let out = extract_file("src/diagnostics/locals_test.dart", LOCALS_FILE);
        assert!(out.skips.is_empty(), "{:?}", out.skips);
        assert_eq!(out.cases.len(), 3);
        let join = &out.cases[0];
        assert_eq!(join.files[1].0, "lib/private_enum.dart");
        let convert = &out.cases[1];
        assert_eq!(convert.files[1].0, "lib/b.dart");
        let local = &out.cases[2];
        assert_eq!(local.files[1].0, "package:bbb/lib/a.dart");
        assert_eq!(local.aux_packages, vec!["bbb"]);
    }

    /// Block-bodied getters feed path interpolation.
    const BLOCK_GETTER_FILE: &str = r#"
@reflectiveTest
class GenTest extends PubPackageResolutionTest {
  String get generatedPath {
    return '$testPackageRootPath/.dart_tool/gen';
  }

  test_gen() async {
    newFile('$generatedPath/a.dart', 'class A {}');
    await resolveTestCodeWithDiagnostics("class A {}\n");
  }
}
"#;

    #[test]
    fn block_getter_initializer_is_used() {
        let out = extract_file("src/diagnostics/gen_test.dart", BLOCK_GETTER_FILE);
        assert!(out.skips.is_empty(), "{:?}", out.skips);
        assert_eq!(out.cases[0].files[1].0, ".dart_tool/gen/a.dart");
    }

    /// A helper method's modeled calls apply with bound arguments.
    const HELPER_FILE: &str = r#"
@reflectiveTest
class HelperTest extends PubPackageResolutionTest {
  test_version() async {
    _configureTestPackageLanguageVersion('2.17');
    await resolveTestCodeWithDiagnostics('var v = 1;');
  }

  void _configureTestPackageLanguageVersion(String versionStr) {
    writeTestPackageConfig(
      PackageConfigFileBuilder(),
      languageVersion: versionStr,
    );
  }
}
"#;

    #[test]
    fn helper_methods_bind_parameters() {
        let out = extract_file("src/diagnostics/helper_test.dart", HELPER_FILE);
        assert!(out.skips.is_empty(), "{:?}", out.skips);
        assert_eq!(out.cases[0].language.as_deref(), Some("2.17"));
    }

    /// `.split("\n").join("\r\n")` source post-processing.
    const SPLIT_JOIN_FILE: &str = r#"
@reflectiveTest
class SplitTest extends PubPackageResolutionTest {
  test_windows() async {
    await resolveTestCodeWithDiagnostics(
      r'''
main() {}
'''
          .split("\n")
          .join("\r\n"),
    );
  }
}
"#;

    #[test]
    fn split_join_transform_is_applied() {
        let out = extract_file("src/diagnostics/split_test.dart", SPLIT_JOIN_FILE);
        assert!(out.skips.is_empty(), "{:?}", out.skips);
        assert_eq!(out.cases[0].files[0].1, "\r\nmain() {}\r\n");
    }

    /// Workspace flavors keep their precise skip names and suppress the
    /// unfaithful case.
    const WORKSPACE_FILE: &str = r#"
@reflectiveTest
class Deprecated_A_BlazeWorkspaceTest extends PubPackageResolutionTest {
  test_dart() async {
    newFile('$workspaceRootPath/foo/bar/lib/a.dart', 'class A {}');
    await resolveTestCodeWithDiagnostics("class A {}\n");
  }
}

@reflectiveTest
class Deprecated_B_GnWorkspaceTest extends PubPackageResolutionTest {
  test_gn() async {
    newFile('$workspaceRootPath/aaa/lib/a.dart', 'class A {}');
    await resolveTestCodeWithDiagnostics("class A {}\n");
  }
}

@reflectiveTest
class Deprecated_C_PackageBuildWorkspaceTest extends PubPackageResolutionTest {
  String get testPackageGeneratedPath {
    return '$testPackageRootPath/.dart_tool/build/generated';
  }

  test_generated() async {
    newFile('$testPackageGeneratedPath/aaa/lib/a.dart', 'class A {}');
    await resolveTestCodeWithDiagnostics("class A {}\n");
  }
}
"#;

    #[test]
    fn workspace_flavors_name_their_skips() {
        let out = extract_file("src/diagnostics/ws_test.dart", WORKSPACE_FILE);
        let reasons: Vec<&str> = out.skips.iter().map(|s| s.reason.as_str()).collect();
        assert!(
            reasons.contains(&"companion_blaze_workspace"),
            "{reasons:?}"
        );
        assert!(reasons.contains(&"companion_gn_workspace"), "{reasons:?}");
        assert!(
            reasons.contains(&"companion_package_build_workspace"),
            "{reasons:?}"
        );
        assert_eq!(out.cases.len(), 0, "all three cases are suppressed");
    }

    /// An abstract path getter (mixin shape) is named distinctly.
    const ABSTRACT_FILE: &str = r#"
@reflectiveTest
class ConcreteTest extends PubPackageResolutionTest with SharedMixin {
}

mixin SharedMixin on ContextResolutionTest {
  String get implFilePath => '$testPackageLibPath/src/foo.dart';

  String get testPackageLibPath;

  test_x() async {
    newFile(implFilePath, 'class A {}');
    await resolveTestCodeWithDiagnostics("class A {}\n");
  }
}
"#;

    #[test]
    fn abstract_path_getter_is_named() {
        let out = extract_file("src/diagnostics/abs_test.dart", ABSTRACT_FILE);
        assert!(
            out.skips
                .iter()
                .any(|s| s.reason == "companion_abstract_path_getter"),
            "{:?}",
            out.skips
        );
        assert!(out.cases.is_empty());
    }

    /// An options file in a registered package root becomes a section.
    const OPTIONS_FILE: &str = r#"
@reflectiveTest
class OptTest extends PubPackageResolutionTest {
  String get fooPackageRootPath => '$workspaceRootPath/foo';

  @override
  void setUp() {
    super.setUp();
    newAnalysisOptionsYamlFile(
      fooPackageRootPath,
      analysisOptionsContent(experiments: experiments),
    );
    writeTestPackageConfig(
      PackageConfigFileBuilder()
        ..add(name: 'foo', rootFolder: getFolder(fooPackageRootPath)),
      meta: true,
    );
  }

  test_x() async {
    newFile('$fooPackageRootPath/lib/src/a.dart', 'class A {}');
    await resolveTestCodeWithDiagnostics("class A {}\n");
  }
}
"#;

    #[test]
    fn package_options_file_becomes_section() {
        let out = extract_file("src/diagnostics/opt_test.dart", OPTIONS_FILE);
        assert!(out.skips.is_empty(), "{:?}", out.skips);
        let case = &out.cases[0];
        assert_eq!(case.aux_packages, vec!["foo"]);
        assert!(
            case.files
                .iter()
                .any(|(p, _)| p == "package:foo/analysis_options.yaml")
        );
    }

    /// A registered auxiliary package without file sections gets a marker
    /// section so materialization can register the package.
    const PLACEHOLDER_FILE: &str = r#"
@reflectiveTest
class PlaceTest extends PubPackageResolutionTest {
  @override
  void setUp() {
    super.setUp();
    writeTestPackageConfig(
      PackageConfigFileBuilder()
        ..add(name: 'aaa', rootFolder: getFolder('$workspaceRootPath/aaa')),
    );
  }

  test_onlyOwnFile() async {
    newFile('$testPackageLibPath/lib2.dart', 'library a;');
    await resolveTestCodeWithDiagnostics("export 'lib2.dart';\n");
  }
}
"#;

    #[test]
    fn registered_fileless_aux_gets_marker_section() {
        let out = extract_file("src/diagnostics/place_test.dart", PLACEHOLDER_FILE);
        assert!(out.skips.is_empty(), "{:?}", out.skips);
        let case = &out.cases[0];
        assert_eq!(case.aux_packages, vec!["aaa"]);
        assert!(
            case.files
                .iter()
                .any(|(p, c)| p == "package:aaa/.package_root" && c.is_empty())
        );
    }

    /// The workspace-root override is honored: derived getters re-resolve.
    const OVERRIDE_FILE: &str = r#"
@reflectiveTest
class MyRootTest extends PubPackageResolutionTest {
  @override
  String get testPackageRootPath => '/home/my';

  test_x() async {
    newFile('$testPackageRootPath/other.dart', 'var x = 1;');
    await resolveTestCodeWithDiagnostics("var x = 1;\n");
  }
}
"#;

    #[test]
    fn root_override_remaps_sections() {
        let out = extract_file("src/diagnostics/root_test.dart", OVERRIDE_FILE);
        assert!(out.skips.is_empty(), "{:?}", out.skips);
        assert_eq!(out.cases[0].files[1].0, "other.dart");
    }

    /// `flutter: true` implies the meta package (official builder).
    #[test]
    fn flutter_implies_meta() {
        let src = r#"
@reflectiveTest
class FlutterTest extends PubPackageResolutionTest {
  test_x() async {
    writeTestPackageConfig(PackageConfigFileBuilder(), flutter: true);
    await resolveTestCodeWithDiagnostics('var f = 1;');
  }
}
"#;
        let out = extract_file("src/diagnostics/flutter_test.dart", src);
        assert!(out.skips.is_empty(), "{:?}", out.skips);
        assert_eq!(out.cases[0].packages, vec!["flutter", "meta"]);
    }

    /// The pinned micro-resolver base class layout (`FileResolutionTest`).
    #[test]
    fn file_resolution_base_table() {
        let src = r#"
@reflectiveTest
class FileResolverXTest extends FileResolutionTest {
  test_x() async {
    newFile('/workspace/dart/test/lib/a.dart', 'var a = 1;');
    await resolveTestCodeWithDiagnostics('var a = 1;');
  }
}
"#;
        let out = extract_file("src/dart/micro/x_test.dart", src);
        assert!(out.skips.is_empty(), "{:?}", out.skips);
        assert_eq!(out.cases[0].files[1].0, "lib/a.dart");
    }

    /// The parse-only harness lands in the `parser/` face: `src/diagnostics`
    /// files keep their bare stem, other files keep their path stem.
    #[test]
    fn parse_calls_route_to_the_parser_face() {
        let src = r#"
@reflectiveTest
class ErrorParserTest extends ParserDiagnosticsTest {
  test_x() {
    parseTestCodeWithDiagnostics(r'''
f(;
''');
  }
}

class ParserDiagnosticsTest {}
"#;
        let out = extract_file("generated/error_parser_test.dart", src);
        assert_eq!(out.cases.len(), 1, "{:?}", out.skips);
        assert_eq!(out.cases[0].id, "parser/generated/error_parser/ErrorParserTest/test_x#0");
        assert_eq!(out.cases[0].packages, Vec::<String>::new());

        let diag = extract_file("src/diagnostics/extraneous_modifier_test.dart", src);
        assert_eq!(
            diag.cases[0].id,
            "parser/extraneous_modifier/ErrorParserTest/test_x#0"
        );
    }

    /// An explicit `featureSet:` override is a different environment; the
    /// case is suppressed with a named skip.
    #[test]
    fn parse_feature_set_override_skips() {
        let src = r#"
class T extends ParserDiagnosticsTest {
  test_x() {
    parseTestCodeWithDiagnostics(
      'var x = 1;',
      featureSet: FeatureSets.forTesting('patterns'),
    );
  }
}

class ParserDiagnosticsTest {}
"#;
        let out = extract_file("generated/p_test.dart", src);
        assert!(out.cases.is_empty(), "{:?}", out.cases);
        assert_eq!(out.skips.len(), 1);
        assert_eq!(out.skips[0].reason, "featureset_custom");
    }

    /// Unmodeled target-shaped calls are recorded as named skips.
    #[test]
    fn unmodeled_target_calls_are_recorded() {
        let src = r#"
class T extends PubPackageResolutionTest {
  test_resolveFile() async {
    var a = getFile('$testPackageLibPath/a.dart');
    await resolveTestFile(a);
  }

  test_ignoringDiagnostics() {
    parseTestCodeIgnoringDiagnostics('var x = 1;');
  }
}
"#;
        let out = extract_file("src/dart/micro/u_test.dart", src);
        assert!(out.cases.is_empty(), "{:?}", out.cases);
        let reasons: Vec<&str> = out.skips.iter().map(|s| s.reason.as_str()).collect();
        assert!(reasons.contains(&"unmodeled_target_call:resolveTestFile"), "{reasons:?}");
        assert!(
            reasons.contains(&"unmodeled_target_call:parseTestCodeIgnoringDiagnostics"),
            "{reasons:?}"
        );
    }

    /// Workspace registration helpers the extractor does not model suppress
    /// the case with a per-helper named skip.
    #[test]
    fn workspace_registration_helpers_suppress() {
        let src = r#"
@reflectiveTest
class GnTest extends PubPackageResolutionTest {
  test_x() async {
    newBuildGnFile('$workspaceRootPath/aaa', '');
    newFile('$testPackageLibPath/a.dart', 'var a = 1;');
    await resolveTestCodeWithDiagnostics("import 'package:aaa/a.dart';\nvar a = 1;\n");
  }
}
"#;
        let out = extract_file("src/diagnostics/gn_test.dart", src);
        assert!(out.cases.is_empty(), "{:?}", out.cases);
        assert!(
            out.skips
                .iter()
                .any(|s| s.reason == "unmodeled_workspace_setup:newBuildGnFile"),
            "{:?}",
            out.skips
        );
    }

    /// `resolveFilesWithDiagnostics`: every map entry becomes a file
    /// section; the first entry is the main file; companions from `newFile`
    /// ride along.
    #[test]
    fn resolve_files_map_becomes_sections() {
        let src = r#"
@reflectiveTest
class LibTest extends PubPackageResolutionTest {
  test_inPart() async {
    var a = getFile('$testPackageLibPath/a.dart');
    var b = getFile('$testPackageLibPath/b.dart');
    newFile('$testPackageLibPath/c.dart', '');
    var results = await resolveFilesWithDiagnostics({
      a: r'''
part 'b.dart';
''',
      b: r'''
part of 'a.dart';
// [diag.unusedImport] x
''',
    });
  }
}
"#;
        let out = extract_file("src/dart/resolution/library_import_test.dart", src);
        assert_eq!(out.cases.len(), 1, "{:?}", out.skips);
        let case = &out.cases[0];
        assert_eq!(
            case.id,
            "diagnostics/_misc/src/dart/resolution/library_import/LibTest/test_inPart#0"
        );
        assert_eq!(case.files.len(), 3);
        assert_eq!(case.files[0].0, "lib/a.dart");
        assert!(case.files[0].1.contains("part 'b.dart';"));
        assert_eq!(case.files[1].0, "lib/b.dart");
        assert_eq!(case.files[2].0, "lib/c.dart");
    }

    /// The front_end parser_testcases corpus: whole-file source-only cases
    /// with the folder.options experiments merged over the defaults.
    #[test]
    fn front_end_parser_corpus_extracted_whole_file() {
        let dir = std::env::temp_dir().join(format!("conf-fep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("primary_constructors")).expect("creates");
        std::fs::write(dir.join("top.dart"), "void f() {}\n").expect("writes");
        std::fs::write(
            dir.join("primary_constructors/folder.options"),
            "--enable-experiment=primary-constructors\n",
        )
        .expect("writes options");
        std::fs::write(
            dir.join("primary_constructors/decl.crash_dart"),
            "class A {",
        )
        .expect("writes");
        let out = extract_front_end_parser(&dir);
        assert_eq!(out.scanned, 2, "{:?}", out.skips);
        assert_eq!(out.cases.len(), 2, "{:?}", out.skips);
        let mut ids: Vec<&str> = out.cases.iter().map(|c| c.id.as_str()).collect();
        ids.sort();
        assert_eq!(
            ids,
            vec![
                "parser-fe/primary_constructors/decl.crash_dart",
                "parser-fe/top"
            ],
            "{ids:?}"
        );
        let top = out.cases.iter().find(|c| c.id == "parser-fe/top").expect("top");
        assert_eq!(top.files[0].0, "parser-fe/top.dart");
        assert_eq!(top.files[0].1, "void f() {}\n");
        assert!(top.options.as_ref().expect("options").contains("augmentations"));
        let decl = out
            .cases
            .iter()
            .find(|c| c.id.ends_with("decl.crash_dart"))
            .expect("crash case");
        assert!(
            decl.options
                .as_ref()
                .expect("options")
                .contains("primary-constructors")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An unresolvable map key or a non-literal value suppresses the case.
    #[test]
    fn resolve_files_map_failures_suppress() {
        let src = r#"
@reflectiveTest
class BadTest extends PubPackageResolutionTest {
  test_unknownKey() async {
    var results = await resolveFilesWithDiagnostics({
      noSuchLocal: r'''
var x = 1;
''',
    });
  }

  test_interpolatedValue() async {
    var a = getFile('$testPackageLibPath/a.dart');
    var suffix = '.dart';
    var results = await resolveFilesWithDiagnostics({
      a: 'var x = 1;' + suffix,
    });
  }
}
"#;
        let out = extract_file("src/dart/resolution/bad_test.dart", src);
        assert!(out.cases.is_empty(), "{:?}", out.cases);
        let reasons: Vec<&str> = out.skips.iter().map(|s| s.reason.as_str()).collect();
        assert!(reasons.contains(&"companion_path"), "{reasons:?}");
        assert!(reasons.contains(&"interpolated_source"), "{reasons:?}");
    }
}
