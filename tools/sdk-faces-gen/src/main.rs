//! `sdk-faces-gen` — offline generator for the dart: SDK facade tables
//! (ticket #2005): the facade data is emitted as checked-in static tables by
//! a one-shot offline extraction over the oracle SDK tree — adjudicated over
//! load-time source parsing (which would pull parser-grade work into the
//! SDK crate) and over hand-grown name lists (which drift silently on every
//! anchor bump; here regeneration is a pinned command and drift is a
//! diff).
//!
//! Reads a Dart SDK root (the oracle of record, e.g. the fvm cache SDK),
//! extracts two checked-in tables, and writes them into the workspace:
//!
//! 1. the flat `libraries.dart` existence table →
//!    `crates/dartq-sdk/src/flat_libraries.rs`;
//! 2. per-public-library top-level export faces →
//!    `crates/dartq-engine/src/sdk_faces_gen.rs`.
//!
//! Usage (from the workspace root; paths are relative to it):
//!
//! ```text
//! cargo run -p sdk-faces-gen -- <dart-sdk-root>
//! ```
//!
//! The SDK root argument may also come from `DARTQ_DART_SDK`. Regeneration
//! is pinned: run against the oracle-of-record anchor and commit the diff.
//! The run fails loudly rather than emit a partial table — scan warnings,
//! missing pinned anchor names, or a library falling below its count floor
//! all abort before any file is written.

mod scan;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;
use std::process::ExitCode;

use scan::{FlatEntry, parse_flat_table, scan_unit};

/// The workspace-relative output paths, keyed by what they hold.
const FLAT_OUT: &str = "crates/dartq-sdk/src/flat_libraries.rs";
const FACES_OUT: &str = "crates/dartq-engine/src/sdk_faces_gen.rs";

/// Names the analyzer's front end injects into a library's namespace without
/// a source-level declaration. `Never` has no declaration anywhere in the
/// SDK's `lib/` sources (the front end synthesizes the type in its type
/// universe and exposes it through dart:core's namespace), so a pure source
/// scan drops it — yet `dart analyze` resolves bare `Never` through the
/// implicit core import, and so must the engine's face table.
const SYNTHETIC_NAMES: &[(&str, &[&str])] = &[("core", &["Never"])];

/// Public top-level names that must be present in a library's face for the
/// run to succeed — the pinned acceptance symbols of the facade census
/// (`Completer`, `base64`, `JS`, `staticInterop`, …) plus the re-export
/// closure shapes (`core` gains `Future` through `export "dart:async"`;
/// `io` gains `HttpStatus` through `export 'dart:_internal'`).
const ANCHORS: &[(&str, &[&str])] = &[
    (
        "core",
        &[
            "Object",
            "String",
            "int",
            "double",
            "num",
            "bool",
            "Never",
            "print",
            "identical",
            "pragma",
            "Deprecated",
            "deprecated",
            "Future",
            "Stream",
            "IterableExtensions",
        ],
    ),
    (
        "async",
        &[
            "Completer",
            "Future",
            "FutureOr",
            "Stream",
            "StreamController",
            "StreamSubscription",
            "StreamTransformer",
            "Timer",
            "Zone",
            "scheduleMicrotask",
            "runZoned",
            "runZonedGuarded",
            "TimeoutException",
            "AsyncError",
            "ParallelWaitError",
            "EventSink",
        ],
    ),
    (
        "convert",
        &[
            "base64",
            "base64Url",
            "ascii",
            "json",
            "utf8",
            "latin1",
            "Base64Codec",
            "JsonCodec",
            "JsonEncoder",
            "LineSplitter",
        ],
    ),
    (
        "io",
        &[
            "File",
            "Directory",
            "Process",
            "Platform",
            "exit",
            "stdout",
            "stdin",
            "stderr",
            "Socket",
            "HttpStatus",
            "BytesBuilder",
            "HttpDate",
            "HttpOverrides",
            "WebSocketStatus",
            "CompressionOptions",
        ],
    ),
    (
        "math",
        &[
            "pi",
            "e",
            "ln10",
            "ln2",
            "sqrt2",
            "sqrt1_2",
            "min",
            "max",
            "pow",
            "sin",
            "cos",
            "sqrt",
            "atan",
            "atan2",
            "exp",
            "log",
            "Point",
            "Rectangle",
            "Random",
            "MutableRectangle",
        ],
    ),
    (
        "html",
        &["window", "document", "Window", "Document", "Element"],
    ),
    (
        "js_interop",
        &[
            "JS",
            "JSObject",
            "JSArray",
            "JSPromise",
            "JSBoolean",
            "JSNumber",
            "JSString",
            "staticInterop",
            "anonymous",
        ],
    ),
    (
        "js",
        &[
            "JsObject",
            "JsArray",
            "allowInterop",
            "allowInteropCaptureThis",
            "context",
        ],
    ),
    (
        "js_util",
        &[
            "allowInterop",
            "promiseToFuture",
            "jsify",
            "getProperty",
            "newObject",
        ],
    ),
    (
        "collection",
        &[
            "HashSet",
            "HashMap",
            "LinkedHashMap",
            "Queue",
            "SplayTreeMap",
            "SplayTreeSet",
            "DoubleLinkedQueueEntry",
            "IterableExtensions",
        ],
    ),
    (
        "typed_data",
        &[
            "Uint8List",
            "Float32List",
            "ByteData",
            "Endian",
            "ByteBuffer",
        ],
    ),
    (
        "isolate",
        &[
            "Isolate",
            "SendPort",
            "ReceivePort",
            "IsolateSpawnException",
        ],
    ),
    (
        "ffi",
        &["DynamicLibrary", "Pointer", "Struct", "Union", "Opaque"],
    ),
    ("developer", &["log", "postEvent", "Service", "Timeline"]),
    (
        "mirrors",
        &[
            "MirrorSystem",
            "ClassMirror",
            "InstanceMirror",
            "DeclarationMirror",
        ],
    ),
];

/// Minimum public-name counts per emitted face, from the facade census's
/// conservative per-library scans (treated as lower bounds). A library below
/// its floor means the extraction regressed against the recorded surface —
/// the run aborts instead of writing a shrunken table.
const FLOORS: &[(&str, usize)] = &[
    ("core", 60),
    ("async", 45),
    ("convert", 30),
    ("io", 70),
    ("math", 20),
    ("html", 400),
    ("html_common", 15),
    ("indexed_db", 10),
    ("js", 5),
    ("js_interop", 80),
    ("js_interop_unsafe", 2),
    ("js_util", 15),
    ("collection", 20),
    ("typed_data", 20),
    ("isolate", 8),
    ("ffi", 15),
    ("developer", 8),
    ("mirrors", 12),
    ("concurrent", 2),
    ("cli", 0),
    ("nativewrappers", 1),
    ("vmservice_io", 1),
    ("svg", 30),
    ("web_audio", 30),
    ("web_gl", 30),
];

fn main() -> ExitCode {
    let root = match std::env::args()
        .nth(1)
        .or_else(|| std::env::var("DARTQ_DART_SDK").ok())
    {
        Some(root) if !root.is_empty() => root,
        _ => {
            eprintln!("usage: cargo run -p sdk-faces-gen -- <dart-sdk-root>");
            return ExitCode::FAILURE;
        }
    };
    if !Path::new("crates").is_dir() {
        eprintln!("sdk-faces-gen: run from the workspace root (no ./crates here)");
        return ExitCode::FAILURE;
    }
    match run(Path::new(&root)) {
        Ok(report) => {
            print!("{report}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("sdk-faces-gen: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The extracted tables of one SDK run.
struct Tables {
    /// The SDK root the tables were extracted from.
    root: Box<Path>,
    version: String,
    /// The flat `libraries.dart` table, source order preserved.
    flat: Vec<FlatEntry>,
    /// Public-library faces; libraries whose sources could not be read are
    /// excluded and reported as residuals (never faked as empty sets).
    faces: BTreeMap<String, BTreeSet<String>>,
    residuals: Vec<String>,
}

fn run(root: &Path) -> Result<String, String> {
    let tables = extract(root)?;
    let report = verify(&tables)?;
    std::fs::write(FLAT_OUT, emit_flat(&tables)).map_err(|e| format!("write {FLAT_OUT}: {e}"))?;
    std::fs::write(FACES_OUT, emit_faces(&tables))
        .map_err(|e| format!("write {FACES_OUT}: {e}"))?;
    Ok(report)
}

/// Reads the SDK: version file, flat table, and every public library face.
fn extract(root: &Path) -> Result<Tables, String> {
    let version = std::fs::read_to_string(root.join("version"))
        .map_err(|e| format!("read version file: {e}"))?
        .trim()
        .to_owned();
    let metadata_path = root.join("lib/_internal/sdk_library_metadata/lib/libraries.dart");
    let metadata = std::fs::read_to_string(&metadata_path)
        .map_err(|e| format!("read {}: {e}", metadata_path.display()))?;
    let flat = parse_flat_table(&metadata)?;

    let mut tables = Tables {
        root: Box::from(root),
        version,
        flat,
        faces: BTreeMap::new(),
        residuals: Vec::new(),
    };
    let mut memo = BTreeMap::new();
    for entry in &tables.flat {
        let name = &entry.name;
        if name.starts_with('_') {
            continue;
        }
        match face(name, &tables, &mut memo, &mut Vec::new()) {
            Ok(names) => {
                tables.faces.insert(name.clone(), names);
            }
            Err(error) => tables.residuals.push(format!("{name}: {error}")),
        }
    }
    Ok(tables)
}

/// The full public export face of library `name`: its own top-level
/// declarations (library file plus parts) plus every transitively re-exported
/// name through `export` directives, `show`/`hide` applied. `_`-private
/// names never enter (the scanner drops them at the source).
fn face(
    name: &str,
    sdk: &Tables,
    memo: &mut BTreeMap<String, BTreeSet<String>>,
    stack: &mut Vec<String>,
) -> Result<BTreeSet<String>, String> {
    if let Some(names) = memo.get(name) {
        return Ok(names.clone());
    }
    if stack.iter().any(|entry| entry == name) {
        return Err(format!("export cycle: {}", {
            let mut cycle = stack.clone();
            cycle.push(name.to_owned());
            cycle.join(" -> ")
        }));
    }
    let entry = sdk
        .flat
        .iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| "not in the flat libraries table".to_owned())?;
    // The dart2js implementation variant carries the face when present
    // (`html_common`): it is the surface the oracle's export chains flow
    // through.
    let path = entry.face_path().to_owned();
    let text = std::fs::read_to_string(sdk.root.join("lib").join(&path))
        .map_err(|e| format!("read lib/{path}: {e}"))?;
    let unit = scan_unit(&text);
    if !unit.warnings.is_empty() {
        return Err(format!(
            "lib/{path}: {} unclassified top-level line(s): {}",
            unit.warnings.len(),
            unit.warnings.first().expect("non-empty")
        ));
    }
    let mut names = unit.names;
    // Parts contribute their top-level surface; their own `part of` shape
    // never carries parts or exports.
    let lib_dir = Path::new(&path)
        .parent()
        .expect("library path has a parent");
    for part in &unit.parts {
        let part_path = lib_dir.join(part);
        let part_text = std::fs::read_to_string(sdk.root.join("lib").join(&part_path))
            .map_err(|e| format!("read lib/{}: {e}", part_path.display()))?;
        let part_unit = scan_unit(&part_text);
        if !part_unit.warnings.is_empty() {
            return Err(format!(
                "lib/{}: {} unclassified top-level line(s): {}",
                part_path.display(),
                part_unit.warnings.len(),
                part_unit.warnings.first().expect("non-empty")
            ));
        }
        names.extend(part_unit.names);
    }
    stack.push(name.to_owned());
    for export in &unit.exports {
        let target = export.target.strip_prefix("dart:").ok_or_else(|| {
            format!(
                "unsupported export target `{}` in lib/{path}",
                export.target
            )
        })?;
        let target_face = face(target, sdk, memo, stack)?;
        match &export.show {
            Some(show) => {
                for shown in show {
                    if !target_face.contains(shown) {
                        stack.pop();
                        return Err(format!(
                            "lib/{path}: export of dart:{target} shows `{shown}`, which the \
                             extracted face of dart:{target} does not carry"
                        ));
                    }
                    names.insert(shown.clone());
                }
            }
            None => {
                names.extend(
                    target_face
                        .iter()
                        .filter(|n| !export.hide.contains(n))
                        .cloned(),
                );
            }
        }
        for hidden in &export.hide {
            if !export.show.as_ref().is_some_and(|s| s.contains(hidden)) {
                names.remove(hidden);
            }
        }
    }
    stack.pop();
    // Front-end synthetics (see [`SYNTHETIC_NAMES`]) join after the export
    // fold — they are part of the oracle's namespace shape for the library
    // despite having no source declaration to scan.
    if let Some((_, names_injected)) = SYNTHETIC_NAMES.iter().find(|(lib, _)| *lib == name) {
        names.extend(names_injected.iter().map(|n| n.to_string()));
    }
    memo.insert(name.to_owned(), names.clone());
    Ok(names)
}

/// Verification: pinned anchors, per-library floors, and no empty faces.
/// Returns the human-readable run report on success.
fn verify(tables: &Tables) -> Result<String, String> {
    for (name, required) in ANCHORS {
        let face = tables.faces.get(*name).ok_or_else(|| {
            format!(
                "anchor library dart:{name} has no extracted face (residual: {:?})",
                tables.residuals.iter().find(|r| r.starts_with(*name))
            )
        })?;
        for anchor in *required {
            if !face.contains(*anchor) {
                return Err(format!(
                    "anchor missing: dart:{name} does not carry `{anchor}`"
                ));
            }
        }
    }
    let mut missed = Vec::new();
    for (name, floor) in FLOORS {
        match tables.faces.get(*name) {
            Some(face) if face.len() >= *floor => {}
            Some(face) => missed.push(format!(
                "dart:{name} extracted {} names, floor is {floor}",
                face.len()
            )),
            None => missed.push(format!("dart:{name} has no face")),
        }
    }
    if !missed.is_empty() {
        return Err(format!("floor missed: {}", missed.join("; ")));
    }
    for (name, face) in &tables.faces {
        if face.is_empty() && FLOORS.iter().any(|(n, f)| n == name && *f > 0) {
            return Err(format!("empty face: dart:{name} extracted zero names"));
        }
    }
    let mut report = format!(
        "sdk-faces-gen: SDK {} — {} flat libraries, {} public faces\n",
        tables.version,
        tables.flat.len(),
        tables.faces.len()
    );
    for (name, face) in &tables.faces {
        let _ = writeln!(report, "  {name}: {} names", face.len());
    }
    for residual in &tables.residuals {
        let _ = writeln!(report, "  RESIDUAL: {residual}");
    }
    let _ = writeln!(report, "  wrote {FLAT_OUT}");
    let _ = writeln!(report, "  wrote {FACES_OUT}");
    Ok(report)
}

/// Renders the generated `flat_libraries.rs`.
fn emit_flat(tables: &Tables) -> String {
    let mut out = String::new();
    out.push_str("// GENERATED FILE — do not edit.\n");
    let _ = writeln!(
        out,
        "// Regenerate: cargo run -p sdk-faces-gen -- <dart-sdk-root> (from the workspace root)"
    );
    let _ = writeln!(
        out,
        "// Source SDK: {} — flat `libraries.dart` table, {} entries.",
        tables.version,
        tables.flat.len()
    );
    out.push_str("//\n");
    out.push_str("// The oracle-of-record's URI-existence face: `dart analyze` builds its\n");
    out.push_str("// `dart:` library map from this table (`sdk_library_metadata`), flat across\n");
    out.push_str("// all target segments. Internal (`_`-prefixed) entries included — the\n");
    out.push_str("// oracle resolves them the same way. `web_sql` stays absent (on disk in\n");
    out.push_str("// the SDK tree but tabulated by neither side).\n\n");
    out.push_str("// Sorted by name so the query face binary-searches the static table.\n\n");
    out.push_str("#[rustfmt::skip]\n");
    out.push_str("pub static FLAT_LIBRARIES: &[(&str, &str)] = &[\n");
    let mut sorted: Vec<&FlatEntry> = tables.flat.iter().collect();
    sorted.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    for entry in sorted {
        let _ = writeln!(out, "    (\"{}\", \"{}\"),", entry.name, entry.path);
    }
    out.push_str("];\n");
    out
}

/// Renders the generated `sdk_faces_gen.rs`.
fn emit_faces(tables: &Tables) -> String {
    let total: usize = tables.faces.values().map(BTreeSet::len).sum();
    let mut out = String::new();
    out.push_str("// GENERATED FILE — do not edit.\n");
    let _ = writeln!(
        out,
        "// Regenerate: cargo run -p sdk-faces-gen -- <dart-sdk-root> (from the workspace root)"
    );
    let _ = writeln!(
        out,
        "// Source SDK: {} — per-library public top-level export faces, {} libraries /",
        tables.version,
        tables.faces.len()
    );
    let _ = writeln!(out, "// {} names total.", total);
    out.push_str("//\n");
    out.push_str("// Each set is the library's FULL top-level public surface: own declarations\n");
    out.push_str("// (library file plus parts) plus every transitively re-exported name\n");
    out.push_str("// through `export` directives with `show`/`hide` applied, `_`-private\n");
    out.push_str("// names dropped — the oracle's import-namespace shape at name granularity.\n");
    out.push_str("// A library joins the table only with its complete set; a partial face\n");
    out.push_str("// would flip used imports into false `unused_import`s.\n\n");
    out.push_str("#[rustfmt::skip]\n");
    out.push_str("pub static SDK_LIBRARY_FACES: &[(&str, &[&str])] = &[\n");
    for (name, face) in &tables.faces {
        out.push_str("    (\n        \"");
        out.push_str(name);
        out.push_str("\",\n        &[\n");
        for name in face {
            let _ = writeln!(out, "            \"{name}\",");
        }
        out.push_str("        ],\n    ),\n");
    }
    out.push_str("];\n");
    out
}
