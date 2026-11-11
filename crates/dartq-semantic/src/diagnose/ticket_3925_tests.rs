//! #3925 pins: parameter-carrying `on` constraints reach an INSTANTIATED
//! verdict (the type-variable arms decide them), and a constraint spelled
//! through an import prefix matches the element it resolves to.
//!
//! Upstream anchors (Dart SDK 3.13.7, the pinned oracle):
//!
//! * `SubtypeVisitor`'s type-variable rules (`pkg/analyzer/lib/src/dart/
//!   element/subtype.dart`): reflexivity of one element (`X0 <: X1` iff the
//!   same element), the bound rules (`X0 <: T1` iff `bound(X0) <: T1`, and
//!   `X0 <: Object` iff `bound(X0) <: Object` — the unbound default
//!   `Object?` is not), and `X <: Y` for two distinct parameters answering
//!   false. An interface pair therefore orders its arguments by those arms:
//!   `B<X> <: B<Y>` is false, `C<X,Y> <: B<X>` holds while `<: B<Y>` does
//!   not (`co19` `Super-mixins/compatible_t04/t06`).
//! * The same file's interface arm compares ELEMENTS, so a constraint
//!   spelled through an import prefix (`p.A`) is the very interface `A`
//!   the closure reaches.

use super::test_rig::{CORE_PRELUDE, RigImport, report_ops, report_ops_imports};

/// The prelude's UTF-16 length (the `report_ops` fixtures' offset shift).
fn shift() -> u32 {
    u32::try_from(CORE_PRELUDE.encode_utf16().count()).expect("prelude fits u32")
}

const NOT_IMPLEMENTED: &str = "mixin_application_not_implemented_interface";

/// The fixture's own rows of one code, as `(offset, message)`, in row order.
fn rows_of(report: &crate::diagnose::FileReport, code: &str) -> Vec<(u32, String)> {
    let shift = shift();
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == code)
        .map(|row| (row.range.offset - shift, row.message.to_string()))
        .collect()
}

/// The UTF-16 offset of `needle`'s first occurrence in `source`.
fn offset_of(source: &str, needle: &str) -> u32 {
    let at = source.find(needle).expect("needle present in fixture");
    u32::try_from(source[..at].encode_utf16().count()).expect("fixture fits u32")
}

/// `C<X>` is `B<X>` (the chain `C<T> extends B<T> extends A<T>`) but the
/// constraint asks for `B<Y>`: two distinct type variables, so `X <: Y`
/// answers false and the SECOND constraint reports.
#[test]
fn constraint_parameter_arguments_report() {
    let source = "\
class S {}
class T {}
class X extends S {}
class Y extends T {}
class A<Ty> {}
abstract class B<Ty> extends A<Ty> {}
class C<Ty> extends B<Ty> {}
mixin M<Xm extends S, Ym extends T> on A<Xm>, B<Ym> {}
class MA extends C<X> with M<X, Y> {}
";
    assert_eq!(
        rows_of(&report_ops(source), NOT_IMPLEMENTED),
        vec![(
            offset_of(source, "M<X, Y>"),
            "'M<X, Y>' can't be mixed onto 'C<X>' because 'C<X>' doesn't implement 'B<Y>'."
                .to_string()
        )]
    );
}

/// The argument ORDER decides it against the declared interfaces: `C<Y, X>`
/// implements `A<Y>` and `B<X>`, so the constraint `A<X>` (the mixin's own
/// parameter substituted by the application's first argument) is not met.
#[test]
fn constraint_parameter_order_mismatch_reports() {
    let source = "\
class S {}
class T {}
class A<Ty> {}
abstract class B<Ty> {}
class C<P, Q> implements A<P>, B<Q> {}
mixin M<Xm extends S, Ym extends T> on A<Ym>, B<Xm> {}
class MA<X, Y> extends C<Y, X> with M<Y, X> {}
";
    assert_eq!(
        rows_of(&report_ops(source), NOT_IMPLEMENTED),
        vec![(
            offset_of(source, "M<Y, X>"),
            "'M<Y, X>' can't be mixed onto 'C<Y, X>' because 'C<Y, X>' doesn't implement 'A<X>'."
                .to_string()
        )]
    );
}

/// The bound arm of `X <: Object`: `T extends num` IS a subtype of `Object`
/// (its bound is), so the constraint `A<Object>` is satisfied.
#[test]
fn constraint_parameter_bound_satisfies_object() {
    let source = "class A<T> {}\n\nmixin M on A<Object> {}\n\nclass C<T extends num> extends A<T> with M {}\n";
    assert_eq!(
        rows_of(&report_ops(source), NOT_IMPLEMENTED),
        Vec::<(u32, String)>::new()
    );
}

/// A constraint reached through an import prefix names the element the
/// closure reaches under its bare spelling: `Cc implements p.A` satisfies
/// `mixin M on p.A`.
#[test]
fn constraint_prefixed_target_matches_the_element() {
    let source = "import 'a.dart';\nimport 'a.dart' as p;\n\nclass Cc implements A {}\n\n\
                  class X extends Cc with p.M {}\n";
    let report = report_ops_imports(
        source,
        &[RigImport {
            uri: "package:t/a.dart",
            prefix: None,
            source: "class A {}\n\nmixin M on A {}\n",
        }],
    );
    assert_eq!(
        rows_of(&report, NOT_IMPLEMENTED),
        Vec::<(u32, String)>::new()
    );
}
