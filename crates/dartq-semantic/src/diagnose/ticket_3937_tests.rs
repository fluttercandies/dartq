//! #3937 pins — the annotation-family verdict faces: the head-metadata
//! scope snapshot (`useLibraryScope`), the parameter/directive/pattern
//! annotation drains (`visitFormalParameter`/`visitImportDirective`/
//! `visitLibraryDirective`/`visitPartDirective` consume their own
//! metadata, `metadata_resolver.dart`), the extension-static bare
//! blanket split (a static is never bare-reachable from outside,
//! `LibraryScope` lookup), the const-constructor aggregate for
//! extension-type representation constructors and forward references,
//! the alias owner-library scope (`_typeAliasGetter` resolves the
//! aliased class in the alias's own library), the multiply-defined
//! fall-through (`invalidAnnotation` beside `ambiguousImport`), and the
//! mixin no-argument static-getter face. Upstream Dart SDK source,
//! `pkg/analyzer/lib/src/dart/resolver/annotation_resolver.dart`.
//!
//! Oracle rows: the conformance corpus pins (dart 3.13.7,
//! `dart analyze --format=machine`) — each test cites its case.

#![allow(clippy::semicolon_if_nothing_returned)]
use super::test_rig::{RigImport, report_ops, report_ops_imports};

const UNDEFINED_ANNOTATION: &str = "Undefined name 'foo' used as an annotation.";
const INVALID_ANNOTATION: &str =
    "Annotation must be either a const variable reference or const constructor invocation.";
const UNDEFINED_IDENTIFIER: &str = "Undefined name 'foo'.";
const NO_CTOR_ARGS: &str = "Annotation creation must have arguments.";

/// The fixture's rows of one code: messages in source order.
fn code_rows(report: &super::FileReport, wanted: &str) -> Vec<String> {
    let mut rows: Vec<(u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == wanted)
        .map(|d| (d.range.offset, d.message.to_string()))
        .collect();
    rows.sort_by_key(|(offset, _)| *offset);
    rows.into_iter().map(|(_, message)| message).collect()
}

// —— head-metadata scope (the container snapshot) ————————————————————

/// Upstream `UndefinedAnnotationTest.test_useLibraryScope`: a class's
/// HEAD metadata resolves BEFORE the class scope opens — a static
/// declared inside the body never answers the head's name. Pre-fix the
/// drain-time verdict read the live (by then open) container and stayed
/// silent against the oracle's row.
#[test]
fn class_head_metadata_resolves_outside_class_scope() {
    let source = "\
@foo
class A {
  static const foo = null;
}
";
    assert_eq!(
        code_rows(&report_ops(source), "undefined_annotation"),
        [UNDEFINED_ANNOTATION]
    );
}

/// The inverse pin: MEMBER metadata resolves inside the container body
/// scope (`metadata_resolver.dart`'s member face) — the same static now
/// answers and the const-variable arm stays silent.
#[test]
fn member_metadata_resolves_inside_class_scope() {
    let source = "\
class A {
  static const foo = null;
  @foo
  void m() {}
}
";
    let report = report_ops(source);
    assert_eq!(
        code_rows(&report, "undefined_annotation"),
        [] as [String; 0]
    );
    assert_eq!(code_rows(&report, "invalid_annotation"), [] as [String; 0]);
}

// —— the parameter/directive/pattern drains ——————————————————————————

/// A plain extension's own declaration site drains its head metadata
/// (`visitExtensionDeclaration` visits it; pre-fix the plain-extension
/// arm opened no `note_decl_site` and the latch never drained). The
/// type-parameter twin (`f<@a E>() {}`) drains at the variable's own
/// declaration since #4340 (`ticket_4340_tests`).
#[test]
fn extension_head_metadata_drains() {
    let source = "\
@foo
extension E on int {}
";
    assert_eq!(
        code_rows(&report_ops(source), "undefined_annotation"),
        [UNDEFINED_ANNOTATION]
    );
}

/// Upstream `visitFormalParameter` consumes each parameter's metadata on
/// its own (a recovered multi-parameter method head drains per
/// parameter, the `MethodDeclarationTest` recovery family): an
/// unresolved annotation name on a formal reports at the parameter.
#[test]
fn parameter_metadata_drains_at_parameter_end() {
    let source = "\
class C {
  void m(@foo int x) {}
}
";
    assert_eq!(
        code_rows(&report_ops(source), "undefined_annotation"),
        [UNDEFINED_ANNOTATION]
    );
}

/// Upstream `visitLibraryDirective`/`visitImportDirective` run the
/// metadata resolution for their own directive — the annotation reports
/// even though the directive binds no declaration site.
#[test]
fn directive_metadata_drains_at_directive_end() {
    let library_source = "\
@foo
library l;
";
    assert_eq!(
        code_rows(&report_ops(library_source), "undefined_annotation"),
        [UNDEFINED_ANNOTATION]
    );
    let import_source = "\
@foo
import 'dart:math';
";
    assert_eq!(
        code_rows(&report_ops(import_source), "undefined_annotation"),
        [UNDEFINED_ANNOTATION]
    );
}

/// Upstream `ForStatementResolutionTest_ForPartsWithPattern
/// .test_metadata_shadowing`: the pattern-head's own binder shadows the
/// top-level constant for the head metadata — `@a` names the non-const
/// pattern variable, `invalidAnnotation`.
#[test]
fn for_parts_pattern_metadata_binder_shadows() {
    let unresolved = "\
void f() {
  for (@foo var (a) = (0); a <= 2; a++) {
    a;
  }
}
";
    assert_eq!(
        code_rows(&report_ops(unresolved), "undefined_annotation"),
        ["Undefined name 'foo' used as an annotation."]
    );
    let source = "\
const a = 42;
void f() {
  for (@a var (a) = (0); a <= 2; a++) {
    a;
  }
}
";
    assert_eq!(
        code_rows(&report_ops(source), "invalid_annotation"),
        [INVALID_ANNOTATION]
    );
}

/// The for-in twin (`ForEachPartsWithPattern.test_metadata_shadowing`).
#[test]
fn for_in_pattern_metadata_binder_shadows() {
    let source = "\
const a = 42;
void f(List<int> x) {
  for (@a var (a) in x) {
    a;
  }
}
";
    assert_eq!(
        code_rows(&report_ops(source), "invalid_annotation"),
        [INVALID_ANNOTATION]
    );
}

// —— the extension-static bare blanket split ————————————————————————

/// Upstream `UndefinedIdentifierTest
/// .test_annotation_references_static_method_in_extension`: an
/// extension's STATIC method is never bare-reachable from outside its
/// own scope — the annotation argument reports `undefinedIdentifier`.
/// Pre-fix the bare blanket resolved every extension member name
/// file-wide and the row never fired.
#[test]
fn extension_static_is_not_bare_reachable_in_annotation() {
    let source = "\
@Annotation(foo)
extension E on int {
  static void foo() {}
}
class Annotation {
  const Annotation(dynamic d);
}
";
    assert_eq!(
        code_rows(&report_ops(source), "undefined_identifier"),
        [UNDEFINED_IDENTIFIER]
    );
}

// —— the const-constructor aggregate (extension types, forward refs) —

/// Upstream `MetadataResolutionTest.test_value_extensionType_
/// unnamedConstructor` and the `SearchTest` extension-type reference:
/// a representation constructor is const when its `const` keyword is
/// written — the site table never latches it (the primary-constructor
/// event updates the model only), so the model's own const aggregate
/// answers the unnamed question. Pre-fix the no-site tail answered
/// `false` and every `@A(0)` over an extension type over-reported.
#[test]
fn extension_type_const_representation_ctor_is_silent() {
    let unnamed = "\
extension type const A(int it) {}

@A(0)
void f() {}
";
    let report = report_ops(unnamed);
    assert_eq!(code_rows(&report, "invalid_annotation"), [] as [String; 0]);
    assert_eq!(
        code_rows(&report, "non_constant_annotation_constructor"),
        [] as [String; 0]
    );
    let named = "\
extension type const A.named(int it) {}

@A.named(42)
void f() {}
";
    let report = report_ops(named);
    assert_eq!(code_rows(&report, "invalid_annotation"), [] as [String; 0]);
    assert_eq!(
        code_rows(&report, "non_constant_annotation_constructor"),
        [] as [String; 0]
    );
}

/// The mixin-application forwarding shape (upstream
/// `MetadataResolutionTest.test_value_genericMixinApplication_inference_
/// unnamedConstructor_functionTypedFormalParameter`): `class B = A with
/// M;` declares no constructor — its unnamed face is the synthesized
/// FORWARDING constructor, const exactly when the superclass's own
/// unnamed is. The application's own `has_const_constructor` aggregate
/// counts only declared constructors and stays false; reading it
/// over-reported `non_constant_annotation_constructor` the moment the
/// parameter-position drain exposed the verdict.
#[test]
fn mixin_application_forwards_superclass_const_ctor() {
    let source = "\
class A<T> {
  final T f;
  const A(this.f);
}
mixin M {}
class B<T> = A<T> with M;

f(@B(42) g()) {}
@B(42)
int top = 0;
";
    let report = report_ops(source);
    assert_eq!(
        code_rows(&report, "non_constant_annotation_constructor"),
        [] as [String; 0]
    );
    assert_eq!(code_rows(&report, "invalid_annotation"), [] as [String; 0]);
}

/// The non-const superclass twin: the forwarding unnamed inherits the
/// non-const answer.
#[test]
fn mixin_application_forwards_superclass_non_const_ctor() {
    let source = "\
class A {
  A();
}
mixin M {}
class B = A with M;

@B(42)
void f() {}
";
    assert_eq!(
        code_rows(&report_ops(source), "non_constant_annotation_constructor"),
        ["Annotation creation can only call a const constructor."]
    );
}

/// The forward-reference shape of the same aggregate: the site table
/// fills in parse order, so an annotation above the class's own `const`
/// constructor reads the model's aggregate — const, silent (and the
/// no-argument spelling keeps `no_annotation_constructor_arguments`).
#[test]
fn forward_referenced_const_ctor_reads_the_aggregate() {
    let with_args = "\
@A()
void f() {}

class A {
  const A();
}
";
    let report = report_ops(with_args);
    assert_eq!(
        code_rows(&report, "non_constant_annotation_constructor"),
        [] as [String; 0]
    );
    assert_eq!(code_rows(&report, "invalid_annotation"), [] as [String; 0]);
    let no_args = "\
@A
void f() {}

class A {
  const A();
}
";
    assert_eq!(
        code_rows(&report_ops(no_args), "no_annotation_constructor_arguments"),
        [NO_CTOR_ARGS]
    );
}

// —— the mixin no-argument static-getter face ————————————————————————

/// Upstream `SearchTest.test_searchReferences_MixinElement_reference_
/// annotation`: a mixin is no `InterfaceElementImpl` — its constructor
/// invocation face stays `invalidAnnotation`, but the no-argument
/// `@M.g` resolves the full dotted name to the static accessor itself
/// (the const-variable question, silent for a static const).
#[test]
fn mixin_static_const_getter_annotation() {
    let getter = "\
mixin A {
  static const int myConstant = 0;
}

@A.myConstant
void f() {}
";
    let report = report_ops(getter);
    assert_eq!(code_rows(&report, "invalid_annotation"), [] as [String; 0]);
    assert_eq!(
        code_rows(&report, "undefined_annotation"),
        [] as [String; 0]
    );
    let invocation = "\
mixin A {}

@A(0)
void f() {}
";
    assert_eq!(
        code_rows(&report_ops(invocation), "invalid_annotation"),
        [INVALID_ANNOTATION]
    );
}

// —— the alias owner-library scope ———————————————————————————————————

/// Upstream `MetadataResolutionTest.test_value_prefix_typeAlias_class_
/// staticConstField`: `@prefix.B.foo` over an imported
/// `typedef B = A;` — the aliased class `A` is named in the ALIAS
/// OWNER's library, not the annotation site's; resolving in the site's
/// library missed the class and over-reported `invalidAnnotation`.
#[test]
fn prefixed_alias_resolves_in_owner_library() {
    let imports = [RigImport {
        uri: "package:t/a.dart",
        prefix: Some("p"),
        source: "typedef B = A;\nclass A {\n  static const int foo = 42;\n}\n",
    }];
    let source = "\
@p.B.foo
void f() {}
";
    let report = report_ops_imports(source, &imports);
    assert_eq!(code_rows(&report, "invalid_annotation"), [] as [String; 0]);
    assert_eq!(
        code_rows(&report, "undefined_annotation"),
        [] as [String; 0]
    );
}

// —— the multiply-defined fall-through ——————————————————————————————

/// Upstream `AmbiguousImportTest.test_annotation_getter`: an ambiguous
/// import head resolves to a multiply-defined element that matches none
/// of the resolver cascade's concrete kinds — `invalidAnnotation` on
/// top of the `ambiguousImport` row (the import family's own row).
#[test]
fn ambiguous_annotation_head_falls_through_invalid() {
    let imports = [
        RigImport {
            uri: "package:t/a.dart",
            prefix: None,
            source: "const foo = 0;\n",
        },
        RigImport {
            uri: "package:t/b.dart",
            prefix: None,
            source: "const foo = 0;\n",
        },
    ];
    let source = "\
@foo
class A {}
";
    assert_eq!(
        code_rows(&report_ops_imports(source, &imports), "invalid_annotation"),
        [INVALID_ANNOTATION]
    );
}
