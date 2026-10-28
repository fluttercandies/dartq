use dartq_ast::NodeKind;

use super::{Check, RuleSpec};
use crate::rules::utf16_token_span;

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/unnecessary_underscores.dart` — a formal
/// parameter or local variable whose name is two or more underscores
/// reports at the name when the body never references it: with the
/// wildcard-variables feature (Dart 3.7) a single `_` binds nothing,
/// so the extra underscores carry no meaning (the registration bails
/// without the feature, :33 — the gate consumes the
/// [`crate::engine::Features::wildcard_variables`] bit). Receiver
/// formals and primary-constructor declaring formals skip (the
/// `isDeclaring` field face, :60-62), as do fields, top-level
/// variables, and every declaration that is not a parameter or local
/// (:59, :106).
///
/// Approximation (documented): the official reference test runs over
/// elements; the name-level face here counts any same-lexeme
/// `Identifier` in the enclosing function-like declaration's subtree
/// — a property access of the same name (`obj.__`) over-counts
/// (conservative, under-reports). The parameter face also reads the
/// declaration's own doc-comment `[name]` references
/// (collectReferences' `documentationComment`, :76-94); comment
/// references of other declarations are invisible to it. Message:
/// `diagnostic.g.dart` `unnecessaryUnderscores`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_underscores",
    &[NodeKind::FormalParameters, NodeKind::VariablesDeclaration],
    check,
);

fn check(c: &mut Check<'_>) {
    // The registration bail (:33): the feature is what turns the
    // multi-underscore name into a plain spelling of `_`.
    if !c.unit.features.wildcard_variables {
        return;
    }
    match c.kind(c.node()) {
        NodeKind::FormalParameters => parameters(c),
        NodeKind::VariablesDeclaration => locals(c),
        _ => {}
    }
}

/// The parameter face (:53-101): every plain formal whose name is two
/// or more underscores and unreferenced in the enclosing declaration
/// reports at the name token.
fn parameters(c: &mut Check<'_>) {
    // Primary-constructor formals declare fields — the `isDeclaring`
    // field face skips them (:60-62, the primary-constructor corpus).
    if c.path
        .iter()
        .rev()
        .skip(1)
        .take(3)
        .any(|&a| c.kind(a) == NodeKind::PrimaryConstructor)
    {
        return;
    }
    let Some(owner_row) = function_owner(c) else {
        return;
    };
    let owner_head = c.head_token(owner_row);
    let node = c.node();
    for param in c.collect_direct(node) {
        if c.kind(param) != NodeKind::FormalParameter {
            continue;
        }
        // A receiver formal (`this.__` / `super.__`) declares or
        // forwards a member — never a plain parameter (:60-62).
        if c.payload(param).first().copied() != Some(crate::rules::NONE_TOKEN) {
            continue;
        }
        let Some(&name) = c.payload(param).get(1) else {
            continue;
        };
        if !is_multi_underscore(c, name) || referenced(c, owner_row, name, owner_head, true) {
            continue;
        }
        c.report(utf16_token_span(c, name), &[]);
    }
}

/// The local face (:104-114): an `InitializedIdentifier` of a
/// `VariablesDeclaration` that is not a field or top-level variable,
/// named two or more underscores and unreferenced in the enclosing
/// declaration, reports at the name token.
fn locals(c: &mut Check<'_>) {
    // Fields and top-level variables are their own elements, never
    // locals (:59, :106).
    if c.path
        .iter()
        .any(|&a| matches!(c.kind(a), NodeKind::Fields | NodeKind::TopLevelFields))
    {
        return;
    }
    let Some(owner_row) = function_owner(c) else {
        return;
    };
    let owner_head = c.head_token(owner_row);
    let node = c.node();
    for var in c.collect_direct(node) {
        if c.kind(var) != NodeKind::InitializedIdentifier {
            continue;
        }
        let Some(name) = crate::rules::initialized_identifier_name(c, var) else {
            continue;
        };
        if !is_multi_underscore(c, name) || referenced(c, owner_row, name, owner_head, false) {
            continue;
        }
        c.report(utf16_token_span(c, name), &[]);
    }
}

/// `isJustUnderscores` (:129-130): a name of two or more underscores.
fn is_multi_underscore(c: &Check<'_>, name: u32) -> bool {
    let lexeme = c.lexeme(name);
    lexeme.len() > 1 && lexeme.bytes().all(|byte| byte == b'_')
}

/// The enclosing function-like declaration the official reference
/// walks walks out to (`ConstructorDeclaration` / `FunctionDeclaration`
/// / `MethodDeclaration`, :71-94).
fn function_owner(c: &Check<'_>) -> Option<u32> {
    c.path.iter().rev().skip(1).copied().find(|&a| {
        matches!(
            c.kind(a),
            NodeKind::Method
                | NodeKind::TopLevelMethod
                | NodeKind::Constructor
                | NodeKind::Factory
                | NodeKind::LocalFunctionDeclaration
                | NodeKind::FunctionExpression
        )
    })
}

/// The name-level reference test: any same-lexeme `Identifier` in the
/// owner's subtree except the declaration's own name row; the
/// parameter face additionally reads the owner's doc comment
/// (collectReferences' `documentationComment`, :79-94).
fn referenced(c: &Check<'_>, owner: u32, name: u32, owner_head: u32, doc_face: bool) -> bool {
    let lexeme = c.lexeme(name);
    for row in c.children(owner) {
        if c.kind(row) == NodeKind::Identifier
            && c.head_token(row) != name
            && c.lexeme(c.head_token(row)) == lexeme
        {
            return true;
        }
    }
    if doc_face {
        for comment in c.comments.attached(owner_head as usize) {
            let text = &c.unit.source[comment.range.start as usize..comment.range.end as usize];
            if text.contains(&format!("[{lexeme}]")) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

    fn run(source: &str) -> Vec<crate::engine::Lint> {
        Fixture::parse(source).run(&one_rule("unnecessary_underscores"))
    }

    /// `test_function_parameter_unused`.
    #[test]
    fn unused_multi_underscore_parameter_reports() {
        let lints = run("void f(int _, int __) {}");
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Unnecessary use of multiple underscores.");
        assert_eq!(lints[0].correction.as_deref(), Some("Try using '_'."));
    }

    /// `test_function_parameter_used` — the reference keeps the name.
    #[test]
    fn used_parameter_is_silent() {
        assert!(run("void f(int _, int __) { print(__); }").is_empty());
    }

    /// `test_local_unused` / `test_local_used`.
    #[test]
    fn local_reports_when_unused_and_is_silent_when_used() {
        assert_eq!(run("void f() { var __ = 0; }").len(), 1);
        assert!(run("void f() { var __ = 0; print(__); }").is_empty());
    }

    /// `test_forPart_unused` / `test_forPart_used` — for-parts
    /// declarations follow the local face.
    #[test]
    fn for_part_declaration_follows_the_local_face() {
        assert_eq!(run("void f() { for (var __ = 0; ; ) {} }").len(), 1);
        assert!(run("void f() { for (var __ = 0; ; ++__) {} }").is_empty());
    }

    /// `test_field_unused` / `test_topLevelVariable_unused` /
    /// `test_enum_field_unused` — fields, top-level variables, and
    /// enum elements are not the lint's face.
    #[test]
    fn fields_and_top_level_variables_are_silent() {
        assert!(run("class C { int __ = 0; }").is_empty());
        assert!(run("int __ = 0;").is_empty());
        assert!(run("enum E { __, }").is_empty());
    }

    /// `test_method_unused` / `test_topLevelFunction_unused` /
    /// `test_localFunction_unused` — declaration names are not the
    /// lint's face.
    #[test]
    fn declaration_names_are_silent() {
        assert!(run("class A { __() {} }").is_empty());
        assert!(run("__() {}").is_empty());
        assert!(run("f() { __() {} }").is_empty());
    }

    /// `test_function_parameter_unused_preWildcards` — the feature
    /// gate (:33).
    #[test]
    fn wildcard_feature_off_is_silent() {
        let mut f = Fixture::parse("void f(int _, int __) {}");
        f.features.wildcard_variables = false;
        assert!(f.run(&one_rule("unnecessary_underscores")).is_empty());
    }

    /// `test_commentReferenceFunction` — a doc-comment `[__]`
    /// reference keeps the parameter name.
    #[test]
    fn doc_comment_reference_is_silent() {
        assert!(run("/// This is a reference to [__].\nvoid f(int __) {}").is_empty());
    }

    /// `test_parameter_declaring_unused` — primary-constructor
    /// declaring formals skip.
    #[test]
    fn primary_constructor_formals_are_silent() {
        assert!(run("class C(final int _, final int __);").is_empty());
    }

    /// `test_localFunction_parameter_unused`.
    #[test]
    fn local_function_parameter_reports() {
        assert_eq!(run("void f() { g(int __) {} }").len(), 1);
    }

    /// The report pins the parameter name token.
    #[test]
    fn report_pins_the_name_token() {
        let source = "void f(int __) {}";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("unnecessary_underscores"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(source, &f.tokens, "__", 0))
        );
    }
}
