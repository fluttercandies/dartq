//! Extraction of the official mock packages from the pinned SDK source: the
//! `MockLibraryUnit('lib/path.dart', r'''...''')` declarations under the
//! analyzer testing package's `src/mock_packages/` tree, composed into
//! packages exactly like the official `mock_packages.dart` `addX()` methods,
//! plus the `angular_meta` mock inlined in the resolution test setup.

use crate::dart::{find_call, match_paren, parse_args, parse_string_expression};
use std::collections::BTreeMap;

/// One materialized mock file: path within the mock package and content.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub struct MockFile {
    /// Package-relative path, e.g. `lib/ffi.dart`.
    pub path: String,
    pub content: String,
}

/// Extracts every mock package. `mock_src_root` is the SDK-side
/// `.../lib/src/mock_packages` directory; `context_collection` is the text of
/// the resolution test setup file that inlines the `angular_meta` mock.
/// Returns packages in name order, files in unit-list order.
pub fn extract(
    mock_src_root: &std::path::Path,
    context_collection: &str,
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
    Ok(packages)
}

/// The package composition: which source files under `mock_packages/` make up
/// which package (the official `addFfi`/`addFlutter`/... members).
fn composition(root: &std::path::Path) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else { return out };
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
        match out.iter_mut().find(|(name, _): &&mut (String, Vec<String>)| *name == package) {
            Some((_, files)) => files.append(&mut sources),
            None => out.push((package, sources)),
        }
    }
    out
}

fn collect_dart(dir: &std::path::Path, rel: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
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
    while let Some((_, open, _)) =
        find_call(text, pos, &|n| n == "MockLibraryUnit")
    {
        pos = open + 1;
        let Some(close) = match_paren(text, open) else { break };
        let args = parse_args(&text[open + 1..close]);
        let (Some(path_arg), Some(content_arg)) = (args.first(), args.get(1)) else { continue };
        let Some(path) = plain_string(&path_arg.expr) else { continue };
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
        declared.push((name, MockFile { path, content: content.value }));
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
        let Some(close) = match_paren(context_collection, open) else { continue };
        let args = parse_args(&context_collection[open + 1..close]);
        // The real path argument interpolates `$angularMetaRootPath`.
        if !args.first().map_or(false, |a| a.expr.contains("angular_meta.dart")) {
            continue;
        }
        let content = args.get(1).and_then(|a| plain_string(&a.expr))?;
        return Some(MockFile { path: "lib/angular_meta.dart".to_string(), content });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
