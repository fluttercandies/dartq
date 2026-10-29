use super::{Check, NodeKind, RuleSpec, same_file, slice};

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/do_not_use_environment.dart`
/// — an instance creation whose static type is `bool`/`int`/`String`
/// with the `fromEnvironment` named constructor, or `bool` with
/// `hasEnvironment`, reports at the constructor name
/// (`reportIfUsingEnvironment` :36-48). The official dispatch faces
/// are the resolved creation and the dot-shorthand invocation (:50-77);
/// the dartq face is the invocation send: a bare core-type receiver
/// (`bool`/`int`/`String`) before a `fromEnvironment`/`hasEnvironment`
/// selector. The element-model gates are the same-file channel: the
/// receiver must not bind to a same-file value and the unit must not
/// declare a type of the same name (the shadowed family stays silent,
/// matching the official's static-type face). The factory-constructor
/// gate (:65) is element-model and skipped.
/// Approximations (documented, under-report): user classes spelling a
/// `fromEnvironment` constructor under-report; the dot-shorthand form
/// (`.fromEnvironment`) under-report. Message: `diagnostic.g.dart`
/// `doNotUseEnvironment`.
pub(crate) static SPEC: RuleSpec =
    crate::spec!("do_not_use_environment", &[NodeKind::Send], check,);

fn check(c: &mut Check<'_>) {
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
    let Some((head, Some(_arguments))) = slice::send_parts(c, &kids, pos) else {
        return;
    };
    // The selector carries the constructor name; the receiver is the
    // bare core-type name before it — the production tiling wraps it
    // in the tear-off group ([`slice::bare_receiver`]), so the read
    // goes through the group face, not raw sibling arithmetic.
    let selector = c.lexeme(c.head_token(kids[head]));
    let Some(receiver) = slice::bare_receiver(c, &kids, head) else {
        return;
    };
    let receiver_name = c.lexeme(c.head_token(receiver));
    let paired = match selector {
        "fromEnvironment" => matches!(receiver_name, "bool" | "int" | "String"),
        "hasEnvironment" => receiver_name == "bool",
        _ => false,
    };
    if !paired {
        return;
    }
    // The shadow gates: a same-file value binding or a same-file type
    // declaration of the receiver's name is not the core type.
    if same_file::binding(c, c.node(), receiver_name).is_some()
        || same_file::declares_type(c, receiver_name)
    {
        return;
    }
    // The official reports at the constructor name — the receiver
    // through the selector.
    let range = c.utf16_span(receiver).start..c.utf16_span(kids[head]).end;
    c.report(range, &[]);
}

#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule};

    fn run(source: &str) -> Vec<crate::engine::Lint> {
        Fixture::parse(source).run(&one_rule("do_not_use_environment"))
    }

    /// The paired constructor faces (`bool`/`int`/`String` +
    /// `fromEnvironment`, `bool` + `hasEnvironment`) report at the
    /// constructor name; other constructors and cross pairs stay
    /// silent (`test_environmentNoForbidden` shapes).
    #[test]
    fn environment_constructor_faces() {
        let lints = run("void f() {\n  var k = bool.fromEnvironment('k');\n}\n");
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Invalid use of an environment declaration."
        );

        assert_eq!(
            run("void f() {\n  var k = int.fromEnvironment('k');\n}\n").len(),
            1
        );
        assert_eq!(
            run("void f() {\n  var k = String.fromEnvironment('k');\n}\n").len(),
            1
        );
        assert_eq!(
            run("void f() {\n  var k = bool.hasEnvironment('k');\n}\n").len(),
            1
        );

        for source in [
            "void f() {\n  var k = bool.parse('true');\n}\n",
            "void f() {\n  var k = int.hasEnvironment('k');\n}\n",
            "void f() {\n  var k = value.fromEnvironment('k');\n}\n",
        ] {
            assert!(run(source).is_empty(), "{source}");
        }
    }
}
