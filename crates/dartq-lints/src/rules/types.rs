//! The type-reference family: `avoid_shadowing_type_parameters`,
//! `library_private_types_in_public_api`.

use dartq_ast::NodeKind;

use crate::engine::Check;
use crate::registry::{Group, RuleSpec};

pub(crate) mod avoid_shadowing_type_parameters {
    use dartq_parser::MemberKind;

    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/avoid_shadowing_type_parameters.dart:38-152`
    /// — a type parameter whose name equals a type parameter of an enclosing
    /// declaration reports at the parameter with the enclosing kind spelled
    /// out. Dispatch covers local functions, anonymous function expressions,
    /// non-static methods, function-typed formal parameters, and generic
    /// function types (whose ancestor walk subsumes the official
    /// `GenericTypeAlias` face — the alias's function type reaches the
    /// `typedef` ancestor through the walk, so one report stands where the
    /// official visitor could raise the alias and the function-type faces
    /// together). Every matching ancestor reports separately (:89-127 — the
    /// walk never stops at the first match). The `wildcard_variables`
    /// feature gate skips `_` parameters (:143-145) through the step-0
    /// feature face. Message: `diagnostic.g.dart`
    /// `avoidShadowingTypeParameters`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "avoid_shadowing_type_parameters",
        group: Group::Core,
        problem: "The type parameter '{0}' shadows a type parameter from the enclosing {1}.",
        correction: "Try renaming one of the type parameters.",
        kinds: &[
            NodeKind::LocalFunctionDeclaration,
            NodeKind::FunctionExpression,
            NodeKind::Method,
            NodeKind::FunctionType,
            NodeKind::FunctionTypedFormalParameter,
        ],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // Static methods have nothing above them to shadow (:76-85).
        if c.kind(node) == NodeKind::Method
            && c.payload(node).first() == Some(&(MemberKind::StaticMethod as u32))
        {
            return;
        }
        let Some(mine) = own_type_variables(c, node) else {
            return;
        };
        // The ancestor walk (:89-127): every enclosing declaration with its
        // own type parameters is checked, kind spelled per match. Both
        // child lists are collected up front — `direct_children` shares a
        // scratch RefCell and a live guard across a nested call panics.
        let mine_parameters = c.collect_direct(mine);
        for &ancestor in c.path.iter().rev().skip(1) {
            let Some((kind, theirs)) = enclosing_type_variables(c, ancestor) else {
                continue;
            };
            let theirs_names: Vec<&str> = c
                .collect_direct(theirs)
                .iter()
                .filter_map(|&p| type_variable_name(c, p).map(|n| c.lexeme(c.head_token(n))))
                .collect();
            let shadowed: Vec<u32> = mine_parameters
                .iter()
                .copied()
                .filter(|&parameter| {
                    let Some(name) = type_variable_name(c, parameter) else {
                        return false;
                    };
                    let lexeme = c.lexeme(c.head_token(name));
                    if lexeme == "_" && c.unit.features.wildcard_variables {
                        return false; // :143-145
                    }
                    theirs_names.contains(&lexeme)
                })
                .collect();
            for parameter in shadowed {
                let Some(name) = type_variable_name(c, parameter) else {
                    continue;
                };
                c.report(
                    c.utf16_span(parameter),
                    &[c.lexeme(c.head_token(name)), kind],
                );
            }
        }
    }

    /// The node's own `TypeVariables` child, if any.
    fn own_type_variables(c: &Check<'_>, node: u32) -> Option<u32> {
        c.direct_children(node)
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::TypeVariables)
    }

    /// `(enclosing kind, type parameters)` of an ancestor declaration, or
    /// `None` when it carries none (the :89-127 chain, kind strings verbatim).
    fn enclosing_type_variables(c: &Check<'_>, node: u32) -> Option<(&'static str, u32)> {
        let (kind, has) = match c.kind(node) {
            NodeKind::ClassDeclaration => ("class", true),
            NodeKind::EnumDeclaration => ("enum", true),
            NodeKind::ExtensionDeclaration => ("extension", true),
            NodeKind::ExtensionTypeDeclaration => ("extension type", true),
            NodeKind::Method => ("method", true),
            NodeKind::MixinDeclaration => ("mixin", true),
            NodeKind::TopLevelMethod
            | NodeKind::LocalFunctionDeclaration
            | NodeKind::FunctionExpression
            | NodeKind::FunctionType => ("function", true),
            NodeKind::FunctionTypedFormalParameter => ("parameter", true),
            NodeKind::Typedef => ("typedef", true),
            _ => ("", false),
        };
        if !has {
            return None;
        }
        let type_variables = own_type_variables(c, node)?;
        Some((kind, type_variables))
    }

    /// The name `Identifier` of a `TypeVariable` (collapse map §3.8: the
    /// name rides a child Identifier).
    fn type_variable_name(c: &Check<'_>, parameter: u32) -> Option<u32> {
        c.direct_children(parameter)
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::Identifier)
    }
}

pub(crate) mod library_private_types_in_public_api {
    use crate::rules::{first_child, type_before};

    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/library_private_types_in_public_api.dart:41-246`
    /// — a `_`-private named type used in a public API signature reports at
    /// the type name. The walk starts at the unit's declarations, skips
    /// everything under a private library name (:245-251) or a private
    /// declaration name, and checks the type positions of public members:
    /// top-level variables/functions, class-like members (methods, fields,
    /// constructors), and typedefs. Element-based sub-faces are skipped
    /// (documented approximation): implicit field-formal/super-formal
    /// parameter types, extension `on`-clause and mixin constraint types,
    /// type-parameter bounds, and prefixed (`p._Foo`) names resolve
    /// imports/elements. Message: `diagnostic.g.dart`
    /// `libraryPrivateTypesInPublicApi`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "library_private_types_in_public_api",
        group: Group::Recommended,
        problem: "Invalid use of a private type in a public API.",
        correction: "Try making the private type public, or making the API that uses the private type also be private.",
        kinds: &[NodeKind::CompilationUnit],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // The library-name guard (:245-251): a `_`-private library is all
        // private — nothing to face the public.
        let declarations = c.collect_direct(node);
        if declarations.first().is_some_and(|&first| {
            c.kind(first) == NodeKind::LibraryName
                && c.direct_children(first)
                    .iter()
                    .copied()
                    .find(|&kid| c.kind(kid) == NodeKind::Identifier)
                    .is_some_and(|name| c.lexeme(c.head_token(name)).starts_with('_'))
        }) {
            return;
        }
        for decl in declarations {
            match c.kind(decl) {
                NodeKind::TopLevelDeclaration => walk_class_like(c, decl),
                NodeKind::Typedef => walk_typedef(c, decl),
                NodeKind::TopLevelFields => walk_variable_declarations(c, decl),
                NodeKind::TopLevelMethod => walk_function(c, decl),
                _ => {}
            }
        }
    }

    /// A class-like declaration behind its wrapper: a private name skips
    /// the whole body (:43-53); otherwise the members walk.
    fn walk_class_like(c: &mut Check<'_>, wrapper: u32) {
        let kids = c.collect_direct(wrapper);
        let Some(name) = kids
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::Identifier)
        else {
            return;
        };
        if c.lexeme(c.head_token(name)).starts_with('_') {
            return;
        }
        let Some(decl) = kids.iter().copied().find(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::ClassDeclaration
                    | NodeKind::MixinDeclaration
                    | NodeKind::EnumDeclaration
                    | NodeKind::ExtensionDeclaration
                    | NodeKind::ExtensionTypeDeclaration
                    | NodeKind::NamedMixinApplication
            )
        }) else {
            return;
        };
        if c.kind(decl) == NodeKind::NamedMixinApplication {
            return; // superclass/mixin-constraint types: element sub-face, skipped
        }
        let Some(body) = c
            .direct_children(decl)
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::DeclarationBody)
        else {
            return;
        };
        let in_enum = c.kind(decl) == NodeKind::EnumDeclaration;
        let members = c.direct_children(body).iter().copied().collect::<Vec<_>>();
        for member in members {
            match c.kind(member) {
                NodeKind::Method => walk_signature(c, member),
                NodeKind::Constructor if !in_enum => walk_constructor(c, member),
                NodeKind::Fields => walk_variable_declarations(c, member),
                _ => {}
            }
        }
    }

    /// A method face: a private name skips; otherwise return type then
    /// parameters (:176-182).
    fn walk_signature(c: &mut Check<'_>, member: u32) {
        let Some(name) = c
            .direct_children(member)
            .iter()
            .copied()
            .find(|&kid| matches!(c.kind(kid), NodeKind::Identifier | NodeKind::OperatorName))
        else {
            return;
        };
        if c.kind(name) == NodeKind::Identifier && c.lexeme(c.head_token(name)).starts_with('_') {
            return;
        }
        if let Some((type_node, kind)) = type_before(c, member, name)
            && kind != NodeKind::NoType
        {
            check_type(c, type_node);
        }
        walk_parameters(c, member);
    }

    /// A top-level function face (:160-166).
    fn walk_function(c: &mut Check<'_>, function: u32) {
        let Some(name) = c
            .direct_children(function)
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::Identifier)
        else {
            return;
        };
        if c.lexeme(c.head_token(name)).starts_with('_') {
            return;
        }
        if let Some((type_node, kind)) = type_before(c, function, name)
            && kind != NodeKind::NoType
        {
            check_type(c, type_node);
        }
        walk_parameters(c, function);
    }

    /// A constructor face (:57-63): a `_`-named constructor is private;
    /// the unnamed form is public-facing. Enum constructors are skipped by
    /// the caller (the official `isEffectivelyPrivate`, :225-235).
    fn walk_constructor(c: &mut Check<'_>, constructor: u32) {
        let named = c
            .direct_children(constructor)
            .iter()
            .copied()
            .filter(|&kid| c.kind(kid) == NodeKind::Identifier)
            .nth(1);
        if named.is_some_and(|name| c.lexeme(c.head_token(name)).starts_with('_')) {
            return;
        }
        walk_parameters(c, constructor);
    }

    /// A fields face (:118-124): any public variable name exposes the
    /// declaration's type.
    fn walk_variable_declarations(c: &mut Check<'_>, owner: u32) {
        let declarations = c.direct_children(owner).iter().copied().collect::<Vec<_>>();
        for decl in declarations {
            if c.kind(decl) != NodeKind::VariablesDeclaration {
                continue;
            }
            let any_public = c.children(decl).any(|descendant| {
                c.kind(descendant) == NodeKind::InitializedIdentifier
                    && !c.lexeme(c.head_token(descendant)).starts_with('_')
            });
            if !any_public {
                continue;
            }
            if let Some(type_node) = first_child(c, decl)
                && c.kind(type_node) == NodeKind::Type
            {
                check_type(c, type_node);
            }
        }
    }

    /// A typedef face (:168-175): new form checks the aliased type, old
    /// form the return type and parameters.
    fn walk_typedef(c: &mut Check<'_>, typedef: u32) {
        let Some(name) = c
            .direct_children(typedef)
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::Identifier)
        else {
            return;
        };
        if c.lexeme(c.head_token(name)).starts_with('_') {
            return;
        }
        let kids = c
            .direct_children(typedef)
            .iter()
            .copied()
            .collect::<Vec<_>>();
        for kid in kids {
            match c.kind(kid) {
                NodeKind::Type => check_type(c, kid),
                NodeKind::FormalParameters => walk_parameters(c, typedef),
                _ => {}
            }
        }
    }

    /// The parameters of a declaration: each parameter's declared type
    /// checks (:186-207, the declared-type branch).
    fn walk_parameters(c: &mut Check<'_>, owner: u32) {
        let Some(params) = c
            .direct_children(owner)
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
        else {
            return;
        };
        let params_list = c
            .direct_children(params)
            .iter()
            .copied()
            .collect::<Vec<_>>();
        for param in params_list {
            let first = c.direct_children(param).first().copied();
            if let Some(first) = first.filter(|&t| c.kind(t) == NodeKind::Type) {
                check_type(c, first);
            }
        }
    }

    /// `visitNamedType` (:208-214): a `_`-leading head name reports
    /// zero-width at the name; type arguments always walk.
    fn check_type(c: &mut Check<'_>, type_node: u32) {
        let head = c.head_token(type_node);
        if c.lexeme(head).starts_with('_') {
            let start = c
                .unit
                .tokens
                .get(head as usize)
                .expect("type name token")
                .utf16_offset;
            c.report(start..start, &[]);
        }
        let args = c
            .direct_children(type_node)
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::TypeArguments);
        if let Some(args) = args {
            let args_list = c.direct_children(args).iter().copied().collect::<Vec<_>>();
            for arg in args_list {
                if c.kind(arg) == NodeKind::Type {
                    check_type(c, arg);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use dartq_ast::NodeKind;

    use crate::testsupport::{Fixture, NONE, identifier, one_rule, unit_of, utf16_at};
    use dartq_parser::MemberKind;

    /// A class with one type-parameterized method:
    /// `class A<T> { void f<T>() {} }` (tokens 0-15).
    fn class_with_method(source: &str, method_kind: MemberKind) -> Fixture {
        // class0 A1 <2 T3 >4 {5 void6 f7 <8 T9 >10 (11 )12 {13 }14 }15
        Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::TypeVariables, 2);
                b.begin(NodeKind::TypeVariable, 3);
                b.patch(0, NONE);
                b.patch(1, NONE);
                identifier(b, 3, 4);
                b.end(NodeKind::TypeVariable, 4);
                b.end(NodeKind::TypeVariables, 5);
                b.begin(NodeKind::DeclarationBody, 5);
                b.begin(NodeKind::Method, 6);
                b.patch(0, method_kind as u32);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.leaf(NodeKind::NoType, 6);
                identifier(b, 7, 8);
                b.begin(NodeKind::TypeVariables, 8);
                b.begin(NodeKind::TypeVariable, 9);
                b.patch(0, NONE);
                b.patch(1, NONE);
                identifier(b, 9, 10);
                b.end(NodeKind::TypeVariable, 10);
                b.end(NodeKind::TypeVariables, 11);
                b.begin(NodeKind::FormalParameters, 11);
                b.end(NodeKind::FormalParameters, 13);
                b.begin(NodeKind::Block, 13);
                b.end(NodeKind::Block, 15);
                b.end(NodeKind::Method, 16);
                b.end(NodeKind::DeclarationBody, 16);
                b.end(NodeKind::ClassDeclaration, 16);
                b.end(NodeKind::TopLevelDeclaration, 16);
            }),
        )
    }

    /// `test/rules/avoid_shadowing_type_parameters_test.dart` — a method
    /// parameter shadowing the class parameter reports with the kind.
    #[test]
    fn shadowing_method_parameter_reports() {
        let f = class_with_method("class A<T> { void f<T>() {} }", MemberKind::NonStaticMethod);
        let lints = f.run(&one_rule("avoid_shadowing_type_parameters"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The type parameter 'T' shadows a type parameter from the enclosing class."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 9));
    }

    /// Distinct names pass (`Fn2`-style).
    #[test]
    fn shadowing_distinct_names_pass() {
        let f = class_with_method("class A<T> { void f<U>() {} }", MemberKind::NonStaticMethod);
        assert!(
            f.run(&one_rule("avoid_shadowing_type_parameters"))
                .is_empty()
        );
    }

    /// Static methods have nothing above them to shadow (:76-85).
    #[test]
    fn shadowing_static_method_passes() {
        let f = class_with_method("class A<T> { void f<T>() {} }", MemberKind::StaticMethod);
        assert!(
            f.run(&one_rule("avoid_shadowing_type_parameters"))
                .is_empty()
        );
    }

    /// A `_` parameter passes under the wildcard-variables feature
    /// (:143-145; the feature face defaults on).
    #[test]
    fn shadowing_wildcard_passes() {
        let f = class_with_method("class A<T> { void f<_>() {} }", MemberKind::NonStaticMethod);
        assert!(
            f.run(&one_rule("avoid_shadowing_type_parameters"))
                .is_empty()
        );
    }

    /// A local function shadowing its enclosing top-level function reports
    /// with the `function` kind.
    #[test]
    fn shadowing_local_function_reports() {
        let source = "void f<T>() { void g<T>() {} }";
        // void0 f1 <2 T3 >4 (5 )6 {7 void8 g9 <10 T11 >12 (13 )14 {15 }16 }17
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Type, 0, 1);
                identifier(b, 1, 2);
                b.begin(NodeKind::TypeVariables, 2);
                b.begin(NodeKind::TypeVariable, 3);
                b.patch(0, NONE);
                b.patch(1, NONE);
                identifier(b, 3, 4);
                b.end(NodeKind::TypeVariable, 4);
                b.end(NodeKind::TypeVariables, 5);
                b.begin(NodeKind::FormalParameters, 5);
                b.end(NodeKind::FormalParameters, 7);
                b.begin(NodeKind::Block, 7);
                b.begin(NodeKind::LocalFunctionDeclaration, 8);
                identifier(b, 9, 10);
                b.begin(NodeKind::TypeVariables, 10);
                b.begin(NodeKind::TypeVariable, 11);
                b.patch(0, NONE);
                b.patch(1, NONE);
                identifier(b, 11, 12);
                b.end(NodeKind::TypeVariable, 12);
                b.end(NodeKind::TypeVariables, 13);
                b.begin(NodeKind::FormalParameters, 13);
                b.end(NodeKind::FormalParameters, 15);
                b.begin(NodeKind::Block, 15);
                b.end(NodeKind::Block, 17);
                b.end(NodeKind::LocalFunctionDeclaration, 18);
                b.end(NodeKind::Block, 18);
                b.end(NodeKind::TopLevelMethod, 18);
            }),
        );
        let lints = f.run(&one_rule("avoid_shadowing_type_parameters"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The type parameter 'T' shadows a type parameter from the enclosing function."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 11));
    }

    /// The alias face via the function-type walk:
    /// `typedef Fn<T> = void Function<T>();` reports at the function type's
    /// parameter with the `typedef` kind.
    #[test]
    fn shadowing_typedef_reports() {
        let source = "typedef F<T> = void Function<T>();";
        // typedef0 F1 <2 T3 >4 =5 void6 Function7 <8 T9 >10 (11 )12 ;13
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::Typedef, 0);
                b.patch(0, 0);
                b.patch(1, 5);
                identifier(b, 1, 2);
                b.begin(NodeKind::TypeVariables, 2);
                b.begin(NodeKind::TypeVariable, 3);
                b.patch(0, NONE);
                b.patch(1, NONE);
                identifier(b, 3, 4);
                b.end(NodeKind::TypeVariable, 4);
                b.end(NodeKind::TypeVariables, 5);
                b.begin(NodeKind::Type, 6);
                b.begin(NodeKind::FunctionType, 6);
                b.patch(0, 7);
                b.patch(1, NONE);
                b.leaf_span(NodeKind::Type, 6, 7);
                b.begin(NodeKind::TypeVariables, 8);
                b.begin(NodeKind::TypeVariable, 9);
                b.patch(0, NONE);
                b.patch(1, NONE);
                identifier(b, 9, 10);
                b.end(NodeKind::TypeVariable, 10);
                b.end(NodeKind::TypeVariables, 11);
                b.begin(NodeKind::FormalParameters, 11);
                b.end(NodeKind::FormalParameters, 13);
                b.end(NodeKind::FunctionType, 14);
                b.end(NodeKind::Type, 14);
                b.end(NodeKind::Typedef, 15);
            }),
        );
        let lints = f.run(&one_rule("avoid_shadowing_type_parameters"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The type parameter 'T' shadows a type parameter from the enclosing typedef."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 9));
    }

    // —— library_private_types_in_public_api ——————————————————————————————

    /// A private type on a public top-level variable reports at the type
    /// name.
    #[test]
    fn private_type_top_level_variable_reports() {
        let source = "_Foo x;";
        // _Foo0 x1 ;2
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelFields, 0);
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Type, 0, 1);
                b.begin(NodeKind::InitializedIdentifier, 1);
                b.end(NodeKind::InitializedIdentifier, 2);
                b.end(NodeKind::VariablesDeclaration, 3);
                b.end(NodeKind::TopLevelFields, 3);
            }),
        );
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Invalid use of a private type in a public API."
        );
        assert_eq!(lints[0].range, 0..0);
    }

    #[test]
    fn private_variable_name_passes() {
        let source = "_Foo _x;";
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelFields, 0);
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Type, 0, 1);
                b.begin(NodeKind::InitializedIdentifier, 1);
                b.end(NodeKind::InitializedIdentifier, 2);
                b.end(NodeKind::VariablesDeclaration, 3);
                b.end(NodeKind::TopLevelFields, 3);
            }),
        );
        assert!(
            f.run(&one_rule("library_private_types_in_public_api"))
                .is_empty()
        );
    }

    #[test]
    fn public_type_public_variable_passes() {
        let source = "Foo x;";
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelFields, 0);
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Type, 0, 1);
                b.begin(NodeKind::InitializedIdentifier, 1);
                b.end(NodeKind::InitializedIdentifier, 2);
                b.end(NodeKind::VariablesDeclaration, 3);
                b.end(NodeKind::TopLevelFields, 3);
            }),
        );
        assert!(
            f.run(&one_rule("library_private_types_in_public_api"))
                .is_empty()
        );
    }

    /// A `_`-private library name silences the unit (:245-251).
    #[test]
    fn private_library_silences() {
        let source = "library _l;\n_Foo x;";
        // library0 _l1 ;2 _Foo3 x4 ;5
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::LibraryName, 0);
                b.patch(0, 0);
                identifier(b, 1, 2);
                b.end(NodeKind::LibraryName, 3);
                b.begin(NodeKind::TopLevelFields, 3);
                b.begin(NodeKind::VariablesDeclaration, 3);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Type, 3, 4);
                b.begin(NodeKind::InitializedIdentifier, 4);
                b.end(NodeKind::InitializedIdentifier, 5);
                b.end(NodeKind::VariablesDeclaration, 6);
                b.end(NodeKind::TopLevelFields, 6);
            }),
        );
        assert!(
            f.run(&one_rule("library_private_types_in_public_api"))
                .is_empty()
        );
    }

    /// A method return type inside a public class reports.
    #[test]
    fn private_type_method_reports() {
        let source = "class A { _Foo f() => throw 0; }";
        // class0 A1 {2 _Foo3 f4 (5 )6 =>7 throw8 09 ;10 }11
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Method, 3);
                b.patch(0, MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.leaf_span(NodeKind::Type, 3, 4);
                identifier(b, 4, 5);
                b.begin(NodeKind::FormalParameters, 5);
                b.end(NodeKind::FormalParameters, 7);
                b.begin(NodeKind::ExpressionBody, 7);
                b.patch(0, 7);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::ThrowExpression, 8);
                b.leaf(NodeKind::LiteralInt, 9);
                b.end(NodeKind::ThrowExpression, 10);
                b.end(NodeKind::ExpressionBody, 11);
                b.end(NodeKind::Method, 12);
                b.end(NodeKind::DeclarationBody, 12);
                b.end(NodeKind::ClassDeclaration, 12);
                b.end(NodeKind::TopLevelDeclaration, 12);
            }),
        );
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 3));
    }

    /// A private class body never walks (:43-53).
    #[test]
    fn private_class_silences() {
        let source = "class _A { _Foo f() => throw 0; }";
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Method, 3);
                b.patch(0, MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.leaf_span(NodeKind::Type, 3, 4);
                identifier(b, 4, 5);
                b.begin(NodeKind::FormalParameters, 5);
                b.end(NodeKind::FormalParameters, 7);
                b.begin(NodeKind::ExpressionBody, 7);
                b.patch(0, 7);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.leaf(NodeKind::LiteralInt, 9);
                b.end(NodeKind::ExpressionBody, 11);
                b.end(NodeKind::Method, 12);
                b.end(NodeKind::DeclarationBody, 12);
                b.end(NodeKind::ClassDeclaration, 12);
                b.end(NodeKind::TopLevelDeclaration, 12);
            }),
        );
        assert!(
            f.run(&one_rule("library_private_types_in_public_api"))
                .is_empty()
        );
    }

    /// Type arguments walk: `List<_Foo>` reports at the argument.
    #[test]
    fn private_type_argument_reports() {
        let source = "f(List<_Foo> x) {}";
        // f0 (1 List2 <3 _Foo4 >5 x6 )7 {8 }9
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::NoType, 0);
                identifier(b, 0, 1);
                b.begin(NodeKind::FormalParameters, 1);
                b.begin(NodeKind::FormalParameter, 2);
                b.patch(0, NONE);
                b.patch(1, 6);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.begin(NodeKind::Type, 2);
                b.begin(NodeKind::TypeArguments, 3);
                b.leaf_span(NodeKind::Type, 4, 5);
                b.end(NodeKind::TypeArguments, 6);
                b.end(NodeKind::Type, 7);
                b.end(NodeKind::FormalParameter, 8);
                b.end(NodeKind::FormalParameters, 9);
                b.begin(NodeKind::Block, 8);
                b.end(NodeKind::Block, 11);
                b.end(NodeKind::TopLevelMethod, 11);
            }),
        );
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 4));
    }
}
