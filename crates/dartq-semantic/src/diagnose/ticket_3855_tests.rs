//! The #3855 `undefined_getter` display pins: an object-pattern getter miss
//! names the head INSTANTIATED TO BOUNDS (`Circle<MetricUnits>` over a bare
//! `class Circle<T extends MetricUnits>`), never the raw written head the
//! `finalize_pattern_getters` miss arms used to spell.
//!
//! Oracle contract (upstream Dart SDK source 3.13.7):
//!
//! * `type_analyzer.dart:1669-1689` (`resolveObjectPatternPropertyGet`) resolves
//!   each field getter against `receiverType` — the pattern type already
//!   instantiated to bounds
//!   (`named_type_resolver.dart:226` `instantiateInterfaceToBounds`,
//!   `:246` `instantiateTypeAliasToBounds`) — and reports the miss with that
//!   receiver's display string.
//! * A non-generic alias keeps its own spelling (`Fn`, `CircleAlias`) and a
//!   non-generic class its plain name (`Plain`): nothing is instantiated when
//!   the declaration declares no parameter.
//!
//! Both directions are pinned: every reporting shape below carries the verbatim
//! 3.13.7 row, and two silence twins (an existing getter on the instantiated
//! head, an `Object` member reached through a function head) must stay quiet —
//! so the display fix cannot widen into an extra row. Every expectation was
//! probed against `dart analyze` 3.13.7.
//!
//! NOT covered here — two independent gaps this display change neither causes
//! nor closes (both probed 3.13.7, each needs a file outside this ticket's
//! scope):
//!
//! * A bare GENERIC ALIAS head (`typedef Square<T extends MetricUnits> = void
//!   Function(T)`, `typedef A2<T extends MetricUnits> = Circle<T>`; used as
//!   `case Square(:var q)` / `case A2(zz: var c1)`) produces NO row at all:
//!   the fold leaves the alias unexpanded, so `container_kind_of` misses and
//!   the arm stands down. The oracle reports
//!   `'Square<MetricUnits>'` / `'A2<MetricUnits>'` — an under-report in the
//!   fold/gate path, not in the display.
//! * A head with WRITTEN type arguments that differ from its bounds
//!   (`case Circle<Sub>(zz: var c1)` over `Circle<T extends MetricUnits>`)
//!   displays the bounds face, not the written one: the pending tuple carries
//!   the head name only. The oracle reports `'Circle<Sub>'`.

use crate::diagnose::FileReport;
use crate::diagnose::test_rig::{RigImport, report_ops, report_ops_imports};

/// The `undefined_getter` messages of `report`, sorted (source order is the
/// engine's, the display text is what this module pins).
fn getter_messages(report: &FileReport) -> Vec<String> {
    let mut rows: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_getter")
        .map(|d| d.message.to_string())
        .collect();
    rows.sort();
    rows
}

/// The co19 `patterns_lib.dart` face: the generic heads the pattern helper
/// destructures through live in their own unit, so the bounds snapshot must
/// project them across the file boundary.
const LIB: &str = "abstract class MetricUnits {}\n\
class Circle<T extends MetricUnits> {\n\
\x20 const Circle();\n\
\x20 int get area => 0;\n\
}\n";

/// A FOREIGN generic head (`Circle<T extends MetricUnits>` in the imported
/// unit) reached through a bare object pattern: the miss names
/// `Circle<MetricUnits>`, and both the named-field and the positional cast
/// field arms of `finalize_pattern_getters` are pinned (the census
/// `object_A05_t01/t02/t03` shapes).
#[test]
fn ticket_3855_foreign_generic_head_names_instantiated_bounds() {
    let source = "import 'package:t/patterns_lib.dart';\n\
void f1(Object shape) {\n\
\x20 switch (shape) {\n\
\x20   case Circle(zz: var c1):\n\
\x20     break;\n\
\x20   default:\n\
\x20     break;\n\
\x20 }\n\
}\n\
void f2(Object shape) {\n\
\x20 switch (shape) {\n\
\x20   case Circle(var c2 as num):\n\
\x20     break;\n\
\x20   default:\n\
\x20     break;\n\
\x20 }\n\
}\n";
    let report = report_ops_imports(
        source,
        &[RigImport {
            uri: "package:t/patterns_lib.dart",
            prefix: None,
            source: LIB,
        }],
    );
    assert_eq!(
        getter_messages(&report),
        [
            "The getter 'c2' isn't defined for the type 'Circle<MetricUnits>'.",
            "The getter 'zz' isn't defined for the type 'Circle<MetricUnits>'.",
        ],
        "both miss arms must name the instantiated foreign head"
    );
}

/// The SAME-file generic head takes the bounds snapshot's own registry arm
/// (the canonical producer the foreign fallback must not displace) and names
/// the same instantiated face.
#[test]
fn ticket_3855_same_file_generic_head_names_instantiated_bounds() {
    let source = "abstract class MetricUnits {}\n\
class Circle<T extends MetricUnits> {\n\
\x20 const Circle();\n\
}\n\
void f(Object shape) {\n\
\x20 switch (shape) {\n\
\x20   case Circle(zz: var c1):\n\
\x20     break;\n\
\x20   default:\n\
\x20     break;\n\
\x20 }\n\
}\n";
    let report = report_ops(source);
    assert_eq!(
        getter_messages(&report),
        ["The getter 'zz' isn't defined for the type 'Circle<MetricUnits>'."],
        "a same-file generic head instantiates to bounds too"
    );
}

/// A NON-generic head keeps its plain name — the fill must not invent an
/// argument list.
#[test]
fn ticket_3855_non_generic_head_keeps_plain_name() {
    let source = "class Plain {\n\
\x20 const Plain();\n\
}\n\
void f(Object shape) {\n\
\x20 switch (shape) {\n\
\x20   case Plain(zz: var c1):\n\
\x20     break;\n\
\x20   default:\n\
\x20     break;\n\
\x20 }\n\
}\n";
    let report = report_ops(source);
    assert_eq!(
        getter_messages(&report),
        ["The getter 'zz' isn't defined for the type 'Plain'."],
        "a non-generic head stays bare"
    );
}

/// Silence twin: a field the instantiated head does declare resolves, so no
/// `undefined_getter` row appears (the display change must not widen the
/// emission gate).
#[test]
fn ticket_3855_existing_getter_stays_silent() {
    let source = "abstract class MetricUnits {}\n\
class Circle<T extends MetricUnits> {\n\
\x20 const Circle();\n\
\x20 int get area => 0;\n\
}\n\
void f(Object shape) {\n\
\x20 switch (shape) {\n\
\x20   case Circle(area: var a):\n\
\x20     break;\n\
\x20   default:\n\
\x20     break;\n\
\x20 }\n\
}\n";
    let report = report_ops(source);
    assert_eq!(
        getter_messages(&report),
        Vec::<String>::new(),
        "a declared getter is no miss"
    );
}

/// The function-head arm: a NON-generic function alias is a non-generic head
/// and keeps its bare spelling (`Fn`, oracle 3.13.7), and an `Object` member
/// reached through the same head stays silent (the `Object` interface closes
/// the lookup).
#[test]
fn ticket_3855_function_head_keeps_alias_name() {
    let source = "typedef Fn = void Function();\n\
void f1(Object shape) {\n\
\x20 switch (shape) {\n\
\x20   case Fn(:var q):\n\
\x20     break;\n\
\x20   default:\n\
\x20     break;\n\
\x20 }\n\
}\n\
void f2(Object shape) {\n\
\x20 switch (shape) {\n\
\x20   case Fn(:var hashCode):\n\
\x20     break;\n\
\x20   default:\n\
\x20     break;\n\
\x20 }\n\
}\n";
    let report = report_ops(source);
    assert_eq!(
        getter_messages(&report),
        ["The getter 'q' isn't defined for the type 'Fn'."],
        "the non-generic alias stays bare and the Object member stays silent"
    );
}

/// A non-generic CLASS alias keeps the written alias spelling even though the
/// folded head carries arguments (`typedef CircleAlias = Circle<MetricUnits>`
/// is no generic declaration, so nothing is instantiated:
/// `instantiateTypeAliasToBounds` has no parameter to fill).
#[test]
fn ticket_3855_non_generic_alias_keeps_alias_spelling() {
    let source = "abstract class MetricUnits {}\n\
class Circle<T extends MetricUnits> {\n\
\x20 const Circle();\n\
}\n\
typedef CircleAlias = Circle<MetricUnits>;\n\
void f(Object shape) {\n\
\x20 switch (shape) {\n\
\x20   case CircleAlias(zz: var c1):\n\
\x20     break;\n\
\x20   default:\n\
\x20     break;\n\
\x20 }\n\
}\n";
    let report = report_ops(source);
    assert_eq!(
        getter_messages(&report),
        ["The getter 'zz' isn't defined for the type 'CircleAlias'."],
        "a non-generic alias keeps its own spelling"
    );
}
