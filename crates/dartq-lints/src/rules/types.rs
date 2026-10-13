//! The type-reference family: `avoid_shadowing_type_parameters`,
//! `library_private_types_in_public_api`, plus the standalone
//! `prefer_constructors_over_static_methods` and
//! `type_annotate_public_apis`.

use dartq_ast::NodeKind;

use crate::engine::Check;
use crate::registry::{Group, RuleSpec};

/// The domain's incoming registrations: a new standalone rule appends its
/// `SPEC` reference here (name order) and joins the registry through the
/// aggregated tail — adding a rule touches only this file. Batch close
/// promotes the block into the registry's frozen skeleton together with
/// the pin-test wave list (`tests/registry_pin.rs`).
pub(crate) static INCOMING_RULES: &[&RuleSpec] = &[
    &prefer_constructors_over_static_methods::SPEC,
    &type_annotate_public_apis::SPEC,
];

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

pub(crate) mod avoid_annotating_with_dynamic {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// The `augment` ModSet bit (bridge `mod_bits`).
    const AUGMENT_BIT: u16 = 1 << 1;

    /// Official: `pkg/linter/lib/src/rules/avoid_annotating_with_dynamic.dart:30-76`
    /// — a formal parameter written with the type `dynamic` reports at the
    /// parameter (:50-55 `type is NamedType && type.type is DynamicType`).
    /// Parameters inside an augmentation skip (:57-63 `inAugmentation`).
    ///
    /// dartq face: the plain single-token `dynamic` `Type` child (a
    /// prefixed spelling resolves elsewhere and stays silent — the
    /// documented face); the augmentation guard walks to the first
    /// declaration-family ancestor, false at a block boundary (:66-76).
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "avoid_annotating_with_dynamic",
        group: Group::Standalone,
        problem: "Unnecessary 'dynamic' type annotation.",
        correction: "Try removing the type 'dynamic'.",
        kinds: &[NodeKind::FormalParameter],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        if in_augmentation(c) {
            return;
        }
        let Some(written) = c
            .collect_direct(node)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::Type)
        else {
            return;
        };
        if c.unit.ast.token_range(written).len() == 1
            && c.lexeme(c.head_token(written)) == "dynamic"
        {
            let range = c.utf16_span(node);
            c.report(range, &[]);
        }
    }

    /// The official `inAugmentation` (:66-76): walk the ancestors, `false`
    /// at a block boundary, otherwise the first declaration's augment bit.
    fn in_augmentation(c: &Check<'_>) -> bool {
        for &ancestor in c.path.iter().rev() {
            match c.kind(ancestor) {
                NodeKind::Block => return false,
                NodeKind::Method
                | NodeKind::TopLevelMethod
                | NodeKind::Constructor
                | NodeKind::Factory
                | NodeKind::Fields
                | NodeKind::TopLevelFields
                | NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::ExtensionDeclaration
                | NodeKind::EnumDeclaration => {
                    return dartq_ast::mod_flags(&c.unit.ast, ancestor) & AUGMENT_BIT != 0;
                }
                _ => {}
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use dartq_ast::NodeKind;

    use crate::testsupport::{Fixture, NONE, identifier, one_rule, tok, unit_of, utf16_at};
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

    // —— avoid_annotating_with_dynamic ——————————————————————————————————

    /// `test/rules/avoid_annotating_with_dynamic_test.dart` — a parameter
    /// annotated with bare `dynamic` reports at the annotation.
    #[test]
    fn annotating_with_dynamic_reports_a_dynamic_formal() {
        let source = "void f(dynamic x) {}";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("avoid_annotating_with_dynamic"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Unnecessary 'dynamic' type annotation.");
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try removing the type 'dynamic'.")
        );
        let dynamic = crate::testsupport::tok(&f.source, &f.tokens, "dynamic", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, dynamic));
    }

    /// Only the `dynamic`-annotated formals report, positional or named.
    #[test]
    fn annotating_with_dynamic_reports_each_dynamic_formal() {
        let f = Fixture::parse("void g(int a, dynamic b) {}\nvoid j({dynamic x}) {}");
        let lints = f.run(&one_rule("avoid_annotating_with_dynamic"));
        assert_eq!(lints.len(), 2, "{lints:?}");
    }

    /// Specific types pass; a nested `dynamic` type argument is a
    /// different face and stays out.
    #[test]
    fn annotating_with_dynamic_valid_faces_pass() {
        let f = Fixture::parse("void h(int a, Object b) {}\nvoid i(List<dynamic> xs) {}");
        assert!(f.run(&one_rule("avoid_annotating_with_dynamic")).is_empty());
    }

    // —— prefer_constructors_over_static_methods ————————————————————————

    /// `test_staticMethod_expressionBody` — the static method whose return
    /// type is the enclosing class and whose body creates it reports at the
    /// method name.
    #[test]
    fn constructors_static_method_expression_body_reports() {
        let f = Fixture::parse("class A {\n  A.named();\n  static A staticM() => A.named();\n}\n");
        let lints = f.run(&one_rule("prefer_constructors_over_static_methods"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Static method should be a constructor.");
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try converting the method into a constructor.")
        );
        let name = tok(&f.source, &f.tokens, "staticM", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, name));
    }

    /// `test_staticMethod_referenceToConstructedInstanceOfClass` — the block
    /// body holding the creation in a local reports the same.
    #[test]
    fn constructors_block_body_creation_reports() {
        let f = Fixture::parse(
            "class A {\n  A.named();\n  static A instanceM() {\n    final a = A.named();\n    return a;\n  }\n}\n",
        );
        assert_eq!(
            f.run(&one_rule("prefer_constructors_over_static_methods"))
                .len(),
            1
        );
    }

    /// `test_staticMethod_expressionBody_extensionType` — the extension type
    /// face reports.
    #[test]
    fn constructors_extension_type_reports() {
        let f = Fixture::parse(
            "extension type E(int i) {\n  static E make(int i) => E(i);\n}\n",
        );
        assert_eq!(
            f.run(&one_rule("prefer_constructors_over_static_methods"))
                .len(),
            1
        );
    }

    /// `test_staticGetter` — a static getter is a static method face and
    /// reports at its name.
    #[test]
    fn constructors_static_getter_reports() {
        let f = Fixture::parse(
            "class A {\n  A.named();\n  static A get getter => A.named();\n}\n",
        );
        assert_eq!(
            f.run(&one_rule("prefer_constructors_over_static_methods"))
                .len(),
            1
        );
    }

    /// `_BodyVisitor` overwrite semantics (:54-63): the LAST non-pruned
    /// creation decides — a later non-matching creation clears the found
    /// flag; the same in reverse reports.
    #[test]
    fn constructors_last_creation_decides() {
        let f = Fixture::parse(
            "class A {\n  static A f() {\n    new A();\n    new B();\n  }\n}\nclass B {}\n",
        );
        assert!(f
            .run(&one_rule("prefer_constructors_over_static_methods"))
            .is_empty());

        let f = Fixture::parse(
            "class A {\n  static A f() {\n    new B();\n    new A();\n  }\n}\nclass B {}\n",
        );
        assert_eq!(
            f.run(&one_rule("prefer_constructors_over_static_methods"))
                .len(),
            1
        );
    }

    /// `test_staticMethod_generic` / `test_staticMethod_returnsNullable` /
    /// `test_staticMethod_returnsUnrelatedType` — a generic method, a
    /// nullable return, and an unrelated return type all pass.
    #[test]
    fn constructors_generic_nullable_unrelated_pass() {
        let f = Fixture::parse(
            "class A {\n  A.named();\n  static A generic<T>() => A.named();\n}\n",
        );
        assert!(f
            .run(&one_rule("prefer_constructors_over_static_methods"))
            .is_empty());

        let f = Fixture::parse(
            "class A {\n  A.named();\n  static A? staticM() => A.named();\n}\n",
        );
        assert!(f
            .run(&one_rule("prefer_constructors_over_static_methods"))
            .is_empty());

        let f = Fixture::parse("class A {\n  static Object staticM() => Object();\n}\n");
        assert!(f
            .run(&one_rule("prefer_constructors_over_static_methods"))
            .is_empty());
    }

    /// `test_staticMethod_returnsSubtype` / `test_extensionMethod` /
    /// `test_factoryConstructor` — a subclass return, an extension static,
    /// and a factory all pass.
    #[test]
    fn constructors_subtype_extension_factory_pass() {
        let f = Fixture::parse("class A {\n  static B f() => B();\n}\nclass B extends A {}\n");
        assert!(f
            .run(&one_rule("prefer_constructors_over_static_methods"))
            .is_empty());

        let f = Fixture::parse(
            "class A {\n  A.named();\n}\nextension E on A {\n  static A foo() => A.named();\n}\n",
        );
        assert!(f
            .run(&one_rule("prefer_constructors_over_static_methods"))
            .is_empty());

        let f = Fixture::parse("class A {\n  A.named();\n  factory A.f() {\n    return A.named();\n  }\n}\n");
        assert!(f
            .run(&one_rule("prefer_constructors_over_static_methods"))
            .is_empty());
    }

    // —— type_annotate_public_apis ——————————————————————————————————————

    /// `test_instanceField_onClass_hasVar_noInitializer` +
    /// `test_instanceField_onClass_inDeclarationList` — an untyped public
    /// field reports per public variable name; the `_`-private sibling is
    /// exempt.
    #[test]
    fn annotate_fields_var_no_initializer_reports() {
        let f = Fixture::parse("class A {\n  var x;\n}\n");
        let lints = f.run(&one_rule("type_annotate_public_apis"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Missing type annotation on a public API.");
        let name = tok(&f.source, &f.tokens, "x", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, name));

        let f = Fixture::parse("class A {\n  var x, _y;\n}\n");
        let lints = f.run(&one_rule("type_annotate_public_apis"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let name = tok(&f.source, &f.tokens, "x", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, name));
    }

    /// `test_instanceField_onClass_hasInitializer` /
    /// `test_instanceField_onClass_nullInitializer` /
    /// `test_staticField_nullInitializer` — `final` with a non-`null`
    /// initializer passes; the `null` initializer reports.
    #[test]
    fn annotate_fields_final_faces() {
        let f = Fixture::parse("class A {\n  final x = 0;\n}\n");
        assert!(f.run(&one_rule("type_annotate_public_apis")).is_empty());

        let f = Fixture::parse("class A {\n  final n = null;\n}\n");
        assert_eq!(f.run(&one_rule("type_annotate_public_apis")).len(), 1);

        let f = Fixture::parse("class A {\n  static final x = null;\n}\n");
        assert_eq!(f.run(&one_rule("type_annotate_public_apis")).len(), 1);
    }

    /// `test_instanceGetter_onClass_noReturnType` /
    /// `test_topLevelGetter_noReturnType` / `test_topLevelGetter_hasReturnType`.
    #[test]
    fn annotate_getter_faces() {
        let f = Fixture::parse("class A {\n  get x => 42;\n}\n");
        assert_eq!(f.run(&one_rule("type_annotate_public_apis")).len(), 1);

        let f = Fixture::parse("get x => 42;\n");
        assert_eq!(f.run(&one_rule("type_annotate_public_apis")).len(), 1);

        let f = Fixture::parse("int get x => 42;\n");
        assert!(f.run(&one_rule("type_annotate_public_apis")).is_empty());
    }

    /// `test_instanceMethod_onClass_noReturnType` /
    /// `test_staticMethod_onClass_noReturnType` /
    /// `test_topLevelFunction_noReturnType` — the missing-return-type report
    /// at the name on every member face.
    #[test]
    fn annotate_missing_return_type_reports() {
        let f = Fixture::parse("class A {\n  m() {}\n}\n");
        let lints = f.run(&one_rule("type_annotate_public_apis"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let name = tok(&f.source, &f.tokens, "m", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, name));

        let f = Fixture::parse("class A {\n  static m() {}\n}\n");
        assert_eq!(f.run(&one_rule("type_annotate_public_apis")).len(), 1);

        let f = Fixture::parse("f() {}\n");
        assert_eq!(f.run(&one_rule("type_annotate_public_apis")).len(), 1);
    }

    /// `test_instanceMethod_onClass_parameterMissingType` /
    /// `test_topLevelFunction_parameterMissingType` /
    /// `test_staticMethod_onClass_parameterMissingType` — the parameter walk
    /// reports the untyped parameter on every member face.
    #[test]
    fn annotate_parameter_missing_type_reports() {
        let f = Fixture::parse("class A {\n  void m(x) {}\n}\n");
        assert_eq!(f.run(&one_rule("type_annotate_public_apis")).len(), 1);

        let f = Fixture::parse("void f(x) {}\n");
        assert_eq!(f.run(&one_rule("type_annotate_public_apis")).len(), 1);

        let f = Fixture::parse("class A {\n  static void m(p) {}\n}\n");
        assert_eq!(f.run(&one_rule("type_annotate_public_apis")).len(), 1);
    }

    /// `test_instanceMethod_parameterNameIsUnderscore` /
    /// `test_instanceMethod_parameterNameIsMultipleUnderscores` /
    /// `test_localFunction` — just-underscores parameters and local
    /// functions never report.
    #[test]
    fn annotate_underscores_and_local_functions_pass() {
        let f = Fixture::parse("class A {\n  void m(_) {}\n}\n");
        assert!(f.run(&one_rule("type_annotate_public_apis")).is_empty());

        let f = Fixture::parse("class A {\n  void m(__) {}\n}\n");
        assert!(f.run(&one_rule("type_annotate_public_apis")).is_empty());

        let f = Fixture::parse("void f() {\n  void g(x) {}\n}\n");
        assert!(f.run(&one_rule("type_annotate_public_apis")).is_empty());
    }

    /// `test_instanceSetter_noReturnType` /
    /// `test_instanceSetter_onClass_parameterMissingType` /
    /// `test_topLevelSetter_parameterMissingType` — setters never report a
    /// missing return type but still walk their parameters.
    #[test]
    fn annotate_setter_faces() {
        let f = Fixture::parse("class A {\n  set x(int p) {}\n}\n");
        assert!(f.run(&one_rule("type_annotate_public_apis")).is_empty());

        let f = Fixture::parse("class A {\n  set x(p) {}\n}\n");
        assert_eq!(f.run(&one_rule("type_annotate_public_apis")).len(), 1);

        let f = Fixture::parse("set x(p) {}\n");
        assert_eq!(f.run(&one_rule("type_annotate_public_apis")).len(), 1);
    }

    /// `test_instanceOperator_binary_noReturnType` /
    /// `test_instanceOperator_indexAssignment_noReturnType` /
    /// `test_instanceOperator_indexAssignment_noParameterType` — operators
    /// report at the operator token, except `[]=` which never reports a
    /// missing return type.
    #[test]
    fn annotate_operator_faces() {
        let f = Fixture::parse("class A {\n  operator +(A a) => a;\n}\n");
        let lints = f.run(&one_rule("type_annotate_public_apis"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let op = tok(&f.source, &f.tokens, "+", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, op));

        let f = Fixture::parse("class A {\n  operator []=(A a, A b) {}\n}\n");
        assert!(f.run(&one_rule("type_annotate_public_apis")).is_empty());

        let f = Fixture::parse("class A {\n  void operator []=(a, A b) {}\n}\n");
        assert_eq!(f.run(&one_rule("type_annotate_public_apis")).len(), 1);
    }

    /// The constructor faces: `test_instanceConstructor_parameterMissingType`
    /// reports; typed, optional-with-type, enum, `_`-private owner and
    /// `_`-private named constructors pass.
    #[test]
    fn annotate_constructor_faces() {
        let f = Fixture::parse("class A {\n  A(p);\n}\n");
        assert_eq!(f.run(&one_rule("type_annotate_public_apis")).len(), 1);

        let f = Fixture::parse("class A {\n  A(int p);\n}\n");
        assert!(f.run(&one_rule("type_annotate_public_apis")).is_empty());

        let f = Fixture::parse("class A {\n  A([int? p]);\n}\n");
        assert!(f.run(&one_rule("type_annotate_public_apis")).is_empty());

        let f = Fixture::parse("enum E {\n  one(0);\n\n  const E(p);\n}\n");
        assert!(f.run(&one_rule("type_annotate_public_apis")).is_empty());

        let f = Fixture::parse("class _A {\n  _A(p);\n}\n");
        assert!(f.run(&one_rule("type_annotate_public_apis")).is_empty());

        let f = Fixture::parse("class A {\n  A._(p);\n}\n");
        assert!(f.run(&one_rule("type_annotate_public_apis")).is_empty());
    }

    /// `test_instanceConstructor_primary_parameterHasType` /
    /// `test_instanceConstructor_primary_parameterMissingType` /
    /// `test_instanceConstructor_primary_private_parameterMissingType` —
    /// the primary-constructor face reports the untyped parameter of a
    /// public primary, never of a `_`-named one.
    #[test]
    fn annotate_primary_constructor_faces() {
        let f = Fixture::parse("class A({int? p});\n");
        assert!(f.run(&one_rule("type_annotate_public_apis")).is_empty());

        let f = Fixture::parse("class A({p});\n");
        assert_eq!(f.run(&one_rule("type_annotate_public_apis")).len(), 1);

        let f = Fixture::parse("class A._({p});\n");
        assert!(f.run(&one_rule("type_annotate_public_apis")).is_empty());
    }

    /// `test_topLevelConst` and the variable faces — a `const` top-level
    /// passes; an untyped `var` with an initializer reports.
    #[test]
    fn annotate_top_level_variables() {
        let f = Fixture::parse("const x = '';\n");
        assert!(f.run(&one_rule("type_annotate_public_apis")).is_empty());

        let f = Fixture::parse("var x = 0;\n");
        assert_eq!(f.run(&one_rule("type_annotate_public_apis")).len(), 1);
    }

    /// `test_typedefLegacy_parameterMissingType` /
    /// `test_typedefLegacy_private_parameterHasType` /
    /// `test_typedefLegacy_private_parameterMissingType` — the legacy
    /// typedef reports a missing return type at the name, privates pass;
    /// the generalized form is no `FunctionTypeAlias` and never enters.
    #[test]
    fn annotate_typedef_faces() {
        let f = Fixture::parse("typedef F(x);\n");
        let lints = f.run(&one_rule("type_annotate_public_apis"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let name = tok(&f.source, &f.tokens, "F", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, name));

        let f = Fixture::parse("typedef _F(int value);\n");
        assert!(f.run(&one_rule("type_annotate_public_apis")).is_empty());

        let f = Fixture::parse("typedef void _F(value);\n");
        assert!(f.run(&one_rule("type_annotate_public_apis")).is_empty());

        let f = Fixture::parse("typedef F = int Function();\n");
        assert!(f.run(&one_rule("type_annotate_public_apis")).is_empty());
    }
}

pub(crate) mod prefer_constructors_over_static_methods {
    use dartq_ast::NodeKind;

    use crate::engine::Check;
    use crate::registry::{Group, RuleSpec};
    use crate::rules::{class_declaration_name, member_name, type_before, type_face};

    /// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/prefer_constructors_over_static_methods.dart`
    /// — a static method with no type parameters whose return type equals the
    /// enclosing non-generic class's (or extension type's) own type, and whose
    /// body contains an instance creation of that type, reports at the method
    /// name (:66-80, the `typeToCheckOrNull` face :83-95 pins
    /// `ClassDeclaration`/`ExtensionTypeDeclaration` with no type parameters).
    ///
    /// dartq faces: the return-type identity reads the written type — head
    /// name equal to the class name, no `?` (the official `interfaceType !=
    /// returnType` bail :75; on the name faces the written name resolves to
    /// the enclosing class, so equality matches on all valid code). The body
    /// scan mirrors `_BodyVisitor`'s overwrite semantics (:54-63): creations
    /// are visited in source order, a matching creation prunes its subtree
    /// (`if (!found)` :62), and the LAST non-pruned creation decides — the
    /// creation shapes are the explicit `new`/`const` forms and the resolved
    /// implicit creations (the analyzer rewrites those invocations),
    /// recognized as `Type(args)` / `Type.name(args)` with the class name;
    /// prefixed (`p.A()`), cascaded (`..A()`) and annotated (`@A()`) names
    /// are not creations of the class. Message: `diagnostic.g.dart`
    /// `preferConstructorsOverStaticMethods`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_constructors_over_static_methods",
        group: Group::Standalone,
        problem: "Static method should be a constructor.",
        correction: "Try converting the method into a constructor.",
        kinds: &[NodeKind::Method],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `!node.isStatic` bails (:69); factories never carry a static kind.
        if !matches!(
            c.payload(node).first().copied(),
            Some(kind)
                if kind == dartq_parser::MemberKind::StaticMethod as u32
                    || kind == dartq_parser::MemberKind::ExtensionStaticMethod as u32
                    || kind == dartq_parser::MemberKind::ExtensionTypeStaticMethod as u32
        ) {
            return;
        }
        // `node.typeParameters != null` bails (:70).
        if c.collect_direct(node)
            .iter()
            .any(|&kid| c.kind(kid) == NodeKind::TypeVariables)
        {
            return;
        }
        // `returnType` is an InterfaceType: a written named type. `void`, a
        // function type, `dynamic`, and `A?` (the nullable face, :74-75) bail.
        let Some(name) = member_name(c, node) else {
            return;
        };
        let Some((type_row, kind)) = type_before(c, node, name) else {
            return;
        };
        if kind != NodeKind::Type {
            return;
        }
        let face = type_face(c, type_row);
        if face.nullable {
            return;
        }
        // `node.parent?.parent.typeToCheckOrNull()` (:74, :83-95): the
        // enclosing class-like decides — the first class-like ancestor, a
        // class or extension type without type parameters.
        let Some(owner) = c.path.iter().rev().skip(1).copied().find(|&ancestor| {
            matches!(
                c.kind(ancestor),
                NodeKind::ClassDeclaration
                    | NodeKind::MixinDeclaration
                    | NodeKind::ExtensionDeclaration
                    | NodeKind::ExtensionTypeDeclaration
                    | NodeKind::EnumDeclaration
            )
        }) else {
            return;
        };
        if !matches!(
            c.kind(owner),
            NodeKind::ClassDeclaration | NodeKind::ExtensionTypeDeclaration
        ) {
            return;
        }
        if c.collect_direct(owner)
            .iter()
            .any(|&kid| c.kind(kid) == NodeKind::TypeVariables)
        {
            return;
        }
        let Some(class_name) = class_declaration_name(c, owner) else {
            return;
        };
        // `interfaceType != returnType` (:75): the written name faces the
        // class's own type.
        if face.name != Some(class_name) {
            return;
        }
        // `_hasNewInvocation` (:20-21, :49-64): the last non-pruned creation
        // in the body decides.
        let Some(body) = c.collect_direct(node).into_iter().find(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::BlockBody
                    | NodeKind::ExpressionBody
                    | NodeKind::Block
                    | NodeKind::EmptyBody
                    | NodeKind::NoBody
            )
        }) else {
            return;
        };
        if last_creation_matches(c, body, class_name) {
            let range = crate::rules::utf16_token_span(c, c.head_token(name));
            c.report(range, &[]);
        }
    }

    /// The body creation scan (:49-64): creations visited in source order;
    /// every visited creation overwrites (`found = node.staticType ==
    /// returnType`), so the last non-pruned one decides, and a matching
    /// creation prunes the creations nested in its invocation (the
    /// `if (!found)` recursion gate :62).
    fn last_creation_matches(c: &Check<'_>, body: u32, class_name: &str) -> bool {
        let range = c.unit.ast.token_range(body);
        let mut found = false;
        // Balanced-paren depth over the body; a matched creation prunes until
        // its invocation's `)` returns the depth to the match's level.
        let mut depth: i32 = 0;
        let mut prune_depth: Option<i32> = None;
        for token in range.start..range.end {
            match c.lexeme(token) {
                "(" => {
                    depth += 1;
                    continue;
                }
                ")" => {
                    depth -= 1;
                    if prune_depth == Some(depth) {
                        prune_depth = None;
                    }
                    continue;
                }
                _ => {}
            }
            if prune_depth.is_some() {
                continue;
            }
            if let Some(matches) = creation_at(c, token, class_name) {
                found = matches;
                if matches {
                    prune_depth = Some(depth);
                }
            }
        }
        found
    }

    /// Whether an instance creation starts at `token`, and whether it
    /// creates `class_name` (`Some(created == class_name)`) — `None` when
    /// the token starts no recognizable creation. The explicit `new`/`const`
    /// forms are necessarily creations of whatever name follows; the
    /// resolved implicit creations (the analyzer rewrites those
    /// invocations) are recognized as `Type(...)` / `Type.name(...)` with
    /// the class name — an implicit `Other(...)` is indistinguishable from
    /// a plain function call on this face and visits as none. Prefixed
    /// (`p.A()`), cascaded (`..A()`) and annotated (`@A()`) names are not
    /// creations of the class.
    fn creation_at(c: &Check<'_>, token: u32, class_name: &str) -> Option<bool> {
        let lexeme = c.lexeme(token);
        if lexeme == "new" || lexeme == "const" {
            let name = token + 1;
            return c
                .unit
                .tokens
                .get(name as usize)
                .filter(|_| invocation_shape(c, name))
                .map(|_| c.lexeme(name) == class_name);
        }
        if lexeme == class_name
            && !matches!(
                token.checked_sub(1).map(|prev| c.lexeme(prev)),
                Some("." | ".." | "@")
            )
            && invocation_shape(c, token + 1)
        {
            return Some(true);
        }
        None
    }

    /// The invocation shape after a created name: `(` opens the arguments,
    /// `. name (` names a constructor.
    fn invocation_shape(c: &Check<'_>, name_end: u32) -> bool {
        if c.unit.tokens.get(name_end as usize).is_none() {
            return false;
        }
        match c.lexeme(name_end) {
            "(" => true,
            "." => c
                .unit
                .tokens
                .get(name_end as usize + 2)
                .is_some_and(|_| c.lexeme(name_end + 2) == "("),
            _ => false,
        }
    }
}

pub(crate) mod type_annotate_public_apis {
    use dartq_ast::NodeKind;

    use crate::engine::Check;
    use crate::registry::{Group, RuleSpec};
    use crate::rules::{
        first_child, get_set_token, initialized_identifier_name, is_just_underscores,
        member_name, type_before, variables_declaration_keyword,
    };

    /// The `augment` ModSet bit (bridge `mod_bits`).
    const AUGMENT_BIT: u16 = 1 << 1;

    /// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/type_annotate_public_apis.dart`
    /// — missing written types on public API declarations: a public
    /// method/getter (not a setter, not `[]=`) or top-level function without
    /// a return type reports at its name (:71-84, :98-109); a public field or
    /// top-level variable with no written type reports per public variable
    /// name, exempting `const` and `final` with a non-`null` initializer
    /// (:62-67, :161-167); a public legacy typedef without a return type
    /// reports at its name, otherwise its parameters walk (:87-95); the
    /// parameters of public methods (always), functions (typed return or
    /// setter), constructors and typedefs walk, and a parameter with no
    /// written type whose name is not just underscores reports
    /// (:56, :81, :108, :138-158). Constructors skip enums and
    /// `_`-private owners or names (:130-135); augmentations skip on every
    /// face (:52, :63, :72, :99, :113, :122).
    ///
    /// dartq faces: the written-type questions read the `Type`/`NoType`
    /// child rows and the declaration keyword (pure AST); the
    /// `hasInferredType` initializer face (:143-148) reads the initializer's
    /// presence and its `null` literal — a `dynamic`-typed initializer's
    /// static type is not derivable, so the exemption holds there where the
    /// official reports (the documented under-report face). The generalized
    /// typedef (the `=` payload word) is not a `FunctionTypeAlias` and never
    /// enters the face. Message: `diagnostic.g.dart` `typeAnnotatePublicApis`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "type_annotate_public_apis",
        group: Group::Standalone,
        problem: "Missing type annotation on a public API.",
        correction: "Try adding a type annotation.",
        kinds: &[
            NodeKind::TopLevelFields,
            NodeKind::Fields,
            NodeKind::TopLevelMethod,
            NodeKind::Method,
            NodeKind::Typedef,
            NodeKind::Constructor,
            NodeKind::PrimaryConstructor,
        ],
        check,
    };

    fn check(c: &mut Check<'_>) {
        match c.kind(c.node()) {
            NodeKind::TopLevelFields | NodeKind::Fields => check_fields(c),
            NodeKind::TopLevelMethod => check_function(c),
            NodeKind::Method => check_method(c),
            NodeKind::Typedef => check_typedef(c),
            NodeKind::Constructor => check_constructor(c),
            NodeKind::PrimaryConstructor => check_primary(c),
            _ => {}
        }
    }

    /// `visitFieldDeclaration`/`visitTopLevelVariableDeclaration`
    /// (:62-67, :122-127): no written type on the row walks its public
    /// variables.
    fn check_fields(c: &mut Check<'_>) {
        let node = c.node();
        if in_augmentation(c) {
            return;
        }
        // `node.fields.type == null` (:65)/`node.variables.type == null`
        // (:125): a written type keeps the whole row out.
        if c.collect_direct(node).iter().any(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::Type | NodeKind::VoidType | NodeKind::FunctionType
            )
        }) {
            return;
        }
        for decl in c.collect_direct(node) {
            if c.kind(decl) != NodeKind::VariablesDeclaration {
                continue;
            }
            // `!node.isConst` / `isFinal` (:163-164): the declaration
            // keyword.
            let keyword = variables_declaration_keyword(c, decl);
            if keyword.is_some_and(|kw| c.lexeme(kw) == "const") {
                continue;
            }
            let is_final = keyword.is_some_and(|kw| c.lexeme(kw) == "final");
            for var in c.collect_direct(decl) {
                if c.kind(var) != NodeKind::InitializedIdentifier {
                    continue;
                }
                let Some(name) = initialized_identifier_name(c, var) else {
                    continue;
                };
                if c.lexeme(name).starts_with('_') {
                    continue;
                }
                // `!(node.isFinal && hasInferredType(node))` (:164, the
                // initializer face :143-148).
                if is_final && has_inferred_initializer(c, var) {
                    continue;
                }
                let range = crate::rules::utf16_token_span(c, c.head_token(var));
                c.report(range, &[]);
            }
        }
    }

    /// `hasInferredType` (:143-148): an initializer present whose value is
    /// not the `null` literal.
    fn has_inferred_initializer(c: &Check<'_>, var: u32) -> bool {
        let Some(initializer) = c
            .collect_direct(var)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer)
        else {
            return false;
        };
        first_child(c, initializer).is_some_and(|value| c.kind(value) != NodeKind::LiteralNull)
    }

    /// `visitFunctionDeclaration` (:71-84): the missing-return-type report
    /// at the name, otherwise the parameter walk — a setter never reports
    /// but always walks.
    fn check_function(c: &mut Check<'_>) {
        let node = c.node();
        if in_augmentation(c) {
            return;
        }
        let Some(name) = member_name(c, node) else {
            return;
        };
        if c.lexeme(c.head_token(name)).starts_with('_') {
            return;
        }
        let is_setter = get_set_token(c, node) == Some("set");
        if return_type_missing(c, node, name) && !is_setter {
            let range = crate::rules::utf16_token_span(c, c.head_token(name));
            c.report(range, &[]);
        } else {
            walk_parameters(c, node);
        }
    }

    /// `visitMethodDeclaration` (:98-109): the missing-return-type report at
    /// the name (setters and `[]=` never report), then the parameter walk —
    /// methods walk their parameters unconditionally (:108).
    fn check_method(c: &mut Check<'_>) {
        let node = c.node();
        if in_augmentation(c) {
            return;
        }
        let Some(name) = member_name(c, node) else {
            return;
        };
        if c.lexeme(c.head_token(name)).starts_with('_') {
            return;
        }
        let is_setter = get_set_token(c, node) == Some("set");
        let index_setter = c.kind(name) == NodeKind::OperatorName
            && c
                .payload(name)
                .first()
                .copied()
                .is_some_and(|op| c.lexeme(op) == "[]=");
        if return_type_missing(c, node, name) && !is_setter && !index_setter {
            let token = match c.kind(name) {
                NodeKind::OperatorName => c.payload(name).first().copied(),
                _ => Some(c.head_token(name)),
            };
            if let Some(token) = token {
                let range = crate::rules::utf16_token_span(c, token);
                c.report(range, &[]);
            }
        }
        walk_parameters(c, node);
    }

    /// `node.returnType == null` (:78, :102, :89): the type face before the
    /// name is `NoType` (or unrecovered).
    fn return_type_missing(c: &Check<'_>, owner: u32, name: u32) -> bool {
        match type_before(c, owner, name) {
            Some((_, kind)) => kind == NodeKind::NoType,
            None => true,
        }
    }

    /// `visitFunctionTypeAlias` (:87-95): the legacy form only — the
    /// generalized typedef carries the `=` payload word and is no
    /// `FunctionTypeAlias`.
    fn check_typedef(c: &mut Check<'_>) {
        let node = c.node();
        if c.payload(node).get(1).copied() != Some(crate::rules::NONE_TOKEN) {
            return;
        }
        let Some(name) = member_name(c, node) else {
            return;
        };
        if c.lexeme(c.head_token(name)).starts_with('_') {
            return;
        }
        if return_type_missing(c, node, name) {
            let range = crate::rules::utf16_token_span(c, c.head_token(name));
            c.report(range, &[]);
        } else {
            walk_parameters(c, node);
        }
    }

    /// `visitConstructorDeclaration` (:51-59) over `_isPublicConstructor`
    /// (:130-135): enums skip, a `_`-private owner or constructor name
    /// skips, then the parameters walk.
    fn check_constructor(c: &mut Check<'_>) {
        let node = c.node();
        if in_augmentation(c) {
            return;
        }
        let Some(owner) = class_like_ancestor(c) else {
            return;
        };
        if !constructor_owner_public(c, owner) {
            return;
        }
        // `!constructor.isPrivate` (:134): a `_`-named constructor is
        // private, the unnamed form is public-facing.
        let ctx = dartq_parser::IdentifierContext::MethodDeclarationContinuation as u32;
        let named = c
            .direct_children(node)
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::Identifier && c.payload(kid).first() == Some(&ctx));
        if named.is_some_and(|name| c.lexeme(c.head_token(name)).starts_with('_')) {
            return;
        }
        walk_parameters(c, node);
    }

    /// `visitPrimaryConstructorDeclaration` (:112-119): the owner is the
    /// wrapper's class-like child (the primary is the wrapper's direct
    /// child, no class-like ancestor).
    fn check_primary(c: &mut Check<'_>) {
        let node = c.node();
        if in_augmentation(c) {
            return;
        }
        let Some(owner) = c
            .unit
            .ast
            .parent(node)
            .map(|wrapper| {
                c.collect_direct(wrapper).into_iter().find(|&kid| {
                    matches!(
                        c.kind(kid),
                        NodeKind::ClassDeclaration
                            | NodeKind::MixinDeclaration
                            | NodeKind::ExtensionDeclaration
                            | NodeKind::ExtensionTypeDeclaration
                            | NodeKind::EnumDeclaration
                    )
                })
            })
            .flatten()
        else {
            return;
        };
        if !constructor_owner_public(c, owner) {
            return;
        }
        // The written primary-constructor name (the primary-constructor
        // context identifier); the unnamed form is public-facing.
        let ctx = dartq_parser::IdentifierContext::PrimaryConstructorDeclaration as u32;
        let named = c
            .direct_children(node)
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::Identifier && c.payload(kid).first() == Some(&ctx));
        if named.is_some_and(|name| c.lexeme(c.head_token(name)).starts_with('_')) {
            return;
        }
        walk_parameters(c, node);
    }

    /// The first class-like ancestor of the dispatched constructor.
    fn class_like_ancestor(c: &Check<'_>) -> Option<u32> {
        c.path
            .iter()
            .rev()
            .skip(1)
            .copied()
            .find(|&ancestor| {
                matches!(
                    c.kind(ancestor),
                    NodeKind::ClassDeclaration
                        | NodeKind::MixinDeclaration
                        | NodeKind::ExtensionDeclaration
                        | NodeKind::ExtensionTypeDeclaration
                        | NodeKind::EnumDeclaration
                )
            })
    }

    /// `_isPublicConstructor`'s owner faces (:131-133): enums never public,
    /// a `_`-private owner never public.
    fn constructor_owner_public(c: &Check<'_>, owner: u32) -> bool {
        if c.kind(owner) == NodeKind::EnumDeclaration {
            return false;
        }
        crate::rules::class_declaration_name(c, owner)
            .is_none_or(|name| !name.starts_with('_'))
    }

    /// `_VisitorHelper.visitRegularFormalParameter` (:150-158): a parameter
    /// with no written type whose name is not just underscores reports at
    /// the parameter. Field/super formals and function-typed formals are
    /// separate visitor faces upstream and never enter here (the receiver
    /// word and the row kind separate them).
    fn walk_parameters(c: &mut Check<'_>, owner: u32) {
        for kid in c.collect_direct(owner) {
            match c.kind(kid) {
                NodeKind::FormalParameters | NodeKind::OptionalFormalParameters => {
                    walk_parameters(c, kid);
                }
                NodeKind::FormalParameter => check_parameter(c, kid),
                _ => {}
            }
        }
    }

    fn check_parameter(c: &mut Check<'_>, param: u32) {
        let receiver = c
            .payload(param)
            .first()
            .copied()
            .unwrap_or(crate::rules::NONE_TOKEN);
        if receiver != crate::rules::NONE_TOKEN {
            return; // `this.x` / `super.x`: not the `RegularFormalParameter` face
        }
        // `param.type == null` (:152): the written type is the row's first
        // child (`NoType` when the grammar allows its absence).
        if let Some(&first) = c.collect_direct(param).first()
            && matches!(
                c.kind(first),
                NodeKind::Type | NodeKind::VoidType | NodeKind::FunctionType
            )
        {
            return;
        }
        let Some(name) = c.payload(param).get(1).copied() else {
            return;
        };
        if name == crate::rules::NONE_TOKEN || is_just_underscores(c.lexeme(name)) {
            return; // `paramName != null && !paramName.isJustUnderscores` (:153-154)
        }
        let range = c.utf16_span(param);
        c.report(range, &[]);
    }

    /// The official `node.isAugmentation` face: the augment bit on the
    /// dispatched declaration row; the primary-constructor face also reads
    /// the owning class-like (`augment class` carries the augmentation).
    fn in_augmentation(c: &Check<'_>) -> bool {
        let node = c.node();
        if dartq_ast::mod_flags(c.unit.ast, node) & AUGMENT_BIT != 0 {
            return true;
        }
        if c.kind(node) == NodeKind::PrimaryConstructor
            && let Some(owner) = c.unit.ast.parent(node)
        {
            return c
                .collect_direct(owner)
                .into_iter()
                .find(|&kid| {
                    matches!(
                        c.kind(kid),
                        NodeKind::ClassDeclaration
                            | NodeKind::MixinDeclaration
                            | NodeKind::ExtensionDeclaration
                            | NodeKind::ExtensionTypeDeclaration
                            | NodeKind::EnumDeclaration
                    )
                })
                .is_some_and(|class_like| {
                    dartq_ast::mod_flags(c.unit.ast, class_like) & AUGMENT_BIT != 0
                });
        }
        false
    }
}
