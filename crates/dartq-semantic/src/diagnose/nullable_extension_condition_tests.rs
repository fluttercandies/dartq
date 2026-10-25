//! The nullable-extension condition face: a member read that resolved
//! through a nullable-applicable extension face on a nullable receiver
//! carries the extension's own non-nullable result, so a staged
//! nullable-condition face (a `&&` right operand reading the member)
//! stands down. Oracle provenance: `dart analyze` 3.13.1 census —
//! ffi_verifier.dart:2644's `element.supertype.isAbiSpecificInteger`
//! over `extension on DartType?`.

use super::test_rig::{code_names, report_ops};

/// A member resolved through a nullable-applicable extension face on a
/// nullable receiver keeps the extension's own nullability: the `&&`
/// right-operand condition face stands down.
#[test]
fn nullable_on_extension_member_keeps_condition_silent() {
    let report = report_ops(
        "class A {}\n\
         class C {\n\
         \x20 A? get supertype => null;\n\
         }\n\
         extension Ext on A? {\n\
         \x20 bool get positive => true;\n\
         }\n\
         bool f(C c) {\n\
         \x20 return c.supertype.positive && true;\n\
         }\n",
    );
    assert_eq!(code_names(&report), Vec::<&str>::new());
}
