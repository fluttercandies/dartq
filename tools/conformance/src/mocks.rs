//! Extraction of the official mock packages from the pinned SDK source: the
//! `MockLibraryUnit('lib/path.dart', r'''...''')` declarations under the
//! analyzer testing package's `src/mock_packages/` tree, composed into
//! packages exactly like the official `mock_packages.dart` `addX()` methods,
//! plus the `angular_meta` mock inlined in the resolution test setup and the
//! `test_api` fixture the linter's `use_test_throws_matchers` suite writes.
//!
//! The SDK-internal test packages that the language/co19 suites import
//! (`expect`, `async_helper`, `dart_internal`) and the `collection` fixture
//! package are emitted from the pin's `pkg/` sources. Files whose transitive
//! dependencies leave the analysis surface (`package:smith`, `dart:_internal`)
//! keep their real public shapes with inert bodies, so the analyzer resolves
//! exactly the API the suites import.

use crate::dart::{find_call, match_paren, parse_args, parse_string_expression};
use std::collections::BTreeMap;
use std::path::Path;

/// One materialized mock file: path within the mock package and content.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub struct MockFile {
    /// Package-relative path, e.g. `lib/ffi.dart`.
    pub path: String,
    pub content: String,
}

/// Extracts every mock package. `mock_src_root` is the SDK-side
/// `.../lib/src/mock_packages` directory; `context_collection` is the text of
/// the resolution test setup file that inlines the `angular_meta` mock;
/// `sdk_src` is the pinned SDK root providing `pkg/expect`, `pkg/async_helper`
/// and `pkg/dart_internal`.
/// Returns packages in name order, files in unit-list order.
pub fn extract(
    mock_src_root: &Path,
    context_collection: &str,
    sdk_src: &Path,
) -> std::io::Result<BTreeMap<String, Vec<MockFile>>> {
    let mut packages: BTreeMap<String, Vec<MockFile>> = BTreeMap::new();
    // Composition: package name -> source files whose `units` lists it merges
    // (the official `addX()` bodies, transcribed structurally below).
    for (package, sources) in composition(mock_src_root) {
        let mut files = Vec::new();
        for source in &sources {
            let text = std::fs::read_to_string(mock_src_root.join(source))?;
            files.extend(units_of(&text));
        }
        packages.insert(package, files);
    }
    if let Some(meta) = angular_meta(context_collection) {
        packages.insert("angular_meta".to_string(), vec![meta]);
    }
    packages.insert("test_api".to_string(), test_api());
    for (package, files) in sdk_packages(sdk_src) {
        packages.insert(package, files);
    }
    Ok(packages)
}

/// One SDK-side mock file: where the content comes from.
enum SdkFile {
    /// Copied verbatim from the pin.
    Verbatim(&'static str),
    /// The public shapes of the real file with bodies the analysis surface
    /// cannot carry (its real dependencies point outside standalone
    /// analysis: `package:smith`, `dart:_internal`, a non-vendored package).
    Shaped(&'static str),
}

/// The SDK-internal test packages the language/co19 suites import, built from
/// the pin's `pkg/` sources. Only the API surface the suites import is
/// emitted: whole verbatim files where their dependency closure stays inside
/// the package, shape-faithful stubs where it does not.
fn sdk_packages(sdk_src: &Path) -> Vec<(String, Vec<MockFile>)> {
    let entries: Vec<(String, &str, Vec<(&str, SdkFile)>)> = vec![
        (
            "expect".to_string(),
            "pkg/expect/lib",
            vec![
                ("lib/expect.dart", SdkFile::Verbatim("expect.dart")),
                (
                    "lib/static_type_helper.dart",
                    SdkFile::Verbatim("static_type_helper.dart"),
                ),
                (
                    "lib/async_helper.dart",
                    SdkFile::Verbatim("async_helper.dart"),
                ),
                ("lib/variations.dart", SdkFile::Verbatim("variations.dart")),
                ("lib/config.dart", SdkFile::Shaped(EXPECT_CONFIG_STUB)),
                (
                    "lib/legacy/async_minitest.dart",
                    SdkFile::Verbatim("legacy/async_minitest.dart"),
                ),
            ],
        ),
        (
            "async_helper".to_string(),
            "pkg/async_helper/lib",
            vec![
                (
                    "lib/async_helper.dart",
                    SdkFile::Verbatim("async_helper.dart"),
                ),
                (
                    "lib/async_minitest.dart",
                    SdkFile::Verbatim("async_minitest.dart"),
                ),
            ],
        ),
        (
            "dart_internal".to_string(),
            "pkg/dart_internal/lib",
            vec![(
                "lib/extract_type_arguments.dart",
                SdkFile::Shaped(DART_INTERNAL_STUB),
            )],
        ),
        (
            "collection".to_string(),
            "pkg/collection/lib",
            vec![(
                "lib/equality.dart",
                SdkFile::Shaped(COLLECTION_EQUALITY_STUB),
            )],
        ),
    ];
    let mut out = Vec::new();
    for (package, base, files) in entries {
        let mut emitted = Vec::new();
        for (path, source) in files {
            let content = match source {
                SdkFile::Verbatim(rel) => {
                    let text = std::fs::read_to_string(sdk_src.join(base).join(rel));
                    match text {
                        Ok(text) => text,
                        Err(e) => {
                            eprintln!("warn: cannot read sdk mock {package}/{rel}: {e}");
                            continue;
                        }
                    }
                }
                SdkFile::Shaped(text) => (*text).to_string(),
            };
            emitted.push(MockFile {
                path: path.to_string(),
                content,
            });
        }
        out.push((package, emitted));
    }
    out
}

/// `pkg/expect/lib/config.dart` with the `package:smith` dependency replaced
/// by inert values: the real file queries the test-runner configuration,
/// which static analysis never observes. Public names are the real ones.
const EXPECT_CONFIG_STUB: &str = r#"
// Mock of pkg/expect/lib/config.dart for standalone analysis: the real
// file parses the test-runner configuration through package:smith, which
// is outside the analysis surface. Public names and types are the real
// ones; values are inert.
library expect_config;

final String configAsString = '';

bool get isDart2jsConfiguration => false;

bool get isDart2WasmConfiguration => false;

bool get isDdcConfiguration => false;

bool get isVmJitConfiguration => false;

bool get isVmAotConfiguration => false;

bool get isVmConfiguration => isVmJitConfiguration || isVmAotConfiguration;

bool get isBrowserConfiguration => false;

bool get isWebConfiguration =>
    isDart2jsConfiguration || isDart2WasmConfiguration || isDdcConfiguration;
"#;

/// `pkg/dart_internal/lib/extract_type_arguments.dart` with the
/// `dart:_internal` forwarding bodies replaced: only that package's own
/// files may import the internal library, and standalone analysis cases
/// cannot. Signatures mirror the real public surface verbatim.
const DART_INTERNAL_STUB: &str = r#"
// Mock of pkg/dart_internal/lib/extract_type_arguments.dart for standalone
// analysis: the real bodies forward to `dart:_internal`, which only that
// package may import. Signatures are the real public ones.
Object? extractIterableTypeArgument(
        Iterable iterable, Object? Function<T>() extract) =>
    null;

Object? extractMapTypeArguments(Map map, Object? Function<K, V>() extract) =>
    null;
"#;

/// The `package:collection/equality.dart` shapes the language suite imports
/// (`regress14348_test.dart`): `Equality`, `UnorderedIterableEquality`,
/// `MapEquality`. The package is not vendored in the SDK tree at this pin,
/// so the imported surface is mirrored here.
const COLLECTION_EQUALITY_STUB: &str = r#"
// Mock of the upstream package:collection equality.dart surface the
// language suite imports; the package is not vendored at this pin.
abstract class Equality<T> {
  const Equality();

  bool equals(T? e1, T? e2);

  int hash(T? e);
}

class DefaultEquality<V> extends Equality<V> {
  const DefaultEquality();

  @override
  bool equals(V? e1, V? e2) => e1 == e2;

  @override
  int hash(V? e) => e.hashCode;
}

class UnorderedIterableEquality<E> extends Equality<Iterable<E>> {
  const UnorderedIterableEquality();

  @override
  bool equals(Iterable<E>? elements1, Iterable<E>? elements2) => true;

  @override
  int hash(Iterable<E>? elements) => 0;
}

class MapEquality<K, V> extends Equality<Map<K, V>> {
  final Equality<V> values;

  const MapEquality({this.values = const DefaultEquality<V>()});

  @override
  bool equals(Map<K, V>? map1, Map<K, V>? map2) => true;

  @override
  int hash(Map<K, V>? map) => 0;
}
"#;

/// The `package:test_api` fixture the linter's `use_test_throws_matchers`
/// suite writes into its environment (upstream
/// `pkg/linter/test/rules/use_test_throws_matchers_test.dart` `setUp`,
/// registering the package through `PackageConfigFileBuilder` and writing
/// these two files). The signatures mirror the real package:test_api
/// frontend API (`expect(actual, matcher)`, `fail` returning `Never`); the
/// `lib/src/frontend/expect.dart` layout is load-bearing — the rule resolves
/// `fail` and asserts its declaring library URI is exactly
/// `package:test_api/src/frontend/expect.dart`.
const TEST_API_EXPECT: &str = r#"
// Mock of the package:test_api frontend surface the use_test_throws_matchers
// suite registers (upstream use_test_throws_matchers_test.dart setUp); the
// signatures are the real public ones.
void expect(dynamic actual, dynamic matcher) {}

Never fail(String message) {}
"#;

const TEST_API_EXPORT: &str = r#"
export 'src/frontend/expect.dart';
"#;

fn test_api() -> Vec<MockFile> {
    vec![
        MockFile {
            path: "lib/src/frontend/expect.dart".to_string(),
            content: TEST_API_EXPECT.to_string(),
        },
        MockFile {
            path: "lib/test_api.dart".to_string(),
            content: TEST_API_EXPORT.to_string(),
        },
    ]
}

/// The package composition: which source files under `mock_packages/` make up
/// which package (the official `addFfi`/`addFlutter`/... members).
fn composition(root: &std::path::Path) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    let mut dirs: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    dirs.sort();
    for dir in dirs {
        let package = match dir.as_str() {
            // `flutter_localizations` ships inside the flutter package (the
            // official `addFlutter` merges it).
            "flutter_localizations" => "flutter".to_string(),
            other => other.to_string(),
        };
        let mut sources: Vec<String> = Vec::new();
        collect_dart(&root.join(&dir), &dir, &mut sources);
        sources.sort();
        match out
            .iter_mut()
            .find(|(name, _): &&mut (String, Vec<String>)| *name == package)
        {
            Some((_, files)) => files.append(&mut sources),
            None => out.push((package, sources)),
        }
    }
    out
}

fn collect_dart(dir: &std::path::Path, rel: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let rel_child = format!("{rel}/{name}");
        if path.is_dir() {
            collect_dart(&path, &rel_child, out);
        } else if name.ends_with(".dart") && name != "mock_library.dart" {
            out.push(rel_child);
        }
    }
}

/// The `MockLibraryUnit` list of one source file, in declaration order,
/// resolving cross-file unit references through `units_of` of the same tree.
fn units_of(text: &str) -> Vec<MockFile> {
    let mut declared: Vec<(String, MockFile)> = Vec::new();
    let mut pos = 0usize;
    while let Some((_, open, _)) = find_call(text, pos, &|n| n == "MockLibraryUnit") {
        pos = open + 1;
        let Some(close) = match_paren(text, open) else {
            break;
        };
        let args = parse_args(&text[open + 1..close]);
        let (Some(path_arg), Some(content_arg)) = (args.first(), args.get(1)) else {
            continue;
        };
        let Some(path) = plain_string(&path_arg.expr) else {
            continue;
        };
        let mut consumed = 0usize;
        let Some(content) = parse_string_expression(&content_arg.expr, &mut consumed) else {
            continue;
        };
        // The constant name: `final [_]name = MockLibraryUnit(`.
        let name = text[..open]
            .rsplit('=')
            .nth(1)
            .and_then(|head| head.split_whitespace().last())
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }
        declared.push((
            name,
            MockFile {
                path,
                content: content.value,
            },
        ));
    }
    // The `units` list orders the package's files.
    let mut order: Vec<MockFile> = Vec::new();
    if let Some(at) = text.find("units = [") {
        let list = &text[at + "units = [".len()..];
        let end = list.find(']').unwrap_or(list.len());
        for item in crate::dart::split_args(&list[..end]) {
            let name = item.trim().trim_start_matches("...").trim();
            if let Some((_, file)) = declared.iter().find(|(n, _)| n == name) {
                order.push(file.clone());
            }
        }
    }
    if order.is_empty() {
        order = declared.into_iter().map(|(_, f)| f).collect();
    }
    order
}

fn plain_string(expr: &str) -> Option<String> {
    let mut pos = 0usize;
    let d = parse_string_expression(expr, &mut pos)?;
    if d.interpolated || pos != expr.len() {
        return None;
    }
    Some(d.value)
}

/// The `angular_meta` mock, inlined as a `newFile(..., r'''...''')` literal
/// in the resolution test setup.
fn angular_meta(context_collection: &str) -> Option<MockFile> {
    let mut pos = 0usize;
    while let Some((_, open, _)) = find_call(context_collection, pos, &|n| n == "newFile") {
        pos = open + 1;
        let Some(close) = match_paren(context_collection, open) else {
            continue;
        };
        let args = parse_args(&context_collection[open + 1..close]);
        // The real path argument interpolates `$angularMetaRootPath`.
        if !args
            .first()
            .map_or(false, |a| a.expr.contains("angular_meta.dart"))
        {
            continue;
        }
        let content = args.get(1).and_then(|a| plain_string(&a.expr))?;
        return Some(MockFile {
            path: "lib/angular_meta.dart".to_string(),
            content,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_fixture_mirrors_the_upstream_setUp_files() {
        let files = test_api();
        assert_eq!(files.len(), 2);
        let expect = files
            .iter()
            .find(|f| f.path == "lib/src/frontend/expect.dart")
            .expect("expect.dart");
        // The signatures the analyzed snippets call, verbatim from the
        // upstream setUp literal.
        assert!(
            expect
                .content
                .contains("void expect(dynamic actual, dynamic matcher) {}")
        );
        assert!(expect.content.contains("Never fail(String message) {}"));
        let export = files
            .iter()
            .find(|f| f.path == "lib/test_api.dart")
            .expect("test_api.dart");
        assert_eq!(export.content, "\nexport 'src/frontend/expect.dart';\n");
    }

    #[test]
    fn parses_unit_declarations() {
        let src = "final List<MockLibraryUnit> units = [_aUnit, bLibrary];\n\nfinal _aUnit = MockLibraryUnit('lib/a.dart', r'''\nclass A {}\n''');\n\nfinal bLibrary = MockLibraryUnit(\n  'lib/b.dart',\n  '''class B {}''',\n);\n";
        let units = units_of(src);
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].path, "lib/a.dart");
        assert_eq!(units[0].content, "\nclass A {}\n");
        assert_eq!(units[1].path, "lib/b.dart");
        assert_eq!(units[1].content, "class B {}");
    }

    #[test]
    fn angular_meta_extraction() {
        let src = "void writeTestPackageConfig() {\n  newFile('$angularMetaRootPath/lib/angular_meta.dart', r'''\nlibrary angular.meta;\n\nconst x = 1;\n''');\n}\n";
        let meta = angular_meta(src).expect("finds the mock");
        assert_eq!(meta.path, "lib/angular_meta.dart");
        assert!(meta.content.contains("library angular.meta;"));
    }

    #[test]
    fn sdk_packages_copy_verbatim_and_shape_files() {
        let sdk = std::env::temp_dir().join(format!("conf-mocks-sdk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&sdk);
        std::fs::create_dir_all(sdk.join("pkg/expect/lib/legacy")).expect("creates");
        std::fs::create_dir_all(sdk.join("pkg/async_helper/lib")).expect("creates");
        std::fs::write(sdk.join("pkg/expect/lib/expect.dart"), "class Expect {}\n")
            .expect("writes");
        std::fs::write(
            sdk.join("pkg/expect/lib/static_type_helper.dart"),
            "var h = 1;\n",
        )
        .expect("writes");
        std::fs::write(sdk.join("pkg/expect/lib/async_helper.dart"), "var a = 1;\n")
            .expect("writes");
        std::fs::write(sdk.join("pkg/expect/lib/variations.dart"), "const v = 1;\n")
            .expect("writes");
        std::fs::write(
            sdk.join("pkg/expect/lib/legacy/async_minitest.dart"),
            "var m = 1;\n",
        )
        .expect("writes");
        std::fs::write(
            sdk.join("pkg/async_helper/lib/async_helper.dart"),
            "export 'package:expect/async_helper.dart';\n",
        )
        .expect("writes");
        std::fs::write(
            sdk.join("pkg/async_helper/lib/async_minitest.dart"),
            "export 'x';\n",
        )
        .expect("writes");
        let packages = sdk_packages(&sdk);
        let by_name = |name: &str| {
            packages
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, f)| f)
                .expect("package present")
        };
        let expect = by_name("expect");
        assert_eq!(expect.len(), 6);
        assert!(
            expect
                .iter()
                .any(|f| f.path == "lib/expect.dart" && f.content == "class Expect {}\n")
        );
        // The smith-dependent file keeps its real public names.
        let config = expect
            .iter()
            .find(|f| f.path == "lib/config.dart")
            .expect("config");
        assert!(config.content.contains("isVmConfiguration"));
        assert!(!config.content.contains("import 'package:smith"));
        // The async_helper package re-exports the expect package, which every
        // features case declares.
        let async_helper = by_name("async_helper");
        assert!(
            async_helper
                .iter()
                .any(|f| f.content.contains("package:expect/async_helper.dart"))
        );
        let dart_internal = by_name("dart_internal");
        assert!(
            dart_internal[0]
                .content
                .contains("extractIterableTypeArgument")
        );
        let collection = by_name("collection");
        assert!(collection[0].content.contains("UnorderedIterableEquality"));
        let _ = std::fs::remove_dir_all(&sdk);
    }
}
