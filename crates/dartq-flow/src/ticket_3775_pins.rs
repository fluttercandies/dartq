//! The #3775 pins: a typedef alias's right-hand-side `?` must reach the
//! declared face at every use site — three seams in `record.rs`, one per
//! alias shape:
//!
//! * **Bare named RHS** (`typedef AAlias1 = A?;` —
//!   [`Flow::type_aliases`]'s substitution in `handle_type`): the row an
//!   `AAlias1 a` declaration fires spells the aliased base with the
//!   right-hand `?` OR'd onto the use site's own, so `a != null` stages
//!   the promotion exactly as the direct `A? a` twin does. This pin is
//!   the regression guard: the substitution already carries the `?` at
//!   the flow layer, so any refactor of the substitution or the
//!   `end_typedef` latch that drops it fails here first.
//! * **Record RHS** (`typedef R1 = (int, String)?;` — `end_record_type`):
//!   the record spelling fires no named-head `handle_type`, so the
//!   top-level header latch (`top_last_type`) must ride the record's own
//!   opaque row; without it `end_typedef` binds its `(Function,
//!   nullable: false)` fallback — a wrong base that also drops the `?`.
//! * **Parameterized RHS** (`typedef L = List<int>?;` — the substitution
//!   guard): an alias whose right-hand side carries type arguments keeps
//!   its name-level face (the argument tree is not spliced), but
//!   nullability is not an argument-tree fact — the right-hand `?` still
//!   ORs onto the declared face.
//!
//! The oracle posture every pin mirrors (Dart SDK 3.13.7, `dart analyze`
//! on the bare fixtures — the same shapes dartq-semantic's
//! `ticket_3724_tests.rs` pins row-for-row): a nullable-alias declaration
//! under a `!= null` guard promotes the in-branch read (its member access
//! stands down), while the unpromoted trailing read reports; the
//! non-nullable twins report nothing at the guard (the official
//! `tryMarkNonNullable` bails on `isSameType`, upstream Dart SDK source,
//! `pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:
//! 4059-4068`, so no promotion fact exists). Here that posture is
//! asserted on the fact face: one `PromotedRead` in the promoted branch,
//! the declared read's nullable face, and zero promotions for the
//! non-nullable twin.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// Parses with the recorder attached and returns the live recorder (for
/// name rendering) alongside the taken facts.
fn analyze(source: &str) -> (SharedFlow<'_>, Vec<FlowFact>) {
    let result = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        dartq_source::FileTable::new().intern(std::path::Path::new("test.dart")),
    );
    assert!(result.diagnostics.is_empty(), "{source:?}");
    let tokens: &'static TokenTable = Box::leak(Box::new(result.tokens));
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let mut flow = SharedFlow(std::rc::Rc::new(std::cell::RefCell::new(Flow::new(
        tokens, source,
    ))));
    let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
    parser.parse_unit();
    drop(parser);
    let facts = flow.0.borrow_mut().take_facts();
    (flow, facts)
}

/// A type's rendered face: the head's lexeme (the opaque head renders
/// `Unknown`), with `?` when nullable — so a nullable declared face is
/// distinguishable from its promoted non-null form.
fn face(ty: crate::ty::Ty, flow: &Flow<'_>) -> String {
    match ty.base {
        crate::ty::Base::Name(id) => {
            let mut name = flow.name_lexeme(id).to_string();
            if ty.nullable {
                name.push('?');
            }
            name
        }
        crate::ty::Base::Dynamic => "dynamic".to_string(),
        crate::ty::Base::Void => "void".to_string(),
        crate::ty::Base::Never => "Never".to_string(),
        crate::ty::Base::Null => "Null".to_string(),
        crate::ty::Base::Function => {
            if ty.nullable {
                "Function?".to_string()
            } else {
                "Function".to_string()
            }
        }
        crate::ty::Base::Unknown => {
            if ty.nullable {
                "Unknown?".to_string()
            } else {
                "Unknown".to_string()
            }
        }
    }
}

/// Every read row as `(byte offset, rendered face)` — the declared or
/// promoted face of each promotable read, in fact order.
fn read_rows(source: &str) -> Vec<(u32, String)> {
    let (flow, facts) = analyze(source);
    let flow = flow.0.borrow();
    facts
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::ReadType { span, ty, .. } => {
                Some((span.range.offset, face(ty, &flow)))
            }
            _ => None,
        })
        .collect()
}

/// The promoted faces (rendered) in fact order.
fn promoted_faces(source: &str) -> Vec<String> {
    let (flow, facts) = analyze(source);
    let flow = flow.0.borrow();
    facts
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::PromotedRead { ty, .. } => Some(face(ty, &flow)),
            _ => None,
        })
        .collect()
}

#[test]
fn bare_alias_rhs_nullability_rides_the_declared_face() {
    let source = "class A {\n  foo() {}\n}\n\
                  typedef AAlias1 = A?;\n\
                  void f() {\n\
                  \x20 AAlias1 a;\n\
                  \x20 if (2 > 1) {\n\
                  \x20   a = A();\n\
                  \x20 }\n\
                  \x20 if (a != null) {\n\
                  \x20   a.foo();\n\
                  \x20 }\n\
                  \x20 a.foo();\n\
                  }\n";
    let guard = source.find("a != null").expect("guard") as u32;
    let in_branch = source.find("a.foo();").expect("in-branch") as u32;
    let trailing = source.rfind("a.foo();").expect("trailing") as u32;
    // The declared face at the guard read is nullable (the right-hand `?`
    // arrived through the substitution), the in-branch read carries the
    // promoted non-null form, and the trailing read reverts to the
    // declared face after the branch join.
    assert_eq!(
        read_rows(source),
        [
            (guard, "A?".to_string()),
            (in_branch, "A".to_string()),
            (trailing, "A?".to_string()),
        ]
    );
    assert_eq!(promoted_faces(source), ["A".to_string()]);
    // The non-nullable twin (`typedef AAlias1 = A;`) is the isSameType
    // bail: no promotion fact.
    let nn = source.replace("AAlias1 = A?", "AAlias1 = A");
    assert!(promoted_faces(&nn).is_empty());
}

#[test]
fn record_alias_rhs_nullability_rides_the_declared_face() {
    let source = "typedef R1 = (int, String)?;\n\
                  void f() {\n\
                  \x20 R1 r1;\n\
                  \x20 if (2 > 1) {\n\
                  \x20   r1 = (42, \"answer\");\n\
                  \x20 }\n\
                  \x20 if (r1 != null) {\n\
                  \x20   r1.$1;\n\
                  \x20 } else {\n\
                  \x20   r1?.$1;\n\
                  \x20 }\n\
                  }\n";
    let guard = source.find("r1 != null").expect("guard") as u32;
    let in_branch = source.find("r1.$1;").expect("in-branch") as u32;
    let else_branch = source.find("r1?.$1;").expect("else-branch") as u32;
    // The `?` rides the opaque record face: the guard read declares
    // `Unknown?`, the promoted read takes the opaque marker's non-null
    // form (#2243's face — never the `Function` fallback a missing latch
    // binds), and the else branch's null-aware read keeps the nullable
    // declared face.
    assert_eq!(
        read_rows(source),
        [
            (guard, "Unknown?".to_string()),
            (in_branch, "Unknown".to_string()),
            (else_branch, "Unknown?".to_string()),
        ]
    );
    assert_eq!(promoted_faces(source), ["Unknown".to_string()]);
    // The non-nullable twin (`typedef R1 = (int, String);`) is the
    // isSameType bail: no promotion fact.
    let nn = source.replace("(int, String)?", "(int, String)");
    assert!(promoted_faces(&nn).is_empty());
}

#[test]
fn parameterized_alias_rhs_nullability_rides_the_declared_face() {
    let source = "typedef L = List<int>?;\n\
                  void f() {\n\
                  \x20 L l;\n\
                  \x20 if (2 > 1) {\n\
                  \x20   l = <int>[];\n\
                  \x20 }\n\
                  \x20 if (l != null) {\n\
                  \x20   l.length;\n\
                  \x20 }\n\
                  }\n";
    let guard = source.find("l != null").expect("guard") as u32;
    let in_branch = source.find("l.length").expect("in-branch") as u32;
    // The parameterized right-hand side keeps its name-level face (`L`,
    // not a spliced `List<int>`), but the right-hand `?` still rides it:
    // the guard read is nullable and the in-branch read promotes.
    assert_eq!(
        read_rows(source),
        [
            (guard, "L?".to_string()),
            (in_branch, "L".to_string()),
        ]
    );
    assert_eq!(promoted_faces(source), ["L".to_string()]);
    // The non-nullable twin (`typedef L = List<int>;`) is the isSameType
    // bail: no promotion fact.
    let nn = source.replace("List<int>?", "List<int>");
    assert!(promoted_faces(&nn).is_empty());
}
