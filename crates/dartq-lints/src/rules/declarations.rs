//! The declaration-shape family: `avoid_return_types_on_setters`,
//! `unnecessary_late`, `use_function_type_syntax_for_parameters`,
//! `prefer_typing_uninitialized_variables`.

use dartq_ast::NodeKind;

use crate::engine::Check;
use crate::registry::{Group, RuleSpec};
use crate::rules::{first_child, member_name, type_before, NONE_TOKEN};

pub(crate) mod avoid_return_types_on_setters {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/avoid_return_types_on_setters.dart:38-56`
    /// — a setter with an explicit return type reports at the type.
    /// Setters carry the `set` keyword as their `get_set` payload
    /// (collapse map §3.3 Method/TopLevelMethod rows). Message:
    /// `diagnostic.g.dart` `avoidReturnTypesOnSetters`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "avoid_return_types_on_setters",
        group: Group::Recommended,
        problem: "Unnecessary return type on a setter.",
        correction: "Try removing the return type.",
        kinds: &[NodeKind::Method, NodeKind::TopLevelMethod],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        if super::get_set_token(c, node) != Some("set") {
            return;
        }
        let Some(name) = super::member_name(c, node) else {
            return;
        };
        let Some((type_node, kind)) = super::type_before(c, node, name) else {
            return;
        };
        if kind == NodeKind::NoType {
            return;
        }
        let range = c.utf16_span(type_node);
        c.report(range, &[]);
    }
}

pub(crate) mod unnecessary_late {
    use super::{Check, Group, NodeKind, RuleSpec};
    use dartq_parser::MemberKind;

    /// Official: `pkg/linter/lib/src/rules/unnecessary_late.dart:36-55` —
    /// `late` on a static field or top-level variable where every variable
    /// of the declaration has an initializer reports at `late`. Instance
    /// fields are excluded (lazy initialization is meaningful there). The
    /// `late` keyword is the declaration's span head when present
    /// (collapse map §3.3 VariablesDeclaration row, `head_is`). Message:
    /// `diagnostic.g.dart` `unnecessaryLate`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "unnecessary_late",
        group: Group::Recommended,
        problem: "Unnecessary 'late' modifier.",
        correction: "Try removing the 'late'.",
        kinds: &[NodeKind::VariablesDeclaration],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `late` ⟺ span head is the keyword (#174 elimination).
        let head = c.head_token(node);
        if c.lexeme(head) != "late" {
            return;
        }
        // Static fields and top-level variables only (:36-45).
        let Some(parent) = c.ancestor(1) else {
            return;
        };
        match c.kind(parent) {
            NodeKind::TopLevelFields => {}
            NodeKind::Fields => {
                // Fields payload word 0 is the `MemberKind` discriminant
                // (§3.3 FieldsEnd row): only the static kind reports.
                if c.payload(parent).first() != Some(&(MemberKind::StaticField as u32)) {
                    return;
                }
            }
            _ => return,
        }
        // Every variable must have an initializer (:51-54: any missing
        // initializer skips the report).
        for var in c.direct_children(node).iter().copied() {
            if c.kind(var) == NodeKind::InitializedIdentifier
                && !has_initializer(c, var)
            {
                return;
            }
        }
        let range = c.utf16_span(node);
        let start = range.start;
        c.report(start..start, &[]);
    }

    /// An `InitializedIdentifier` has an initializer iff a
    /// `VariableInitializer` child is present (`NoVariableInitializer`
    /// collapses to absence, collapse map §3.3 D1 rows).
    fn has_initializer(c: &Check<'_>, var: u32) -> bool {
        c.children(var)
            .any(|ch| c.kind(ch) == NodeKind::VariableInitializer)
    }
}

pub(crate) mod use_function_type_syntax_for_parameters {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/use_function_type_syntax_for_parameters.dart:34-49`
    /// — a formal parameter written with the old function syntax
    /// (`void f()`) reports at the parameter. The old syntax is its own AST
    /// shape: [`NodeKind::FunctionTypedFormalParameter`] (collapse map
    /// §3.3). Message: `diagnostic.g.dart`
    /// `useFunctionTypeSyntaxForParameters`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "use_function_type_syntax_for_parameters",
        group: Group::Recommended,
        problem: "Use the generic function type syntax to declare the parameter '{0}'.",
        correction: "Try using the generic function type syntax.",
        kinds: &[NodeKind::FunctionTypedFormalParameter],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `node.name?.lexeme ?? ''` (:44).
        let name = super::member_name(c, node)
            .filter(|&n| c.kind(n) == NodeKind::Identifier)
            .map(|n| c.lexeme(c.head_token(n)))
            .unwrap_or("");
        let range = c.utf16_span(node);
        c.report(range, &[name]);
    }
}

pub(crate) mod prefer_typing_uninitialized_variables {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/prefer_typing_uninitialized_variables.dart:31-51`
    /// — an untyped declaration (`node.type == null`) with an uninitialized
    /// variable reports at the variable; fields and locals carry different
    /// message texts (`...ForField` / `...ForLocalVariable`,
    /// `diagnostic.g.dart:2839/2852`). The type is the child before the
    /// first variable (`NoType` = absent, collapse map §3.3). Core set
    /// (`.third/lints/lib/core.yaml`).
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_typing_uninitialized_variables",
        group: Group::Core,
        problem: "An uninitialized variable should have an explicit type annotation.",
        correction: "Try adding a type annotation.",
        kinds: &[NodeKind::VariablesDeclaration],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `node.type != null` skips the whole declaration (:34).
        match super::first_child(c, node) {
            Some(first) if c.kind(first) == NodeKind::NoType => {}
            Some(_) => return,
            None => return,
        }
        let is_field = c.ancestor(1).is_some_and(|parent| {
            matches!(c.kind(parent), NodeKind::Fields | NodeKind::TopLevelFields)
        });
        // Gather untyped variables under the scratch borrow, then report
        // (the report path needs `&mut Check`); allocates only on hits.
        let mut hits: Vec<u32> = Vec::new();
        for var in c.direct_children(node).iter().copied() {
            if c.kind(var) != NodeKind::InitializedIdentifier {
                continue;
            }
            let initialized = c
                .children(var)
                .any(|ch| c.kind(ch) == NodeKind::VariableInitializer);
            if !initialized {
                hits.push(var);
            }
        }
        for var in hits {
            if is_field {
                c.report_texts(
                    c.utf16_span(var),
                    "An uninitialized field should have an explicit type annotation.",
                    "Try adding a type annotation.",
                    &[],
                );
            } else {
                let range = c.utf16_span(var);
                c.report(range, &[]);
            }
        }
    }
}

// —— shared helpers ———————————————————————————————————————————————————————

/// The `get`/`set` keyword token of a member declaration, by lexeme.
/// `Method` payload word 1 and `TopLevelMethod` payload word 0 are the
/// `get_set` option (collapse map §3.3).
fn get_set_token<'a>(c: &'a Check<'_>, node: u32) -> Option<&'a str> {
    let word = match c.kind(node) {
        NodeKind::Method => 1,
        NodeKind::TopLevelMethod => 0,
        _ => return None,
    };
    match c.payload(node).get(word) {
        Some(&token) if token != NONE_TOKEN => Some(c.lexeme(token)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use dartq_ast::NodeKind;

    use crate::testsupport::{identifier, NONE, one_rule, tok, utf16_at, Fixture, unit_of};
    use dartq_parser::MemberKind;

    /// `test/rules/avoid_return_types_on_setters_test.dart` `test_typedSet`.
    #[test]
    fn avoid_return_types_reports_a_typed_setter() {
        let source = "void set x(v) {}";
        // void0 set1 x2 (3 v4 )5 {6 }7
        let f = Fixture::build(source, unit_of(|b, t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, tok(source, t, "set", 0));
            b.leaf_span(NodeKind::Type, 0, 1);
            identifier(b, 2, 3);
            b.begin(NodeKind::FormalParameters, 3);
            b.end(NodeKind::FormalParameters, 6);
            b.begin(NodeKind::Block, 6);
            b.end(NodeKind::Block, 8);
            b.end(NodeKind::TopLevelMethod, 8);
        }));
        let lints = f.run(&one_rule("avoid_return_types_on_setters"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Unnecessary return type on a setter.");
        assert_eq!(lints[0].range.start, utf16_at(&f, 0));
        assert_eq!(lints[0].range.end, utf16_at(&f, 0) + 4); // after `void`
    }

    #[test]
    fn avoid_return_types_untyped_setter_passes() {
        let source = "set x(v) {}";
        let f = Fixture::build(source, unit_of(|b, t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, tok(source, t, "set", 0));
            b.leaf(NodeKind::NoType, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::FormalParameters, 2);
            b.end(NodeKind::FormalParameters, 5);
            b.begin(NodeKind::Block, 5);
            b.end(NodeKind::Block, 7);
            b.end(NodeKind::TopLevelMethod, 7);
        }));
        assert!(
            f.run(&one_rule("avoid_return_types_on_setters"))
                .is_empty()
        );
    }

    /// Getters never report — only the `set` keyword qualifies.
    #[test]
    fn avoid_return_types_getter_passes() {
        let source = "int get x => 1;";
        // int0 get1 x2 =>3 14 ;5
        let f = Fixture::build(source, unit_of(|b, t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, tok(source, t, "get", 0));
            b.leaf_span(NodeKind::Type, 0, 1);
            identifier(b, 2, 3);
            b.leaf(NodeKind::LiteralInt, 4);
            b.end(NodeKind::TopLevelMethod, 6);
        }));
        assert!(
            f.run(&one_rule("avoid_return_types_on_setters"))
                .is_empty()
        );
    }

    /// `test/rules/unnecessary_late_test.dart` `test_topLevelVariable`.
    #[test]
    fn unnecessary_late_reports_a_top_level_variable() {
        let source = "late var x = 1;";
        // late0 var1 x2 =3 14 ;5 — `late` is the span head (#174)
        let f = Fixture::build(source, unit_of(|b, t| {
            b.begin(NodeKind::TopLevelFields, 0);
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, tok(source, t, "var", 0));
            b.begin(NodeKind::InitializedIdentifier, 2);
            b.begin(NodeKind::VariableInitializer, 3);
            b.leaf(NodeKind::LiteralInt, 4);
            b.end(NodeKind::VariableInitializer, 5);
            b.end(NodeKind::InitializedIdentifier, 5);
            b.end(NodeKind::VariablesDeclaration, 6);
            b.end(NodeKind::TopLevelFields, 6);
        }));
        let lints = f.run(&one_rule("unnecessary_late"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Unnecessary 'late' modifier.");
        assert_eq!(lints[0].range.start, utf16_at(&f, 0));
    }

    /// An uninitialized declaration keeps `late` meaningful (:51-54).
    #[test]
    fn unnecessary_late_uninitialized_passes() {
        let source = "late var x;";
        let f = Fixture::build(source, unit_of(|b, t| {
            b.begin(NodeKind::TopLevelFields, 0);
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, tok(source, t, "var", 0));
            b.begin(NodeKind::InitializedIdentifier, 2);
            b.end(NodeKind::InitializedIdentifier, 3);
            b.end(NodeKind::VariablesDeclaration, 3);
            b.end(NodeKind::TopLevelFields, 3);
        }));
        assert!(f.run(&one_rule("unnecessary_late")).is_empty());
    }

    /// Instance fields are excluded — lazy init is meaningful there.
    #[test]
    fn unnecessary_late_instance_field_passes() {
        let source = "class A { late var x = 1; }";
        // class0 A1 {2 late3 var4 x5 =6 17 ;8 }9
        let f = Fixture::build(source, unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Fields, 2);
            b.patch(0, MemberKind::NonStaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, 4);
            b.begin(NodeKind::InitializedIdentifier, 5);
            b.begin(NodeKind::VariableInitializer, 6);
            b.leaf(NodeKind::LiteralInt, 7);
            b.end(NodeKind::VariableInitializer, 8);
            b.end(NodeKind::InitializedIdentifier, 8);
            b.end(NodeKind::VariablesDeclaration, 9);
            b.end(NodeKind::Fields, 9);
            b.end(NodeKind::DeclarationBody, 10);
            b.end(NodeKind::ClassDeclaration, 10);
            b.end(NodeKind::TopLevelDeclaration, 10);
        }));
        assert!(f.run(&one_rule("unnecessary_late")).is_empty());
    }

    /// Static fields report (`unnecessary_late.dart:36-45`).
    #[test]
    fn unnecessary_late_static_field_reports() {
        let source = "class A { late var x = 1; }";
        let f = Fixture::build(source, unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Fields, 2);
            b.patch(0, MemberKind::StaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, 4);
            b.begin(NodeKind::InitializedIdentifier, 5);
            b.begin(NodeKind::VariableInitializer, 6);
            b.leaf(NodeKind::LiteralInt, 7);
            b.end(NodeKind::VariableInitializer, 8);
            b.end(NodeKind::InitializedIdentifier, 8);
            b.end(NodeKind::VariablesDeclaration, 9);
            b.end(NodeKind::Fields, 9);
            b.end(NodeKind::DeclarationBody, 10);
            b.end(NodeKind::ClassDeclaration, 10);
            b.end(NodeKind::TopLevelDeclaration, 10);
        }));
        let lints = f.run(&one_rule("unnecessary_late"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 3));
    }

    /// `test/rules/use_function_type_syntax_for_parameters_test.dart`
    /// `test_functionTypedParameter`.
    #[test]
    fn function_type_syntax_reports_the_old_form() {
        let source = "f(void g()) {}";
        // f0 (1 void2 g3 (4 )5 )6 {7 }8
        let f = Fixture::build(source, unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, NONE);
            b.leaf(NodeKind::NoType, 0);
            identifier(b, 0, 1);
            b.begin(NodeKind::FormalParameters, 1);
            b.begin(NodeKind::FunctionTypedFormalParameter, 2);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Type, 2, 3);
            identifier(b, 3, 4);
            b.begin(NodeKind::FormalParameters, 4);
            b.end(NodeKind::FormalParameters, 6);
            b.end(NodeKind::FunctionTypedFormalParameter, 6);
            b.end(NodeKind::FormalParameters, 7);
            b.begin(NodeKind::Block, 7);
            b.end(NodeKind::Block, 9);
            b.end(NodeKind::TopLevelMethod, 9);
        }));
        let lints = f.run(&one_rule("use_function_type_syntax_for_parameters"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use the generic function type syntax to declare the parameter 'g'."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 2));
    }

    #[test]
    fn function_type_syntax_generic_form_passes() {
        let source = "f(void Function() g) {}";
        // f0 (1 void2 Function3 (4 )5 g6 )7 {8 }9
        let f = Fixture::build(source, unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, NONE);
            b.leaf(NodeKind::NoType, 0);
            identifier(b, 0, 1);
            b.begin(NodeKind::FormalParameters, 1);
            b.begin(NodeKind::FormalParameter, 6);
            b.patch(0, NONE);
            b.patch(1, 6);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.patch(4, 0);
            b.leaf_span(NodeKind::Type, 2, 6);
            b.end(NodeKind::FormalParameter, 7);
            b.end(NodeKind::FormalParameters, 8);
            b.begin(NodeKind::Block, 8);
            b.end(NodeKind::Block, 10);
            b.end(NodeKind::TopLevelMethod, 10);
        }));
        assert!(
            f.run(&one_rule("use_function_type_syntax_for_parameters"))
                .is_empty()
        );
    }

    /// `test/rules/prefer_typing_uninitialized_variables_test.dart`
    /// `test_untypedLocalWithoutInitializer`.
    #[test]
    fn prefer_typing_reports_an_untyped_local() {
        let source = "var x;";
        // var0 x1 ;2
        let f = Fixture::build(source, unit_of(|b, _t| {
            b.begin(NodeKind::Block, 0);
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, 0);
            b.leaf(NodeKind::NoType, 0);
            b.begin(NodeKind::InitializedIdentifier, 1);
            b.end(NodeKind::InitializedIdentifier, 2);
            b.end(NodeKind::VariablesDeclaration, 2);
            b.end(NodeKind::Block, 3);
        }));
        let lints = f.run(&one_rule("prefer_typing_uninitialized_variables"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "An uninitialized variable should have an explicit type annotation."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 1));
    }

    /// Fields carry the second official message variant
    /// (`diagnostic.g.dart` `preferTypingUninitializedVariablesForField`).
    #[test]
    fn prefer_typing_reports_an_untyped_field_with_field_text() {
        let source = "class A { var x; }";
        // class0 A1 {2 var3 x4 ;5 }6
        let f = Fixture::build(source, unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Fields, 2);
            b.patch(0, MemberKind::NonStaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, 3);
            b.leaf(NodeKind::NoType, 3);
            b.begin(NodeKind::InitializedIdentifier, 4);
            b.end(NodeKind::InitializedIdentifier, 5);
            b.end(NodeKind::VariablesDeclaration, 6);
            b.end(NodeKind::Fields, 6);
            b.end(NodeKind::DeclarationBody, 7);
            b.end(NodeKind::ClassDeclaration, 7);
            b.end(NodeKind::TopLevelDeclaration, 7);
        }));
        let lints = f.run(&one_rule("prefer_typing_uninitialized_variables"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "An uninitialized field should have an explicit type annotation."
        );
    }

    #[test]
    fn prefer_typing_typed_declaration_passes() {
        let source = "int x;";
        // int0 x1 ;2
        let f = Fixture::build(source, unit_of(|b, _t| {
            b.begin(NodeKind::Block, 0);
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Type, 0, 1);
            b.begin(NodeKind::InitializedIdentifier, 1);
            b.end(NodeKind::InitializedIdentifier, 2);
            b.end(NodeKind::VariablesDeclaration, 2);
            b.end(NodeKind::Block, 3);
        }));
        assert!(
            f.run(&one_rule("prefer_typing_uninitialized_variables"))
                .is_empty()
        );
    }

    #[test]
    fn prefer_typing_initialized_variable_passes() {
        let source = "var x = 1;";
        // var0 x1 =2 13 ;4
        let f = Fixture::build(source, unit_of(|b, _t| {
            b.begin(NodeKind::Block, 0);
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, 0);
            b.leaf(NodeKind::NoType, 0);
            b.begin(NodeKind::InitializedIdentifier, 1);
            b.begin(NodeKind::VariableInitializer, 2);
            b.leaf(NodeKind::LiteralInt, 3);
            b.end(NodeKind::VariableInitializer, 4);
            b.end(NodeKind::InitializedIdentifier, 4);
            b.end(NodeKind::VariablesDeclaration, 5);
            b.end(NodeKind::Block, 5);
        }));
        assert!(
            f.run(&one_rule("prefer_typing_uninitialized_variables"))
                .is_empty()
        );
    }
}
