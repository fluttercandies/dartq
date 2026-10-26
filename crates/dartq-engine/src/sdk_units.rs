//! The SDK libraries as indexed units: `dart:core` and every `dart:`
//! library the live libraries reach (their imports and exports, then the
//! SDK libraries' own) load from the SDK sources through the shared
//! per-file pipeline — scan, parse, element collection — and join the
//! dependency index exactly as a pub-cache file does. The semantic layer
//! then resolves `List`, `Map`, `FormatException` and every other core
//! face through the same element model and clause capture the workspace's
//! own declarations use: real members and signatures, real supertype
//! edges, no hand-kept face table. A library whose sources the SDK root
//! does not carry keeps the generated names-only face.
//!
//! # Shape
//!
//! Each library is its entry file plus its `part` files (every `dart:`
//! library is part-structured); all of them register under the library's
//! graph node, and only the entry carries the library's import/export
//! edges. The loaded set is reachable-only — `dart:html` and its kin load
//! when something imports them — and lives for the engine's life beside
//! the dependency units (SDK content is session-stable). The units do not
//! count against [`super::deps::DEP_FILE_CAP`]: the cap bounds the open
//! pub-cache closure, the SDK closure is bounded by the SDK itself.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dartq_resolution::ResolvedTarget;
use dartq_source::{FileId, SourceText};
use rustc_hash::FxHashSet;

use crate::deps::DepUnit;
use crate::l1;
use crate::state::{DirectiveKind, EngineState, UnitFacts};

/// The `dart:` library name a directive URI addresses (`dart:core` →
/// `core`, `dart:core/bool.dart` → `core`), or `None` for any other URI.
pub(crate) fn sdk_library_of(uri: &str) -> Option<&str> {
    let rest = uri.strip_prefix("dart:")?;
    let name = rest.split('/').next().unwrap_or(rest);
    (!name.is_empty()).then_some(name)
}

/// Seeds and drains the reachable SDK libraries into the dependency index.
/// Called at every graph build before the pub-cache closure; a library
/// already loaded reuses.
pub(crate) fn load_sdk_units(state: &mut EngineState) {
    if state.residency.is_empty() {
        return;
    }
    let mut queue: Vec<String> = vec!["core".to_string()];
    for facts in state.residency.values() {
        for directive in &facts.directives {
            if !matches!(directive.kind, DirectiveKind::Import | DirectiveKind::Export) {
                continue;
            }
            if let Some(name) = directive.uri.as_deref().and_then(sdk_library_of) {
                queue.push(name.to_string());
            }
        }
    }
    let mut seen: FxHashSet<String> = FxHashSet::default();
    while let Some(name) = queue.pop() {
        if !seen.insert(name.clone()) || state.deps.sdk_libraries.contains_key(&name) {
            continue;
        }
        let Some(entry_path) = state.sdk.flat_library_to_path(&name, "") else {
            continue;
        };
        let entry = state.files.intern(&entry_path);
        let Some(entry_facts) = load_sdk_file(state, entry) else {
            continue;
        };
        let mut files = vec![entry];
        let entry_dir = entry_path.parent().map(Path::to_path_buf);
        for directive in &entry_facts.directives {
            match directive.kind {
                DirectiveKind::Part => {
                    let (Some(uri), Some(dir)) = (directive.uri.as_deref(), entry_dir.as_deref())
                    else {
                        continue;
                    };
                    let part = state.files.intern(&lexical_join(dir, uri));
                    if load_sdk_file(state, part).is_some() {
                        files.push(part);
                    }
                }
                DirectiveKind::Import | DirectiveKind::Export => {
                    if let Some(next) = directive.uri.as_deref().and_then(sdk_library_of) {
                        queue.push(next.to_string());
                    }
                }
                _ => {}
            }
        }
        state.deps.sdk_libraries.insert(name, files);
    }
}

/// Loads one SDK file through the shared per-file pipeline and registers
/// it as a dependency unit; a file already loaded reuses. Every `dart:`
/// import/export target is the external face (the graph build maps it onto
/// the library's node by name); nothing else resolves. Unreadable files
/// count as load failures and skip silently.
fn load_sdk_file(state: &mut EngineState, file: FileId) -> Option<Arc<UnitFacts>> {
    if let Some(unit) = state.deps.units.get(&file) {
        return Some(Arc::clone(&unit.facts));
    }
    let path = state.files.path(file).map(Path::to_path_buf)?;
    let Ok(bytes) = std::fs::read(&path) else {
        state.deps.load_failures += 1;
        return None;
    };
    let text = SourceText::canonical(&bytes);
    let content = state.store.put_text(&text);
    let version = state.sdk.language_metadata;
    let (facts, _capture, scanned) = l1::compute_unit(
        file,
        content,
        text.as_str(),
        version,
        &state.strings,
        &state.grants(),
    );
    let targets: Vec<ResolvedTarget> = facts
        .directives
        .iter()
        .map(|directive| {
            let external = matches!(directive.kind, DirectiveKind::Import | DirectiveKind::Export)
                && directive.uri.as_deref().and_then(sdk_library_of).is_some();
            if external { ResolvedTarget::External } else { ResolvedTarget::Missing }
        })
        .collect();
    let facts = Arc::new(facts);
    state.deps.units.insert(
        file,
        DepUnit {
            facts: Arc::clone(&facts),
            version: scanned.version,
            targets: targets.into_boxed_slice(),
        },
    );
    state.deps.sdk_file_count += 1;
    Some(facts)
}

/// `dir/relative` with `.` and `..` segments folded lexically — the
/// analyzer's own part-path join over the SDK tree (every SDK part is a
/// plain relative path; a leading `/` would only ever appear in a
/// malformed SDK and folds into the directory like the oracle's string
/// concatenation does).
fn lexical_join(dir: &Path, relative: &str) -> PathBuf {
    let mut out = dir.to_path_buf();
    for segment in relative.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            segment => out.push(segment),
        }
    }
    out
}
