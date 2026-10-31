//! #3180 pattern args-fill pins: a FOREIGN generic object-pattern head
//! (`Circle<T extends MetricUnits>` living in the pattern helper's own
//! unit) reaches the bound variable's row face as its
//! instantiate-to-bounds form (`Circle<MetricUnits>`), never the raw
//! `Circle` head the same-file bounds snapshot cannot spell. The
//! type-production seam is `instantiated_head_ty`'s foreign fill
//! (creation_args.rs): when `itb_default_args` misses the snapshot
//! registry the container's declared `type_param_bounds` — projected
//! across the file boundary through `boundary.ty(owner, …)` — supplies the
//! defaults, and `bind_var_type` applies the same I2B the declared-type
//! arms already bind. Every expectation is a verbatim oracle 3.13.7 row.

use super::test_rig::{RigImport, report_ops_imports};

/// A `patterns_lib`-style foreign library: generic `Shape`/`Unit` heads
/// the fixture's object patterns destructure through (the co19
/// `patterns_lib.dart` face).
const LIB: &str = "abstract class MetricUnits {}\n\
class Unit<T extends MetricUnits> {\n  const Unit();\n}\n\
class Shape<T extends MetricUnits> {\n  const Shape();\n}\n\
class Circle<T extends MetricUnits> extends Shape<T> {\n  const Circle();\n  Unit<T> get area => const Unit();\n}\n";

/// The `undefined_operator` rows of `report_ops_imports`, as (offset,
/// length, message) triples — the prelude rides the main unit, so offsets
/// shift by one line; only the operator family rows matter.
fn rows(source: &str, lib: &str) -> Vec<(u32, u32, String)> {
    report_ops_imports(
        source,
        &[RigImport {
            uri: "package:t/patterns_lib.dart",
            prefix: None,
            source: lib,
        }],
    )
    .diagnostics
    .iter()
    .filter(|d| d.code.name == "undefined_operator")
    .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
    .collect()
}

/// `Circle() && final transformer` — the type-free binder on the right of
/// a logical-and inside a case window binds the left object pattern's
/// head (`getMatchedValueType` over the and-pattern). The foreign
/// `Circle<T extends MetricUnits>` fills `Circle<MetricUnits>`; an
/// operator applied to `transformer` misses and reports
/// `undefined_operator` naming the instantiated head, never the bare
/// `Circle`.
#[test]
fn logical_and_binder_instantiates_foreign_head() {
    let source = "import 'package:t/patterns_lib.dart';\n\
String f(Shape<MetricUnits> s) {\n\
\x20 switch (s) {\n\
\x20   case Circle() && final transformer:\n\
\x20     return transformer + 0;\n\
\x20 }\n\
}\n";
    let report = rows(source, LIB);
    assert!(
        report
            .iter()
            .any(|(_, _, m)| m.contains("Circle<MetricUnits>")),
        "expected the foreign head's I2B face Circle<MetricUnits>, got {report:?}"
    );
    assert!(
        !report
            .iter()
            .any(|(_, _, m)| m.contains("'Circle'") || m.ends_with("Circle'.")),
        "the raw Circle head must not reach the row: {report:?}"
    );
}

/// A SAME-file generic head keeps the `itb_default_args` face — the
/// foreign fallback never shadows the same-file snapshot (the canonical
/// producer wins; the fallback only fires on its miss). Same
/// `Head() && final x` shape as the foreign pin so the only variable
/// under test is same-file vs foreign resolution.
#[test]
fn same_file_head_still_instantiates_via_snapshot() {
    // No import — a same-file generic class exercises the
    // `itb_default_args` arm (the snapshot's own registry), proving the
    // foreign fallback does not displace it.
    let source = "abstract class MetricUnits {}\n\
class Unit<T extends MetricUnits> {\n  const Unit();\n}\n\
class Box<T extends MetricUnits> {\n  const Box();\n  Unit<T> get area => const Unit();\n}\n\
String f(Box<MetricUnits> s) {\n\
\x20 switch (s) {\n\
\x20   case Box() && final transformer:\n\
\x20     return transformer + 0;\n\
\x20 }\n\
}\n";
    let report = rows(source, "class Unused {}\n");
    assert!(
        report
            .iter()
            .any(|(_, _, m)| m.contains("Box<MetricUnits>")),
        "expected the same-file I2B face Box<MetricUnits>, got {report:?}"
    );
    assert!(
        !report
            .iter()
            .any(|(_, _, m)| m.contains("'Box'") || m.ends_with("Box'.")),
        "the raw Box head must not reach the row: {report:?}"
    );
}
