use dartq_ast::NodeKind;

use super::{Check, NONE_TOKEN, PayloadField, RuleSpec, last_child};
use crate::rules::utf16_token_span;

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/no_default_cases.dart` — a `default`
/// member of a switch statement whose scrutinee's static type is an
/// interface type whose element is an enum (or an enum-like class)
/// reports at the member (:42-61): the first default only (the walk
/// returns there), and a switch without a default never reports —
/// the lint flags the clause's presence, not exhaustive completion.
/// No feature gate: the registration consults no `isFeatureEnabled`
/// despite the experimental state.
///
/// Faces over the syntax substrate: the enum verdict keys on the
/// scrutinee root identifier's same-file value binding (the
/// shadow-correct innermost walk) whose written type names a
/// same-file [`NodeKind::EnumDeclaration`] — `void f(E e) { switch
/// (e) … }` against a same-file `enum E`, the local, parameter, and
/// implicit-`this` field faces alike. Under-reports (type-system
/// faces the substrate cannot see): imported enums, inferred
/// bindings (`var e = E.a`), non-root spellings (`switch (e.value)`,
/// `switch (E.a)`), and the official enum-like class / extension
/// type faces (`isEnumLikeType`). A bare enum NAME scrutinee stays
/// silent here matching the official — a type literal's static type
/// is `Type`, not the enum. Over-reports: none — a binding's written
/// type is its static type. Message: `diagnostic.g.dart`
/// `noDefaultCases`.
pub(crate) static SPEC: RuleSpec =
    crate::spec!("no_default_cases", &[NodeKind::SwitchStatement], check,);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // The scrutinee run over the statement tiling (the adopted-begin
    // leak rows before the condition container, then the container's
    // window; the `switch_on_type` recipe): the expression root is
    // the run's last row.
    // The scrutinee root over the statement tiling (the adopted-begin
    // leak rows before the condition container, then the container's
    // window; the `switch_on_type` recipe). The bare-identifier
    // scrutinee tiles as the send group `[Identifier, wrappers,
    // NoArguments, Send]` straddling the container, so the root is the
    // run's single non-marker row: any other shape (a member chain, a
    // call) is a non-root spelling and stays silent.
    let kids = c.collect_direct(node);
    let Some(position) = kids
        .iter()
        .position(|&kid| c.kind(kid) == NodeKind::ParenthesizedCondition)
    else {
        return;
    };
    let container = kids[position];
    // A `case` clause in the switch head is not the lint's face (the
    // container's `CaseKeyword` word carries the `case` token when
    // present).
    if c.field(container, PayloadField::CaseKeyword)
        .is_some_and(|case| case != NONE_TOKEN)
    {
        return;
    }
    let run: Vec<u32> = kids[..position]
        .iter()
        .copied()
        .chain(c.children(container))
        .filter(|&row| !crate::rules::slice::is_send_group_piece(c, row))
        .collect();
    let [root] = run.as_slice() else {
        return;
    };
    if c.kind(*root) != NodeKind::Identifier {
        return;
    }
    let name = c.lexeme(c.head_token(*root));
    // The binding's written type names the scrutinee's static type at
    // the syntax level; untyped (`var`, dynamic) bails.
    let Some(type_name) = crate::rules::expressions::same_file::written_type(c, *root, name) else {
        return;
    };
    let Some(declaration) = crate::rules::same_file_declaration(c, type_name) else {
        return;
    };
    if c.kind(declaration) != NodeKind::EnumDeclaration {
        return;
    }
    // The members ride the `SwitchBlock` child; the first `default`
    // member reports (payload word 0 is the `default` keyword, NONE
    // for a `case` — the official `member is SwitchDefault` face).
    let Some(block) = last_child(c, node).filter(|&block| c.kind(block) == NodeKind::SwitchBlock)
    else {
        return;
    };
    let Some(default_kw) = c
        .collect_direct(block)
        .into_iter()
        .find(|&member| {
            c.kind(member) == NodeKind::SwitchCase
                && c.field(member, PayloadField::DefaultKeyword)
                    .is_some_and(|kw| kw != NONE_TOKEN)
        })
        .and_then(|member| c.field(member, PayloadField::DefaultKeyword))
    else {
        return;
    };
    c.report(utf16_token_span(c, default_kw), &[]);
}

#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

    fn run(source: &str) -> Vec<crate::engine::Lint> {
        Fixture::parse(source).run(&one_rule("no_default_cases"))
    }

    /// `test_enum` — the exhaustive-enum shape: a parameter written
    /// with a same-file enum type, message and report verbatim.
    #[test]
    fn enum_parameter_default_reports() {
        let source = "void f(E e) {\n  switch(e) {\n    case E.a :\n      print('a');\n      break;\n    default:\n      print('default');\n  }\n}\n\nenum E {\n  a, b, c;\n}\n";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("no_default_cases"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Invalid use of 'default' member in a switch."
        );
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try enumerating all the possible values of the switch expression.")
        );
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(source, &f.tokens, "default", 0))
        );
    }

    /// The local-binding face: a local written with the enum type.
    #[test]
    fn enum_typed_local_reports() {
        let source = "enum E { a, b }\n\nvoid f() {\n  E e = E.a;\n  switch (e) {\n    default:\n      break;\n  }\n}\n";
        assert_eq!(run(source).len(), 1);
    }

    /// The implicit-`this` field face: an instance field written with
    /// the enum type, switched bare inside a method.
    #[test]
    fn enum_typed_field_reports() {
        let source = "enum Color { r, g }\n\nclass X {\n  Color c;\n  X(this.c);\n  void m() {\n    switch (c) {\n      default:\n        break;\n    }\n  }\n}\n";
        assert_eq!(run(source).len(), 1);
    }

    /// `test_class_notEnumLike` — a non-enum scrutinee stays silent.
    #[test]
    fn non_enum_scrutinee_is_silent() {
        let source = "void f(int i) {\n  switch (i) {\n    case 1 :\n      print('1');\n      break;\n    default:\n      print('default');\n  }\n}\n";
        assert!(run(source).is_empty());
    }

    /// A same-file CLASS-typed scrutinee stays silent (the official
    /// enum element face; the enum-like class is the documented
    /// under-report).
    #[test]
    fn class_typed_scrutinee_is_silent() {
        let source = "class C {\n  const C();\n}\n\nvoid f(C c) {\n  switch (c) {\n    default:\n      break;\n  }\n}\n";
        assert!(run(source).is_empty());
    }

    /// A switch without a default never reports (the lint flags the
    /// clause's presence).
    #[test]
    fn no_default_clause_is_silent() {
        let source = "enum E { a, b }\n\nvoid f(E e) {\n  switch (e) {\n    case E.a:\n      break;\n    case E.b:\n      break;\n  }\n}\n";
        assert!(run(source).is_empty());
    }

    /// An untyped binding (dynamic) stays silent.
    #[test]
    fn untyped_binding_is_silent() {
        let source =
            "enum E { a, b }\n\nvoid f(e) {\n  switch (e) {\n    default:\n      break;\n  }\n}\n";
        assert!(run(source).is_empty());
    }

    /// The unknown-type under-report face: no same-file enum named
    /// `E` — an imported enum's shape — stays silent.
    #[test]
    fn unknown_type_name_is_silent() {
        let source = "void f(E e) {\n  switch (e) {\n    default:\n      break;\n  }\n}\n";
        assert!(run(source).is_empty());
    }

    /// A bare enum-name scrutinee stays silent — matching the
    /// official, a type literal's static type is `Type`.
    #[test]
    fn bare_enum_name_scrutinee_is_silent() {
        let source =
            "enum E { a, b }\n\nvoid f() {\n  switch (E) {\n    default:\n      break;\n  }\n}\n";
        assert!(run(source).is_empty());
    }
}
