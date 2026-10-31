//! #3183 object-pattern shorthand env pins: a FOREIGN generic head's
//! destructure (`Circle(area: v)` over `Circle<T extends MetricUnits>` in
//! the pattern helper's own unit) substitutes the bound variable's
//! declared member type through the head's parameter names —
//! `Unit<T>` answers `Unit<MetricUnits>`, never the raw `Unit<T>` schema
//! nor the unknown the same-file `container_params` snapshot's miss used
//! to leak. The env-key producer is `object_pattern_binder_ty`'s
//! parameter-name lookup (row_resolution.rs): same-file heads ride the
//! `container_params` snapshot registry, foreign heads fall back to the
//! declaring unit's `type_param_names` via `foreign_container_params`.
//! Every expectation is a verbatim oracle 3.13.7 row.

use super::test_rig::{RigImport, report_ops_imports};

/// A `patterns_lib`-style foreign library: generic `Shape`/`Unit` heads
/// the fixture's object patterns destructure through (the co19
/// `patterns_lib.dart` face).
const LIB: &str = "abstract class MetricUnits {}\n\
class Unit<T extends MetricUnits> {\n  const Unit();\n}\n\
class Shape<T extends MetricUnits> {\n  const Shape();\n}\n\
class Circle<T extends MetricUnits> extends Shape<T> {\n  const Circle();\n  Unit<T> get area => const Unit();\n}\n";

/// The `undefined_operator` rows of `report_ops_imports`, as (offset,
/// length, message) triples — only the operator family rows matter.
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

/// `Circle(:var area)` — the object-pattern SHORTHAND binder destructures
/// the `area` getter (`Unit<T>`) through the FOREIGN head's parameter
/// names. The env `T → MetricUnits` substitutes the member's declared
/// return, so `area` binds `Unit<MetricUnits>` and an applied operator
/// reports `undefined_operator` naming the substituted face — never the
/// raw `Unit<T>` schema and never the unknown the env's missing names
/// used to produce.
#[test]
fn shorthand_binder_substitutes_through_foreign_param_names() {
    let source = "import 'package:t/patterns_lib.dart';\n\
String f(Shape<MetricUnits> s) {\n\
\x20 switch (s) {\n\
\x20   case Circle(:var area):\n\
\x20     return area + 0;\n\
\x20 }\n\
}\n";
    let report = rows(source, LIB);
    assert!(
        report
            .iter()
            .any(|(_, _, m)| m.contains("Unit<MetricUnits>")),
        "expected the substituted member face Unit<MetricUnits>, got {report:?}"
    );
    assert!(
        !report.iter().any(|(_, _, m)| m.contains("Unit<T>")),
        "the raw Unit<T> schema must not reach the row: {report:?}"
    );
}

/// A SAME-file generic head keeps the `container_params` snapshot as its
/// env producer — the foreign `type_param_names` fallback only fires on
/// the snapshot's miss, never displacing the canonical names.
#[test]
fn same_file_head_still_substitutes_via_snapshot() {
    let source = "abstract class MetricUnits {}\n\
class Unit<T extends MetricUnits> {\n  const Unit();\n}\n\
class Box<T extends MetricUnits> {\n  const Box();\n  Unit<T> get area => const Unit();\n}\n\
String f(Box<MetricUnits> s) {\n\
\x20 switch (s) {\n\
\x20   case Box(:var area):\n\
\x20     return area + 0;\n\
\x20 }\n\
}\n";
    let report = rows(source, "class Unused {}\n");
    assert!(
        report
            .iter()
            .any(|(_, _, m)| m.contains("Unit<MetricUnits>")),
        "expected the same-file substituted face Unit<MetricUnits>, got {report:?}"
    );
    assert!(
        !report.iter().any(|(_, _, m)| m.contains("Unit<T>")),
        "the raw Unit<T> schema must not reach the row: {report:?}"
    );
}
