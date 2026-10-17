//! `FileElements` residency pin (#1778): the semantic element model's
//! per-file bytes per KB of source, over the same synthetic ~10 KB fixture
//! posture as `memory_accounting.rs`. The name faces are dense sorted rows
//! (`dartq_semantic::model::NameMap`), so the walk is exact: every
//! name-keyed face costs `row_count × (key + value)` plus the value's own
//! boxed payloads — no bucket arrays, no reservation slack to audit.
//!
//! The estimator walks public accessors and public fields only. The one
//! composition, not a measurement: the per-unit argument pool interns no
//! generic type arguments on this fixture (asserted), so its live mass is
//! the inline header over two reserved empty columns.
//!
//! Run: `cargo test -p dartq-engine --test memory_file_elements -- --nocapture`

#![allow(clippy::cast_possible_truncation)] // measurement math only
#![allow(clippy::cast_precision_loss)] // byte counts are far below 2^52

mod common;

use std::fmt::Write as _;
use std::mem::{size_of, size_of_val};

use common::Fixture;
use dartq_engine::CancellationToken;
use dartq_semantic::model::{ArgPool, ExtensionFace, Formals};
use dartq_semantic::{FileElements, MemberEntry, TopDecl};
use dartq_source::StringId;

/// Full-model ceiling: 12 KB of resident element model per KB of source.
/// The name faces are the dense-row shape, so the honest measurement sits
/// near 3-4 KB/KB on this fixture (58 top-level rows, 33 containers, ~130
/// member rows, no constructor formals); ~3x headroom trips only a real
/// shape regression — a face re-growing per-name hash buckets or per-row
/// heap structures beyond the modeled payloads. The fail message prints
/// the measured numbers, so a trip is itself the recalibration evidence.
const FILE_ELEMENTS_KB_PER_KB_CEILING: f64 = 12.0;

/// Deep bytes of one formal-parameter table: the two boxed parameter
/// columns at their row mass (a `Formal` is flat: id + `Ty` + flag).
fn formals_bytes(formals: &Formals) -> usize {
    size_of::<Formals>() + size_of_val(&*formals.positional) + size_of_val(&*formals.named)
}

/// Deep bytes of one member row: the flat entry plus its type-parameter
/// names and its optional formal table.
fn member_bytes(entry: &MemberEntry) -> usize {
    size_of::<MemberEntry>()
        + size_of_val(&*entry.type_param_names)
        + entry.formals.as_deref().map_or(0, formals_bytes)
}

/// Deep bytes of one top-level declaration row.
fn top_decl_bytes(decl: &TopDecl) -> usize {
    size_of::<TopDecl>() + decl.formals.as_deref().map_or(0, formals_bytes)
}

/// Deep bytes of one file's element model: the inline header, the
/// argument pool (reserved-slot-only here, see the module docs), every
/// name-face row at its exact dense size, and the non-name columns
/// (clause lists, constructors, annotations, enum constants,
/// representation formals) at length × item — those are flat id/tag
/// columns whose finish-time shrink keeps at capacity == len.
fn file_elements_bytes(elements: &FileElements) -> usize {
    let pool = elements.type_args();
    assert!(
        pool.is_empty(),
        "fixture declares no generic type arguments; the pool composition \
         below (inline header only) assumes the reserved empty slots"
    );
    let mut bytes = size_of::<FileElements>() + size_of::<ArgPool>();

    for (_, decl) in elements.declarations() {
        bytes += size_of::<StringId>() + top_decl_bytes(decl);
    }
    for (_, container) in elements.containers() {
        bytes += size_of::<StringId>();
        for (_, entry) in container.members.iter() {
            bytes += size_of::<StringId>() + member_bytes(entry);
        }
        for (_, formals) in container.ctor_formals.iter() {
            bytes += size_of::<StringId>() + formals_bytes(formals);
        }
        bytes += (container.with_clause.len()
            + container.implements_clause.len()
            + container.on_clause.len()
            + container.header_residue.len()
            + container.enum_constants.len())
            * size_of::<StringId>();
        bytes += size_of_val(&*container.primary_formals);
        bytes += size_of_val(&*container.constructors);
        bytes += size_of_val(&*container.annotations);
    }
    for face in elements.extension_faces() {
        bytes += size_of::<ExtensionFace>();
        for (_, entry) in face.members.iter() {
            bytes += size_of::<StringId>() + member_bytes(entry);
        }
    }
    bytes
}

/// The pin's normalization unit: resident KB per KB of source.
fn kb_per_kb(bytes: usize, source_bytes: usize) -> f64 {
    bytes as f64 / source_bytes as f64
}

/// The synthetic fixture, byte-identical in shape to the
/// `memory_accounting.rs` generator: one ~10 KB unit — 32 mid-sized
/// classes (two fields, two methods, an extends edge each), 24 top-level
/// functions, and a main exercising every declaration. Deterministic and
/// pure ASCII.
fn synthetic_source() -> String {
    const CLASSES: usize = 32;
    const TOPS: usize = 24;

    let mut source = String::from("class Base {}\n\n");
    for i in 0..CLASSES {
        let _ = write!(
            source,
            "/// A synthetic domain holder {i}.\n\
             class Shape{i} extends Base {{\n  \
             int x{i} = 0;\n  \
             int y{i} = 0;\n  \
             void move{i}(int dx, int dy) {{\n    \
             x{i} = x{i} + dx;\n    \
             y{i} = y{i} + dy;\n  \
             }}\n  \
             int area{i}() => x{i} * y{i};\n\
             }}\n\n"
        );
    }
    for i in 0..TOPS {
        let _ = writeln!(source, "double ratio{i}(int w, int h) => w / h;");
    }
    source.push_str("\nvoid main() {\n  var total = 0;\n");
    for i in 0..CLASSES {
        let _ = write!(
            source,
            "  Shape{i} s{i} = Shape{i}();\n  \
             s{i}.move{i}(3, 4);\n  \
             total += s{i}.area{i}();\n"
        );
    }
    for i in 0..TOPS {
        let _ = writeln!(source, "  total += ratio{i}(3, 4).toInt();");
    }
    source.push_str("  print(total);\n}\n");
    source
}

#[test]
fn file_elements_residency_pin_bytes_per_kb() {
    let source = synthetic_source();
    let source_bytes = source.len();
    assert!(
        (9 * 1024..=12 * 1024).contains(&source_bytes),
        "fixture drifted off the ~10 KB design point: {source_bytes} bytes"
    );

    let fixture = Fixture::new(&[("lib/main.dart", "")]);
    let mut engine = fixture.engine();
    let file = engine.set_content(&fixture.path("lib/main.dart"), source.as_bytes());
    engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze");

    let facts = engine.file_facts(file).expect("analyzed file facts");
    let elements: &FileElements = &facts.elements;

    // The fixture must exercise every counted face, or the pin is vacuous.
    let containers = elements.containers().count();
    let decls = elements.declarations().count();
    let members: usize = elements.containers().map(|(_, c)| c.members.len()).sum();
    assert!(decls > 40, "fixture must carry top-level mass: {decls}");
    assert!(
        containers > 20,
        "fixture must carry container mass: {containers}"
    );
    assert!(members > 80, "fixture must carry member mass: {members}");

    let bytes = file_elements_bytes(elements);
    let per_kb = kb_per_kb(bytes, source_bytes);
    assert!(
        per_kb < FILE_ELEMENTS_KB_PER_KB_CEILING,
        "FileElements residency ballooned: {bytes} bytes for a \
         {source_bytes}-byte file ({per_kb:.2} KB per KB; ceiling \
         {FILE_ELEMENTS_KB_PER_KB_CEILING}; decls {decls}, containers \
         {containers}, members {members})"
    );

    println!(
        "file elements: {source_bytes} bytes of source, {bytes} resident \
         ({per_kb:.2} KB per KB; decls {decls}, containers {containers}, \
         members {members})"
    );
}
