//! Generator self-tests: the extraction rules pinned on miniature SDK-source
//! shapes (the offline twin of the corpus pin tests).

use std::collections::BTreeSet;

use super::scan::{FlatEntry, parse_flat_table, scan_unit};

fn names(source: &str) -> BTreeSet<String> {
    scan_unit(source).names
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

#[test]
fn flat_table_entries_parse_in_source_order() {
    let table = parse_flat_table(
        r#"// docs mentioning LibraryInfo and 'core' elsewhere
const Map<String, LibraryInfo> libraries = const {
  'async': const LibraryInfo(
    'async/async.dart',
    categories: 'Client,Server',
  ),
  'html': const LibraryInfo(
    'html/dart2js/html_dart2js.dart',
    platforms: DART2JS_PLATFORM,
  ),
  "_wasm": const LibraryInfo(
    '_wasm/wasm_types.dart',
    documented: false,
  ),
};
"#,
    )
    .expect("parses");
    assert_eq!(
        table,
        vec![
            FlatEntry {
                name: "async".to_owned(),
                path: "async/async.dart".to_owned(),
                dart2js_path: None,
            },
            FlatEntry {
                name: "html".to_owned(),
                path: "html/dart2js/html_dart2js.dart".to_owned(),
                dart2js_path: None,
            },
            FlatEntry {
                name: "_wasm".to_owned(),
                path: "_wasm/wasm_types.dart".to_owned(),
                dart2js_path: None,
            },
        ]
    );
}

#[test]
fn class_mixin_enum_extension_typedef_families() {
    let face = names(
        r#"abstract class Foo {}
abstract final class Bar<T> extends Baz {}
base mixin class MixinClass {}
mixin PlainMixin {}
sealed class Sealed {}
enum Color { red, green }
extension StringExt on String {}
extension type IdNumber(int i) {}
extension type const ConstExt(int i) {}
typedef Name = void Function();
typedef Compare<T> = int Function(T a, T b);
typedef int OldStyle(Object a);
"#,
    );
    assert_eq!(
        face,
        set(&[
            "Bar",
            "Compare",
            "Color",
            "ConstExt",
            "Foo",
            "IdNumber",
            "MixinClass",
            "Name",
            "OldStyle",
            "PlainMixin",
            "Sealed",
            "StringExt",
        ])
    );
}

#[test]
fn functions_getters_setters_and_variables() {
    let face = names(
        r#"void print(Object? object) {}
external void exit(int code);
Future<int> spawn<T extends void Function()>(T entry) async => throw 0;
void Function() handler() => throw 0;
int get zero => 0;
external JsObject get context;
set exitCode(int value) {}
const Base64Codec base64 = Base64Codec();
const String ascii = 'ascii';
final List<int> items = [];
late final String lazy;
var bare = 3;
external final int wired;
const Duration _private = Duration();
"#,
    );
    assert_eq!(
        face,
        set(&[
            "print",
            "exit",
            "spawn",
            "handler",
            "zero",
            "context",
            "exitCode=",
            "base64",
            "ascii",
            "items",
            "lazy",
            "bare",
            "wired",
        ])
    );
}

#[test]
fn string_uris_survive_comment_stripping_and_privates_drop() {
    let face = names(
        r#"/// A doc comment mentioning `class Ghost` at column zero.
const String url = 'https://example.com/a//b'; // trailing comment
final String _hidden = 'x';
/* a block comment
   class Commented extends Ghost {} */
class Real {}
"#,
    );
    assert_eq!(face, set(&["url", "Real"]));
}

#[test]
fn exports_parts_and_unclassified_lines() {
    let scan = scan_unit(
        r#"library dart.fake;
import 'dart:core';
export 'dart:js_util' show allowInterop, allowInteropCaptureThis;
export "dart:async"
    show
        FutureIterable,
        FutureRecord2;
export 'dart:_metadata';
export 'dart:math' hide zeta, alpha;
part 'src/piece.dart';
part of dart.other;
class C {}
"#,
    );
    assert_eq!(scan.parts, vec!["src/piece.dart".to_owned()]);
    assert_eq!(scan.exports.len(), 4);
    let first = &scan.exports[0];
    assert_eq!(first.target, "dart:js_util");
    assert_eq!(
        first.show.as_ref().expect("show list"),
        &[
            "allowInterop".to_owned(),
            "allowInteropCaptureThis".to_owned()
        ]
    );
    let second = &scan.exports[1];
    assert_eq!(
        second.show.as_ref().expect("show list"),
        &["FutureIterable".to_owned(), "FutureRecord2".to_owned()]
    );
    let wildcard = &scan.exports[2];
    assert_eq!(wildcard.target, "dart:_metadata");
    assert!(wildcard.show.is_none() && wildcard.hide.is_empty());
    let hidden = &scan.exports[3];
    assert!(hidden.show.is_none());
    assert_eq!(hidden.hide, vec!["zeta".to_owned(), "alpha".to_owned()]);
    assert_eq!(scan.names, set(&["C"]));

    // A column-0 line no rule classifies surfaces as a warning, never a
    // guessed name.
    let scan = scan_unit("what even is this {}\nclass Real {}\n");
    assert_eq!(scan.names, set(&["Real"]));
    assert_eq!(scan.warnings, vec!["what even is this {}".to_owned()]);
}

#[test]
fn dart2js_variant_paths_and_bare_variables() {
    let table = parse_flat_table(
        r#"
const Map<String, LibraryInfo> libraries = const {
  'html_common': const LibraryInfo(
    'html/html_common/html_common.dart',
    categories: 'Client',
    dart2jsPath: 'html/html_common/html_common_dart2js.dart',
    documented: false,
  ),
};
"#,
    )
    .expect("parses");
    assert_eq!(
        table[0].face_path(),
        "html/html_common/html_common_dart2js.dart"
    );
    assert_eq!(table[0].path, "html/html_common/html_common.dart");

    // A keyword-less variable whose head does not end in an identifier is a
    // declaration-shaped line the rules do not claim — surfaced, never
    // guessed.
    let scan = scan_unit("Type name;\nType? other = null;\nwhat even is this {}\n");
    assert_eq!(scan.names, set(&["name", "other"]));
    assert_eq!(scan.warnings, vec!["what even is this {}".to_owned()]);
}
