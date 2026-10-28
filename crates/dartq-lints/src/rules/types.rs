//! The type-reference family: `avoid_shadowing_type_parameters`,
//! `library_private_types_in_public_api`, plus the standalone
//! `prefer_constructors_over_static_methods`,
//! `type_annotate_public_apis`, `strict_top_level_inference`, the
//! type-annotation readers `prefer_void_to_null`, `no_raw_types`,
//! `always_specify_types`, `avoid_futureor_void`, the resource-leak
//! detectors `cancel_subscriptions` and `close_sinks`,
//! `tighten_type_of_initializing_formals`, and the variance checker
//! `unsafe_variance`.

use dartq_ast::{NodeKind, PayloadField};

use crate::engine::Check;
use crate::registry::RuleSpec;

/// The domain's incoming registrations: a new standalone rule appends its
/// `SPEC` reference here (name order) and joins the registry through the
/// aggregated tail — adding a rule touches only this file. Batch close
/// promotes the block into the registry's frozen skeleton together with
/// the pin-test wave list (`tests/registry_pin.rs`). The slice is empty
/// since the W6 closeout (#1213): the L3 `unsafe_variance` face was
/// promoted into the frozen skeleton in that one edit.
pub(crate) static INCOMING_RULES: &[&RuleSpec] = &[];

// —— shared type-annotation readers ———————————————————————————————————————
//
// The written-type rules read one and the same face: the width-0 `Type`
// leaf (bridge.rs `handle_type` — the span covers the name plus a trailing
// `?`), whose `TypeArguments` ride a sibling row. The helpers below are the
// domain-private key to that face; the per-rule approximations stay in
// their modules.

/// The dart:core / dart:async generic types the pure-syntax raw-type face
/// recognizes, with their type-parameter counts. The stand-in for the
/// resolver question `element.typeParameters.isNotEmpty` on names that
/// leave the file — every entry is genuinely generic, so the face never
/// over-reports; imported user generics (and the dart:collection tail)
/// stay silent.
const CORE_GENERICS: &[(&str, usize)] = &[
    ("Comparable", 1),
    ("Future", 1),
    ("FutureOr", 1),
    ("HashMap", 2),
    ("Iterable", 1),
    ("LinkedHashMap", 2),
    ("List", 1),
    ("Map", 2),
    ("MapEntry", 2),
    ("Set", 1),
    ("SplayTreeMap", 2),
    ("Stream", 1),
];

/// The type-annotation row kinds (`Type`/`VoidType`/`FunctionType`/
/// `RecordType` — the written-type family every reader walks).
const TYPE_FAMILY: [NodeKind; 4] = [
    NodeKind::Type,
    NodeKind::VoidType,
    NodeKind::FunctionType,
    NodeKind::RecordType,
];

/// The `TypeArguments` sibling row of a `Type` row. Production completion
/// order puts the sibling before the row (the leaf seals after the
/// argument list parses); hand-built trees may place it after.
fn type_arguments_row(c: &Check<'_>, type_row: u32) -> Option<u32> {
    let owner = c.unit.ast.parent(type_row)?;
    let members = c.collect_direct(owner);
    let index = members.iter().position(|&m| m == type_row)?;
    let is_args = |row: &u32| c.kind(*row) == NodeKind::TypeArguments;
    members[..index]
        .iter()
        .rev()
        .find(|row| is_args(row))
        .or_else(|| members[index + 1..].iter().find(|row| is_args(row)))
        .copied()
}

/// The type-annotation rows held by a `TypeArguments` row, in order.
fn type_argument_rows(c: &Check<'_>, args_row: u32) -> Vec<u32> {
    c.collect_direct(args_row)
        .into_iter()
        .filter(|&row| TYPE_FAMILY.contains(&c.kind(row)))
        .collect()
}

/// The first type-annotation child of `row`, if any (`NoType` does not
/// count — it marks the absence the caller asks about).
fn first_type_child(c: &Check<'_>, row: u32) -> Option<u32> {
    c.collect_direct(row)
        .into_iter()
        .find(|&kid| TYPE_FAMILY.contains(&c.kind(kid)))
}

/// The written-type row directly before `row` among its parent's children —
/// the production bridge anchors a declaration's written type as the
/// declaration's left sibling (the `Type`/`NoType`/`FunctionType`/`RecordType`
/// child of `Fields`, `TopLevelFields`, `Method`, `TopLevelMethod`,
/// `Constructor`, `Typedef`, `TypeVariable`, or a parameter list's owner),
/// never inside `VariablesDeclaration`/`InitializedIdentifier` themselves.
/// `None` when `row` is the first child or the preceding sibling carries
/// no type family kind (`NoType` DOES count: callers distinguish "no
/// written type" from "written type" by the returned kind).
fn type_sibling_before(c: &Check<'_>, row: u32) -> Option<u32> {
    let parent = c.unit.ast.parent(row)?;
    let siblings = c.collect_direct(parent);
    let pos = siblings.iter().position(|&kid| kid == row)?;
    let prev = siblings[..pos].iter().rev().copied().find(|&kid| {
        matches!(
            c.kind(kid),
            NodeKind::Type
                | NodeKind::VoidType
                | NodeKind::FunctionType
                | NodeKind::RecordType
                | NodeKind::NoType
        )
    })?;
    Some(prev)
}

/// The member/function's own name — the `Identifier` or `OperatorName`
/// directly before the parameters node (a `TypeVariables`/`NoTypeVariables`
/// row may sit between them), else the sole name child. A written return
/// type tiles its own `Identifier`/`Type` pair *before* the member name, so
/// the shared `member_name` first-child pick lands on the type name; this
/// is the params-anchored read.
fn member_name_typed(c: &Check<'_>, owner: u32) -> Option<u32> {
    let direct = c.collect_direct(owner);
    if let Some(op) = direct
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::OperatorName)
    {
        return Some(op);
    }
    // The operator name anchors as the member's left sibling under the
    // declaration body (`operator []=`: `OperatorName` precedes the
    // `Method` row rather than nesting inside it).
    if let Some(op) = c
        .unit
        .ast
        .parent(owner)
        .and_then(|parent| {
            let siblings = c.collect_direct(parent);
            siblings
                .iter()
                .position(|&s| s == owner)
                .and_then(|i| siblings[..i].iter().rev().copied().next())
        })
        .filter(|&prev| c.kind(prev) == NodeKind::OperatorName)
    {
        return Some(op);
    }
    direct
        .iter()
        .copied()
        .find(|&kid| {
            if c.kind(kid) != NodeKind::Identifier {
                return false;
            }
            let Some(i) = direct.iter().position(|&s| s == kid) else {
                return false;
            };
            direct[i + 1..]
                .iter()
                .copied()
                .find(|&next| {
                    !matches!(
                        c.kind(next),
                        NodeKind::TypeVariables | NodeKind::NoTypeVariables
                    )
                })
                .is_some_and(|next| {
                    matches!(
                        c.kind(next),
                        NodeKind::FormalParameters | NodeKind::NoFormalParameters
                    )
                })
        })
        .or_else(|| {
            direct
                .iter()
                .copied()
                .find(|&kid| c.kind(kid) == NodeKind::Identifier)
        })
}

/// Whether the written type at `head` carries a prefix (`p.List`): the
/// preceding token is the `.` of a qualified spelling.
fn prefixed_type(c: &Check<'_>, head: u32) -> bool {
    head.checked_sub(1)
        .is_some_and(|prev| c.lexeme(prev) == ".")
}

/// The raw-generic face of a written named type: `Some((fills,
/// optional_type_args))` when the name is a same-file generic declaration
/// or a [`CORE_GENERICS`] entry — the pure-syntax stand-in for the resolver
/// questions behind `no_raw_types`/`always_specify_types`
/// (`element.typeParameters`, the alias/interface instantiation).
///
/// Same-file fills read each type variable's bound (the
/// `instantiateToBounds` face: a bounded variable fills with its bound, an
/// unbounded one with `dynamic`); core types fill all-`dynamic`. The
/// `optional_type_args` word reads a same-file `@optionalTypeArgs`
/// annotation (the upstream `element.metadata.hasOptionalTypeArgs` face is
/// element-based, so imported annotations stay silent).
fn raw_generic_face(c: &Check<'_>, row: u32) -> Option<(Vec<String>, bool)> {
    let head = c.head_token(row);
    let name = c.lexeme(head);
    if prefixed_type(c, head) {
        return None;
    }
    if let Some(decl) = crate::rules::same_file_declaration(c, name) {
        // The `TypeVariables` row tiles under the declaration's
        // `TopLevelDeclaration` wrapper (a sibling of the class-like row),
        // not inside the class-like itself.
        let type_variables = c
            .unit
            .ast
            .parent(decl)
            .map(|wrapper| {
                c.collect_direct(wrapper)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::TypeVariables)
            })
            .flatten()
            .or_else(|| {
                c.collect_direct(decl)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::TypeVariables)
            })?;
        let optional = optional_type_args_annotation(c, decl);
        let fills = c
            .collect_direct(type_variables)
            .into_iter()
            .filter(|&tv| c.kind(tv) == NodeKind::TypeVariable)
            .map(|tv| match first_type_child(c, tv) {
                Some(bound) => written_display(c, bound),
                None => "dynamic".to_owned(),
            })
            .collect();
        return Some((fills, optional));
    }
    let arity = CORE_GENERICS
        .iter()
        .find(|(core, _)| *core == name)
        .map(|&(_, arity)| arity)?;
    Some((vec!["dynamic".to_owned(); arity], false))
}

/// Whether the class-like declaration carries an `@optionalTypeArgs`
/// annotation. The metadata precedes the top-level `TopLevelDeclaration`
/// *wrapper* at unit level (`member_annotations` on the wrapper does not
/// reach it).
fn optional_type_args_annotation(c: &Check<'_>, decl: u32) -> bool {
    let metas = c
        .unit
        .ast
        .parent(decl)
        .and_then(|wrapper| c.unit.ast.parent(wrapper).map(|unit| (unit, wrapper)))
        .map(|(unit, wrapper)| crate::rules::member_annotations(c, unit, wrapper))
        .unwrap_or_default();
    metas.iter().any(|&meta| {
        c.collect_direct(meta).into_iter().any(|piece| {
            c.kind(piece) == NodeKind::Identifier
                && c.lexeme(c.head_token(piece)) == "optionalTypeArgs"
        })
    })
}

/// The written display of a type-annotation row (`List<int>`, `dynamic`,
/// `Foo<num>?`) — the `{0}` fill of the raw-type messages. A function or
/// record bound renders as a `Function`/`(...)` placeholder (the
/// documented under-approximation; those bounds are exotic).
fn written_display(c: &Check<'_>, row: u32) -> String {
    match c.kind(row) {
        NodeKind::VoidType => "void".to_owned(),
        NodeKind::FunctionType => "Function".to_owned(),
        NodeKind::RecordType => "(...)".to_owned(),
        _ => {
            let mut out = c.lexeme(c.head_token(row)).to_owned();
            if let Some(args) = type_arguments_row(c, row) {
                let parts: Vec<String> = type_argument_rows(c, args)
                    .iter()
                    .map(|&arg| written_display(c, arg))
                    .collect();
                if !parts.is_empty() {
                    out.push('<');
                    out.push_str(&parts.join(", "));
                    out.push('>');
                }
            }
            if dartq_ast::question(c.unit.tokens, c.unit.ast, row) {
                out.push('?');
            }
            out
        }
    }
}

pub(crate) mod avoid_shadowing_type_parameters {
    use dartq_parser::MemberKind;

    use super::{PayloadField, Check, NodeKind, RuleSpec};

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
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_shadowing_type_parameters",
    &[
            NodeKind::LocalFunctionDeclaration,
            NodeKind::FunctionExpression,
            NodeKind::Method,
            NodeKind::FunctionType,
            NodeKind::FunctionTypedFormalParameter,
        ],
    check,
);

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // Static methods have nothing above them to shadow (:76-85).
        if c.kind(node) == NodeKind::Method
            && c.field(node, PayloadField::DeclarationKind)
                == Some(MemberKind::StaticMethod as u32)
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
                .filter_map(|&p| type_variable_name(c, p).map(|n| c.lexeme(n)))
                .collect();
            let shadowed: Vec<u32> = mine_parameters
                .iter()
                .copied()
                .filter(|&parameter| {
                    let Some(name) = type_variable_name(c, parameter) else {
                        return false;
                    };
                    let lexeme = c.lexeme(name);
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
                c.report(c.utf16_span(parameter), &[c.lexeme(name), kind]);
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

    /// The name token of a `TypeVariable` row: the first identifier-kind
    /// token of its span. Production completion order emits the name
    /// `Identifier` before `begin_type_variable` (`parse_variables`), so
    /// the name tiles as the row's PRECEDING sibling — the token range,
    /// not the child set, is the link (the same read the
    /// `avoid_types_as_parameter_names` type-variable face makes).
    fn type_variable_name(c: &Check<'_>, parameter: u32) -> Option<u32> {
        let range = c.unit.ast.token_range(parameter);
        (range.start..range.end).find(|&t| {
            c.unit
                .tokens
                .get(t as usize)
                .is_some_and(|token| token.kind == dartq_scanner::TokenKind::Identifier)
        })
    }
}

pub(crate) mod library_private_types_in_public_api {
    use crate::rules::type_before;

    use super::{PayloadField, Check, NodeKind, RuleSpec, first_type_child, member_name_typed, record_type_fields, type_argument_rows, type_arguments_row};

    /// Official: `pkg/linter/lib/src/rules/library_private_types_in_public_api.dart:41-246`
    /// — a `_`-private named type used in a public API signature reports at
    /// the type name. The walk starts at the unit's declarations, skips
    /// everything under a private library name (:245-251) or a private
    /// declaration name, and checks the type positions of public members:
    /// top-level variables/functions, class-like members (methods, fields,
    /// constructors), and typedefs. The header faces read same-file: the
    /// extension `on` type (:80-88), the mixin `on` constraints (:196-203),
    /// and every type-parameter bound (:275-277); a prefixed written
    /// reference (`p._Foo`) reads the continuation identifier (the
    /// `PrefixedType.parseType` tiling — the `Type` row spans the head
    /// name only); an unwritten field formal `this.x` reads the
    /// declaring class's instance-field type (:123-142), an unwritten
    /// super formal `super.x` the same-file superclass's (:238-258 — the
    /// element arm; a cross-file superclass, a super constructor parameter
    /// that is no field, and a superclass chain beyond the direct step
    /// under-report). Message: `diagnostic.g.dart`
    /// `libraryPrivateTypesInPublicApi`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "library_private_types_in_public_api",
    &[NodeKind::CompilationUnit],
    check,
);

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
                // Production top-level declarations ride a
                // `TopLevelDeclaration` wrapper (the metadata/name prelude);
                // the faces dispatch through it.
                NodeKind::TopLevelDeclaration => walk_wrapper(c, decl),
                NodeKind::Typedef => walk_typedef(c, decl),
                NodeKind::TopLevelFields => walk_variable_declarations(c, decl),
                NodeKind::TopLevelMethod => walk_function(c, decl),
                _ => {}
            }
        }
    }

    /// One production declaration wrapper: class-likes ride the wrapper
    /// itself (the name `Identifier` precedes the declaration row inside
    /// it), the other faces dispatch on the wrapped row.
    fn walk_wrapper(c: &mut Check<'_>, wrapper: u32) {
        let kids = c.collect_direct(wrapper);
        for &kid in &kids {
            match c.kind(kid) {
                NodeKind::TopLevelMethod => walk_function(c, kid),
                NodeKind::TopLevelFields => walk_variable_declarations(c, kid),
                NodeKind::Typedef => walk_typedef(c, kid),
                _ => {}
            }
        }
        if kids.iter().any(|kid| {
            matches!(
                c.kind(*kid),
                NodeKind::ClassDeclaration
                    | NodeKind::MixinDeclaration
                    | NodeKind::EnumDeclaration
                    | NodeKind::ExtensionDeclaration
                    | NodeKind::ExtensionTypeDeclaration
                    | NodeKind::NamedMixinApplication
            )
        }) {
            walk_class_like(c, wrapper);
        }
    }

    /// A class-like declaration behind its wrapper: a private name skips
    /// the whole body (:43-53); otherwise the members walk.
    fn walk_class_like(c: &mut Check<'_>, wrapper: u32) {
        let kids = c.collect_direct(wrapper);
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
        // The declaration name rides as a wrapper `Identifier` for
        // class/mixin/enum; an extension or extension type instead carries
        // its name token in the declaration's payload word 2 — the
        // extension's `on` type is a child `Identifier`/`Type` pair, not the
        // name. A private name — or, for an extension, no name (:82-85
        // `name == null` bails) — skips the whole body.
        let name_token = kids
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::Identifier)
            .map(|name| c.head_token(name))
            .or_else(|| {
                // The declaration's `Name` word (`begin_extension_declaration`
                // / `begin_extension_type_declaration` patch the name token).
                if !matches!(
                    c.kind(decl),
                    NodeKind::ExtensionDeclaration | NodeKind::ExtensionTypeDeclaration
                ) {
                    return None;
                }
                c.field(decl, PayloadField::Name)
                    .filter(|&tok| tok != crate::rules::NONE_TOKEN)
            });
        match name_token {
            Some(tok) if !c.lexeme(tok).starts_with('_') => {}
            _ => return,
        }
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
        walk_header_types(c, decl, wrapper, body);
        let in_enum = c.kind(decl) == NodeKind::EnumDeclaration;
        let members = c.direct_children(body).iter().copied().collect::<Vec<_>>();
        for member in members {
            match c.kind(member) {
                NodeKind::Method => walk_signature(c, member),
                NodeKind::Constructor if !in_enum => walk_constructor(c, decl, member),
                NodeKind::Fields => walk_variable_declarations(c, member),
                _ => {}
            }
        }
    }

    /// The header type faces: type-parameter bounds (every class-like,
    /// :47/:59/:75/:85/:96/:180/:191/:201 — `visitTypeParameter` :275-277
    /// walks each bound) and, for extensions and mixins, the `on`-clause
    /// types (:86, :200). The rows tile in the *wrapper's* completion window
    /// before the body — the `TypeVariables` row is a sibling of the
    /// declaration under the wrapper (a bound is the `Type` row under its
    /// [`NodeKind::TypeVariable`]), while an `on` type stays a `Type` row
    /// whose parent is the declaration itself. A class's
    /// `extends`/`implements` header types share that parentage and stay
    /// unchecked (:42-52 walks the type parameters and members only).
    fn walk_header_types(c: &mut Check<'_>, decl: u32, wrapper: u32, body: u32) {
        let body_start = c.byte_span(body).start;
        let on_clause_kind = matches!(
            c.kind(decl),
            NodeKind::ExtensionDeclaration | NodeKind::MixinDeclaration
        );
        for row in c.children(wrapper) {
            if c.byte_span(row).start >= body_start {
                break;
            }
            match c.kind(row) {
                NodeKind::TypeVariable => {
                    for inner in c.children(row) {
                        if c.kind(inner) == NodeKind::Type && c.unit.ast.parent(inner) == Some(row)
                        {
                            check_type(c, inner);
                            break;
                        }
                    }
                }
                NodeKind::Type => {
                    if on_clause_kind && c.unit.ast.parent(row) == Some(decl) {
                        check_type(c, row);
                    }
                }
                _ => {}
            }
        }
    }

    /// A method face: a private name skips; otherwise return type,
    /// the member's own type-parameter bounds (`node.typeParameters`,
    /// :191), then parameters (:176-182).
    fn walk_signature(c: &mut Check<'_>, member: u32) {
        let Some(name) = member_name_typed(c, member) else {
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
        walk_type_variables(c, member);
        walk_parameters(c, member);
    }

    /// A top-level function face (:160-166): return type, then the
    /// function's own type-parameter bounds (`functionExpression
    /// .typeParameters`, :155), then the parameters.
    fn walk_function(c: &mut Check<'_>, function: u32) {
        let Some(name) = member_name_typed(c, function) else {
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
        walk_type_variables(c, function);
        walk_parameters(c, function);
    }

    /// The declaration's own `TypeVariables` bounds face: each
    /// `TypeVariable`'s bound is the `Type` row under it (the
    /// `visitTypeParameter` walk, :275-277 — the same read the header
    /// faces make over the wrapper window).
    fn walk_type_variables(c: &mut Check<'_>, owner: u32) {
        let Some(type_variables) = c
            .collect_direct(owner)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::TypeVariables)
        else {
            return;
        };
        for variable in c.collect_direct(type_variables) {
            if c.kind(variable) != NodeKind::TypeVariable {
                continue;
            }
            for inner in c.children(variable) {
                if c.kind(inner) == NodeKind::Type && c.unit.ast.parent(inner) == Some(variable) {
                    check_type(c, inner);
                    break;
                }
            }
        }
    }

    /// A constructor face (:57-63): a `_`-named constructor is private;
    /// the unnamed form is public-facing. Enum constructors are skipped by
    /// the caller (the official `isEffectivelyPrivate`, :225-235).
    fn walk_constructor(c: &mut Check<'_>, decl: u32, constructor: u32) {
        let named = c
            .direct_children(constructor)
            .iter()
            .copied()
            .filter(|&kid| c.kind(kid) == NodeKind::Identifier)
            .nth(1);
        if named.is_some_and(|name| c.lexeme(c.head_token(name)).starts_with('_')) {
            return;
        }
        let Some(params) = c
            .direct_children(constructor)
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
            match c.kind(param) {
                NodeKind::FormalParameter => {
                    // The declared-type branch (:128-132/:243-248) walks
                    // first — a written type checks and returns. The type
                    // row is the parameter's *second* child: the written
                    // name's `Identifier` heads the pair, so a first-child
                    // check lands on the name and misses the type.
                    let written = c.direct_children(param).iter().copied().find(|&kid| {
                        matches!(
                            c.kind(kid),
                            NodeKind::Type | NodeKind::FunctionType | NodeKind::RecordType
                        )
                    });
                    if let Some(written) = written {
                        check_type(c, written);
                        continue;
                    }
                    check_implicit_formal(c, decl, param);
                }
                // The function-typed suffix's return type (:228-231).
                NodeKind::FunctionTypedFormalParameter => {
                    let written = c.direct_children(param).iter().copied().find(|&kid| {
                        matches!(
                            c.kind(kid),
                            NodeKind::Type | NodeKind::FunctionType | NodeKind::RecordType
                        )
                    });
                    if let Some(written) = written {
                        check_type(c, written);
                    }
                }
                _ => {}
            }
        }
    }

    /// The implicit initializing-formal faces (:133-142/:249-258): a field
    /// formal `this.x` without a written type reads the declaring class's
    /// instance-field type, a super formal `super.x` the same-file
    /// superclass's (the element arm — a cross-file superclass, a
    /// non-field super parameter, and a chain beyond the direct step
    /// under-report). A `_`-private field type reports at the parameter
    /// name (`reportAtToken(node.name)`); a named private formal skips
    /// (:124-126/:239-241).
    fn check_implicit_formal(c: &mut Check<'_>, decl: u32, param: u32) {
        let receiver = c
            .field(param, PayloadField::Receiver)
            .unwrap_or(crate::rules::NONE_TOKEN);
        if receiver == crate::rules::NONE_TOKEN {
            return;
        }
        let Some(name_token) = c.field(param, PayloadField::Name) else {
            return;
        };
        let name = c.lexeme(name_token);
        let named = matches!(
            c.field(param, PayloadField::FormalParameterKind),
            Some(kind)
                if kind == dartq_parser::FormalParameterKind::RequiredNamed as u32
                    || kind == dartq_parser::FormalParameterKind::OptionalNamed as u32
        );
        if named && name.starts_with('_') {
            return;
        }
        let field_type = match c.lexeme(receiver) {
            "this" => instance_field_type(c, decl, name),
            "super" => super_field_type(c, decl, name),
            _ => None,
        };
        if let Some(field_type) = field_type {
            if c.lexeme(c.head_token(field_type)).starts_with('_') {
                c.report(crate::rules::utf16_token_span(c, name_token), &[]);
            }
        }
    }

    /// The written type of the instance field `name` in the class-like
    /// `decl` — the same face the shared field-type index reads: a
    /// non-static `Fields` member whose group type (or per-variable type)
    /// names the field.
    fn instance_field_type(c: &Check<'_>, decl: u32, name: &str) -> Option<u32> {
        let Some(body) = c
            .direct_children(decl)
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::DeclarationBody)
        else {
            return None;
        };
        for member in c.direct_children(body).iter().copied() {
            if c.kind(member) != NodeKind::Fields
                || c.field(member, PayloadField::MemberKind)
                    != Some(dartq_parser::MemberKind::NonStaticField as u32)
            {
                continue;
            }
            let group_type = c
                .collect_direct(member)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Type);
            for group in c.collect_direct(member) {
                if c.kind(group) != NodeKind::VariablesDeclaration {
                    continue;
                }
                let written = group_type.or_else(|| {
                    c.collect_direct(group)
                        .into_iter()
                        .find(|&kid| c.kind(kid) == NodeKind::Type)
                });
                for var in c.collect_direct(group) {
                    if c.kind(var) != NodeKind::InitializedIdentifier {
                        continue;
                    }
                    if crate::rules::initialized_identifier_name(c, var)
                        .is_some_and(|token| c.lexeme(token) == name)
                    {
                        return written;
                    }
                }
            }
        }
        None
    }

    /// The written type of the field `name` in the same-file superclass —
    /// the direct `extends` name read off the declaration's `ExtendsKeyword`
    /// word (`class B extends A`: the keyword rides the word, the
    /// superclass head the next token), then the superclass row found
    /// among the unit's class-like wrappers.
    fn super_field_type(c: &Check<'_>, decl: u32, name: &str) -> Option<u32> {
        let extends_kw = c.field(decl, PayloadField::ExtendsKeyword)?;
        if extends_kw == crate::rules::NONE_TOKEN || c.lexeme(extends_kw) != "extends" {
            return None;
        }
        let super_name = c.lexeme(extends_kw + 1);
        let Some(&unit) = c
            .path
            .iter()
            .find(|&&n| c.kind(n) == NodeKind::CompilationUnit)
        else {
            return None;
        };
        for top in c.collect_direct(unit) {
            let kids = c.collect_direct(top);
            let Some(super_decl) = kids
                .iter()
                .copied()
                .find(|&kid| c.kind(kid) == NodeKind::ClassDeclaration)
            else {
                continue;
            };
            let Some(wrapper_name) = kids
                .iter()
                .copied()
                .find(|&kid| c.kind(kid) == NodeKind::Identifier)
            else {
                continue;
            };
            if c.lexeme(c.head_token(wrapper_name)) == super_name {
                return instance_field_type(c, super_decl, name);
            }
        }
        None
    }

    /// A fields face (visitFieldDeclaration :112-120 /
    /// visitTopLevelVariableDeclaration :260-266): any public variable name
    /// exposes the declaration's written type. The `Type` row is the
    /// `VariablesDeclaration`'s SIBLING under the fields row — `parse_fields`
    /// rewinds to the type start and parses the type with the fields row
    /// open, before the variables group — the same read the shared field-type
    /// index makes (`build_field_type_rows`).
    fn walk_variable_declarations(c: &mut Check<'_>, owner: u32) {
        let kids = c.collect_direct(owner);
        // The written-type sibling: a `Type` (`_Foo cb`), a
        // `FunctionType` (`void Function<T extends _Foo>() cb`), or a
        // `RecordType` (`(_Foo, int) cb`).
        let type_row = kids.iter().copied().find(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::Type | NodeKind::FunctionType | NodeKind::RecordType
            )
        });
        for decl in kids {
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
            if let Some(type_row) = type_row {
                check_type(c, type_row);
            }
        }
    }

    /// A typedef face (:168-175): new form checks the aliased type, old
    /// form the return type and parameters; both forms also walk the
    /// typedef's own type-parameter bounds (`node.typeParameters` —
    /// `visitGenericTypeAlias` :183 / `visitFunctionTypeAlias` :171).
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
                // `node.functionType` (:182-183) — the new form's aliased
                // body: a `Type` (`_Foo Function()`'s return family), a
                // `FunctionType` (`void Function<T extends _Foo>()`), or a
                // `RecordType` (`typedef R = (_Foo, int)`).
                NodeKind::Type | NodeKind::FunctionType | NodeKind::RecordType => {
                    check_type(c, kid);
                }
                NodeKind::TypeVariables => walk_type_variables(c, typedef),
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
            let written = c.direct_children(param).iter().copied().find(|&kid| {
                matches!(
                    c.kind(kid),
                    NodeKind::Type | NodeKind::FunctionType | NodeKind::RecordType
                )
            });
            if let Some(written) = written {
                check_type(c, written);
            }
        }
    }

    /// `visitNamedType` (:208-214): a `_`-leading head name reports at the
    /// name token (`reportAtToken(node.name)` — the full token span);
    /// type arguments always walk. The prefixed written face: the
    /// `PrefixedType.parseType` tiling spans the `Type` row at the head
    /// name only, so a period immediately followed by a `_`-leading
    /// continuation identifier reports over the whole `p._Foo` spelling.
    ///
    /// A `void Function<T extends _Foo>(...)` written type tiles its own
    /// `FunctionType` row (the type-annotation sibling, not a `Type`
    /// wrapper) — the generic function type face
    /// (`visitGenericFunctionType` :170-175) walks its return type, its
    /// own type-parameter bounds, and its parameters. The nested faces
    /// carry the same `Type`/`VoidType`, `TypeVariables`, and
    /// `FormalParameters` direct children the declaration faces read.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "u32 is the substrate's token-space width (utf16 offsets, spans)"
    )]
    fn check_type(c: &mut Check<'_>, type_node: u32) {
        if c.kind(type_node) == NodeKind::FunctionType {
            let kids = c
                .direct_children(type_node)
                .iter()
                .copied()
                .collect::<Vec<_>>();
            for kid in kids {
                match c.kind(kid) {
                    // The return type: a `Type`, or a nested
                    // `FunctionType`/`RecordType` row (a record GFT return
                    // tiles directly — `parse_record_type` emits inside the
                    // `FunctionType` span, type_parse.rs:298-304).
                    NodeKind::Type | NodeKind::FunctionType | NodeKind::RecordType => {
                        check_type(c, kid);
                    }
                    NodeKind::TypeVariables => walk_type_variables(c, type_node),
                    NodeKind::FormalParameters => walk_parameters(c, type_node),
                    _ => {}
                }
            }
            return;
        }
        // A record type annotation (`visitRecordTypeAnnotation` —
        // positionalFields + namedFields each accept'd; upstream Dart SDK
        // source, `pkg/linter/lib/src/rules/library_private_types_in_public_api.dart`)
        // walks every field type — records nest (`(_Foo, (int, _Bar))`).
        if c.kind(type_node) == NodeKind::RecordType {
            for field in record_type_fields(c, type_node) {
                check_type(c, field);
            }
            return;
        }
        // A `Type` row may wrap a function/record type (the alias-body and
        // field-shape tilings): walk the wrapped row when present.
        if let Some(inner) = first_type_child(c, type_node)
            && matches!(c.kind(inner), NodeKind::FunctionType | NodeKind::RecordType)
        {
            check_type(c, inner);
            return;
        }
        let head = c.head_token(type_node);
        let tokens = c.unit.tokens;
        let continuation = match (tokens.get(head as usize + 1), tokens.get(head as usize + 2)) {
            (Some(period), Some(seg))
                if period.kind == dartq_scanner::TokenKind::Period
                    && seg.kind == dartq_scanner::TokenKind::Identifier
                    && c.lexeme(head + 2).starts_with('_') =>
            {
                Some(head + 2)
            }
            _ => None,
        };
        if c.lexeme(head).starts_with('_') || continuation.is_some() {
            let end = match continuation {
                Some(seg) => c.utf16_end_of_token(seg),
                None => c.utf16_end_of_token(head),
            };
            if let (Some(start), Some(end)) = (c.utf16_of_token(head), end) {
                c.report(start..end, &[]);
            }
        }
        // `node.typeArguments?.accept(this)` (:211) — the `TypeArguments`
        // row is the `Type`'s *sibling* in the production tiling (it seals
        // after the argument list parses); a hand-built tree may nest it
        // as a child. `type_arguments_row` reads the sibling order first.
        let args = type_arguments_row(c, type_node).or_else(|| {
            c.collect_direct(type_node)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::TypeArguments)
        });
        if let Some(args) = args {
            for arg in type_argument_rows(c, args) {
                // `node.arguments.accept(this)` (:270-272) — each argument
                // is a `Type`, a nested `FunctionType`
                // (`List<void Function<T extends _Foo>()>`), or a
                // `RecordType` (`List<(_Foo, int)>`).
                check_type(c, arg);
            }
        }
    }
}

pub(crate) mod avoid_annotating_with_dynamic {
    use super::{Check, NodeKind, RuleSpec};

    /// The `augment` `ModSet` bit (bridge `mod_bits`).
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
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_annotating_with_dynamic",
    &[NodeKind::FormalParameter],
    check,
);

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
                    return dartq_ast::mod_flags(c.unit.ast, ancestor) & AUGMENT_BIT != 0;
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

    /// Runs `rule` over `source` with the semantic face mounted: the
    /// source folded as the sole analyzed file of `package:t` — the
    /// driver's per-file build shape (the `avoid_dynamic_calls` test
    /// mount). The context tables leak (`'static`); the tests are
    /// one-shot processes.
    fn run_semantic(source: &str, rule: &str) -> Vec<crate::engine::Lint> {
        use dartq_language::{FeatureSet, LanguageVersion};
        use dartq_parser::ParserFeatures;
        use dartq_resolution::{LibraryClassModifiers, LibraryGraph, LibraryGraphBuilder, Namespaces};
        use dartq_semantic::{ElementIndex, MicroKind, StrictModes, analyze_unit, collect};
        use dartq_source::{FileTable, StringTable};

        let fixture = Fixture::parse(source);
        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut files = FileTable::new();
        let file = files.intern(std::path::Path::new("lint_semantic.dart"));
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            LanguageVersion::new(3, 15),
        ));
        let scanned = dartq_scanner::scan(source, crate::testsupport::CONFIG, file);
        assert!(
            scanned.diagnostics.is_empty(),
            "scanner diagnostics: {source:?}"
        );
        let elements = collect(source, &scanned.tokens, features, strings);
        let mut builder = LibraryGraphBuilder::new(strings);
        let lib = builder.intern("package:t/lint_semantic.dart");
        for (name, declaration) in elements.declarations() {
            if declaration.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        let mut index = ElementIndex::default();
        index.insert(file, elements);
        let graph: &'static LibraryGraph = Box::leak(Box::new(builder.finish()));
        let namespaces: &'static Namespaces = Box::leak(Box::new(Namespaces::build(graph)));
        let index: &'static ElementIndex = Box::leak(Box::new(index));
        let analysis = analyze_unit(
            source,
            &scanned.tokens,
            features,
            strings,
            file,
            lib,
            graph,
            namespaces,
            index,
            StrictModes::default(),
            &LibraryClassModifiers::default(),
        );
        let unit = crate::engine::Unit {
            source: &fixture.source,
            tokens: &fixture.tokens,
            ast: &fixture.ast,
            lines: &fixture.lines,
            path: Some(&fixture.path),
            features: fixture.features,
            version: fixture.version,
            workspace: None,
            reachability: None,
            async_context_uses: None,
            library_type_uses: None,
        };
        unit.run_with_semantics(
            &one_rule(rule),
            &rustc_hash::FxHashSet::default(),
            Some(&analysis),
        )
    }

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
    /// name (the production bridge shape: the `Type` is the
    /// `VariablesDeclaration`'s sibling under the fields row).
    #[test]
    fn private_type_top_level_variable_reports() {
        let source = "_Foo x;";
        // _Foo0 x1 ;2
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelFields, 0);
                b.leaf_span(NodeKind::Type, 0, 1);
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, NONE);
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
        assert_eq!(lints[0].range, 0..4);
    }

    /// The corpus faces (the w3-measured set): a `late` top-level
    /// variable's type, a top-level function's return type, and the two
    /// public top-level variables of the typedef fixture (the declarations
    /// ride `TopLevelDeclaration` wrappers). Oracle rows: `late _Td td;` at
    /// the type (len 3), `_Vault openVault()`'s return type (len 6), and
    /// `_Used used1;`/`_Used used2;` at each type (len 4); the private
    /// class body and the typedef are skipped.
    #[test]
    fn corpus_faces_report_through_the_wrappers() {
        let source = concat!(
            "typedef _Td = int Function();\n",
            "late _Td td;\n",
            "class _Vault {}\n",
            "_Vault openVault() => _Vault();\n",
            "typedef _Used = void Function();\n",
            "_Used used1;\n",
            "_Used used2;\n",
        );
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        assert_eq!(lints.len(), 4, "{lints:?}");
        let td = tok(&f.source, &f.tokens, "_Td", 1);
        let start = utf16_at(&f, td);
        assert_eq!(lints[0].range, start..start + 3);
        let vault = tok(&f.source, &f.tokens, "_Vault", 1);
        let start = utf16_at(&f, vault);
        assert_eq!(lints[1].range, start..start + 6);
        let used1 = tok(&f.source, &f.tokens, "_Used", 1);
        let start = utf16_at(&f, used1);
        assert_eq!(lints[2].range, start..start + 5);
        let used2 = tok(&f.source, &f.tokens, "_Used", 2);
        let start = utf16_at(&f, used2);
        assert_eq!(lints[3].range, start..start + 5);
    }

    #[test]
    fn private_variable_name_passes() {
        let source = "_Foo _x;";
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelFields, 0);
                b.leaf_span(NodeKind::Type, 0, 1);
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, NONE);
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
                b.leaf_span(NodeKind::Type, 0, 1);
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, NONE);
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
                b.leaf_span(NodeKind::Type, 3, 4);
                b.begin(NodeKind::VariablesDeclaration, 3);
                b.patch(0, NONE);
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

    // —— the header and implicit faces (production-bridge pins) ——————————

    /// The extension `on` type reports for a public extension
    /// (:80-88); a private-named extension skips whole.
    #[test]
    fn extension_on_private_type_reports() {
        let f = Fixture::parse("extension E on _Foo {}\nclass _Foo {}\n");
        assert_eq!(
            f.run(&one_rule("library_private_types_in_public_api"))
                .len(),
            1
        );

        let f = Fixture::parse("extension _E on _Foo {}\nclass _Foo {}\n");
        assert!(
            f.run(&one_rule("library_private_types_in_public_api"))
                .is_empty()
        );
    }

    /// The mixin `on` constraints report (:196-203); a class's
    /// `extends` header type stays unchecked (:42-52 walks the type
    /// parameters and members only).
    #[test]
    fn mixin_on_constraint_reports_class_extends_passes() {
        let f = Fixture::parse("mixin M on _Foo {}\nclass _Foo {}\n");
        assert_eq!(
            f.run(&one_rule("library_private_types_in_public_api"))
                .len(),
            1
        );

        let f = Fixture::parse("class C extends _Foo {}\nclass _Foo {}\n");
        assert!(
            f.run(&one_rule("library_private_types_in_public_api"))
                .is_empty()
        );
    }

    /// Type-parameter bounds report (:275-277).
    #[test]
    fn type_parameter_bound_reports() {
        let f = Fixture::parse("class C<T extends _Foo> {}\nclass _Foo {}\n");
        assert_eq!(
            f.run(&one_rule("library_private_types_in_public_api"))
                .len(),
            1
        );
    }

    /// A generic member's own type-parameter bounds walk
    /// (`node.typeParameters`, :191 method / :155 function) — `f<T extends
    /// _Priv>` reports at the bound. The `TypeVariables` row between the
    /// member name and the parameters must not mis-bind the name onto the
    /// return type (`_Priv f<T>(int x)` reports at `_Priv`).
    #[test]
    fn member_type_parameter_bound_reports() {
        let f = Fixture::parse("void f<T extends _Foo>() {}\nclass _Foo {}\n");
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let bound = tok(&f.source, &f.tokens, "_Foo", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, bound));

        let f = Fixture::parse("class C {\n  void f<T extends _Foo>() {}\n}\nclass _Foo {}\n");
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        assert_eq!(lints.len(), 1, "{lints:?}");

        let f = Fixture::parse("_Priv f<T>(int x) => x;\nclass _Priv {}\n");
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let priv_head = tok(&f.source, &f.tokens, "_Priv", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, priv_head));
    }

    /// The typedef's own type-parameter bound walks
    /// (`node.typeParameters?.accept(this)` — `visitGenericTypeAlias` :183
    /// and `visitFunctionTypeAlias` :171; each bound through
    /// `visitTypeParameter` :275-277): `typedef F<T extends _Foo>` reports
    /// at `_Foo`. Upstream Dart SDK source,
    /// `pkg/linter/lib/src/rules/library_private_types_in_public_api.dart`.
    #[test]
    fn typedef_type_parameter_bound_reports() {
        let f = Fixture::parse("typedef F<T extends _Foo> = void Function();\nclass _Foo {}\n");
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let bound = tok(&f.source, &f.tokens, "_Foo", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, bound));
    }

    /// The aliased generic function type's own type-parameter bounds walk
    /// (`visitGenericFunctionType` :170-175 — `node.typeParameters`):
    /// `typedef G = void Function<T extends _Foo>()` reports at `_Foo`.
    /// The nested `FunctionType`'s `TypeVariables` row tiles under the
    /// aliased `Type` row, not under the `Typedef`. Upstream Dart SDK
    /// source, `pkg/linter/lib/src/rules/library_private_types_in_public_api.dart`.
    #[test]
    fn typedef_generic_function_type_parameter_bound_reports() {
        let f = Fixture::parse("typedef G = void Function<T extends _Foo>();\nclass _Foo {}\n");
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let bound = tok(&f.source, &f.tokens, "_Foo", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, bound));
    }

    /// The aliased generic function type's return type and parameter
    /// types walk too (`visitGenericFunctionType` :170-175 —
    /// `node.returnType` / `node.parameters`): `typedef H1 = _Foo
    /// Function()` and `typedef H2 = void Function(_Foo)` each report at
    /// `_Foo`.
    #[test]
    fn typedef_generic_function_type_return_and_parameter_report() {
        let f = Fixture::parse("typedef H1 = _Foo Function();\nclass _Foo {}\n");
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        assert_eq!(lints.len(), 1, "{lints:?}");

        let f = Fixture::parse("typedef H2 = void Function(_Foo);\nclass _Foo {}\n");
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let bound = tok(&f.source, &f.tokens, "_Foo", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, bound));
    }

    /// The same `FunctionType` written-type row in the other declaration
    /// faces (`visitFieldDeclaration` :113-120 / the parameter
    /// declared-type branch :186-207 — each `node.type` accept reaches
    /// `visitGenericFunctionType`): a field `void Function<T extends
    /// _Foo>() cb;` and a parameter `void f(void Function<T extends
    /// _Foo>() cb) {}` each report at the bound.
    #[test]
    fn generic_function_type_faces_report() {
        let f = Fixture::parse("void Function<T extends _Foo>() cb;\nclass _Foo {}\n");
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let bound = tok(&f.source, &f.tokens, "_Foo", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, bound));

        let f = Fixture::parse("void f(void Function<T extends _Foo>() cb) {}\nclass _Foo {}\n");
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        assert_eq!(lints.len(), 1, "{lints:?}");
    }

    /// A prefixed written reference reports over the whole `p._Foo`
    /// spelling (`visitNamedType` :206-212 — the continuation carries the
    /// private name).
    #[test]
    fn prefixed_private_reference_reports() {
        let f = Fixture::parse("import 'p.dart' as p;\n\np._Foo x;\n");
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let p = tok(&f.source, &f.tokens, "p", 1);
        let start = utf16_at(&f, p);
        assert_eq!(lints[0].range, start..start + 6);
    }

    /// The implicit field-formal face (:133-142): `this.x` without a
    /// written type reads the field's written type — a `_`-private field
    /// type reports at the parameter name; a written `_Foo` type reports
    /// through the declared-type branch; only a *named* private formal
    /// skips (:124-126), so positional `this._x` still reports
    /// (`C(this.[!_x!])` in the official corpus), while the private field
    /// declaration itself stays silent (:115-117).
    #[test]
    fn implicit_field_formal_private_type_reports() {
        let f = Fixture::parse("class C {\n  _Foo x;\n  C(this.x);\n}\nclass _Foo {}\n");
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        // The public field's `_Foo` type reports (:113-120), then the
        // formal's implicit type at the name.
        assert_eq!(lints.len(), 2, "{lints:?}");
        let x = tok(&f.source, &f.tokens, "x", 1);
        let start = utf16_at(&f, x);
        assert_eq!(lints[1].range, start..start + 1);

        let f = Fixture::parse("class C {\n  _Foo _x;\n  C(this._x);\n}\nclass _Foo {}\n");
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let x = tok(&f.source, &f.tokens, "_x", 1);
        let start = utf16_at(&f, x);
        assert_eq!(lints[0].range, start..start + 2);

        let f = Fixture::parse("class C {\n  C(_Foo x);\n}\nclass _Foo {}\n");
        assert_eq!(
            f.run(&one_rule("library_private_types_in_public_api"))
                .len(),
            1
        );
    }

    /// The implicit super-formal face (:249-258), the same-file arm:
    /// `super.x` reads the superclass's instance-field type — a
    /// `_`-private field type reports at the parameter name, after the
    /// superclass's own public-field type report (:113-120).
    #[test]
    fn implicit_super_formal_private_type_reports() {
        let f = Fixture::parse(
            "class A {\n  _Foo x;\n}\nclass B extends A {\n  B(super.x);\n}\nclass _Foo {}\n",
        );
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        assert_eq!(lints.len(), 2, "{lints:?}");
        let x = tok(&f.source, &f.tokens, "x", 1);
        let start = utf16_at(&f, x);
        assert_eq!(lints[1].range, start..start + 1);
    }

    /// A record type annotation's field types walk
    /// (`visitRecordTypeAnnotation` — positionalFields + namedFields each
    /// accept'd; upstream Dart SDK source,
    /// `pkg/linter/lib/src/rules/library_private_types_in_public_api.dart`):
    /// `typedef R = (_Foo, int)` reports at `_Foo`, a named-field record
    /// `({int a, _Foo b})` reports at `_Foo`, and a nested record
    /// `((_Foo, int), int)` reports at the inner `_Foo`.
    #[test]
    fn record_type_faces_report() {
        let f = Fixture::parse("typedef R = (_Foo, int);\nclass _Foo {}\n");
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let foo = tok(&f.source, &f.tokens, "_Foo", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, foo));

        let f = Fixture::parse("typedef S = ({int a, _Foo b});\nclass _Foo {}\n");
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let foo = tok(&f.source, &f.tokens, "_Foo", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, foo));

        let f = Fixture::parse("typedef N = ((_Foo, int), int);\nclass _Foo {}\n");
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let foo = tok(&f.source, &f.tokens, "_Foo", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, foo));
    }

    /// The same `RecordType` written-type row in the other declaration
    /// faces: a `(_Foo, int) f()` return type, a `void f((_Foo, int) r)`
    /// parameter, a `(_Foo, int) x;` top-level variable, and a
    /// `List<(_Foo, int)>` type argument each report at `_Foo`.
    #[test]
    fn record_type_declaration_faces_report() {
        let f = Fixture::parse("(_Foo, int) f() => throw 0;\nclass _Foo {}\n");
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let foo = tok(&f.source, &f.tokens, "_Foo", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, foo));

        let f = Fixture::parse("void f((_Foo, int) r) {}\nclass _Foo {}\n");
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let foo = tok(&f.source, &f.tokens, "_Foo", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, foo));

        let f = Fixture::parse("(_Foo, int) x = (throw 0, 0);\nclass _Foo {}\n");
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let foo = tok(&f.source, &f.tokens, "_Foo", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, foo));

        let f = Fixture::parse("List<(_Foo, int)> x = [];\nclass _Foo {}\n");
        let lints = f.run(&one_rule("library_private_types_in_public_api"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let foo = tok(&f.source, &f.tokens, "_Foo", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, foo));
    }

    /// A public record field type stays silent; a private *declaration*
    /// still silences the record face (the private-name guard precedes the
    /// type walk).
    #[test]
    fn record_type_public_fields_pass() {
        let f = Fixture::parse("typedef R = (int, String);\n");
        assert!(
            f.run(&one_rule("library_private_types_in_public_api"))
                .is_empty()
        );

        let f = Fixture::parse("typedef _R = (_Foo, int);\nclass _Foo {}\n");
        assert!(
            f.run(&one_rule("library_private_types_in_public_api"))
                .is_empty()
        );
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
        let f = Fixture::parse("extension type E(int i) {\n  static E make(int i) => E(i);\n}\n");
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
        let f = Fixture::parse("class A {\n  A.named();\n  static A get getter => A.named();\n}\n");
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
        assert!(
            f.run(&one_rule("prefer_constructors_over_static_methods"))
                .is_empty()
        );

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
        let f =
            Fixture::parse("class A {\n  A.named();\n  static A generic<T>() => A.named();\n}\n");
        assert!(
            f.run(&one_rule("prefer_constructors_over_static_methods"))
                .is_empty()
        );

        let f = Fixture::parse("class A {\n  A.named();\n  static A? staticM() => A.named();\n}\n");
        assert!(
            f.run(&one_rule("prefer_constructors_over_static_methods"))
                .is_empty()
        );

        let f = Fixture::parse("class A {\n  static Object staticM() => Object();\n}\n");
        assert!(
            f.run(&one_rule("prefer_constructors_over_static_methods"))
                .is_empty()
        );
    }

    /// `test_staticMethod_returnsSubtype` / `test_extensionMethod` /
    /// `test_factoryConstructor` — a subclass return, an extension static,
    /// and a factory all pass.
    #[test]
    fn constructors_subtype_extension_factory_pass() {
        let f = Fixture::parse("class A {\n  static B f() => B();\n}\nclass B extends A {}\n");
        assert!(
            f.run(&one_rule("prefer_constructors_over_static_methods"))
                .is_empty()
        );

        let f = Fixture::parse(
            "class A {\n  A.named();\n}\nextension E on A {\n  static A foo() => A.named();\n}\n",
        );
        assert!(
            f.run(&one_rule("prefer_constructors_over_static_methods"))
                .is_empty()
        );

        let f = Fixture::parse(
            "class A {\n  A.named();\n  factory A.f() {\n    return A.named();\n  }\n}\n",
        );
        assert!(
            f.run(&one_rule("prefer_constructors_over_static_methods"))
                .is_empty()
        );
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

    /// The `hasInferredType` initializer-type faces (:143-148) — a `final`
    /// public's bare identifier initializer types through the read
    /// target's declared type: a concrete target exempts; a `dynamic`
    /// target reports (the divergence row's inference-bound face — the
    /// official reads `staticType is! DynamicType && !isDartCoreNull`);
    /// a written type keeps the row out; a private name never reports;
    /// an unresolvable target stays exempt (the under-report posture).
    #[test]
    fn annotate_fields_final_initializer_type_faces() {
        // A concrete-typed target: the exemption holds.
        let lints = run_semantic(
            "int someInt = 0;\nclass A {\n  final x = someInt;\n}\n",
            "type_annotate_public_apis",
        );
        assert!(lints.is_empty(), "{lints:?}");

        // A `dynamic`-typed target: the official reports. The anchor is
        // `x` — utf16 43 in the all-ASCII source.
        let lints = run_semantic(
            "dynamic someDynamic = 0;\nclass A {\n  final x = someDynamic;\n}\n",
            "type_annotate_public_apis",
        );
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range.start, 43);

        // A written type keeps the row out even over a `dynamic`
        // initializer.
        let lints = run_semantic(
            "dynamic someDynamic = 0;\nclass A {\n  int x = someDynamic;\n}\n",
            "type_annotate_public_apis",
        );
        assert!(lints.is_empty(), "{lints:?}");

        // A private name never reports, whatever the initializer types.
        let lints = run_semantic(
            "dynamic someDynamic = 0;\nclass A {\n  final _y = someDynamic;\n}\n",
            "type_annotate_public_apis",
        );
        assert!(lints.is_empty(), "{lints:?}");

        // An unresolvable initializer name types no row — the exemption
        // holds (the documented under-report posture).
        let lints = run_semantic(
            "class A {\n  final x = unresolved;\n}\n",
            "type_annotate_public_apis",
        );
        assert!(lints.is_empty(), "{lints:?}");
    }

    /// The send faces of the initializer-type question (:143-148): a
    /// `dynamic`-returning call's static type is `dynamic` — the official
    /// reports; a concrete return type exempts (the fold's send face at
    /// the invocation row).
    #[test]
    fn annotate_fields_final_initializer_send_faces() {
        // `dynamic dynFn()` — `final x = dynFn();` reports.
        let lints = run_semantic(
            "dynamic dynFn() => 0;\nclass A {\n  final x = dynFn();\n}\n",
            "type_annotate_public_apis",
        );
        assert_eq!(lints.len(), 1, "{lints:?}");

        // `int intFn()` — `final x = intFn();` exempts.
        let lints = run_semantic(
            "int intFn() => 0;\nclass A {\n  final x = intFn();\n}\n",
            "type_annotate_public_apis",
        );
        assert!(lints.is_empty(), "{lints:?}");
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

    /// An operator WITH a written return type never reports — the
    /// `OperatorName` row tiles before the declaration row, so the type
    /// read anchors on the row's own children ahead of the parameters
    /// (the family-corpus typed-`==`/typed-`+` faces).
    #[test]
    fn annotate_typed_operators_pass() {
        let f = Fixture::parse(
            "class A {\n  bool operator ==(Object o) => true;\n  B operator +(B other) => this;\n}\n",
        );
        assert!(f.run(&one_rule("type_annotate_public_apis")).is_empty());
    }

    /// A function-typed formal (`void action()`) is no regular formal
    /// upstream — the inline `FunctionTypedFormalParameter` row inside the
    /// parameter row keeps the walk out (the old-style-formal corpus face).
    #[test]
    fn annotate_function_typed_formal_pass() {
        let f = Fixture::parse("void schedule(void action()) {\n  action();\n}\n");
        assert!(f.run(&one_rule("type_annotate_public_apis")).is_empty());
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
    use dartq_ast::{NodeKind, PayloadField};

    use crate::engine::Check;
    use crate::registry::RuleSpec;
    use crate::rules::{class_declaration_name, type_before, type_face};

    use super::member_name_typed;

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
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_constructors_over_static_methods",
    &[NodeKind::Method],
    check,
);

    /// The static `ModSet` bit (bridge `mod_bits`).
    const STATIC_BIT: u16 = 1 << 3;

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `!node.isStatic` bails (:69); factories never carry a static kind.
        // The `static` modifier rides the header flag bits — the payload's
        // `DeclarationKind` word records the enclosing declaration (class,
        // extension, extension type), never the member's own staticity.
        if dartq_ast::mod_flags(c.unit.ast, node) & STATIC_BIT == 0 {
            return;
        }
        // `node.parent?.parent.typeToCheckOrNull()` (:74, :83-95) lands on
        // the enclosing class-like below; only class and extension-type
        // owners qualify. Extension members are `Extension` declarations —
        // the `DeclarationKind` payload word filters them here.
        if !matches!(
            c.field(node, PayloadField::DeclarationKind),
            Some(kind)
                if kind == dartq_parser::DeclarationKind::Class as u32
                    || kind == dartq_parser::DeclarationKind::ExtensionType as u32
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
        // The member name anchors at the parameters node — `member_name`
        // picks the return type's `Identifier` (the wrong head) here.
        let Some(name) = member_name_typed(c, node) else {
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
        // The extension-type name is not a direct Identifier child — it is
        // the declaration's payload word 2 (`begin_extension_type_declaration`
        // patches the name token there; `namePart` upstream).
        let class_name = class_declaration_name(c, owner).or_else(|| {
            if c.kind(owner) != NodeKind::ExtensionTypeDeclaration {
                return None;
            }
            let name = c.field(owner, PayloadField::Name)?;
            (name != crate::rules::NONE_TOKEN).then(|| c.lexeme(name))
        });
        let Some(class_name) = class_name else {
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
                .filter(|_| invocation_shape(c, name + 1))
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

pub(crate) mod strict_top_level_inference {
    use dartq_ast::{NodeKind, PayloadField};

    use crate::engine::Check;
    use crate::registry::RuleSpec;
    use crate::rules::{MemberQuery, get_set_token, initialized_identifier_name, member_annotations, same_file_ancestors, utf16_token_span, variables_declaration_keyword};

    const PROBLEM: &str = "Missing type annotation.";
    const ADD_TYPE_CORRECTION: &str = "Try adding a type annotation.";
    const REPLACE_KEYWORD_CORRECTION: &str = "Try replacing '{0}' with a type annotation.";
    const SPLIT_TO_TYPES_CORRECTION: &str =
        "Try splitting the declaration and specifying the different type annotations.";

    /// The static `ModSet` bit (header flag bits; the
    /// `avoid_renaming_method_parameters` reading).
    const STATIC_BIT: u16 = 1 << 3;

    /// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/strict_top_level_inference.dart`
    /// — a declaration whose types inference would have to fill reports
    /// `Missing type annotation.` in three diagnostic shapes: untyped
    /// function/method/getter return faces, untyped simple formals, and
    /// untyped top-level/field variables without initializers
    /// (`strictTopLevelInferenceAddType`/`ReplaceKeyword`/`SplitToTypes`,
    /// `diagnostic.g.dart`). The `wildcard_variables` feature (:40-42) is a
    /// REPORT-time bit, not a registration gate — a lone `_` formal passes
    /// only with it on — so the rule registers unconditionally.
    ///
    /// Faces (the registered visitors, :52-135): top-level functions
    /// (accessors included, setters only through their formal), methods
    /// (the getter/setter/method split :201-241), constructors
    /// (formals only), and top-level/field variable lists (:97-135).
    ///
    /// Approximations (documented): the override face
    /// (`overriddenMember`, :122/:131/:189/:216) is the same-file ancestor
    /// chain, abstract members included — cross-file supertypes
    /// under-report. `isDeclaringFieldTypeInferred` lands as the
    /// receiver-formal face: a `this.`/`super.` formal without a written
    /// type skips (its type arrives from the field/super parameter), with
    /// a written one stops the list. `element.typeInferenceError`
    /// (:214-219) is dropped (an error-code face only). Primary
    /// constructors (:92-95) stay unported (the experiment is unshipped;
    /// the syntax is a compile error at every shipped version).
    /// `container.isReflectiveTest` (:251) lands as any `@reflectiveTest`
    /// annotation on the enclosing class-like. The verbatim abort
    /// polarity ports: a function-typed formal (:176-186) or a formal
    /// with a written type (:183) stops the whole parameter list.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "strict_top_level_inference",
    &[
            NodeKind::TopLevelMethod,
            NodeKind::Method,
            NodeKind::Constructor,
            NodeKind::Factory,
            NodeKind::VariablesDeclaration,
        ],
    check,
);

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        match c.kind(node) {
            // `visitFunctionDeclaration` (:63-75): the parent-is-unit gate
            // is the dispatch kind (top-level only).
            NodeKind::TopLevelMethod => top_level(c, node),
            // `visitMethodDeclaration` (:77-90): the accessor/method split.
            NodeKind::Method => method(c, node),
            // `visitConstructorDeclaration` (:56-61): formals only,
            // never an override face.
            NodeKind::Constructor | NodeKind::Factory => check_formals(c, node, None),
            // `visitVariableDeclarationList` (:97-135).
            NodeKind::VariablesDeclaration => variable_list(c, node),
            _ => {}
        }
    }

    /// The top-level face: an untyped non-setter name reports, then the
    /// formals (no override face at the top level).
    fn top_level(c: &mut Check<'_>, node: u32) {
        if get_set_token(c, node) != Some("set")
            && let Some((name, untyped)) = untyped_name(c, node)
            && untyped
        {
            report(c, name, None);
        }
        check_formals(c, node, None);
    }

    /// The method face (:201-241): the getter/setter/method split.
    fn method(c: &mut Check<'_>, node: u32) {
        let Some((name, untyped)) = untyped_name(c, node) else {
            return;
        };
        let name_lexeme = c.lexeme(name);
        let is_static = dartq_ast::mod_flags(c.unit.ast, node) & STATIC_BIT != 0;
        let container = container_of(c);
        match get_set_token(c, node) {
            // `_checkGetter` (:201-208): an untyped getter reports unless
            // it overrides (a static or extension accessor never does).
            Some("get") => {
                if untyped
                    && overridden(
                        c,
                        is_static,
                        container,
                        name_lexeme,
                        &[MemberQuery::Getter, MemberQuery::Variable],
                    )
                    .is_none()
                {
                    report(c, name, None);
                }
                return;
            }
            // `_checkSetter` (:232-241).
            Some("set") => {
                check_setter(c, node, name, is_static, container);
                return;
            }
            _ => {}
        }
        // `_checkMethod` (:210-230).
        let no_override = is_static
            || container.is_some_and(|row| {
                matches!(
                    c.kind(row),
                    NodeKind::ExtensionDeclaration | NodeKind::ExtensionTypeDeclaration
                )
            });
        let overridden = if no_override {
            None
        } else {
            overridden(c, false, container, name_lexeme, &[MemberQuery::Method])
        };
        if untyped
            && name_lexeme != "[]="
            && (no_override
                || (overridden.is_none() && !reflective_exempt(c, container, name_lexeme)))
        {
            report(c, name, None);
        }
        check_formals(c, node, overridden.as_deref());
    }

    /// `_checkSetter` (:232-241): the first formal decides — a
    /// function-typed or written-typed formal skips, a non-override
    /// setter name reports (always the add-type correction).
    fn check_setter(
        c: &mut Check<'_>,
        node: u32,
        name: u32,
        is_static: bool,
        container: Option<u32>,
    ) {
        let Some(first) = formals(c, node).into_iter().next() else {
            return;
        };
        if c.kind(first) != NodeKind::FormalParameter || written_type(c, first).is_some() {
            return;
        }
        let name_lexeme = c.lexeme(name);
        if overridden(c, is_static, container, name_lexeme, &[MemberQuery::Setter]).is_none() {
            report(c, name, None);
        }
    }

    /// `visitVariableDeclarationList` (:97-135): an untyped top-level/field
    /// list with initializer-less variables reports — a one-variable list
    /// carries the keyword-named face (:119-127), a longer one the split
    /// face (:129-134); an overridden variable skips.
    fn variable_list(c: &mut Check<'_>, node: u32) {
        // `node.type != null` (:99): the written type is the preceding
        // sibling under `Fields`/`TopLevelFields` (the bridge anchors it
        // beside the declaration, not inside).
        match super::type_sibling_before(c, node).map(|prev| c.kind(prev)) {
            Some(NodeKind::NoType) => {}
            _ => return,
        }
        let Some(parent) = c.ancestor(1) else {
            return;
        };
        let is_field = c.kind(parent) == NodeKind::Fields;
        if !is_field && c.kind(parent) != NodeKind::TopLevelFields {
            return; // the top-level/field gate (:100-102)
        }
        let missing: Vec<u32> = c
            .direct_children(node)
            .iter()
            .copied()
            .filter(|&var| c.kind(var) == NodeKind::InitializedIdentifier)
            .filter(|&var| {
                !c.children(var)
                    .any(|kid| c.kind(kid) == NodeKind::VariableInitializer)
            })
            .collect();
        if missing.is_empty() {
            return;
        }
        let keyword = variables_declaration_keyword(c, node);
        // The override lookup answers only for instance fields (a static
        // or top-level variable never overrides).
        let overridable = is_field
            && c.field(parent, PayloadField::MemberKind)
                == Some(dartq_parser::MemberKind::NonStaticField as u32);
        let container = container_of(c);
        let variables: Vec<u32> = c
            .direct_children(node)
            .iter()
            .copied()
            .filter(|&var| c.kind(var) == NodeKind::InitializedIdentifier)
            .collect();
        if variables.len() == 1 {
            let Some(name) = initialized_identifier_name(c, missing[0]) else {
                return;
            };
            if overridable
                && overridden(
                    c,
                    false,
                    container,
                    c.lexeme(name),
                    &[MemberQuery::Getter, MemberQuery::Variable],
                )
                .is_some()
            {
                return;
            }
            report(c, name, keyword);
        } else {
            for var in missing {
                let Some(name) = initialized_identifier_name(c, var) else {
                    continue;
                };
                if overridable
                    && overridden(
                        c,
                        false,
                        container,
                        c.lexeme(name),
                        &[MemberQuery::Getter, MemberQuery::Variable],
                    )
                    .is_some()
                {
                    continue;
                }
                let range = utf16_token_span(c, name);
                c.report_texts(range, PROBLEM, SPLIT_TO_TYPES_CORRECTION, &[]);
            }
        }
    }

    /// `_checkFormalParameters` (:166-199): the formals in source order;
    /// the abort faces port verbatim — a function-typed formal (:176-186)
    /// or a formal with a written type (:183) stops the whole list, a
    /// receiver formal without one skips (:184).
    fn check_formals(c: &mut Check<'_>, member: u32, overridden: Option<&[bool]>) {
        for (index, row) in formals(c, member).into_iter().enumerate() {
            match c.kind(row) {
                NodeKind::FunctionTypedFormalParameter => return,
                NodeKind::FormalParameter if !check_formal(c, row, index, overridden) => {
                    return;
                }
                _ => {}
            }
        }
    }

    /// One formal. `false` aborts the caller's list.
    fn check_formal(
        c: &mut Check<'_>,
        param: u32,
        index: usize,
        overridden: Option<&[bool]>,
    ) -> bool {
        // `parameter is! RegularFormalParameterImpl ||
        // parameter.functionTypedSuffix != null` (:172-182): a function-typed
        // suffix inside the parameter row has its function type one way or
        // another and stops the whole list (the walk only ever sees the
        // wrapper row).
        if c.collect_direct(param)
            .iter()
            .any(|&kid| c.kind(kid) == NodeKind::FunctionTypedFormalParameter)
        {
            return false;
        }
        let receiver = c
            .field(param, PayloadField::Receiver)
            .unwrap_or(crate::rules::NONE_TOKEN);
        let Some(name) = c.field(param, PayloadField::Name) else {
            return true;
        };
        if name == crate::rules::NONE_TOKEN {
            return true; // `parameterName == null` (:170-171)
        }
        // `isWildcardIdentifier` (:40-42): the report-time feature bit.
        if c.lexeme(name) == "_" && c.unit.features.wildcard_variables {
            return true;
        }
        let written = written_type(c, param);
        if receiver != crate::rules::NONE_TOKEN {
            // A receiver formal's type arrives from the field/super
            // parameter (:184); a written one stops the list (:183).
            return written.is_none();
        }
        if written.is_some() {
            return false; // `parameter.type != null` (:183)
        }
        // The override correspondence (:187-198).
        let needed = match overridden {
            None => true,
            Some(list) => {
                if named_formal(c, param) {
                    !list.iter().any(|&over| over)
                } else {
                    list.get(index).is_none_or(|&over| over)
                }
            }
        };
        if needed {
            report(c, name, param_keyword(c, param));
        }
        true
    }

    /// The formal's `final`/`var` modifier (`constFinalOrVarKeyword`,
    /// :190/:194): the only legal occurrences of the lexemes inside a
    /// formal's span — reserved words never name, and a nested
    /// function-typed formal already stopped the list.
    fn param_keyword(c: &Check<'_>, param: u32) -> Option<u32> {
        c.unit
            .ast
            .token_range(param)
            .into_iter()
            .find(|&t| matches!(c.lexeme(t), "final" | "var"))
    }

    /// The formal's written type child (`NoType` first child = unwritten;
    /// the `type_init_formals` written set). `VoidType` counts — a written
    /// `void` is a written type (`parameter.type != null`, :183).
    fn written_type(c: &Check<'_>, param: u32) -> Option<u32> {
        c.collect_direct(param).into_iter().find(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::Type | NodeKind::VoidType | NodeKind::FunctionType | NodeKind::RecordType
            )
        })
    }

    /// The member's formal-parameter rows in source order — the required
    /// group, then the `[]`/`{}` section (`node.parameters.parameters`).
    fn formals(c: &Check<'_>, member: u32) -> Vec<u32> {
        let Some(params) = c
            .collect_direct(member)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for section in c.collect_direct(params) {
            match c.kind(section) {
                NodeKind::FormalParameter | NodeKind::FunctionTypedFormalParameter => {
                    out.push(section);
                }
                NodeKind::OptionalFormalParameters => {
                    out.extend(c.collect_direct(section).into_iter().filter(|&row| {
                        matches!(
                            c.kind(row),
                            NodeKind::FormalParameter | NodeKind::FunctionTypedFormalParameter
                        )
                    }));
                }
                _ => {}
            }
        }
        out
    }

    /// The `isNamed` face of a formal row (the `FormalParameterKind`
    /// word); a function-typed row answers positional (exotic shape).
    fn named_formal(c: &Check<'_>, row: u32) -> bool {
        let kind = c
            .field(row, PayloadField::FormalParameterKind)
            .unwrap_or(u32::MAX);
        kind == dartq_parser::FormalParameterKind::OptionalNamed as u32
            || kind == dartq_parser::FormalParameterKind::RequiredNamed as u32
    }

    /// `_report` (:289-302): a `var` keyword names the replacement, a
    /// `const` keyword falls through both branches (invalid code),
    /// everything else carries the plain add-type correction.
    fn report(c: &mut Check<'_>, name: u32, keyword: Option<u32>) {
        let range = utf16_token_span(c, name);
        match keyword.map(|k| c.lexeme(k)) {
            Some("var") => c.report_texts(range, PROBLEM, REPLACE_KEYWORD_CORRECTION, &["var"]),
            Some("const") => {}
            _ => c.report_texts(range, PROBLEM, ADD_TYPE_CORRECTION, &[]),
        }
    }

    /// The declaration's `(name token, untyped)` pair: the name is the last
    /// `Identifier` *ahead of the parameter row* (a written return type
    /// tiles its own identifier/type rows earlier; expression-body
    /// identifiers tile after the parameters and must not name the member),
    /// operators name through the left-sibling [`NodeKind::OperatorName`]
    /// row. `untyped` reads the rows before the parameters (`NoType` =
    /// absent).
    fn untyped_name(c: &Check<'_>, node: u32) -> Option<(u32, bool)> {
        let children = c.collect_direct(node);
        // The written return type: any type-family row before the parameter
        // list (`NoType` = absent — the scan reads
        // the same face for named methods and operators, whose name rides a
        // left sibling).
        let params = children
            .iter()
            .position(|&kid| {
                matches!(
                    c.kind(kid),
                    NodeKind::FormalParameters | NodeKind::NoFormalParameters
                )
            })
            .unwrap_or(children.len());
        let untyped = !children[..params].iter().any(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::Type | NodeKind::VoidType | NodeKind::FunctionType | NodeKind::RecordType
            )
        });
        if let Some(name) = children[..params]
            .iter()
            .rev()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::Identifier)
        {
            return Some((c.head_token(name), untyped));
        }
        let operator = operator_name_row(c, node)?;
        let name = c.field(operator, PayloadField::Name)?;
        Some((name, untyped))
    }

    /// The `OperatorName` left sibling of a method row (the production
    /// wrapper shape — the operator name is never a child row).
    fn operator_name_row(c: &Check<'_>, node: u32) -> Option<u32> {
        let parent = c.unit.ast.parent(node)?;
        let siblings = c.collect_direct(parent);
        let pos = siblings.iter().position(|&kid| kid == node)?;
        siblings[..pos]
            .iter()
            .rev()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::OperatorName)
    }

    /// The nearest class-like/extension-like container (the official
    /// `fragment.enclosingFragment.element`, :216).
    fn container_of(c: &Check<'_>) -> Option<u32> {
        c.path.iter().rev().skip(1).copied().find(|&ancestor| {
            matches!(
                c.kind(ancestor),
                NodeKind::ClassDeclaration
                    | NodeKind::MixinDeclaration
                    | NodeKind::EnumDeclaration
                    | NodeKind::ExtensionDeclaration
                    | NodeKind::ExtensionTypeDeclaration
            )
        })
    }

    /// The overridden-member face (`overriddenMember != null`): the
    /// same-file ancestor chain, any-member walk (abstract declarations
    /// count), then the implicit-`Object` root — `getInheritedMember` walks
    /// every class-like chain to `Object`, so the five root instance
    /// members answer by name + namespace alone. A static member or an
    /// extension/extension-type container never overrides (`_isOverride`,
    /// :243-250). The answer carries the overridden member's formal
    /// named-flags (the method face's correspondence list); the root
    /// members' formals are all positional and typed.
    fn overridden(
        c: &Check<'_>,
        is_static: bool,
        container: Option<u32>,
        name: &str,
        queries: &[MemberQuery],
    ) -> Option<Vec<bool>> {
        if is_static {
            return None;
        }
        let container = container?;
        if matches!(
            c.kind(container),
            NodeKind::ExtensionDeclaration | NodeKind::ExtensionTypeDeclaration
        ) {
            return None;
        }
        let row = same_file_ancestors(c, container, false)
            .into_iter()
            .find_map(|ancestor| {
                crate::rules::body_members(c, ancestor)
                    .into_iter()
                    .find(|&member| {
                        queries.iter().any(|&query| {
                            !crate::rules::member_matches(c, member, query, name).is_empty()
                        })
                    })
            });
        if let Some(row) = row {
            return Some(
                formals(c, row)
                    .into_iter()
                    .map(|formal| named_formal(c, formal))
                    .collect(),
            );
        }
        object_member_flags(queries, name)
    }

    /// The `Object` root arm: `==`/`toString`/`noSuchMethod` in the method
    /// namespace, `hashCode`/`runtimeType` in the getter namespace (Object
    /// declares no setters or variables). The flags list the root formals —
    /// one positional typed parameter on `==`/`noSuchMethod`, none on the
    /// getters or `toString`.
    fn object_member_flags(queries: &[MemberQuery], name: &str) -> Option<Vec<bool>> {
        let method = matches!(name, "==" | "toString" | "noSuchMethod");
        let getter = matches!(name, "hashCode" | "runtimeType");
        queries.iter().find_map(|&query| match query {
            MemberQuery::Method if method => Some(vec![false]),
            MemberQuery::Getter if getter => Some(vec![]),
            _ => None,
        })
    }

    /// The reflective-test exemption (:251): a `test_*`/`solo_test_*`
    /// method of a class-like carrying an `@reflectiveTest` annotation
    /// skips the return-type report.
    fn reflective_exempt(c: &Check<'_>, container: Option<u32>, name: &str) -> bool {
        if !(name.starts_with("test_") || name.starts_with("solo_test_")) {
            return false;
        }
        let Some(container) = container else {
            return false;
        };
        // The `@reflectiveTest` metadata precedes the top-level
        // `TopLevelDeclaration` *wrapper* at unit level — the member's own
        // annotation read (`member_annotations` on the wrapper) does not
        // reach it.
        let Some(wrapper) = c.unit.ast.parent(container) else {
            return false;
        };
        let Some(unit) = c.unit.ast.parent(wrapper) else {
            return false;
        };
        member_annotations(c, unit, wrapper).iter().any(|&meta| {
            c.collect_direct(meta).into_iter().any(|piece| {
                c.kind(piece) == NodeKind::Identifier
                    && c.lexeme(c.head_token(piece)) == "reflectiveTest"
            })
        })
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

        /// `test/rules/strict_top_level_inference_test.dart`
        /// `test_constructorParameter_positional` — an untyped constructor
        /// formal reports at its name.
        #[test]
        fn reports_an_untyped_constructor_parameter() {
            let f = Fixture::parse("class C {\n  C(p1);\n}\n");
            let lints = f.run(&one_rule("strict_top_level_inference"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(lints[0].message, "Missing type annotation.");
            assert_eq!(
                lints[0].correction.as_deref(),
                Some("Try adding a type annotation.")
            );
            let name = tok(&f.source, &f.tokens, "p1", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, name));
        }

        /// `test_constructorParameter_named_final` /
        /// `test_constructorParameter_named_var` — the keyword decides the
        /// correction: `final` carries the plain face, `var` names itself
        /// in the replace-keyword face.
        #[test]
        fn constructor_keyword_correction_faces() {
            let f = Fixture::parse("class C {\n  C({final p1});\n}\n");
            let lints = f.run(&one_rule("strict_top_level_inference"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].correction.as_deref(),
                Some("Try adding a type annotation.")
            );
            let name = tok(&f.source, &f.tokens, "p1", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, name));

            let f = Fixture::parse("class C {\n  C({var p1});\n}\n");
            let lints = f.run(&one_rule("strict_top_level_inference"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].correction.as_deref(),
                Some("Try replacing 'var' with a type annotation.")
            );
        }

        /// `test_constructorParameter_named_typed` /
        /// `test_constructorParameter_positional_initializingFormal` /
        /// `test_constructorParameter_positional_superParameter` — typed,
        /// initializing, and super formals pass.
        #[test]
        fn constructor_typed_and_receiver_formals_pass() {
            let f = Fixture::parse("class C {\n  C({int? p1});\n}\n");
            assert!(f.run(&one_rule("strict_top_level_inference")).is_empty());

            let f = Fixture::parse("class C {\n  int? p1;\n  C({this.p1});\n}\n");
            assert!(f.run(&one_rule("strict_top_level_inference")).is_empty());

            let f = Fixture::parse(
                "class S {\n  S(int p1);\n}\nclass C extends S {\n  C(super.p1);\n}\n",
            );
            assert!(f.run(&one_rule("strict_top_level_inference")).is_empty());
        }

        /// `test_wildcardVariable_constructorParameter` — a lone `_` formal
        /// passes with the wildcards feature on (the shipped default).
        #[test]
        fn wildcard_formal_passes() {
            let f = Fixture::parse("class C {\n  C(_) {}\n}\n");
            assert!(f.run(&one_rule("strict_top_level_inference")).is_empty());
        }

        /// `test_wildcardVariable_constructorParameter_preWildcards` — the
        /// upstream `// @dart = 3.4` face: with the feature off the `_`
        /// formal reports.
        #[test]
        fn wildcard_formal_reports_pre_wildcards() {
            let mut f = Fixture::parse("class C {\n  C(_) {}\n}\n");
            f.features.wildcard_variables = false;
            let lints = f.run(&one_rule("strict_top_level_inference"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            let name = tok(&f.source, &f.tokens, "_", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, name));
        }

        /// The verbatim `parameter.type != null` abort (:183): a written
        /// type stops the whole list, so a later untyped formal stays
        /// silent while an earlier one still reports.
        #[test]
        fn the_typed_formal_aborts_the_list() {
            let f = Fixture::parse("void m(int a, b) {}\n");
            assert!(f.run(&one_rule("strict_top_level_inference")).is_empty());

            let f = Fixture::parse("void m(p1, int p2) {}\n");
            let lints = f.run(&one_rule("strict_top_level_inference"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            let name = tok(&f.source, &f.tokens, "p1", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, name));
        }

        /// `test_topLevelVariable_var_noInitializer` /
        /// `test_instanceField_var_noInitializer` /
        /// `test_instanceField_final_noInitializer` /
        /// `test_staticField_var_noInitializer` — the initializer-less
        /// variable faces: `var` names itself, `final` carries the plain
        /// face, at the variable name.
        #[test]
        fn variable_keyword_faces() {
            let f = Fixture::parse("var f;\n");
            let lints = f.run(&one_rule("strict_top_level_inference"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].correction.as_deref(),
                Some("Try replacing 'var' with a type annotation.")
            );
            let name = tok(&f.source, &f.tokens, "f", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, name));

            let f = Fixture::parse("class C {\n  var x;\n}\n");
            assert_eq!(f.run(&one_rule("strict_top_level_inference")).len(), 1);

            let f = Fixture::parse("class C {\n  final f;\n  C(this.f);\n}\n");
            let lints = f.run(&one_rule("strict_top_level_inference"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].correction.as_deref(),
                Some("Try adding a type annotation.")
            );

            let f = Fixture::parse("class C {\n  static var f;\n}\n");
            assert_eq!(f.run(&one_rule("strict_top_level_inference")).len(), 1);
        }

        /// `test_topLevelVariable_var` / `test_instanceField_typed` — an
        /// initializer or a written type passes.
        #[test]
        fn initialized_and_typed_variables_pass() {
            let f = Fixture::parse("final f = 0;\nvar g = 0;\n");
            assert!(f.run(&one_rule("strict_top_level_inference")).is_empty());

            let f = Fixture::parse("class C {\n  int? x;\n}\n");
            assert!(f.run(&one_rule("strict_top_level_inference")).is_empty());
        }

        /// `test_topLevelVariable_multiple_someMissingInitializer` /
        /// `test_instanceField_multiple_someMissingInitializer` — a longer
        /// list carries the split face at each initializer-less variable.
        #[test]
        fn split_faces() {
            let f = Fixture::parse("var x = '', y = '', z;\n");
            let lints = f.run(&one_rule("strict_top_level_inference"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].correction.as_deref(),
                Some(
                    "Try splitting the declaration and specifying the different type annotations."
                )
            );
            let name = tok(&f.source, &f.tokens, "z", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, name));

            let f = Fixture::parse("class C {\n  var x = '', y = '', z;\n}\n");
            assert_eq!(f.run(&one_rule("strict_top_level_inference")).len(), 1);
        }

        /// `test_instanceField_multiple_someMissingInitializer_butOverride`
        /// — an overridden variable skips (the field face reads the
        /// same-file ancestors).
        #[test]
        fn an_overridden_variable_passes() {
            let f = Fixture::parse(
                "class C {\n  int? x = 1;\n}\nclass D implements C {\n  var x, y = '';\n}\n",
            );
            assert!(f.run(&one_rule("strict_top_level_inference")).is_empty());
        }

        /// `test_instanceGetter` / `test_topLevelGetter` /
        /// `test_instanceGetter_inExtension` — untyped getters report at
        /// the name, extensions included.
        #[test]
        fn getter_faces() {
            let f = Fixture::parse("class C {\n  get g => 1;\n}\n");
            let lints = f.run(&one_rule("strict_top_level_inference"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            let name = tok(&f.source, &f.tokens, "g", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, name));

            let f = Fixture::parse("get g => 1;\n");
            assert_eq!(f.run(&one_rule("strict_top_level_inference")).len(), 1);

            let f = Fixture::parse("extension E on int {\n  get g => 1;\n}\n");
            assert_eq!(f.run(&one_rule("strict_top_level_inference")).len(), 1);

            let f = Fixture::parse("class C {\n  int get g => 1;\n}\n");
            assert!(f.run(&one_rule("strict_top_level_inference")).is_empty());
        }

        /// `test_instanceMethod_returnType` /
        /// `test_staticMethod_returnType` /
        /// `test_instanceMethod_returnType_onExtension` /
        /// `test_instanceMethod_returnType_onExtensionType` /
        /// `test_instanceMethod_returnType_override` — the untyped
        /// return-type faces; an overridden method passes.
        #[test]
        fn method_return_type_faces() {
            let f = Fixture::parse("class C {\n  m() {}\n}\n");
            let lints = f.run(&one_rule("strict_top_level_inference"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            let name = tok(&f.source, &f.tokens, "m", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, name));

            let f = Fixture::parse("class C {\n  static m() {}\n}\n");
            assert_eq!(f.run(&one_rule("strict_top_level_inference")).len(), 1);

            let f = Fixture::parse("extension E on int {\n  m() {}\n}\n");
            assert_eq!(f.run(&one_rule("strict_top_level_inference")).len(), 1);

            let f = Fixture::parse("extension type ET(int it) {\n  m() {}\n}\n");
            assert_eq!(f.run(&one_rule("strict_top_level_inference")).len(), 1);

            let f = Fixture::parse(
                "abstract class I {\n  int m();\n}\nabstract class C implements I {\n  m();\n}\n",
            );
            assert!(f.run(&one_rule("strict_top_level_inference")).is_empty());
        }

        /// `test_instanceOperator_binary_returnType` /
        /// `test_instanceOperator_indexAssignment_returnType` — an untyped
        /// operator reports at the operator token; `[]=` never does.
        #[test]
        fn operator_return_type_faces() {
            let f = Fixture::parse("class C {\n  operator +(int p1) {}\n}\n");
            let lints = f.run(&one_rule("strict_top_level_inference"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            let name = tok(&f.source, &f.tokens, "+", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, name));

            let f = Fixture::parse("class C {\n  operator []=(int i, C c) {}\n}\n");
            assert!(f.run(&one_rule("strict_top_level_inference")).is_empty());
        }

        /// The `Object` root of the overridden-member chain: an untyped
        /// `==` overrides the root member (`getInheritedMember` walks every
        /// class-like chain to `Object`) and stays silent — block or
        /// expression body alike; so does an untyped `hashCode` getter.
        #[test]
        fn untyped_object_member_overrides_pass() {
            let f = Fixture::parse(
                "class C {\n  operator ==(Object? other) => other != null && other is C;\n}\n",
            );
            assert!(f.run(&one_rule("strict_top_level_inference")).is_empty());

            let f = Fixture::parse(
                "class C {\n  operator ==(Object? other) { return other != null; }\n}\n",
            );
            assert!(f.run(&one_rule("strict_top_level_inference")).is_empty());

            let f = Fixture::parse("class C {\n  get hashCode => 1;\n}\n");
            assert!(f.run(&one_rule("strict_top_level_inference")).is_empty());
        }

        /// Expression-body identifiers tile after the parameters and never
        /// steal the name read: an untyped method names at its declaration.
        #[test]
        fn expression_body_names_at_the_declaration() {
            let f = Fixture::parse("class C {\n  m(int other) => other;\n}\n");
            let lints = f.run(&one_rule("strict_top_level_inference"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            let name = tok(&f.source, &f.tokens, "m", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, name));

            let f = Fixture::parse("class C {\n  get g => g + 1;\n}\n");
            let lints = f.run(&one_rule("strict_top_level_inference"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            let name = tok(&f.source, &f.tokens, "g", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, name));
        }

        /// The function-typed-formal abort (:172-182): `void action()` has
        /// its function type one way or another and stops the whole list —
        /// a later untyped formal never reports.
        #[test]
        fn function_typed_formal_aborts_the_list() {
            let f = Fixture::parse("void schedule(void action()) {\n  action();\n}\n");
            assert!(f.run(&one_rule("strict_top_level_inference")).is_empty());

            let f = Fixture::parse("void m(void f(), b) {}\n");
            assert!(f.run(&one_rule("strict_top_level_inference")).is_empty());
        }

        /// `test_instanceMethod_parameter_named` /
        /// `test_instanceMethod_parameter_positional` /
        /// `test_instanceMethod_parameter_positional_typed` — the untyped
        /// simple-formal faces on methods.
        #[test]
        fn method_parameter_faces() {
            let f = Fixture::parse("class C {\n  void m({p1}) {}\n}\n");
            let lints = f.run(&one_rule("strict_top_level_inference"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            let name = tok(&f.source, &f.tokens, "p1", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, name));

            let f = Fixture::parse("class C {\n  void m(p1) {}\n}\n");
            assert_eq!(f.run(&one_rule("strict_top_level_inference")).len(), 1);

            let f = Fixture::parse("class C {\n  void m(int p1) {}\n}\n");
            assert!(f.run(&one_rule("strict_top_level_inference")).is_empty());
        }

        /// `test_instanceMethod_parameter_override_partOfSuperSignature` /
        /// `test_instanceMethod_parameter_override_additionalNamed` — an
        /// overriding formal passes in correspondence; a named formal
        /// without an inherited counterpart reports.
        #[test]
        fn method_parameter_override_faces() {
            let f = Fixture::parse(
                "class C {\n  void m(int p1) {}\n}\nclass D implements C {\n  void m(p1) {}\n}\n",
            );
            assert!(f.run(&one_rule("strict_top_level_inference")).is_empty());

            let f = Fixture::parse(
                "class C {\n  void m(int p1) {}\n}\nclass D implements C {\n  void m(p1, {p2}) {}\n}\n",
            );
            let lints = f.run(&one_rule("strict_top_level_inference"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            let name = tok(&f.source, &f.tokens, "p2", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, name));
        }

        /// `test_instanceSetter_parameterType` /
        /// `test_topLevelSetter_parameterType` /
        /// `test_instanceSetter_returnType` — the setter faces: an
        /// instance setter reports at its name, a top-level setter through
        /// its formal, a typed setter passes.
        #[test]
        fn setter_faces() {
            let f = Fixture::parse("class C {\n  set s(value) {}\n}\n");
            let lints = f.run(&one_rule("strict_top_level_inference"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            let name = tok(&f.source, &f.tokens, "s", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, name));

            let f = Fixture::parse("set s(value) {}\n");
            let lints = f.run(&one_rule("strict_top_level_inference"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            let formal = tok(&f.source, &f.tokens, "value", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, formal));

            let f = Fixture::parse("class C {\n  set s(int value) {}\n}\n");
            assert!(f.run(&one_rule("strict_top_level_inference")).is_empty());
        }

        /// `test_topLevelFunction_returnType` — the untyped top-level
        /// function reports at the name.
        #[test]
        fn reports_an_untyped_top_level_function() {
            let f = Fixture::parse("m() {}\n");
            let lints = f.run(&one_rule("strict_top_level_inference"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(lints[0].range.start, 0);
        }

        /// `test_localFunction` / `test_localVariable` — declarations
        /// under a function body never report.
        #[test]
        fn local_declarations_pass() {
            let f = Fixture::parse("void f() {\n  m(p1) {}\n}\n");
            assert!(f.run(&one_rule("strict_top_level_inference")).is_empty());

            let f = Fixture::parse("void f() {\n  var x;\n}\n");
            assert!(f.run(&one_rule("strict_top_level_inference")).is_empty());
        }

        /// `test_reflectiveTest_nonTest` / `test_reflectiveTest_test` /
        /// `test_reflectiveTest_soloTest` — the `test_*`/`solo_test_*`
        /// methods of an `@reflectiveTest` class pass; other members
        /// report.
        #[test]
        fn reflective_test_faces() {
            let base = "import 'package:test_reflective_loader/test_reflective_loader.dart';\n\n@reflectiveTest\nclass ReflectiveTest {\n";
            let f = Fixture::parse(&format!("{base}  foo() {{}}\n}}\n"));
            let lints = f.run(&one_rule("strict_top_level_inference"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            let name = tok(&f.source, &f.tokens, "foo", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, name));

            let f = Fixture::parse(&format!("{base}  test_foo() {{}}\n}}\n"));
            assert!(f.run(&one_rule("strict_top_level_inference")).is_empty());

            let f = Fixture::parse(&format!("{base}  solo_test_foo() {{}}\n}}\n"));
            assert!(f.run(&one_rule("strict_top_level_inference")).is_empty());
        }
    }
}

pub(crate) mod type_annotate_public_apis {
    use dartq_ast::{NodeKind, PayloadField};
    use dartq_semantic::model::Ty;

    use crate::engine::Check;
    use crate::registry::RuleSpec;
    use crate::rules::expressions::{query_site, static_type_faces as faces};
    use crate::rules::{first_child, get_set_token, initialized_identifier_name, is_just_underscores, type_before, variables_declaration_keyword};

    use super::member_name_typed;

    /// The `augment` `ModSet` bit (bridge `mod_bits`).
    const AUGMENT_BIT: u16 = 1 << 1;

    /// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/type_annotate_public_apis.dart`
    /// — missing written types on public API declarations: a public
    /// method/getter (not a setter, not `[]=`) or top-level function without
    /// a return type reports at its name (:71-84, :98-109); a public field or
    /// top-level variable with no written type reports per public variable
    /// name, exempting `const`, and `final` whose initializer types to
    /// neither `dynamic` nor `Null` (:62-67, :143-148, :161-167); a public
    /// legacy typedef without a return type
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
    /// `hasInferredType` initializer face (:143-148) reads the
    /// initializer's static type — the `null` literal is the
    /// `isDartCoreNull` face, a bare identifier initializer answers
    /// through the reference-resolution face (`Analysis::callee_type`,
    /// the read target's declared type), and the remaining send shapes
    /// answer through the fold's send face — a `dynamic`-typed (or
    /// implicitly dynamic) target reports, a concrete one exempts. Shapes
    /// the M1 fold cannot type — literals, unresolved names, `Unknown`
    /// declared types — stay exempt (the under-report posture), and a
    /// bare tear-off of a dynamic-returning function over-reports (the
    /// tear-off's type is the function type; the declared-type read
    /// answers the return type). The generalized typedef (the `=` payload
    /// word) is not a `FunctionTypeAlias` and never enters the face.
    /// Message: `diagnostic.g.dart` `typeAnnotatePublicApis`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "type_annotate_public_apis",
    &[
            NodeKind::TopLevelFields,
            NodeKind::Fields,
            NodeKind::TopLevelMethod,
            NodeKind::Method,
            NodeKind::Typedef,
            NodeKind::Constructor,
            NodeKind::PrimaryConstructor,
        ],
    check,
);

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
                if is_final && has_inferred_type(c, var) {
                    continue;
                }
                let range = crate::rules::utf16_token_span(c, c.head_token(var));
                c.report(range, &[]);
            }
        }
    }

    /// `hasInferredType` (:143-148): an initializer whose static type is
    /// neither `dynamic` nor `Null`. The M1 reads: the `null` literal is
    /// the `isDartCoreNull` face (the fold carries no row for the
    /// literal), and a bare identifier initializer answers through the
    /// reference-resolution face — `Analysis::callee_type`, the read
    /// target's declared type — so a `dynamic`-typed (or implicitly
    /// dynamic) target reports. The remaining send shapes answer through
    /// the same fold's send face (`foo()`'s declared return type, a
    /// member chain's tail type), so a `dynamic`-returning call reports
    /// like the official. The shapes the M1 fold cannot type stay
    /// exempt: the under-report posture, matching the engine's
    /// every-check-suppresses `Unknown` rule.
    fn has_inferred_type(c: &Check<'_>, var: u32) -> bool {
        let Some(initializer) = c
            .collect_direct(var)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer)
        else {
            return false;
        };
        let Some(value) = first_child(c, initializer) else {
            return false;
        };
        if c.kind(value) == NodeKind::LiteralNull {
            return false;
        }
        let Some(analysis) = c.semantics() else {
            return true;
        };
        if c.kind(value) == NodeKind::Identifier
            && matches!(
                analysis.callee_type(query_site(c, value)),
                Some(Ty::Dynamic | Ty::Null)
            )
        {
            return false;
        }
        if !matches!(
            c.kind(value),
            NodeKind::Identifier | NodeKind::ImplicitCreation | NodeKind::NewExpression
        ) {
            // The send face (`foo()`, `a.b.c()`): the declared return
            // type at the fold's invocation row. A provably `dynamic`
            // answer reports; an untyped shape stays exempt.
            let run = c.collect_direct(initializer);
            if matches!(
                faces::expr_type(c, analysis, &run),
                Some(Ty::Dynamic | Ty::Null)
            ) {
                return false;
            }
        }
        true
    }

    /// `visitFunctionDeclaration` (:71-84): the missing-return-type report
    /// at the name, otherwise the parameter walk — a setter never reports
    /// but always walks.
    fn check_function(c: &mut Check<'_>) {
        let node = c.node();
        if in_augmentation(c) {
            return;
        }
        let Some(name) = member_name_typed(c, node) else {
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
        let Some(name) = member_name_typed(c, node) else {
            return;
        };
        if c.lexeme(c.head_token(name)).starts_with('_') {
            return;
        }
        let is_setter = get_set_token(c, node) == Some("set");
        let index_setter = c.kind(name) == NodeKind::OperatorName
            && c
                .field(name, PayloadField::Name)
                .is_some_and(|op| c.lexeme(op) == "[]=");
        if return_type_missing(c, node, name) && !is_setter && !index_setter {
            let token = match c.kind(name) {
                NodeKind::OperatorName => c.field(name, PayloadField::Name),
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
    /// name is `NoType` (or unrecovered). Operators name through the
    /// `OperatorName` row that tiles *before* the declaration row
    /// (`handleOperatorName` precedes `beginMethod`), so the name-side read
    /// cannot anchor — the row's own children carry the return type (the
    /// re-parse lands inside the row, ahead of the parameters).
    fn return_type_missing(c: &Check<'_>, owner: u32, name: u32) -> bool {
        if c.unit.ast.parent(name) != Some(owner) {
            return row_return_type_missing(c, owner);
        }
        match type_before(c, owner, name) {
            Some((_, kind)) => kind == NodeKind::NoType,
            None => true,
        }
    }

    /// The row-anchored return-type read: the first type-family child ahead
    /// of the parameters decides (`Type`/`VoidType`/`FunctionType`/
    /// `RecordType` = written, `NoType` = absent), and reaching the
    /// parameters first means nothing was recovered.
    fn row_return_type_missing(c: &Check<'_>, owner: u32) -> bool {
        for kid in c.collect_direct(owner) {
            match c.kind(kid) {
                NodeKind::NoType | NodeKind::FormalParameters | NodeKind::NoFormalParameters => {
                    return true;
                }
                NodeKind::Type
                | NodeKind::VoidType
                | NodeKind::FunctionType
                | NodeKind::RecordType => return false,
                _ => {}
            }
        }
        true
    }

    /// `visitFunctionTypeAlias` (:87-95): the legacy form only — the
    /// generalized typedef carries the `Equals` payload word and is no
    /// `FunctionTypeAlias`.
    fn check_typedef(c: &mut Check<'_>) {
        let node = c.node();
        if c.field(node, PayloadField::Equals) != Some(crate::rules::NONE_TOKEN) {
            return;
        }
        let Some(name) = member_name_typed(c, node) else {
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
        let named = c.direct_children(node).iter().copied().find(|&kid| {
            c.kind(kid) == NodeKind::Identifier && c.field(kid, PayloadField::Context) == Some(ctx)
        });
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
        // The primary constructor is a *direct child* of the class-like it
        // declares — the owner is the parent row itself, not a sibling under
        // a wrapper.
        let Some(owner) = c.unit.ast.parent(node).filter(|&parent| {
            matches!(
                c.kind(parent),
                NodeKind::ClassDeclaration
                    | NodeKind::MixinDeclaration
                    | NodeKind::ExtensionDeclaration
                    | NodeKind::ExtensionTypeDeclaration
                    | NodeKind::EnumDeclaration
            )
        }) else {
            return;
        };
        if !constructor_owner_public(c, owner) {
            return;
        }
        // The written primary-constructor name (the primary-constructor
        // context identifier); the unnamed form is public-facing.
        let ctx = dartq_parser::IdentifierContext::PrimaryConstructorDeclaration as u32;
        let named = c.direct_children(node).iter().copied().find(|&kid| {
            c.kind(kid) == NodeKind::Identifier && c.field(kid, PayloadField::Context) == Some(ctx)
        });
        if named.is_some_and(|name| c.lexeme(c.head_token(name)).starts_with('_')) {
            return;
        }
        walk_parameters(c, node);
    }

    /// The first class-like ancestor of the dispatched constructor.
    fn class_like_ancestor(c: &Check<'_>) -> Option<u32> {
        c.path.iter().rev().skip(1).copied().find(|&ancestor| {
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
        crate::rules::class_declaration_name(c, owner).is_none_or(|name| !name.starts_with('_'))
    }

    /// `_VisitorHelper.visitRegularFormalParameter` (:150-158): a parameter
    /// with no written type whose name is not just underscores reports at
    /// the parameter. Field/super formals and function-typed formals are
    /// separate visitor faces upstream and never enter here (the receiver
    /// word separates the receiver face, and a `FunctionTypedFormalParameter`
    /// row inside the parameter row separates the function-typed face — the
    /// walk only ever sees the wrapper row).
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
            .field(param, PayloadField::Receiver)
            .unwrap_or(crate::rules::NONE_TOKEN);
        if receiver != crate::rules::NONE_TOKEN {
            return; // `this.x` / `super.x`: not the `RegularFormalParameter` face
        }
        // `param.type == null` (:152): the written type is a `Type`/
        // `VoidType`/`FunctionType`/`RecordType` child of the parameter row
        // (the type name is its own `Identifier` child before the wrapper
        // row — the first-child read picks the name, not the type). An
        // inline `FunctionTypedFormalParameter` child marks the
        // function-typed face (`void action()`), which is no regular formal
        // upstream and never enters this visitor.
        if c.collect_direct(param).iter().any(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::Type
                    | NodeKind::VoidType
                    | NodeKind::FunctionType
                    | NodeKind::RecordType
                    | NodeKind::FunctionTypedFormalParameter
            )
        }) {
            return;
        }
        let Some(name) = c.field(param, PayloadField::Name) else {
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

pub(crate) mod prefer_void_to_null {
    use super::{Check, NodeKind, RuleSpec, member_name_typed, prefixed_type};
    use crate::rules::{MemberQuery, body_members, member_matches, same_file_ancestors, type_before};

    /// The `augment` `ModSet` bit (bridge `mod_bits`).
    const AUGMENT_BIT: u16 = 1 << 1;
    /// The static `ModSet` bit (header flag bits).
    const STATIC_BIT: u16 = 1 << 3;

    /// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/prefer_void_to_null.dart`
    /// — every written `Null` type that is not one of the exemptions reports
    /// at its name (`visitNamedType`, :59-132). The exemptions (:68-122):
    /// type literals (an expression identifier — never dispatches here),
    /// `Null Function()` return positions, cast patterns, `as` expressions,
    /// `Function(Null)` parameter lists, local and top-level variables (the
    /// class-field face reports), empty `<Null>[]`/`<Null, Null>{}` literals,
    /// extension on-clauses, the extension-type representation formal, and
    /// the void-incompatible-override return face (:44-56 — an overridden
    /// member whose return type is neither `void` nor `FutureOr<void>`
    /// silences); an augmentation ancestor skips (:125-129).
    ///
    /// dartq faces: the exemption walk reads the parent chain (the `Type`
    /// leaf under its real parent — the upstream `parent is X` chain with
    /// the `TypeArguments` sibling interposed). The override face is the
    /// same-file ancestor chain, static methods and extension members never
    /// overriding — cross-file supertypes under-report. A prefixed `p.Null`
    /// resolves through the prefix upstream and stays silent here (the
    /// documented under-report face). Message: `diagnostic.g.dart`
    /// `preferVoidToNull`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_void_to_null",
    &[NodeKind::Type],
    check,
);

    fn check(c: &mut Check<'_>) {
        let row = c.node();
        let head = c.head_token(row);
        // `nodeType.isDartCoreNull` — the written `Null` face.
        if c.lexeme(head) != "Null" || prefixed_type(c, head) {
            return;
        }
        if exemption(c, row) || in_augmentation(c) {
            return;
        }
        let range = crate::rules::utf16_token_span(c, head);
        c.report(range, &[]);
    }

    /// The parent-chain exemptions (:65-122), one arm per upstream parent.
    /// The written `Null` row sits as a direct child of the declaration's
    /// owner — `Fields`/`TopLevelFields` for variable lists, `Method`/
    /// `TopLevelMethod` for return types, `FormalParameter` for formals,
    /// `FunctionType` for function-type returns — never inside the
    /// `VariablesDeclaration`/`InitializedIdentifier` rows themselves.
    fn exemption(c: &Check<'_>, row: u32) -> bool {
        let Some(parent) = c.unit.ast.parent(row) else {
            return false;
        };
        match c.kind(parent) {
            // `Null Function()` (:71-72), `case _ as Null` (:73-74),
            // `a as Null` (:76-77), `extension _ on Null {}` (:103-105).
            NodeKind::FunctionType
            | NodeKind::CastPattern
            | NodeKind::AsExpression
            | NodeKind::ExtensionDeclaration => true,
            // `Function(Null)` (:79-84) and the extension-type
            // representation `extension type N(Null _)` (:108-117).
            NodeKind::FormalParameter => formal_exemption(c, parent),
            // Local and top-level variables stay silent; the class-field
            // face reports (:86-91). The `Null` row's parent is the
            // `Fields`/`TopLevelFields`/statement wrapper.
            NodeKind::Fields => false,
            NodeKind::TopLevelFields
            | NodeKind::Block
            | NodeKind::BlockBody
            | NodeKind::ForStatement
            | NodeKind::ForIn
            | NodeKind::SwitchCase => true,
            // `<Null>[]` / `<Null, Null>{}` (:93-101): empty literals only.
            NodeKind::TypeArguments => type_argument_exemption(c, parent),
            // The void-incompatible-override return face (:119-122) and the
            // top-level function return face (:59-67).
            NodeKind::Method => method_return_exemption(c, row, parent),
            NodeKind::TopLevelMethod => true,
            _ => false,
        }
    }

    /// The formal-parent exemptions: a `Function`-type parameter list or an
    /// extension type's primary-constructor representation. The primary is
    /// the `ExtensionTypeDeclaration`'s own child, so the owner check reads
    /// the parent row directly.
    fn formal_exemption(c: &Check<'_>, formal: u32) -> bool {
        let Some(params) = c.unit.ast.parent(formal) else {
            return false;
        };
        let Some(owner) = c.unit.ast.parent(params) else {
            return false;
        };
        match c.kind(owner) {
            // `Function(Null)`; the extension type's representation formals
            // ride the declaration row directly.
            NodeKind::FunctionType | NodeKind::ExtensionTypeDeclaration => true,
            // The representation formal of the extension type's primary —
            // the primary's parent IS the `ExtensionTypeDeclaration` row.
            NodeKind::PrimaryConstructor => c
                .unit
                .ast
                .parent(owner)
                .is_some_and(|wrapper| c.kind(wrapper) == NodeKind::ExtensionTypeDeclaration),
            _ => false,
        }
    }

    /// The empty-collection-literal exemption (:93-101): any non-type
    /// element kills the exemption. The `TypeArguments` row, the literal's
    /// element rows, and the `ListLiteral`/`SetOrMapLiteral` row are all
    /// siblings under the initializer — an empty literal's sibling set is
    /// just `[TypeArguments, ListLiteral]`.
    fn type_argument_exemption(c: &Check<'_>, args: u32) -> bool {
        let Some(parent) = c.unit.ast.parent(args) else {
            return false;
        };
        let siblings = c.collect_direct(parent);
        let has_literal = siblings.iter().any(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::ListLiteral | NodeKind::SetOrMapLiteral
            )
        });
        has_literal
            && siblings.iter().all(|&kid| {
                matches!(
                    c.kind(kid),
                    NodeKind::TypeArguments | NodeKind::ListLiteral | NodeKind::SetOrMapLiteral
                )
            })
    }

    /// The `isVoidIncompatibleOverride` face (:44-56): only the return-type
    /// position counts (:46); a static method or an extension member never
    /// overrides; the overridden return type is read off the same-file
    /// ancestor chain — `void` and `FutureOr<void>` stay report-worthy
    /// (:52-53), everything else (an untyped member's dynamic included)
    /// silences.
    fn method_return_exemption(c: &Check<'_>, row: u32, method: u32) -> bool {
        let Some(name) = member_name_typed(c, method) else {
            return false;
        };
        if type_before(c, method, name) != Some((row, NodeKind::Type)) {
            return false;
        }
        let container = c.path.iter().rev().skip(1).copied().find(|&ancestor| {
            matches!(
                c.kind(ancestor),
                NodeKind::ClassDeclaration
                    | NodeKind::MixinDeclaration
                    | NodeKind::EnumDeclaration
                    | NodeKind::ExtensionDeclaration
                    | NodeKind::ExtensionTypeDeclaration
            )
        });
        let Some(container) = container else {
            return false;
        };
        if dartq_ast::mod_flags(c.unit.ast, method) & STATIC_BIT != 0
            || matches!(
                c.kind(container),
                NodeKind::ExtensionDeclaration | NodeKind::ExtensionTypeDeclaration
            )
        {
            return false;
        }
        let name_lexeme = c.lexeme(c.head_token(name));
        for ancestor in same_file_ancestors(c, container, false) {
            for member in body_members(c, ancestor) {
                if member_matches(c, member, MemberQuery::Method, name_lexeme).is_empty() {
                    continue;
                }
                return !matches!(
                    overridden_return(c, member),
                    Some(face) if face.is_void || face.is_future_or_void
                );
            }
        }
        false
    }

    /// The overridden member's written return type, if any. `void` carries
    /// a `VoidType` row, not `Type` (`is_void`/`is_future_or_void` both
    /// answer off the face).
    fn overridden_return<'a>(c: &'a Check<'_>, member: u32) -> Option<crate::rules::TypeFace<'a>> {
        let name = member_name_typed(c, member)?;
        let (row, kind) = type_before(c, member, name)?;
        (kind == NodeKind::Type || kind == NodeKind::VoidType)
            .then(|| crate::rules::type_face(c, row))
    }

    /// The official `declaration?.isAugmentation` guard (:125-129): the
    /// nearest class-member or unit-member ancestor carrying the augment
    /// bit silences the whole face.
    fn in_augmentation(c: &Check<'_>) -> bool {
        c.path
            .iter()
            .rev()
            .skip(1)
            .copied()
            .find(|&ancestor| {
                matches!(
                    c.kind(ancestor),
                    NodeKind::Method
                        | NodeKind::TopLevelMethod
                        | NodeKind::Constructor
                        | NodeKind::Factory
                        | NodeKind::Fields
                        | NodeKind::TopLevelFields
                        | NodeKind::Typedef
                        | NodeKind::ClassDeclaration
                        | NodeKind::MixinDeclaration
                        | NodeKind::ExtensionDeclaration
                        | NodeKind::ExtensionTypeDeclaration
                        | NodeKind::EnumDeclaration
                )
            })
            .is_some_and(|row| dartq_ast::mod_flags(c.unit.ast, row) & AUGMENT_BIT != 0)
    }
}

pub(crate) mod no_raw_types {
    use super::{Check, NodeKind, RuleSpec, raw_generic_face, type_arguments_row};

    /// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/no_raw_types.dart`
    /// — a written generic type without type arguments whose instantiation
    /// fills `dynamic` reports at the annotation (:41-82): explicit
    /// arguments keep the row out (:48), the `@optionalTypeArgs` annotation
    /// keeps it out (:67), and the "strict raw type" positions — `as`,
    /// cast patterns, `is`, object patterns, type literals (:69-79) — are
    /// exempt as too noisy; instance creations never enter (:43-46).
    ///
    /// dartq faces and the approximation register: genericity is the
    /// pure-syntax face ([`super::raw_generic_face`]) — same-file
    /// class-like declarations (arity and per-variable bounds read off the
    /// written `TypeVariables`, the `instantiateToBounds` face) plus the
    /// [`super::CORE_GENERICS`] table. Under-report: imported user
    /// generics, same-file typedef/extension-type aliases, and prefixed
    /// spellings (`p.List`); over-report: none (every recognized name is
    /// genuinely generic). The `@optionalTypeArgs` exemption reads the
    /// same-file declaration only. The creation skip is face-free (the
    /// creation's name is an expression identifier, never a `Type` row).
    /// The message fill is the written instantiation display
    /// (`List<dynamic>`); a function/record bound renders as a
    /// `Function`/`(...)` placeholder. Message: `diagnostic.g.dart`
    /// `noRawTypes`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "no_raw_types",
    &[NodeKind::Type],
    check,
);

    fn check(c: &mut Check<'_>) {
        let row = c.node();
        let Some((fills, optional_type_args)) = raw_generic_face(c, row) else {
            return;
        };
        // `node.typeArguments != null` (:48).
        if type_arguments_row(c, row).is_some() {
            return;
        }
        // `element.metadata.hasOptionalTypeArgs` (:67).
        if optional_type_args {
            return;
        }
        // The raw instantiation carries a `dynamic` fill (:66) — every
        // variable bounded to a written bound fills with that bound.
        if !fills.iter().any(|fill| fill == "dynamic") {
            return;
        }
        // The escaping positions (:69-79): the first non-type ancestor is
        // the exempt face.
        if escaping_position(c, row) {
            return;
        }
        let display = format!("{}<{}>", c.lexeme(c.head_token(row)), fills.join(", "));
        c.report(c.utf16_span(row), &[display.as_str()]);
    }

    /// The `parentEscapingTypeArguments` walk (:85-97): climb through the
    /// type-argument nesting; an `as`/`is`/cast-pattern/object-pattern
    /// landing exempts (:70-74). The `List;` type-literal face is an
    /// expression identifier and never dispatches.
    fn escaping_position(c: &Check<'_>, row: u32) -> bool {
        let mut ancestor = row;
        for _ in 0..256 {
            let Some(parent) = c.unit.ast.parent(ancestor) else {
                return false;
            };
            match c.kind(parent) {
                NodeKind::TypeArguments | NodeKind::Type => ancestor = parent,
                NodeKind::AsExpression
                | NodeKind::IsExpression
                | NodeKind::CastPattern
                | NodeKind::ObjectPattern => return true,
                _ => return false,
            }
        }
        false
    }
}

pub(crate) mod always_specify_types {
    use super::{PayloadField, Check, NodeKind, RuleSpec, TYPE_FAMILY, first_type_child, raw_generic_face, type_arguments_row};
    use crate::rules::{is_just_underscores, variables_declaration_keyword};

    const PROBLEM: &str = "Missing type annotation.";
    const ADD_TYPE: &str = "Try adding a type annotation.";
    const REPLACE_KEYWORD: &str = "Try replacing '{0}' with '{1}'.";
    const SPECIFY_TYPE: &str = "Try specifying the type '{0}'.";
    const SPLIT_TO_TYPES: &str =
        "Try splitting the declaration and specifying the different type annotations.";

    /// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/always_specify_types.dart`
    /// — every implicit type reports `Missing type annotation.` in four
    /// diagnostic shapes (`alwaysSpecifyTypesAddType`/`ReplaceKeyword`/
    /// `SpecifyType`/`SplitToTypes`, `diagnostic.g.dart`). The registered
    /// visitors (:45-52): raw generic named types (`visitNamedType`,
    /// :117-132 — add-type at the annotation, `is` expressions exempt),
    /// untyped variable lists with a keyword (:178-225 — the four-way
    /// initializer-type split at the keyword), untyped formals
    /// (:135-170 — the keyword at the keyword, else the parameter; the
    /// static type of an unwritten formal is `dynamic`, so both land on
    /// add-type), declared variable patterns (:90-109) and for-in declared
    /// identifiers (:67-87).
    ///
    /// Approximation register: the initializer types behind
    /// `ReplaceKeyword`/`SpecifyType` are read off the obvious literal
    /// shapes (int/double/bool/String/null) — a compound initializer
    /// contributes nothing and the list falls back to the add-type face
    /// (problem text and range match upstream; the correction
    /// under-approximates). The pattern and for-in faces read
    /// `matchedValueType`/the element type upstream — resolver-only, so
    /// they land on the add-type face. Genericity is the pure-syntax face
    /// ([`super::raw_generic_face`]; under-report: imported user generics
    /// and prefixed spellings); the `@optionalTypeArgs` and `is`-parent
    /// exemptions follow the upstream faces.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "always_specify_types",
    &[
            NodeKind::Type,
            NodeKind::VariablesDeclaration,
            NodeKind::FormalParameter,
            NodeKind::DeclaredVariablePattern,
        ],
    check,
);

    fn check(c: &mut Check<'_>) {
        match c.kind(c.node()) {
            NodeKind::Type => named_type(c),
            NodeKind::VariablesDeclaration => variable_list(c),
            NodeKind::FormalParameter => formal(c),
            NodeKind::DeclaredVariablePattern => pattern(c),
            _ => {}
        }
    }

    /// `visitNamedType` (:117-132): a raw generic type reports add-type at
    /// the annotation; `is` expressions are exempt (:124), `as` and the
    /// pattern faces are not.
    fn named_type(c: &mut Check<'_>) {
        let row = c.node();
        let Some((_, optional_type_args)) = raw_generic_face(c, row) else {
            return;
        };
        if type_arguments_row(c, row).is_some() || optional_type_args {
            return;
        }
        if c.unit
            .ast
            .parent(row)
            .is_some_and(|parent| c.kind(parent) == NodeKind::IsExpression)
        {
            return;
        }
        c.report(c.utf16_span(row), &[]);
    }

    /// `visitVariableDeclarationList` (:178-225): an untyped list with a
    /// keyword reports at the keyword — the for-in declared variable takes
    /// its own face (:67-87), the other parents (:182-193) share the
    /// four-way split.
    fn variable_list(c: &mut Check<'_>) {
        let row = c.node();
        // `list.type == null` (:180): the written type is the preceding
        // sibling under the declaration's owner (`Fields`/`TopLevelFields`/
        // the statement wrapper), not a child of `VariablesDeclaration`.
        if !matches!(
            super::type_sibling_before(c, row).map(|prev| c.kind(prev)),
            Some(NodeKind::NoType)
        ) {
            return;
        }
        let Some(keyword) = variables_declaration_keyword(c, row) else {
            return;
        };
        let Some(parent) = c.ancestor(1) else {
            return;
        };
        let siblings = c.collect_direct(parent);
        let has_kind = |kind: NodeKind| siblings.iter().any(|&kid| c.kind(kid) == kind);
        if c.kind(parent) == NodeKind::ForIn || has_kind(NodeKind::ForInParts) {
            // The `for (var x in xs)` declared identifier: the element type
            // rides the resolver — the add-type face at the keyword (the
            // documented correction under-approximation).
            let range = crate::rules::utf16_token_span(c, keyword);
            c.report_texts(range, PROBLEM, ADD_TYPE, &[]);
            return;
        }
        if !matches!(
            c.kind(parent),
            NodeKind::TopLevelFields
                | NodeKind::Fields
                | NodeKind::Block
                | NodeKind::BlockBody
                | NodeKind::SwitchCase
                | NodeKind::ForStatement
                | NodeKind::ForParts
        ) && !has_kind(NodeKind::ForParts)
        {
            return;
        }
        // `_getTypes` (:227-248): the distinct initializer types. The
        // obvious literal shapes fill; anything else contributes nothing.
        let mut types: Vec<&'static str> = Vec::new();
        for decl in c.collect_direct(row) {
            if c.kind(decl) != NodeKind::InitializedIdentifier {
                continue;
            }
            let Some(initializer) = c
                .collect_direct(decl)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer)
            else {
                continue;
            };
            let Some(display) = literal_display(c, crate::rules::first_child(c, initializer))
            else {
                continue;
            };
            if !types.contains(&display) {
                types.push(display);
            }
        }
        let range = crate::rules::utf16_token_span(c, keyword);
        let keyword_lexeme = c.lexeme(keyword);
        match types.len() {
            0 => c.report_texts(range, PROBLEM, ADD_TYPE, &[]),
            1 if keyword_lexeme == "var" => {
                c.report_texts(range, PROBLEM, REPLACE_KEYWORD, &[keyword_lexeme, types[0]]);
            }
            1 => c.report_texts(range, PROBLEM, SPECIFY_TYPE, &[types[0]]),
            _ if keyword_lexeme == "var" => {
                c.report_texts(range, PROBLEM, SPLIT_TO_TYPES, &[]);
            }
            _ => c.report_texts(range, PROBLEM, ADD_TYPE, &[]),
        }
    }

    /// The obvious literal-initializer displays (the documented
    /// approximation of `_getTypes`' static types).
    fn literal_display(c: &Check<'_>, value: Option<u32>) -> Option<&'static str> {
        Some(match c.kind(value?) {
            NodeKind::LiteralInt => "int",
            NodeKind::LiteralDouble => "double",
            NodeKind::LiteralBool => "bool",
            NodeKind::StringLiteral => "String",
            NodeKind::LiteralNull => "Null",
            _ => return None,
        })
    }

    /// `visitRegularFormalParameter` (:135-170): an untyped named formal
    /// whose name is not just underscores — the keyword branch reports at
    /// the keyword, the keyword-less branch at the parameter; both land on
    /// the add-type code (the unwritten formal's static type is dynamic).
    fn formal(c: &mut Check<'_>) {
        let row = c.node();
        // Receiver formals are the Field/Super formal visitors upstream and
        // never enter (:135 registration).
        if c.field(row, PayloadField::Receiver).unwrap_or(crate::rules::NONE_TOKEN)
            != crate::rules::NONE_TOKEN
        {
            return;
        }
        let Some(name) = c.field(row, PayloadField::Name) else {
            return;
        };
        if name == crate::rules::NONE_TOKEN || is_just_underscores(c.lexeme(name)) {
            return; // :137
        }
        // `param.type == null` (:137).
        if first_type_child(c, row).is_some() {
            return;
        }
        let range = match formal_keyword(c, row) {
            Some(keyword) => crate::rules::utf16_token_span(c, keyword),
            None => c.utf16_span(row),
        };
        c.report_texts(range, PROBLEM, ADD_TYPE, &[]);
    }

    /// The formal's `var`/`final`/`const` keyword (`constFinalOrVarKeyword`,
    /// :138) — the only legal occurrences of the lexemes inside the
    /// formal's span.
    fn formal_keyword(c: &Check<'_>, formal: u32) -> Option<u32> {
        c.unit
            .ast
            .token_range(formal)
            .into_iter()
            .find(|&token| matches!(c.lexeme(token), "var" | "final" | "const"))
    }

    /// `visitDeclaredVariablePattern` (:90-109): an untyped declared
    /// pattern reports at the keyword (the head token is the keyword when
    /// written, the variable otherwise). `matchedValueType` rides the
    /// resolver — the add-type face (the documented correction
    /// under-approximation).
    fn pattern(c: &mut Check<'_>) {
        let row = c.node();
        // `node.type == null` (:91): a typed pattern carries its type
        // annotation as the left sibling (the parse emits the type before
        // the pattern row).
        let Some(parent) = c.unit.ast.parent(row) else {
            return;
        };
        let siblings = c.collect_direct(parent);
        let Some(pos) = siblings.iter().position(|&kid| kid == row) else {
            return;
        };
        if pos > 0 && TYPE_FAMILY.contains(&c.kind(siblings[pos - 1])) {
            return;
        }
        let range = crate::rules::utf16_token_span(c, c.head_token(row));
        c.report_texts(range, PROBLEM, ADD_TYPE, &[]);
    }
}

pub(crate) mod avoid_futureor_void {
    use super::{PayloadField, Check, NodeKind, RuleSpec, TYPE_FAMILY, first_type_child, type_argument_rows, type_arguments_row, type_sibling_before};
    use crate::rules::NONE_TOKEN;

    /// The variance of a type position (the upstream `VarianceChecker`
    /// walk): `Out` result positions, `In` contravariant positions, `InOut`
    /// bounds. Every Dart class type parameter is legacy-covariant (no
    /// variance syntax; `isLegacyCovariant` is `_variance == null`), so the
    /// ambient variance continues into every type-argument position.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Variance {
        Out,
        In,
        InOut,
    }

    impl Variance {
        fn inverse(self) -> Self {
            match self {
                Variance::Out => Variance::In,
                Variance::In => Variance::Out,
                Variance::InOut => Variance::InOut,
            }
        }
    }

    /// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/avoid_futureor_void.dart`
    /// — a written `FutureOr<void>` in a non-contravariant result position
    /// reports at its annotation (`checkNamedType`, :59-73: `in` positions
    /// skip; the `FutureOr` head with a lone `void` argument reports). The
    /// registered positions (:37-50): as/cast/is/object-pattern types,
    /// extends/with/on/implements clauses, function and method return types
    /// with their formals at `in`, primary-constructor formals at out,
    /// variable-list types, and type-parameter bounds at inout
    /// (`checkInOut`, :137).
    ///
    /// dartq faces: the variance walk recurses through type arguments with
    /// the ambient variance (legacy covariance), flips at function-type
    /// formal parameters, and reports at the annotation's own span. Bounds
    /// ride the `TypeVariable` dispatch alone (upstream re-walks them from
    /// the method face too — the same entry set). The mixin application's
    /// superclass position has no upstream visitor and stays out; prefixed
    /// (`p.FutureOr`) and `FutureOr<void>?`-with-nested-non-void spellings
    /// keep the documented under-report face. Message: `diagnostic.g.dart`
    /// `avoidFutureorVoid`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_futureor_void",
    &[
            NodeKind::TopLevelMethod,
            NodeKind::Method,
            NodeKind::VariablesDeclaration,
            NodeKind::TypeVariable,
            NodeKind::AsExpression,
            NodeKind::IsExpression,
            NodeKind::CastPattern,
            NodeKind::ObjectPattern,
            NodeKind::PrimaryConstructor,
            NodeKind::ClassDeclaration,
            NodeKind::MixinDeclaration,
            NodeKind::ExtensionDeclaration,
            NodeKind::NamedMixinApplication,
        ],
    check,
);

    fn check(c: &mut Check<'_>) {
        match c.kind(c.node()) {
            NodeKind::TopLevelMethod | NodeKind::Method => callable(c),
            NodeKind::VariablesDeclaration => {
                // `visitVariableDeclarationList` (:50, :122): the written
                // type rides the declaration row's left sibling — the
                // `NoType`/`Type`/`FunctionType`/`RecordType` child of the
                // owning `Fields`/`TopLevelFields`/statement wrapper, never
                // inside `VariablesDeclaration` itself.
                let row = c.node();
                let written =
                    type_sibling_before(c, row).filter(|&prev| c.kind(prev) != NodeKind::NoType);
                checked(c, Variance::Out, written);
            }
            NodeKind::AsExpression | NodeKind::IsExpression => {
                checked(c, Variance::Out, first_type_child(c, c.node()));
            }
            // `checkBound` (:116-117): the bound at inout.
            NodeKind::TypeVariable => checked(c, Variance::InOut, first_type_child(c, c.node())),
            NodeKind::CastPattern => cast(c),
            NodeKind::ObjectPattern => object_pattern(c),
            NodeKind::PrimaryConstructor => primary(c),
            // The clause visitors (:87-93, :106-122, :143-145): every
            // header supertype — extends, with, on, implements — checks at
            // out. The clause keywords are payload words (no wrapper rows),
            // so the direct `Type` rows before the body cover the header.
            NodeKind::ClassDeclaration
            | NodeKind::MixinDeclaration
            | NodeKind::ExtensionDeclaration => {
                let node = c.node();
                let rows: Vec<u32> = c
                    .collect_direct(node)
                    .into_iter()
                    .take_while(|&kid| !matches!(c.kind(kid), NodeKind::DeclarationBody))
                    .filter(|&kid| c.kind(kid) == NodeKind::Type)
                    .collect();
                for row in rows {
                    checked(c, Variance::Out, Some(row));
                }
            }
            // `class A = B with C;` — the with/implements types ride the
            // clause visitors upstream; the superclass position has no
            // registered visitor.
            NodeKind::NamedMixinApplication => {
                let node = c.node();
                let rows: Vec<u32> = c
                    .collect_direct(node)
                    .into_iter()
                    .filter(|&kid| c.kind(kid) == NodeKind::Type)
                    .filter(|&row| clause(c, row) != "extends")
                    .collect();
                for row in rows {
                    checked(c, Variance::Out, Some(row));
                }
            }
            _ => {}
        }
    }

    /// The clause keyword governing the supertype named at `row`: the
    /// nearest preceding clause lexeme back to the header head (a header is
    /// a handful of tokens); the mixin application's `=` marks the
    /// superclass position.
    fn clause(c: &Check<'_>, row: u32) -> &'static str {
        let mut token = c.head_token(row);
        for _ in 0..256 {
            let Some(prev) = token.checked_sub(1) else {
                return "extends";
            };
            token = prev;
            match c.lexeme(token) {
                "extends" | "=" => return "extends",
                "with" => return "with",
                "on" => return "on",
                "implements" => return "implements",
                _ => {}
            }
        }
        "extends"
    }

    /// The callable faces (:95-118): the return type at out, the formals
    /// at `in` (`checkFormalParameterIn`).
    fn callable(c: &mut Check<'_>) {
        let node = c.node();
        checked(c, Variance::Out, first_type_child(c, node));
        let Some(params) = c
            .collect_direct(node)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
        else {
            return;
        };
        check_formals(c, Variance::In, c.collect_direct(params));
    }

    /// The formal rows of a parameter list: the required group, then the
    /// `[]`/`{}` section.
    fn check_formals(c: &mut Check<'_>, variance: Variance, sections: Vec<u32>) {
        for section in sections {
            match c.kind(section) {
                NodeKind::FormalParameter | NodeKind::FunctionTypedFormalParameter => {
                    checked_formal(c, variance, section);
                }
                NodeKind::OptionalFormalParameters => {
                    let rows = c.collect_direct(section);
                    check_formals(c, variance, rows);
                }
                _ => {}
            }
        }
    }

    /// `checkFormalParameter` (:120-137): an untyped formal skips
    /// (`isExplicitlyTyped`); the written type walks with the incoming
    /// variance; a function-typed formal's parameters walk with the inverse
    /// (its bounds ride the `TypeVariable` dispatch). The bridge nests the
    /// `FunctionTypedFormalParameter` inside the outer `FormalParameter`
    /// row, so the plain arm descends into it first.
    fn checked_formal(c: &mut Check<'_>, variance: Variance, formal: u32) {
        match c.kind(formal) {
            NodeKind::FormalParameter => {
                if let Some(inner) = c
                    .collect_direct(formal)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::FunctionTypedFormalParameter)
                {
                    checked_formal(c, variance, inner);
                    return;
                }
                checked(c, variance, first_type_child(c, formal));
            }
            NodeKind::FunctionTypedFormalParameter => {
                checked(c, variance, first_type_child(c, formal));
                if let Some(params) = c
                    .collect_direct(formal)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
                {
                    let rows = c.collect_direct(params);
                    check_formals(c, variance.inverse(), rows);
                }
            }
            _ => {}
        }
    }

    /// The primary-constructor face (:128-134): the regular formals'
    /// written types at out (receiver formals are not `RegularFormalParameter`).
    fn primary(c: &mut Check<'_>) {
        let node = c.node();
        let Some(params) = c
            .collect_direct(node)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
        else {
            return;
        };
        for section in c.collect_direct(params) {
            if c.kind(section) != NodeKind::FormalParameter {
                continue;
            }
            let receiver = c
                .field(section, PayloadField::Receiver)
                .unwrap_or(NONE_TOKEN);
            if receiver == NONE_TOKEN {
                checked(c, Variance::Out, first_type_child(c, section));
            }
        }
    }

    /// `visitCastPattern` (:84): the type parses after the `as` handler
    /// seals the pattern (bridge.rs `handle_cast_pattern`), so the type row
    /// is the right sibling.
    fn cast(c: &mut Check<'_>) {
        let node = c.node();
        let Some(parent) = c.unit.ast.parent(node) else {
            return;
        };
        let siblings = c.collect_direct(parent);
        let Some(pos) = siblings.iter().position(|&kid| kid == node) else {
            return;
        };
        let next = siblings.get(pos + 1).copied();
        checked(
            c,
            Variance::Out,
            next.filter(|&kid| c.kind(kid) == NodeKind::Type),
        );
    }

    /// `visitObjectPattern` (:125): the pattern's written type — the head
    /// identifier with the type-argument rows that parsed before the fields
    /// (siblings preceding the pattern row).
    fn object_pattern(c: &mut Check<'_>) {
        let node = c.node();
        if c.lexeme(c.head_token(node)) != "FutureOr" {
            return;
        }
        let Some(parent) = c.unit.ast.parent(node) else {
            return;
        };
        let siblings = c.collect_direct(parent);
        let Some(pos) = siblings.iter().position(|&kid| kid == node) else {
            return;
        };
        let has_void_argument = siblings[..pos].iter().rev().any(|&kid| {
            c.kind(kid) == NodeKind::TypeArguments
                && matches!(
                    type_argument_rows(c, kid).as_slice(),
                    [only] if c.kind(*only) == NodeKind::VoidType
                )
        });
        if has_void_argument {
            let range = c.utf16_span(node);
            c.report(range, &[]);
        }
    }

    /// The `check` walk (:38-110): named types recurse into their
    /// arguments with the ambient variance first, then answer the hook —
    /// `in` positions skip, the `FutureOr<void>` shape reports at the
    /// annotation (:59-73). Function types walk the return type with the
    /// ambient variance and the formal parameters with the inverse
    /// (:92-97); record types walk their fields (:98-109).
    fn checked(c: &mut Check<'_>, variance: Variance, row: Option<u32>) {
        let Some(row) = row else {
            return;
        };
        match c.kind(row) {
            NodeKind::Type => {
                let args = type_arguments_row(c, row)
                    .map(|args| type_argument_rows(c, args))
                    .unwrap_or_default();
                for arg in args {
                    checked(c, variance, Some(arg));
                }
                if variance != Variance::In && is_future_or_void(c, row) {
                    let range = c.utf16_span(row);
                    c.report(range, &[]);
                }
            }
            NodeKind::FunctionType => {
                checked(c, variance, first_type_child(c, row));
                if let Some(params) = c
                    .collect_direct(row)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
                {
                    let rows = c.collect_direct(params);
                    check_formals(c, variance.inverse(), rows);
                }
            }
            NodeKind::RecordType => {
                for kid in c.collect_direct(row) {
                    match c.kind(kid) {
                        kind if TYPE_FAMILY.contains(&kind) => checked(c, variance, Some(kid)),
                        NodeKind::RecordTypeEntry | NodeKind::RecordTypeNamedFields => {
                            let fields = c.collect_direct(kid);
                            for field in fields {
                                if TYPE_FAMILY.contains(&c.kind(field)) {
                                    checked(c, variance, Some(field));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// The `FutureOr<void>` shape (:64-72): the unprefixed `FutureOr` head
    /// with exactly one `void` argument.
    fn is_future_or_void(c: &Check<'_>, row: u32) -> bool {
        let head = c.head_token(row);
        if c.lexeme(head) != "FutureOr" {
            return false;
        }
        if head
            .checked_sub(1)
            .is_some_and(|prev| c.lexeme(prev) == ".")
        {
            return false;
        }
        let Some(args) = type_arguments_row(c, row) else {
            return false;
        };
        matches!(
            type_argument_rows(c, args).as_slice(),
            [only] if c.kind(*only) == NodeKind::VoidType
        )
    }
}

pub(crate) mod erase_dart_type_extension_types {
    use super::{Check, NodeKind, RuleSpec};

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/erase_dart_type_extension_types.dart` — an
    /// `is` check whose tested type implements the kernel `DartType`
    /// interface reports at the expression (`visitIsExpression`): the
    /// guard for erased extension-type `DartType` values. Upstream ships
    /// the rule with `state: RuleState.internal()` — experimental, in no
    /// release set, with a header note marking the description
    /// provisional; the dartq face registers the plain trigger.
    ///
    /// Approximation register: the resolved
    /// `type.implementsInterface('DartType', 'kernel.ast')` question is
    /// the pure-syntax name face — the checked type row's name is `Type`
    /// (the dart:core `DartType` implementor a user can name). Prefixed
    /// spellings (`core.Type`) and non-`Type` implementors stay silent
    /// (under-report); a user class named `Type` over-reports. The node
    /// anchors at the `is` operator, so the report span opens at the
    /// operand to keep the official's whole-expression shape.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "erase_dart_type_extension_types",
    &[NodeKind::IsExpression],
    check,
);

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let kids = c.collect_direct(node);
        let is_type_check = kids
            .iter()
            .copied()
            .any(|kid| c.kind(kid) == NodeKind::Type && c.lexeme(c.head_token(kid)) == "Type");
        if !is_type_check {
            return;
        }
        let span = c.utf16_span(node);
        let start = kids
            .first()
            .map_or(span.start, |&first| c.utf16_span(first).start);
        c.report(start..span.end, &[]);
    }
}

// —— the shared leak-detector face ———————————————————————————————————————
//
// `cancel_subscriptions` and `close_sinks` are one upstream implementation
// (`upstream Dart SDK source, pkg/linter/lib/src/util/leak_detector_visitor.dart`)
// parameterized by (watched types, releasing calls); the shared face below
// ports it once and the two rules carry only their predicate tables and
// message templates. All name-level approximations are registered here once.

use crate::rules::{NONE_TOKEN, initialized_identifier_name, same_file_declaration, slice, variables_declaration_keyword};

/// The library-type spellings one leak rule watches and the calls that
/// count as a release. Upstream the predicates are interface questions
/// (`type.implementsInterface('StreamSubscription', 'dart.async')`); the
/// name-level face answers by written spelling with the same-file shadow
/// gate ([`same_file_declaration`]), so `IOSink`/`Socket` join `Sink`'s
/// list explicitly — both implement `Sink` upstream, and a `Socket`
/// answers the `destroy` predicate too, so the merged method set is
/// exactly upstream's `_hasMatch`, which accepts any predicate whose call
/// name matches (leak_detector_visitor.dart:16-20).
struct LeakPredicates {
    types: &'static [&'static str],
    methods: &'static [&'static str],
    /// Whether the field and primary-constructor faces mount. The live
    /// oracle the differential gate keys on (fvm 3.13.1, the lints-new
    /// expectations rebuilt 2026-10-18) answers the twin rules
    /// asymmetrically on this face — `cancel_subscriptions` reports an
    /// uncancelled field or primary formal, `close_sinks` stays silent
    /// there (its 3.13 rule predates the shared 3.15 leak detector's
    /// field face; the corpus README pins the drift). Locals mount for
    /// both rules.
    field_faces: bool,
}

/// `cancel_subscriptions`: a `StreamSubscription` is released by `cancel`
/// (leak_detector_visitor.dart:36-44).
const CANCEL_PREDICATES: LeakPredicates = LeakPredicates {
    types: &["StreamSubscription"],
    methods: &["cancel"],
    field_faces: true,
};

/// `close_sinks`: a `Sink` (or an `IOSink`/`Socket` implementor) is
/// released by `close`/`destroy` (leak_detector_visitor.dart:36-50).
const CLOSE_PREDICATES: LeakPredicates = LeakPredicates {
    types: &["Sink", "IOSink", "Socket"],
    methods: &["close", "destroy"],
    field_faces: false,
};

/// One watched declaration: the row the report anchors at, its name, the
/// watch verdict, and the upstream `_VariableType` split that gates the
/// field-formal, field-initializer, and return faces.
struct LeakCandidate<'a> {
    /// The report row: an `InitializedIdentifier` (name plus initializer —
    /// the official `VariableDeclaration` node) or a primary-constructor
    /// formal parameter.
    row: u32,
    name: &'a str,
    watched: bool,
    field: bool,
    /// The declaration's own span for the in-initializer invocation face
    /// (`thisOrAncestorMatching((a) => a == variable)`,
    /// `leak_detector_visitor.dart:257`); `None` for primary formals (no
    /// initializer subtree exists).
    declaration: Option<u32>,
}

/// The shared check behind both leak rules: collect the declaration node's
/// candidates, then scan the upstream container (the compilation unit for
/// fields and primary formals, the enclosing function body for locals) for
/// a valid use; an untyped or validly used candidate stays silent.
#[expect(
    clippy::too_many_lines,
    reason = "one flat candidate-collection ladder over the declaration               shapes; splitting it would scatter the official line citations"
)]
fn leak_detector_check(c: &mut Check<'_>, predicates: &LeakPredicates) {
    let node = c.node();
    let mut candidates: Vec<LeakCandidate<'_>> = Vec::new();
    match c.kind(node) {
        // `visitFieldDeclaration` (:68-85): every variable of every field
        // declaration, container = the compilation unit. The face is
        // per-rule (the oracle-drift gate on `LeakPredicates::field_faces`).
        NodeKind::Fields => {
            if !predicates.field_faces {
                return;
            }
            let direct = c.collect_direct(node);
            // The written type's head spelling. The plain shape tiles the
            // `Type` row beside the `VariablesDeclaration` (the row anchors
            // at the type start); the keyword-anchored shape (`late`/`var`/
            // `final` heads the row, members.rs `decl_head`) tiles it INSIDE
            // the declaration row, before the names — read there as the
            // fallback.
            let type_spelling = direct
                .iter()
                .copied()
                .find(|&kid| c.kind(kid) == NodeKind::Type)
                .or_else(|| {
                    direct
                        .iter()
                        .copied()
                        .find(|&kid| c.kind(kid) == NodeKind::VariablesDeclaration)
                        .and_then(|decl| {
                            c.collect_direct(decl)
                                .into_iter()
                                .find(|&kid| c.kind(kid) == NodeKind::Type)
                        })
                })
                .map(|type_row| c.lexeme(c.head_token(type_row)).to_owned());
            for decl in direct {
                if c.kind(decl) != NodeKind::VariablesDeclaration {
                    continue;
                }
                for var in c.collect_direct(decl) {
                    if c.kind(var) != NodeKind::InitializedIdentifier {
                        continue;
                    }
                    push_leak_candidate(
                        c,
                        &mut candidates,
                        var,
                        type_spelling.as_deref(),
                        true,
                        predicates,
                    );
                }
            }
        }
        // `visitVariableDeclarationStatement` (:114-132): local statements
        // only (the function-body gate — a for-each loop variable and the
        // C-style for initializer are other upstream node types and stay
        // unvisited), container = the enclosing function body.
        NodeKind::VariablesDeclaration => {
            if !matches!(
                c.ancestor(1).map(|parent| c.kind(parent)),
                Some(NodeKind::Block | NodeKind::BlockBody)
            ) {
                return;
            }
            let type_spelling =
                local_written_type_head(c, node).map(|token| c.lexeme(token).to_owned());
            for var in c.collect_direct(node) {
                if c.kind(var) != NodeKind::InitializedIdentifier {
                    continue;
                }
                push_leak_candidate(
                    c,
                    &mut candidates,
                    var,
                    type_spelling.as_deref(),
                    false,
                    predicates,
                );
            }
        }
        // `visitPrimaryConstructorDeclaration` (:88-111): each formal that
        // declares a field; the extension-type representation skips (the
        // extension type IS the field, :91). The `element is
        // FieldFormalParameterElement && element.isDeclaring` gate (:99)
        // admits only the declaring forms — `var`/`final` on the formal
        // (`finalOrVarKeyword != null`, element_builder.dart:2091); a plain
        // formal and a `this.`/`super.` initializing formal are not
        // declaring and never become candidates. The face is per-rule (the
        // oracle-drift gate).
        NodeKind::PrimaryConstructor => {
            if !predicates.field_faces {
                return;
            }
            if c.ancestor(1)
                .is_some_and(|parent| c.kind(parent) == NodeKind::ExtensionTypeDeclaration)
            {
                return;
            }
            for param in leak_formal_rows(c, node) {
                if !formal_declaring(c, param) {
                    continue;
                }
                let Some(name) = c.field(param, PayloadField::Name) else {
                    continue;
                };
                if name == NONE_TOKEN {
                    continue;
                }
                let watched = c
                    .collect_direct(param)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::Type)
                    .is_some_and(|type_row| {
                        leak_watched(c, c.lexeme(c.head_token(type_row)), predicates)
                    });
                candidates.push(LeakCandidate {
                    row: param,
                    name: c.lexeme(name),
                    watched,
                    field: true,
                    declaration: None,
                });
            }
        }
        _ => {}
    }
    for candidate in &candidates {
        if !candidate.watched {
            continue;
        }
        // The container (:77-82/:115-119): the compilation unit for fields
        // and primary formals, the enclosing function body for locals.
        // Upstream walks the unit but matches by element identity, so a
        // same-named field of another class never counts as this field's
        // use; the declaring container's body is the name-level proxy (the
        // oracle reports a field cancelled only outside its class too —
        // upstream's `@FailingTest` canceled_outsideTheClass).
        let container =
            if candidate.field {
                c.path
                    .iter()
                    .rev()
                    .copied()
                    .find(|&row| {
                        matches!(
                            c.kind(row),
                            NodeKind::ClassDeclaration
                                | NodeKind::MixinDeclaration
                                | NodeKind::EnumDeclaration
                                | NodeKind::ExtensionDeclaration
                                | NodeKind::ExtensionTypeDeclaration
                        )
                    })
                    .or_else(|| c.unit.ast.root())
            } else {
                c.path.iter().rev().copied().find(|&row| {
                    matches!(c.kind(row), NodeKind::BlockBody | NodeKind::ExpressionBody)
                })
            };
        let Some(container) = container else {
            continue;
        };
        if !leak_valid_use(c, container, candidate, predicates) {
            c.report(c.utf16_span(candidate.row), &[]);
        }
    }
}

/// Pushes one `InitializedIdentifier` candidate: the watched verdict from
/// the written type spelling, and the `hasSimpleInitializer` skip (the
/// initializer is exactly one identifier token — an alias of an existing
/// reference, leak_detector_visitor.dart:316-319).
fn push_leak_candidate<'a>(
    c: &Check<'a>,
    candidates: &mut Vec<LeakCandidate<'a>>,
    var: u32,
    type_spelling: Option<&str>,
    field: bool,
    predicates: &LeakPredicates,
) {
    let Some(name) = initialized_identifier_name(c, var) else {
        return;
    };
    let watched = type_spelling.is_some_and(|spelling| leak_watched(c, spelling, predicates));
    if !watched {
        return;
    }
    // `hasSimpleInitializer` (:316-319): `= otherVar` alone skips — the
    // declaration aliases an existing reference instead of creating one.
    // The initializer row opens at the `=` token.
    let initializer = c
        .collect_direct(var)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer);
    if initializer.is_some_and(|init| {
        let range = c.unit.ast.token_range(init);
        range.len() >= 2
            && c.lexeme(range.start) == "="
            && is_identifier_token(c, range.start + 1)
            && (range.start + 2..range.end).all(|t| c.lexeme(t) == ";")
    }) {
        return;
    }
    candidates.push(LeakCandidate {
        row: var,
        name: c.lexeme(name),
        watched,
        field,
        declaration: Some(var),
    });
}

/// Whether the written type spelling names a watched library type: the
/// spelling matches a predicate entry and no same-file declaration owns
/// the name (a user class of a library name is not the library type — the
/// element-resolution posture this face stands in for). An extension type
/// conforming to a watched type through its `implements` clause is
/// watched too (`implementsInterface` folds to the same-file clause read
/// — `extension type E(…) implements StreamSubscription` spells the
/// interface in the header). The extension type resolves through its own
/// top-level scan — the class-like index does not carry the kind.
fn leak_watched(c: &Check<'_>, spelling: &str, predicates: &LeakPredicates) -> bool {
    if predicates.types.contains(&spelling) && same_file_declaration(c, spelling).is_none() {
        return true;
    }
    extension_type_implements(c, spelling, predicates)
}

/// Whether `spelling` names a same-file `ExtensionTypeDeclaration` whose
/// `implements` clause names a watched library type (the upstream
/// `implementsInterface` conformance face, folded to the header's
/// written `implements` `Type` rows).
fn extension_type_implements(c: &Check<'_>, spelling: &str, predicates: &LeakPredicates) -> bool {
    let Some(root) = c.unit.ast.root() else {
        return false;
    };
    for top in c.collect_direct(root) {
        for kid in c.collect_direct(top) {
            if c.kind(kid) != NodeKind::ExtensionTypeDeclaration {
                continue;
            }
            // The extension-type name is the header's `Name` word — the
            // `begin_extension_type_declaration` name anchor.
            let names = c
                .field(kid, PayloadField::Name)
                .filter(|&token| token != crate::rules::NONE_TOKEN)
                .map(|token| c.lexeme(token));
            if names != Some(spelling) {
                continue;
            }
            return crate::rules::supertypes(c, kid)
                .iter()
                .any(|&(name, clause)| {
                    clause == crate::rules::SupertypeClause::Implements
                        && predicates.types.contains(&name)
                        && same_file_declaration(c, name).is_none()
                });
        }
    }
    false
}

/// Whether the container holds a valid use of the candidate
/// (`_ValidUseVisitor`, leak_detector_visitor.dart:164-310): a release
/// call on the variable, an escape (return, assignment, argument), or one
/// of the field faces. The walk covers the container's whole subtree (the
/// completion window holds exactly its rows).
fn leak_valid_use(
    c: &Check<'_>,
    container: u32,
    candidate: &LeakCandidate<'_>,
    predicates: &LeakPredicates,
) -> bool {
    let window = c.children(container);
    for row in window.start..=container {
        match c.kind(row) {
            NodeKind::Send => {
                if leak_send_use(c, row, candidate, predicates)
                    || leak_bare_argument_use(c, row, candidate)
                {
                    return true;
                }
            }
            NodeKind::Cascade => {
                if leak_cascade_use(c, row, candidate, predicates) {
                    return true;
                }
            }
            NodeKind::DotAccess => {
                if leak_tear_off_use(c, row, candidate, predicates) {
                    return true;
                }
            }
            NodeKind::Assignment => {
                if leak_assignment_use(c, row, candidate) {
                    return true;
                }
            }
            // `visitConstructorFieldInitializer` (:222-228): the field is
            // initialized in some constructor's initializer list.
            NodeKind::FieldInitializer if candidate.field => {
                if c
                    .field(row, PayloadField::Name)
                    .is_some_and(|name| name != NONE_TOKEN && c.lexeme(name) == candidate.name)
                {
                    return true;
                }
            }
            // `visitFieldFormalParameter` (:231-241): `this.name` declares
            // or assigns the field (fields only, :232).
            NodeKind::FormalParameter if candidate.field => {
                if c
                    .field(row, PayloadField::Receiver)
                    .is_some_and(|kw| kw != NONE_TOKEN && c.lexeme(kw) == "this")
                    && c
                        .field(row, PayloadField::Name)
                        .is_some_and(|name| name != NONE_TOKEN && c.lexeme(name) == candidate.name)
                {
                    return true;
                }
            }
            // `visitReturnStatement` (:300-310): the variable escapes the
            // function (locals only, :301).
            NodeKind::ReturnStatement if !candidate.field && leak_return_use(c, row, candidate) => {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// The release-call face (:244-261): a send whose selector names a release
/// and whose receiver is the variable — bare (`s.cancel()`), through
/// `this` (`this._sink.close()`), or bang-postfixed (`s!.cancel()`); or
/// whose invocation sits inside the candidate's own declaration subtree
/// (:257 — `var s = stream.listen((_) {})..cancel()`, where the cascade
/// section's invocation is the initializer).
fn leak_send_use(
    c: &Check<'_>,
    send: u32,
    candidate: &LeakCandidate<'_>,
    predicates: &LeakPredicates,
) -> bool {
    let Some(parent) = c.unit.ast.parent(send) else {
        return false;
    };
    let kids = c.collect_direct(parent);
    let Some(pos) = kids.iter().position(|&kid| kid == send) else {
        return false;
    };
    let Some((head, _arguments)) = slice::send_parts(c, &kids, pos) else {
        return false;
    };
    let marker = slice::access_marker(c, &kids, pos, NodeKind::DotAccess);
    // The selector: a member send reads the access marker's name; a bare
    // send's head identifier is the selector itself (no receiver — only
    // the declaration-subtree face can answer).
    let selector = match marker {
        Some(marker) => marker_dot_name(c, marker),
        None => kids.get(head).map(|&row| c.head_token(row)),
    };
    let Some(selector) = selector else {
        return false;
    };
    if !predicates.methods.contains(&c.lexeme(selector)) {
        return false;
    }
    if marker.is_some() {
        // A `!` on the receiver (`_s!.cancel()`) is transparent — the
        // postfix-operand face (`_isPostfixExpressionOperandEqualToVariable`,
        // leak_detector_visitor.dart:37-44): the `!` row tiles right before
        // the selector, so the receiver is the run before it.
        let window = match kids[..head].last() {
            Some(&bang) if c.kind(bang) == NodeKind::NonNullAssert => &kids[..head - 1],
            _ => &kids[..=head],
        };
        let receiver = slice::access_run(c, window);
        let mut names = receiver
            .iter()
            .filter(|&&row| c.kind(row) == NodeKind::Identifier)
            .map(|&row| c.lexeme(c.head_token(row)));
        let matches_candidate = match (names.next(), names.next()) {
            (Some(only), None) => only == candidate.name,
            _ => false,
        };
        if matches_candidate {
            return true;
        }
    }
    // The in-initializer face (:257).
    if let Some(declaration) = candidate.declaration {
        let declaration_span = c.byte_span(declaration);
        let send_span = c.byte_span(send);
        if declaration_span.start <= send_span.start && send_span.end <= declaration_span.end {
            return true;
        }
    }
    false
}

/// The argument-escape suppression (:268-280): any invocation holding the
/// variable as a direct bare argument suppresses — the callee may invoke
/// the release internally, and its body may be unavailable. The group face:
/// every piece is the bare identifier, an argument-list row, or a send
/// row (nested bare arguments include — the upstream suppresses through
/// the inner invocation's own argument face).
fn leak_bare_argument_use(c: &Check<'_>, send: u32, candidate: &LeakCandidate<'_>) -> bool {
    let Some(parent) = c.unit.ast.parent(send) else {
        return false;
    };
    let kids = c.collect_direct(parent);
    let Some(pos) = kids.iter().position(|&kid| kid == send) else {
        return false;
    };
    let Some((_head, Some(arguments))) = slice::send_parts(c, &kids, pos) else {
        return false;
    };
    let pieces = c.collect_direct(arguments);
    for group in slice::arguments(c, &pieces) {
        let rows = &pieces[group];
        let bare = rows.iter().all(|&row| {
            matches!(
                c.kind(row),
                NodeKind::Identifier
                    | NodeKind::Arguments
                    | NodeKind::Send
                    | NodeKind::NoTypeArguments
                    | NodeKind::TypeArguments
                    | NodeKind::NoArguments
            )
        });
        if bare
            && rows.iter().any(|&row| {
                c.kind(row) == NodeKind::Identifier && c.lexeme(c.head_token(row)) == candidate.name
            })
        {
            return true;
        }
    }
    false
}

/// The cascade face (`_isInvocationThroughCascadeExpression`, :22-35): the
/// cascade target is the bare variable and some section selects a release
/// (`s..cancel()` — every section's receiver is the cascade target). The
/// production tiling keeps the target send OUTSIDE the `Cascade` row — the
/// row opens at the first `..` marker, so the target is the token directly
/// before it; the release scan reads selector tokens (a release lexeme
/// directly after a section dot) over the cascade's own token range.
fn leak_cascade_use(
    c: &Check<'_>,
    cascade: u32,
    candidate: &LeakCandidate<'_>,
    predicates: &LeakPredicates,
) -> bool {
    let range = c.unit.ast.token_range(cascade);
    let Some(marker) = range
        .clone()
        .find(|&token| matches!(c.lexeme(token), ".." | "?.."))
    else {
        return false;
    };
    // The target is the single name token directly before the marker (the
    // row opens at the marker, so `marker - 1` reads the target — a bare
    // name is exactly one token). A dotted tail (`obj.s..cancel()`) is a
    // `PropertyAccess` target upstream, not a `SimpleIdentifier` (:26).
    if marker < 2
        || c.lexeme(marker - 1) != candidate.name
        || matches!(c.lexeme(marker - 2), "." | "?." | "..")
    {
        return false;
    }
    range.clone().any(|token| {
        token > 0
            && predicates.methods.contains(&c.lexeme(token))
            && matches!(c.lexeme(token - 1), "." | "?." | "..")
    })
}

/// The tear-off face (`visitPrefixedIdentifier`, :286-297): a whole-access
/// marker whose selector names a release over the bare variable
/// (`s.cancel;` — counting a tear-off as a release is the upstream
/// behavior, leak_detector_visitor.dart:78-79). `this.s.cancel` counts too:
/// upstream gates on the two-part `PrefixedIdentifier` only, so the
/// property tear-off matching here is a documented name-level
/// approximation.
fn leak_tear_off_use(
    c: &Check<'_>,
    marker: u32,
    candidate: &LeakCandidate<'_>,
    predicates: &LeakPredicates,
) -> bool {
    let Some(dot) = c.field(marker, PayloadField::Operator) else {
        return false;
    };
    if dot
        .checked_add(1)
        .is_none_or(|selector| !predicates.methods.contains(&c.lexeme(selector)))
    {
        return false;
    }
    // The bare receiver: one identifier token directly before the dot.
    dot.checked_sub(1)
        .is_some_and(|receiver| c.lexeme(receiver) == candidate.name)
}

/// The assignment faces (`visitAssignmentExpression`, :192-219): gated on
/// `rightHandSide is SimpleIdentifier` (:195), the variable is written
/// another reference (`s = otherRef`, :197-200), a like-named property is
/// written from a reference (`obj.s = ref`, :201-207), or the variable
/// escapes into an unresolvable write (`x = s` with `writeElement == null`,
/// :208-217). The token face reads both sides off the operator payload
/// word.
fn leak_assignment_use(c: &Check<'_>, assignment: u32, candidate: &LeakCandidate<'_>) -> bool {
    let range = c.unit.ast.token_range(assignment);
    let Some(op) = c.field(assignment, PayloadField::Operator) else {
        return false;
    };
    if op == NONE_TOKEN || op >= range.end {
        return false;
    }
    // The lhs tiles before the operator and may sit outside the row's own
    // range (a `s = other` constructor-initializer's assignment opens at the
    // rhs): read the target tokens back from `op`. A bare name is exactly
    // one token not preceded by an access dot; a property write names the
    // field after `.`/`?.`.
    let lhs_bare = op > 0
        && c.lexeme(op - 1) == candidate.name
        && !(op > 1 && matches!(c.lexeme(op - 2), "." | "?."));
    let lhs_prop = op > 1 && matches!(c.lexeme(op - 2), "." | "?.");
    let lhs_written = lhs_prop && c.lexeme(op - 1) == candidate.name;
    let rhs = op + 1..range.end;
    // The rhs is a bare identifier exactly — the `SimpleIdentifier` gate
    // (:195) admits `= s`/`= other` (optionally with the statement's `;`
    // inside the row range) and rejects `= compute()`/`= f().x`.
    let rhs_bare = !rhs.is_empty()
        && is_identifier_token(c, rhs.start)
        && rhs.clone().skip(1).all(|t| c.lexeme(t) == ";");
    if !rhs_bare {
        return false;
    }
    let rhs_is_candidate = c.lexeme(rhs.start) == candidate.name;
    // The third arm (:208-217) approximates `writeElement == null`: the
    // variable on the rhs of a write the flat face cannot resolve (a bare
    // `x`, `obj.x`, `this.f` — any lhs that is not the candidate itself)
    // escapes.
    (lhs_bare || lhs_written) || (rhs_is_candidate && !lhs_bare)
}

/// The return-escape face (`visitReturnStatement`, :300-310, locals): the
/// returned expression is the bare variable (`return s;` — the exact
/// upstream shape; a wrapped or invoked form is no `SimpleIdentifier`).
/// The bare read tiles as the tear-off group's `Send` — the name is the
/// group's leading `Identifier`.
fn leak_return_use(c: &Check<'_>, statement: u32, candidate: &LeakCandidate<'_>) -> bool {
    let window = c.children(statement);
    if window.start >= window.end {
        return false;
    }
    let root = window.end - 1;
    let name = match c.kind(root) {
        NodeKind::Identifier => c.lexeme(c.head_token(root)),
        NodeKind::Send => {
            let Some(start) = root.checked_sub(3) else {
                return false;
            };
            if start < window.start
                || !slice::is_tear_off_group(c, &[start, start + 1, start + 2, root], 0)
            {
                return false;
            }
            let head = c.head_token(start);
            if head == NONE_TOKEN {
                return false;
            }
            c.lexeme(head)
        }
        _ => return false,
    };
    name == candidate.name
}

/// The selector a whole-access marker names (the `Operator` word is the
/// dot; the name follows it).
fn marker_dot_name(c: &Check<'_>, marker: u32) -> Option<u32> {
    c.field(marker, PayloadField::Operator).map(|dot| dot + 1)
}

/// The head token of a local declaration's written type (the face
/// declarations.rs `local_written_type` reads): the token after the
/// `var`/`final`/`const` keyword, else the bounded backward walk over the
/// type's own tokens (`StreamSubscription<int>?`) stopped by the first
/// foreign token.
fn local_written_type_head(c: &Check<'_>, row: u32) -> Option<u32> {
    let head = c.head_token(row);
    if let Some(keyword) = variables_declaration_keyword(c, row) {
        (head > keyword + 1).then_some(keyword + 1)
    } else {
        let mut start = head;
        while let Some(prev) = start.checked_sub(1) {
            if matches!(c.lexeme(prev), "." | "?" | "<" | ">" | ">>")
                || is_identifier_token(c, prev)
            {
                start = prev;
            } else {
                break;
            }
        }
        (start < head).then_some(start)
    }
}

/// The formal parameter rows of a constructor or primary constructor: the
/// required group's rows plus the optional group's, in source order.
/// The formal-parameter rows under `owner` — descending through a
/// `FormalParameters` child when `owner` is the declaration (constructor,
/// primary constructor, method) rather than the parameter list itself, and
/// through `OptionalFormalParameters` sections inside it.
/// The formal-parameter rows under `owner` — descending through a
/// `FormalParameters` child when `owner` is the declaration (constructor,
/// primary constructor, method) rather than the parameter list itself, and
/// through `OptionalFormalParameters` sections inside it.
pub(crate) fn leak_formal_rows(c: &Check<'_>, owner: u32) -> Vec<u32> {
    let mut rows = Vec::new();
    let mut stack = c.collect_direct(owner);
    while let Some(kid) = stack.pop() {
        match c.kind(kid) {
            NodeKind::FormalParameter => rows.push(kid),
            NodeKind::FormalParameters | NodeKind::OptionalFormalParameters => {
                stack.extend(c.collect_direct(kid))
            }
            _ => {}
        }
    }
    rows.reverse();
    rows
}

/// Whether a primary-constructor formal declares its field — the
/// `isDeclaring` face (`finalOrVarKeyword != null`, element_builder.dart
/// :2091): a `var`/`final` keyword heads the formal. `const` never declares
/// (the parser's `VAR_FINAL_OR_CONST` bit covers it, so the keyword lexeme
/// decides); a `this.`/`super.` initializing formal and a plain formal
/// stay non-declaring.
fn formal_declaring(c: &Check<'_>, param: u32) -> bool {
    c.unit
        .ast
        .token_range(param)
        .into_iter()
        .find(|&token| matches!(c.lexeme(token), "var" | "final" | "const"))
        .is_some_and(|token| matches!(c.lexeme(token), "var" | "final"))
}

/// Whether the token is a plain identifier.
fn is_identifier_token(c: &Check<'_>, token: u32) -> bool {
    c.unit
        .tokens
        .get(token as usize)
        .is_some_and(|t| matches!(t.kind, dartq_scanner::TokenKind::Identifier))
}

pub(crate) mod cancel_subscriptions {
    use super::{CANCEL_PREDICATES, Check, NodeKind, RuleSpec, leak_detector_check};

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/cancel_subscriptions.dart` — a
    /// `StreamSubscription`-typed field, local, or primary-constructor
    /// field formal with no `cancel` release in its container reports at
    /// the declaration (the shared leak-detector face).
    ///
    /// Approximation (documented): the interface question folds to the
    /// written-type spelling with the same-file shadow gate — `var`-typed
    /// (inferred) candidates, prefixed spellings, and subtype implementors
    /// under-report, and a user class of the name stays silent through the
    /// shadow gate; an extension type conforming through its `implements`
    /// clause is watched (the header spells the interface). Message:
    /// `diagnostic.g.dart` `cancelSubscriptions`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "cancel_subscriptions",
    &[
            NodeKind::Fields,
            NodeKind::VariablesDeclaration,
            NodeKind::PrimaryConstructor,
        ],
    check,
);

    fn check(c: &mut Check<'_>) {
        leak_detector_check(c, &CANCEL_PREDICATES);
    }
}

pub(crate) mod close_sinks {
    use super::{CLOSE_PREDICATES, Check, NodeKind, RuleSpec, leak_detector_check};

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/close_sinks.dart` — a `Sink`-typed (or
    /// `IOSink`/`Socket`) field, local, or primary-constructor field
    /// formal with no `close`/`destroy` release in its container reports
    /// at the declaration (the shared leak-detector face; the
    /// approximation register is the shared module's). The field and
    /// primary faces stay unmounted (the shared face's `field_faces`
    /// gate): the differential gate's live oracle (fvm 3.13.1) answers
    /// this rule local-only there, the documented version drift the
    /// lints-new corpus pins. Message: `diagnostic.g.dart` `closeSinks`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "close_sinks",
    &[
            NodeKind::Fields,
            NodeKind::VariablesDeclaration,
            NodeKind::PrimaryConstructor,
        ],
    check,
);

    fn check(c: &mut Check<'_>) {
        leak_detector_check(c, &CLOSE_PREDICATES);
    }
}

pub(crate) mod tighten_type_of_initializing_formals {
    use super::{PayloadField, Check, NodeKind, RuleSpec, leak_formal_rows};
    use crate::rules::{NONE_TOKEN, TypeFace, body_members, initialized_identifier_name, same_file_ancestors, slice, type_face, variables_declaration_keyword};

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/tighten_type_of_initializing_formals.dart`
    /// — an `assert(x != null)` / `assert(null != x)` initializer whose
    /// `x` is nullable reports at the formals it could tighten: a `this.p`
    /// field formal whose field is separately declared
    /// (`!element.isDeclaring`, :61) and a `super.a` super formal (:62) —
    /// over both constructors (:43-48) and primary constructors
    /// (:51-58, initializers from the paired body).
    ///
    /// Approximation (documented): `typeSystem.isNullable` folds to the
    /// written faces — the checked identifier's static type is its
    /// formal's written type when one is spelled (a non-nullable written
    /// type silences, the `test_thisInit_tightens` shape), else the
    /// same-file field's written type (the field formal's field, the
    /// super formal's first same-file ancestor field — `test_superInit`'s
    /// untyped `super.a` is nullable through A's `String? a`), with a `?`
    /// tail, a `dynamic` spelling, and an untyped `var` field with no
    /// initializer (`dynamic`) reading nullable. A cross-file field, an
    /// inferred type, and an unresolvable ancestor under-report. The
    /// element identity of `x` folds to the constructor's own formal names
    /// (the initializer-list scope); a same-named field with no matching
    /// formal stays silent exactly as upstream (a `FieldElement` is
    /// neither face, :60-67), and a plain formal silences the same way.
    /// The condition must be a bare binary (`condition is!
    /// BinaryExpression`, :77 — a parenthesized condition skips).
    /// Message: `diagnostic.g.dart` `tightenTypeOfInitializingFormals`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "tighten_type_of_initializing_formals",
    &[NodeKind::Constructor, NodeKind::PrimaryConstructor],
    check,
);

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let Some(class_row) = c.path.iter().rev().skip(1).copied().find(|&row| {
            matches!(
                c.kind(row),
                NodeKind::ClassDeclaration
                    | NodeKind::EnumDeclaration
                    | NodeKind::ExtensionTypeDeclaration
            )
        }) else {
            return;
        };
        // The initializer list: the constructor's own, or the paired
        // primary-constructor body's (`node.body?.initializers`, :52).
        let initializers = match c.kind(node) {
            NodeKind::Constructor => c
                .collect_direct(node)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Initializers),
            _ => body_members(c, class_row)
                .into_iter()
                .find(|&member| c.kind(member) == NodeKind::PrimaryConstructorBody)
                .and_then(|body| {
                    c.collect_direct(body)
                        .into_iter()
                        .find(|&kid| c.kind(kid) == NodeKind::Initializers)
                }),
        };
        let Some(initializers) = initializers else {
            return;
        };
        let parameters = leak_formal_rows(c, node);
        for entry in c.collect_direct(initializers) {
            let Some(assert_row) = c
                .collect_direct(entry)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Assert)
            else {
                continue;
            };
            check_assert(c, assert_row, class_row, &parameters);
        }
    }

    /// One assert initializer's tightening verdict (:73-100): the
    /// `x != null` / `null != x` condition, the checked operand's
    /// parameter, the nullable face, and the report at the formal.
    fn check_assert(c: &mut Check<'_>, assert_row: u32, class_row: u32, parameters: &[u32]) {
        let Some(condition) = c
            .collect_direct(assert_row)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::BinaryExpression)
        else {
            return;
        };
        if c
            .field(condition, PayloadField::Operator)
            .is_none_or(|op| c.lexeme(op) != "!=")
        {
            return;
        }
        let operands = c.collect_direct(condition);
        let (Some(first), Some(second)) = (operands.first(), operands.get(1)) else {
            return;
        };
        let checked = if c.kind(*second) == NodeKind::LiteralNull {
            Some(*first)
        } else if c.kind(*first) == NodeKind::LiteralNull {
            Some(*second)
        } else {
            None
        };
        let Some(checked) = checked else {
            return;
        };
        // A bare name read tiles as a `Send` closing the tear-off group
        // `[Identifier, NoTypeArguments, NoArguments, Send]` — the group's
        // pieces are the three rows right before the send (they scatter
        // across the expression's ancestors rather than nesting inside it,
        // so the read is row-positional, not structural). Upstream the
        // operand face is `is Identifier` (:82/:91): a call `f() != null`
        // closes in `Arguments` instead of `NoArguments` and skips.
        let name = match c.kind(checked) {
            NodeKind::Identifier => c.lexeme(c.head_token(checked)),
            NodeKind::Send => {
                let Some(start) = checked.checked_sub(3) else {
                    return;
                };
                if !slice::is_tear_off_group(c, &[start, start + 1, start + 2, checked], 0) {
                    return;
                }
                let head = c.head_token(start);
                if head == NONE_TOKEN {
                    return;
                }
                c.lexeme(head)
            }
            _ => return,
        };
        // The identifier names one of the constructor's own formals (the
        // initializer-list scope); a bare field reference or a plain
        // formal is neither face and stays silent (:60-67).
        let Some(param) = parameters.iter().copied().find(|&param| {
            c.field(param, PayloadField::Name)
                .is_some_and(|name_token| name_token != NONE_TOKEN && c.lexeme(name_token) == name)
        }) else {
            return;
        };
        let receiver = c
            .field(param, PayloadField::Receiver)
            .unwrap_or(NONE_TOKEN);
        if receiver == NONE_TOKEN {
            return;
        }
        // The static type: the formal's written type when spelled, else
        // the field's declared type (the super formal reads the first
        // same-file ancestor's field).
        let written = c
            .collect_direct(param)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::Type);
        let nullable = match c.lexeme(receiver) {
            "this" => {
                // `!element.isDeclaring` (:61): the field must be
                // separately declared — a declaring primary formal skips.
                let Some(field) = class_field_nullable(c, class_row, name) else {
                    return;
                };
                Some(written.map_or(field, |row| type_nullable(&type_face(c, row))))
            }
            "super" => Some(written.map_or(
                ancestor_field_nullable(c, class_row, name).unwrap_or(false),
                |row| type_nullable(&type_face(c, row)),
            )),
            _ => None,
        };
        if nullable == Some(true) {
            c.report(c.utf16_span(param), &[]);
        }
    }

    /// The written-nullable face of a type row: a `?` tail or a `dynamic`
    /// spelling (the written faces of `typeSystem.isNullable`).
    fn type_nullable(face: &TypeFace<'_>) -> bool {
        face.nullable || face.name == Some("dynamic")
    }

    /// The written-nullable verdict of the class-like's own field `name`:
    /// its type row's face, or the untyped `var` face (no initializer —
    /// the `dynamic` declaration). `None` when no such field is declared.
    fn class_field_nullable(c: &Check<'_>, class_row: u32, name: &str) -> Option<bool> {
        for member in body_members(c, class_row) {
            if c.kind(member) != NodeKind::Fields {
                continue;
            }
            let type_row = c
                .collect_direct(member)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Type);
            for decl in c.collect_direct(member) {
                if c.kind(decl) != NodeKind::VariablesDeclaration {
                    continue;
                }
                for var in c.collect_direct(decl) {
                    if c.kind(var) != NodeKind::InitializedIdentifier {
                        continue;
                    }
                    let Some(token) = initialized_identifier_name(c, var) else {
                        continue;
                    };
                    if c.lexeme(token) != name {
                        continue;
                    }
                    return Some(match type_row {
                        Some(row) => type_nullable(&type_face(c, row)),
                        None => {
                            variables_declaration_keyword(c, decl)
                                .is_some_and(|kw| c.lexeme(kw) == "var")
                                && c.collect_direct(var)
                                    .into_iter()
                                    .all(|kid| c.kind(kid) != NodeKind::VariableInitializer)
                        }
                    });
                }
            }
        }
        None
    }

    /// The first same-file ancestor declaring a nullable verdict for the
    /// field `name` (the super formal's static type reads the inherited
    /// field; a chain that leaves the file under-reports).
    fn ancestor_field_nullable(c: &Check<'_>, class_row: u32, name: &str) -> Option<bool> {
        same_file_ancestors(c, class_row, true)
            .into_iter()
            .find_map(|ancestor| class_field_nullable(c, ancestor, name))
    }
}

#[cfg(test)]
mod wave1_types_tests {
    use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

    // —— prefer_void_to_null ——————————————————————————————————————————————

    /// `test/rules/prefer_void_to_null_test.dart` — a `Null`-typed formal
    /// reports at the type.
    #[test]
    fn void_to_null_formal_reports() {
        let f = Fixture::parse("void f(Null x) {}\n");
        let lints = f.run(&one_rule("prefer_void_to_null"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Unnecessary use of the type 'Null'.");
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try using 'void' instead.")
        );
        let null = tok(&f.source, &f.tokens, "Null", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, null));
    }

    /// A `Null` method return type reports.
    #[test]
    fn void_to_null_method_return_reports() {
        let f = Fixture::parse("class C {\n  Null f() => throw 0;\n}\n");
        assert_eq!(f.run(&one_rule("prefer_void_to_null")).len(), 1);
    }

    /// A `Null` field reports; local and top-level variables stay silent
    /// (`test_localVariable`).
    #[test]
    fn void_to_null_variable_faces() {
        let f = Fixture::parse("class C {\n  Null x;\n}\n");
        assert_eq!(f.run(&one_rule("prefer_void_to_null")).len(), 1);

        let f = Fixture::parse("void f() {\n  Null x;\n}\n");
        assert!(f.run(&one_rule("prefer_void_to_null")).is_empty());

        let f = Fixture::parse("Null x;\n");
        assert!(f.run(&one_rule("prefer_void_to_null")).is_empty());
    }

    /// `test_castAsExpression` — `as` and cast-pattern positions stay
    /// silent; `is` reports.
    #[test]
    fn void_to_null_as_is_faces() {
        let f = Fixture::parse("void f(Object a) {\n  a as Null;\n}\n");
        assert!(f.run(&one_rule("prefer_void_to_null")).is_empty());

        let f = Fixture::parse("void f(Object a) {\n  if (a is Null) {}\n}\n");
        let lints = f.run(&one_rule("prefer_void_to_null"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let null = tok(&f.source, &f.tokens, "Null", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, null));
    }

    /// `test_listLiteralTypeArg_null_empty` /
    /// `test_listLiteralTypeArg_null_nonEmpty` — the empty literal's
    /// argument stays silent, the non-empty literal's reports.
    #[test]
    fn void_to_null_empty_literal_arguments() {
        let f = Fixture::parse("var l = <Null>[];\n");
        assert!(f.run(&one_rule("prefer_void_to_null")).is_empty());

        let f = Fixture::parse("var l = <Null>[null];\n");
        assert_eq!(f.run(&one_rule("prefer_void_to_null")).len(), 1);
    }

    /// `test_extension` — the on-clause type stays silent.
    #[test]
    fn void_to_null_extension_on_clause_stays_silent() {
        let f = Fixture::parse("extension E on Null {}\n");
        assert!(f.run(&one_rule("prefer_void_to_null")).is_empty());
    }

    /// `Null Function()` and `Function(Null)` stay silent.
    #[test]
    fn void_to_null_function_type_faces_stay_silent() {
        let f = Fixture::parse("Null Function() f = throw 0;\n");
        assert!(f.run(&one_rule("prefer_void_to_null")).is_empty());

        let f = Fixture::parse("void g(void Function(Null) cb) {}\n");
        assert!(f.run(&one_rule("prefer_void_to_null")).is_empty());
    }

    /// `test_extensionTypeRepresentation` — the representation formal stays
    /// silent.
    #[test]
    fn void_to_null_extension_type_representation_stays_silent() {
        let f = Fixture::parse("extension type E(Null _) {}\n");
        assert!(f.run(&one_rule("prefer_void_to_null")).is_empty());
    }

    /// The void-incompatible-override face: an overridden member whose
    /// return type is neither `void` nor `FutureOr<void>` silences; a
    /// `void`-returning override keeps the report.
    #[test]
    fn void_to_null_override_faces() {
        let f = Fixture::parse(
            "class A {\n  int f() => 0;\n}\nclass B extends A {\n  Null f() => throw 0;\n}\n",
        );
        assert!(f.run(&one_rule("prefer_void_to_null")).is_empty());

        let f = Fixture::parse(
            "class A {\n  void f() {}\n}\nclass B extends A {\n  Null f() => throw 0;\n}\n",
        );
        assert_eq!(f.run(&one_rule("prefer_void_to_null")).len(), 1);
    }

    /// The override face over a generic method: `Null f<T>()` overriding a
    /// non-void `f` stays exempt (the member name binds past the
    /// `TypeVariables` row), while overriding a `void` member still
    /// reports.
    #[test]
    fn void_to_null_generic_override_faces() {
        let f = Fixture::parse(
            "class A {\n  int f() => 0;\n}\nclass B extends A {\n  Null f<T>() => throw 0;\n}\n",
        );
        assert!(f.run(&one_rule("prefer_void_to_null")).is_empty());

        let f = Fixture::parse(
            "class A {\n  void f() {}\n}\nclass B extends A {\n  Null f<T>() => throw 0;\n}\n",
        );
        assert_eq!(f.run(&one_rule("prefer_void_to_null")).len(), 1);
    }

    /// `test_augmentedTopLevelGetter` — an augmented declaration stays
    /// silent.
    #[test]
    fn void_to_null_augmentation_stays_silent() {
        let f = Fixture::parse("augment Null f() => throw 0;\n");
        assert!(f.run(&one_rule("prefer_void_to_null")).is_empty());
    }

    // —— no_raw_types —————————————————————————————————————————————————————

    /// A raw core generic reports with the instantiation display.
    #[test]
    fn no_raw_types_core_generic_reports() {
        let f = Fixture::parse("List x = throw 0;\n");
        let lints = f.run(&one_rule("no_raw_types"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The generic type 'List<dynamic>' should have explicit type arguments but doesn't."
        );
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Use explicit type arguments for 'List<dynamic>'.")
        );
        let list = tok(&f.source, &f.tokens, "List", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, list));

        let f = Fixture::parse("Map m = throw 0;\n");
        let lints = f.run(&one_rule("no_raw_types"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The generic type 'Map<dynamic, dynamic>' should have explicit type arguments but doesn't."
        );
    }

    /// Explicit arguments and non-generic names stay silent
    /// (`test_localVariable_withTypeArg`).
    #[test]
    fn no_raw_types_typed_and_plain_stay_silent() {
        let f = Fixture::parse("List<int> x = throw 0;\n");
        assert!(f.run(&one_rule("no_raw_types")).is_empty());

        let f = Fixture::parse("int x = 0;\n");
        assert!(f.run(&one_rule("no_raw_types")).is_empty());
    }

    /// `test_localVariable_missingTypeArg` — a same-file generic reports
    /// with its written instantiation.
    #[test]
    fn no_raw_types_same_file_generic_reports() {
        let f = Fixture::parse("class Foo<T> {}\n\nFoo f = throw 0;\n");
        let lints = f.run(&one_rule("no_raw_types"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The generic type 'Foo<dynamic>' should have explicit type arguments but doesn't."
        );
    }

    /// A same-file generic whose variables are all bounded stays silent —
    /// the raw instantiation fills the bounds, not `dynamic`.
    #[test]
    fn no_raw_types_bounded_fill_stays_silent() {
        let f = Fixture::parse("class Foo<T extends num> {}\n\nFoo f = throw 0;\n");
        assert!(f.run(&one_rule("no_raw_types")).is_empty());
    }

    /// A same-file non-generic class stays silent.
    #[test]
    fn no_raw_types_same_file_plain_class_stays_silent() {
        let f = Fixture::parse("class Bar {}\n\nBar b = throw 0;\n");
        assert!(f.run(&one_rule("no_raw_types")).is_empty());
    }

    /// `test_isExpression` / `test_asExpression` — the strict-raw-type
    /// positions stay silent; a raw type argument reports.
    #[test]
    fn no_raw_types_escaping_faces() {
        let f = Fixture::parse("void f(Object o) {\n  if (o is List) {}\n}\n");
        assert!(f.run(&one_rule("no_raw_types")).is_empty());

        let f = Fixture::parse("void f(Object o) {\n  o as Map;\n}\n");
        assert!(f.run(&one_rule("no_raw_types")).is_empty());

        let f = Fixture::parse("var l = <List>[];\n");
        assert_eq!(f.run(&one_rule("no_raw_types")).len(), 1);
    }

    /// `test_functionParts_optionalTypeArg` — an `@optionalTypeArgs`
    /// declaration stays silent.
    #[test]
    fn no_raw_types_optional_type_args_stay_silent() {
        let base = "@optionalTypeArgs\nclass F<T> {}\n\nF f = throw 0;\n";
        let f = Fixture::parse(base);
        assert!(f.run(&one_rule("no_raw_types")).is_empty());

        let f = Fixture::parse("class F<T> {}\n\nF f = throw 0;\n");
        assert_eq!(f.run(&one_rule("no_raw_types")).len(), 1);
    }

    // —— always_specify_types —————————————————————————————————————————————

    /// `var` with a single literal initializer carries the replace-keyword
    /// face at the keyword (`test_0` shape).
    #[test]
    fn specify_var_replaces_keyword() {
        let f = Fixture::parse("var x = 1;\n");
        let lints = f.run(&one_rule("always_specify_types"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Missing type annotation.");
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try replacing 'var' with 'int'.")
        );
        let var = tok(&f.source, &f.tokens, "var", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, var));
    }

    /// `final` with a single literal initializer carries the specify-type
    /// face; distinct initializer types split the declaration.
    #[test]
    fn specify_final_and_split_faces() {
        let f = Fixture::parse("final x = '';\n");
        let lints = f.run(&one_rule("always_specify_types"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try specifying the type 'String'.")
        );

        let f = Fixture::parse("var x = 1, y = '';\n");
        let lints = f.run(&one_rule("always_specify_types"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try splitting the declaration and specifying the different type annotations.")
        );

        // The same literal type twice stays single.
        let f = Fixture::parse("var x = 1, y = 2;\n");
        let lints = f.run(&one_rule("always_specify_types"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try replacing 'var' with 'int'.")
        );
    }

    /// An initializer-less or non-literal-initializer list carries the
    /// add-type face; a typed list stays silent.
    #[test]
    fn specify_add_type_fallback_faces() {
        let f = Fixture::parse("var x;\n");
        let lints = f.run(&one_rule("always_specify_types"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try adding a type annotation.")
        );

        let f = Fixture::parse("final x = 1, y = '';\n");
        let lints = f.run(&one_rule("always_specify_types"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try adding a type annotation.")
        );

        let f = Fixture::parse("int x = 0;\nfinal int y = 0;\n");
        assert!(f.run(&one_rule("always_specify_types")).is_empty());
    }

    /// The field face: `test_instanceField` — a `var` field with an
    /// initializer reports at the keyword.
    #[test]
    fn specify_field_face_reports() {
        let f = Fixture::parse("class C {\n  var x = 1;\n}\n");
        let lints = f.run(&one_rule("always_specify_types"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let var = tok(&f.source, &f.tokens, "var", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, var));
    }

    /// `test_constructorParameter_positional_omitted` and the keyword form
    /// — untyped formals report add-type at the keyword or the parameter;
    /// typed and just-underscore formals stay silent.
    #[test]
    fn specify_parameter_faces() {
        let f = Fixture::parse("void f(p) {}\n");
        let lints = f.run(&one_rule("always_specify_types"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let p = tok(&f.source, &f.tokens, "p", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, p));

        let f = Fixture::parse("void f(var p) {}\n");
        let lints = f.run(&one_rule("always_specify_types"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let var = tok(&f.source, &f.tokens, "var", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, var));

        let f = Fixture::parse("void f(int p, _) {}\n");
        assert!(f.run(&one_rule("always_specify_types")).is_empty());
    }

    /// A generic function's untyped formal reports at the parameter — the
    /// `TypeVariables` row between the name and the parameters must not
    /// mis-bind the name onto the return type (`int f<T>(p)` reads `f`, so
    /// the written return type leaves only `p` reportable).
    #[test]
    fn specify_generic_parameter_reports() {
        let f = Fixture::parse("int f<T>(p) => 0;\n");
        let lints = f.run(&one_rule("always_specify_types"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let p = tok(&f.source, &f.tokens, "p", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, p));

        // A method shape: the same mis-bind would report the return type.
        let f = Fixture::parse("class C {\n  int f<T>(p) => 0;\n}\n");
        let lints = f.run(&one_rule("always_specify_types"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let p = tok(&f.source, &f.tokens, "p", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, p));
    }

    /// The raw-generic named-type face: add-type at the annotation, `is`
    /// exempt, `as` reporting.
    #[test]
    fn specify_named_type_faces() {
        let f = Fixture::parse("List x = throw 0;\n");
        let lints = f.run(&one_rule("always_specify_types"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try adding a type annotation.")
        );

        let f = Fixture::parse("void f(Object o) {\n  if (o is List) {}\n}\n");
        assert!(f.run(&one_rule("always_specify_types")).is_empty());

        let f = Fixture::parse("void f(Object o) {\n  o as List;\n}\n");
        assert_eq!(f.run(&one_rule("always_specify_types")).len(), 1);
    }

    /// The for-in declared identifier: an untyped loop variable reports at
    /// its keyword, a typed one stays silent.
    #[test]
    fn specify_for_in_faces() {
        let f = Fixture::parse("void f(List<int> xs) {\n  for (var x in xs) {}\n}\n");
        let lints = f.run(&one_rule("always_specify_types"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let var = tok(&f.source, &f.tokens, "var", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, var));

        let f = Fixture::parse("void f(List<int> xs) {\n  for (final int x in xs) {}\n}\n");
        assert!(f.run(&one_rule("always_specify_types")).is_empty());
    }

    /// The C-style for initializer keeps the variable-list face.
    #[test]
    fn specify_c_style_for_initializer_reports() {
        let f = Fixture::parse("void f() {\n  for (var i = 0; i < 1; i++) {}\n}\n");
        let lints = f.run(&one_rule("always_specify_types"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try replacing 'var' with 'int'.")
        );
    }

    /// The declared variable pattern: an untyped pattern reports at its
    /// keyword, a typed one stays silent.
    #[test]
    fn specify_pattern_faces() {
        let f = Fixture::parse(
            "void f(Object o) {\n  switch (o) {\n    case var x:\n      break;\n  }\n}\n",
        );
        let lints = f.run(&one_rule("always_specify_types"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let var = tok(&f.source, &f.tokens, "var", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, var));

        let f = Fixture::parse(
            "void f(Object o) {\n  switch (o) {\n    case final int x:\n      break;\n  }\n}\n",
        );
        assert!(f.run(&one_rule("always_specify_types")).is_empty());
    }

    /// `test_catchVariable_omitted` — an untyped catch variable reports.
    #[test]
    fn specify_catch_variable_reports() {
        let f = Fixture::parse("void f() {\n  try {} catch (e) {}\n}\n");
        let lints = f.run(&one_rule("always_specify_types"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let e = tok(&f.source, &f.tokens, "e", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, e));
    }

    // —— avoid_futureor_void ——————————————————————————————————————————————

    /// A `FutureOr<void>` return type and a top-level variable report at
    /// the annotation.
    #[test]
    fn futureor_void_result_positions_report() {
        let f = Fixture::parse("FutureOr<void> f() => throw 0;\n");
        let lints = f.run(&one_rule("avoid_futureor_void"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Don't use the type 'FutureOr<void>'.");
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try using 'Future<void>?' or 'void'.")
        );
        let futureor = tok(&f.source, &f.tokens, "FutureOr", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, futureor));

        let f = Fixture::parse("FutureOr<void> x = throw 0;\n");
        assert_eq!(f.run(&one_rule("avoid_futureor_void")).len(), 1);

        let f = Fixture::parse("class C {\n  FutureOr<void> m() => throw 0;\n}\n");
        assert_eq!(f.run(&one_rule("avoid_futureor_void")).len(), 1);
    }

    /// The contravariant positions stay silent: a direct formal, a nested
    /// covariant-core argument, and a function-type formal's return.
    #[test]
    fn futureor_void_contravariant_positions_stay_silent() {
        let f = Fixture::parse("void f(FutureOr<void> x) {}\n");
        assert!(f.run(&one_rule("avoid_futureor_void")).is_empty());

        let f = Fixture::parse("void f(List<FutureOr<void>> x) {}\n");
        assert!(f.run(&one_rule("avoid_futureor_void")).is_empty());

        let f = Fixture::parse("void f(FutureOr<void> Function() g) {}\n");
        assert!(f.run(&one_rule("avoid_futureor_void")).is_empty());

        let f = Fixture::parse("class C {\n  C(FutureOr<void> x);\n}\n");
        assert!(f.run(&one_rule("avoid_futureor_void")).is_empty());
    }

    /// The variance continuation: a covariant return nests down to the
    /// `FutureOr<void>` argument; a contravariant formal nests silent; a
    /// function-typed formal's own parameters invert back to reporting.
    #[test]
    fn futureor_void_variance_continuation() {
        let f = Fixture::parse("List<FutureOr<void>> f() => throw 0;\n");
        let lints = f.run(&one_rule("avoid_futureor_void"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let futureor = tok(&f.source, &f.tokens, "FutureOr", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, futureor));

        let f = Fixture::parse("void f(void g(FutureOr<void> x)) {}\n");
        assert_eq!(f.run(&one_rule("avoid_futureor_void")).len(), 1);
    }

    /// The clause and bound faces: implements clauses and type-parameter
    /// bounds report; the mixin on-clause and extension on-clause report.
    #[test]
    fn futureor_void_clause_faces() {
        let f = Fixture::parse("class C implements FutureOr<void> {}\n");
        assert_eq!(f.run(&one_rule("avoid_futureor_void")).len(), 1);

        let f = Fixture::parse("class C<T extends FutureOr<void>> {}\n");
        assert_eq!(f.run(&one_rule("avoid_futureor_void")).len(), 1);

        let f = Fixture::parse("mixin M on FutureOr<void> {}\n");
        assert_eq!(f.run(&one_rule("avoid_futureor_void")).len(), 1);

        let f = Fixture::parse("extension E on FutureOr<void> {}\n");
        assert_eq!(f.run(&one_rule("avoid_futureor_void")).len(), 1);
    }

    /// The as/is and function-type faces: `FutureOr<void> Function()`
    /// variables report through the return type.
    #[test]
    fn futureor_void_expression_faces() {
        let f = Fixture::parse("void f(Object o) {\n  if (o is FutureOr<void>) {}\n}\n");
        assert_eq!(f.run(&one_rule("avoid_futureor_void")).len(), 1);

        let f = Fixture::parse("void f(Object o) {\n  o as FutureOr<void>;\n}\n");
        assert_eq!(f.run(&one_rule("avoid_futureor_void")).len(), 1);

        let f = Fixture::parse("FutureOr<void> Function() f = throw 0;\n");
        assert_eq!(f.run(&one_rule("avoid_futureor_void")).len(), 1);
    }

    // —— erase_dart_type_extension_types —————————————————————————————————

    /// `test/rules/erase_dart_type_extension_types_test.dart` — an `is`
    /// check against `Type` reports at the expression.
    #[test]
    fn erase_dart_type_is_type_reports() {
        let f = Fixture::parse("void f(Object o) {\n  if (o is Type) {}\n}\n");
        let lints = f.run(&one_rule("erase_dart_type_extension_types"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Unsafe use of 'DartType' in an 'is' check."
        );
    }

    /// The `is!` face reports too; other checked types stay silent.
    #[test]
    fn erase_dart_type_negative_and_other_types() {
        let f = Fixture::parse("void f(Object o) {\n  if (o is! Type) {}\n}\n");
        assert_eq!(f.run(&one_rule("erase_dart_type_extension_types")).len(), 1);

        let f = Fixture::parse("void f(Object o) {\n  if (o is int) {}\n}\n");
        assert!(
            f.run(&one_rule("erase_dart_type_extension_types"))
                .is_empty()
        );
    }

    // —— cancel_subscriptions ————————————————————————————————————————————

    /// Oracle-pinned (dart 3.13.1): a field cancelled through `!`
    /// (`_s!.cancel()`) is released, and a same-named field of another
    /// class never releases this one (upstream matches by element).
    #[test]
    fn cancel_subscriptions_bang_receiver_and_class_scoped_fields() {
        let f = Fixture::parse(
            "class A {\n  StreamSubscription? _s;\n  void g() { _s!.cancel(); }\n}\n",
        );
        assert!(f.run(&one_rule("cancel_subscriptions")).is_empty());
        let f = Fixture::parse(
            "class A {\n  StreamSubscription? _s;\n}\nclass B {\n  StreamSubscription? _s;\n  void g() { _s?.cancel(); }\n}\n",
        );
        assert_eq!(f.run(&one_rule("cancel_subscriptions")).len(), 1);
    }

    /// `test/rules/cancel_subscriptions_test.dart` — an uncancelled local
    /// and an uncancelled field each report at their declaration.
    #[test]
    fn cancel_subscriptions_local_and_field_report() {
        let f = Fixture::parse("void f() {\n  StreamSubscription s;\n}\n");
        let lints = f.run(&one_rule("cancel_subscriptions"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Uncancelled instance of 'StreamSubscription'."
        );

        let f = Fixture::parse("class C {\n  StreamSubscription s;\n}\n");
        let lints = f.run(&one_rule("cancel_subscriptions"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Uncancelled instance of 'StreamSubscription'."
        );
    }

    /// Production parse over the bridge shapes: the primary-constructor
    /// declaring-formal face and the field face in one file — both
    /// unclosed subscriptions report. The upstream gate
    /// (`element is FieldFormalParameterElement && element.isDeclaring`,
    /// leak_detector_visitor.dart:99) admits only the `var`/`final`
    /// declaring forms — a plain `String? p` formal and a `this.p`
    /// initializing formal are not declaring and stay silent.
    #[test]
    fn cancel_subscriptions_primary_formal_reports() {
        let f = Fixture::parse(
            "class C(var StreamSubscription s);\n\nclass D {\n  StreamSubscription t;\n}\n",
        );
        let lints = f.run(&one_rule("cancel_subscriptions"));
        assert_eq!(lints.len(), 2, "{lints:?}");

        let f = Fixture::parse("class C(final StreamSubscription s);\n");
        let lints = f.run(&one_rule("cancel_subscriptions"));
        assert_eq!(lints.len(), 1, "{lints:?}");

        // Non-declaring faces: a plain formal and a `this.` initializing
        // formal are no field candidates (:99).
        for source in ["class C(StreamSubscription s);\n", "class C(this.s);\n"] {
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("cancel_subscriptions"));
            assert!(lints.is_empty(), "{source:?} -> {lints:?}");
        }
    }

    /// Every valid use stays silent: a cancel send, a cascade release, a
    /// tear-off, a field initializer, a field formal, an assignment pair,
    /// an escape into an instance member, a bare-argument pass, a return,
    /// and a simple-identifier initializer alias.
    #[test]
    fn cancel_subscriptions_valid_uses_stay_silent() {
        let sources = [
            "void f() {\n  StreamSubscription s;\n  s.cancel();\n}\n",
            "void f() {\n  StreamSubscription s;\n  s..cancel();\n}\n",
            "void f() {\n  StreamSubscription s;\n  s.cancel;\n}\n",
            "class C {\n  StreamSubscription s;\n  C() : s = other;\n}\n",
            "class C {\n  StreamSubscription s;\n  C(this.s);\n}\n",
            "void f() {\n  StreamSubscription s;\n  s = other;\n}\n",
            "void f() {\n  StreamSubscription s;\n  obj.x = s;\n}\n",
            // The unresolvable-write escape (:208-217): a bare `x` lhs that
            // resolves to nothing counts the write as a use.
            "void f() {\n  StreamSubscription s;\n  x = s;\n}\n",
            "void f() {\n  StreamSubscription s;\n  g(s);\n}\n",
            "StreamSubscription f() {\n  StreamSubscription s;\n  return s;\n}\n",
            "void f() {\n  StreamSubscription s = other;\n}\n",
            // `!element.isDeclaring` mirrors: a primary constructor formals
            // face skips extension types.
            "extension type E(StreamSubscription s) {}\n",
        ];
        for source in sources {
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("cancel_subscriptions"));
            assert!(lints.is_empty(), "{source:?} -> {lints:?}");
        }
    }

    /// Untyped (`var`) declarations, other types, and a user class owning
    /// a watched name all stay silent; a second candidate in one file
    /// reports twice.
    #[test]
    fn cancel_subscriptions_negative_faces() {
        let sources = [
            "void f() {\n  var s;\n}\n",
            "void f() {\n  int s;\n}\n",
            "class StreamSubscription {}\n\nvoid f() {\n  StreamSubscription s;\n}\n",
        ];
        for source in sources {
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("cancel_subscriptions"));
            assert!(lints.is_empty(), "{source:?} -> {lints:?}");
        }
    }

    /// The assignment faces gate on `rightHandSide is SimpleIdentifier`
    /// (:195) — `s = compute()` and `obj.s = f()` are no valid uses, so the
    /// leak still reports.
    #[test]
    fn cancel_subscriptions_assignment_rhs_gate() {
        let sources = [
            "void f() {\n  StreamSubscription s;\n  s = compute();\n}\n",
            "void f() {\n  StreamSubscription s;\n  obj.s = f();\n}\n",
        ];
        for source in sources {
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("cancel_subscriptions"));
            assert_eq!(lints.len(), 1, "{source:?} -> {lints:?}");
        }
    }

    /// The extension-type `implements` conformance faces: a local declared
    /// with an extension type that `implements` a watched library type is
    /// a leak candidate (`implementsInterface` folds to the header's
    /// written `implements` clause); the same conformance under
    /// `close_sinks` reports through `IOSink`, and a canceled extension
    /// type or a representation-only extension type stays silent.
    #[test]
    fn cancel_subscriptions_extension_type_implements_reports() {
        // `implements StreamSubscription` conformance (the corpus's
        // `test_extensionType_implementsStreamSubscription_notCanceled`).
        let f = Fixture::parse(
            "void f(Stream stream) {\n  ECancelExt e = ECancelExt(stream.listen((_) {}));\n}\n\
             extension type ECancelExt(StreamSubscription _s)\n    implements StreamSubscription {}\n",
        );
        assert_eq!(f.run(&one_rule("cancel_subscriptions")).len(), 1);

        // The `close_sinks` twin: `implements IOSink`.
        let f = Fixture::parse(
            "void f(IOSink sink) {\n  ECloseExt e = ECloseExt(sink);\n}\n\
             extension type ECloseExt(IOSink _s) implements IOSink {}\n",
        );
        assert_eq!(f.run(&one_rule("close_sinks")).len(), 1);
    }

    /// The conformance's negative faces: an extension type whose
    /// subscription is canceled and a representation-only extension type
    /// (no `implements`) stay silent.
    #[test]
    fn cancel_subscriptions_extension_type_negatives() {
        for source in [
            // Canceled through `e.cancel()`.
            "void f(Stream stream) {\n  ECancelExtClosed e = ECancelExtClosed(stream.listen((_) {}));\n  e.cancel();\n}\n\
             extension type ECancelExtClosed(StreamSubscription _s)\n    implements StreamSubscription {}\n",
            // Representation only — no `implements`, never watched.
            "void f(Stream stream) {\n  ERepr e = ERepr(stream.listen((_) {}));\n}\n\
             extension type ERepr(StreamSubscription _s) {}\n",
        ] {
            let f = Fixture::parse(source);
            assert!(
                f.run(&one_rule("cancel_subscriptions")).is_empty(),
                "{source}"
            );
        }
    }

    /// The corpus's late-field faces (lints-new
    /// `cancel_subscriptions.dart:36/55`): a `late` field assigned from
    /// `listen` in a method and never canceled reports at its declaration,
    /// private or public; the released twin (a `cancel()` anywhere in the
    /// unit) stays silent. The keyword-anchored tiling (`late` heads the
    /// VariablesDeclaration row, members.rs `decl_head`) puts the written
    /// type INSIDE the declaration row — the lookup reads it there.
    #[test]
    fn cancel_subscriptions_late_field_reports() {
        for name in ["_subscription", "subscription"] {
            let f = Fixture::parse(&format!(
                "class C {{\n  late StreamSubscription {name};\n  void f(Stream stream) {{\n    \
                 {name} = stream.listen((_) {{}});\n  }}\n}}\n"
            ));
            assert_eq!(f.run(&one_rule("cancel_subscriptions")).len(), 1, "{name}");
        }

        let f = Fixture::parse(
            "class C {\n  late StreamSubscription s;\n  void f(Stream stream) {\n    \
             s = stream.listen((_) {});\n  }\n  void g() {\n    s.cancel();\n  }\n}\n",
        );
        assert!(f.run(&one_rule("cancel_subscriptions")).is_empty());
    }

    /// The corpus's primary-constructor face
    /// (`cancel_subscriptions.dart:47`): a declaring formal assigned from
    /// `listen` in a method and never canceled reports at the formal.
    #[test]
    fn cancel_subscriptions_primary_formal_method_assignment_reports() {
        let f = Fixture::parse(
            "class C(var StreamSubscription s) {\n  void f(Stream stream) {\n    \
             s = stream.listen((_) {});\n  }\n}\n",
        );
        assert_eq!(f.run(&one_rule("cancel_subscriptions")).len(), 1);
    }

    /// The oracle-drift face (lints-new `close_sinks.dart` expected
    /// [18, 56]): the close rule's field and primary faces stay silent —
    /// the gate's live oracle (fvm 3.13.1) answers this rule local-only
    /// there, unlike the cancel twin — while its local face still fires.
    #[test]
    fn close_sinks_field_and_primary_faces_stay_silent() {
        let f = Fixture::parse(
            "class C {\n  late IOSink s;\n  void f() {\n    s = File('').openWrite();\n  }\n}\n",
        );
        assert!(f.run(&one_rule("close_sinks")).is_empty());

        let f = Fixture::parse(
            "class C(var IOSink s) {\n  void f() {\n    s = File('').openWrite();\n  }\n}\n",
        );
        assert!(f.run(&one_rule("close_sinks")).is_empty());
    }

    /// `test/rules/close_sinks_test.dart` — an unclosed `Sink` local
    /// reports; the `IOSink`/`Socket` family folds into the same predicate
    /// table.
    #[test]
    fn close_sinks_family_reports() {
        for ty in ["Sink", "IOSink", "Socket"] {
            let source = format!("void f() {{\n  {ty} s;\n}}\n");
            let f = Fixture::parse(&source);
            let lints = f.run(&one_rule("close_sinks"));
            assert_eq!(lints.len(), 1, "{source:?} -> {lints:?}");
            assert_eq!(lints[0].message, "Unclosed instance of 'Sink'.");
        }
    }

    /// `destroy` releases too; `cancel` does not close a sink.
    #[test]
    fn close_sinks_release_methods() {
        let f = Fixture::parse("void f() {\n  Sink s;\n  s.destroy();\n}\n");
        assert!(f.run(&one_rule("close_sinks")).is_empty());

        let f = Fixture::parse("void f() {\n  Sink s;\n  s.cancel();\n}\n");
        assert_eq!(f.run(&one_rule("close_sinks")).len(), 1);
    }

    // —— tighten_type_of_initializing_formals —————————————————————————————

    /// `test/rules/tighten_type_of_initializing_formals_test.dart` — a
    /// field formal and a super formal asserted non-nullable over a
    /// nullable field each report at the formal; `null != x` reports too.
    #[test]
    fn tighten_field_and_super_formals_report() {
        let f = Fixture::parse("class A {\n  String? p;\n  A(this.p) : assert(p != null);\n}\n");
        let lints = f.run(&one_rule("tighten_type_of_initializing_formals"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Use a type annotation rather than 'assert' to enforce non-nullability."
        );

        let f = Fixture::parse("class A {\n  String? p;\n  A(this.p) : assert(null != p);\n}\n");
        assert_eq!(
            f.run(&one_rule("tighten_type_of_initializing_formals"))
                .len(),
            1
        );

        let f = Fixture::parse(
            "class A {\n  String? a;\n  A(this.a);\n}\n\nclass B extends A {\n  B(super.a) : assert(a != null);\n}\n",
        );
        assert_eq!(
            f.run(&one_rule("tighten_type_of_initializing_formals"))
                .len(),
            1
        );
    }

    /// The primary-constructor face: the asserted field formal over a
    /// declared nullable field reports; a declaring formal (no separate
    /// field) stays silent (`test_thisInit_asserts_primaryConstructor` /
    /// `test_thisInit_asserts_primaryConstructor_declaring`).
    #[test]
    fn tighten_primary_constructor_faces() {
        let f = Fixture::parse("class A(this.p) {\n  String? p;\n  this : assert(p != null);\n}\n");
        assert_eq!(
            f.run(&one_rule("tighten_type_of_initializing_formals"))
                .len(),
            1
        );

        let f = Fixture::parse("class A(final String? p) {\n  this : assert(p != null);\n}\n");
        assert!(
            f.run(&one_rule("tighten_type_of_initializing_formals"))
                .is_empty()
        );
    }

    /// Non-nullable written types, plain formals, non-assert conditions,
    /// and absent initializers all stay silent
    /// (`test_thisInit_tightens` / `test_thisInit_noAssert`).
    #[test]
    fn tighten_negative_faces() {
        let sources = [
            // The written type is the static type — non-nullable silences.
            "class A {\n  String? p;\n  A(String this.p) : assert(p != null);\n}\n",
            // A plain formal is neither face (:60-67).
            "class A {\n  String? p;\n  A(String p) : assert(p != null);\n}\n",
            // No assert initializer at all.
            "class A {\n  String? p;\n  A(this.p);\n}\n",
            // A non-`!=`-null condition skips (`condition is!
            // BinaryExpression` faces).
            "class A {\n  String? p;\n  A(this.p) : assert(p.length > 0);\n}\n",
        ];
        for source in sources {
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("tighten_type_of_initializing_formals"));
            assert!(lints.is_empty(), "{source:?} -> {lints:?}");
        }
    }
}

// —— unsafe_variance: the local variance machinery ————————————————————————
//
// `variance_builder.dart` computes each type parameter's variance from its
// alias body; `unsafe_variance` reads those variances through
// `variance_checker.dart`'s walk. The port computes both faces
// lints-locally over the written type tree — the engine has no resolution
// face for variance, and the written shape carries everything the two
// upstream passes observe on valid code.

/// The variance of one type parameter as `variance_builder.dart` computes
/// it for a same-file typedef (`shared_type.dart` lattice). The 0..3
/// encodings make [`AliasVariance::meet`] a bitwise OR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AliasVariance {
    Unrelated = 0,
    Covariant = 1,
    Contravariant = 2,
    Invariant = 3,
}

impl AliasVariance {
    /// The lattice meet: `unrelated` is the identity, `invariant`
    /// dominates, opposing directions meet at `invariant`.
    fn meet(self, other: Self) -> Self {
        match (self as u8) | (other as u8) {
            0 => Self::Unrelated,
            1 => Self::Covariant,
            2 => Self::Contravariant,
            _ => Self::Invariant,
        }
    }

    /// `Variance.combine` (`shared_type.dart`): `unrelated` absorbs,
    /// `invariant` dominates, agreeing directions stay, opposing meet at
    /// `contravariant`.
    fn combine(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unrelated, _) | (_, Self::Unrelated) => Self::Unrelated,
            (Self::Invariant, _) | (_, Self::Invariant) => Self::Invariant,
            (Self::Covariant, Self::Covariant) => Self::Covariant,
            _ => Self::Contravariant,
        }
    }
}

/// The run-wide computed-variance face of the same-file typedefs: the name
/// index (built once on first query) plus typedef row -> its per-parameter
/// variances (computed on demand). [`crate::engine::RunCaches`] holds one
/// per run.
#[derive(Default)]
pub(crate) struct AliasVariances {
    names: Option<rustc_hash::FxHashMap<String, u32>>,
    variances: rustc_hash::FxHashMap<u32, Vec<AliasVariance>>,
}

/// The computed per-parameter variances of the same-file typedef `row`
/// (`variance_builder.dart` `_compute` with the `_visit` cycle recovery:
/// a re-entrant alias reads provisional all-covariant values). The guard
/// is held across one computation; borrows never cross a report.
fn alias_variances(c: &Check<'_>, row: u32) -> Vec<AliasVariance> {
    let mut cache = c.caches.alias_variances.borrow_mut();
    let AliasVariances { names, variances } = &mut *cache;
    let names = names.get_or_insert_with(|| alias_names(c));
    alias_variances_uncached(c, names, variances, row)
}

/// The cached computation body: the name index shared, the variance map
/// passed mutably down the recursion so nested typedef detours share one
/// guard.
fn alias_variances_uncached(
    c: &Check<'_>,
    names: &rustc_hash::FxHashMap<String, u32>,
    variances: &mut rustc_hash::FxHashMap<u32, Vec<AliasVariance>>,
    row: u32,
) -> Vec<AliasVariance> {
    if let Some(computed) = variances.get(&row) {
        return computed.clone();
    }
    let parameters: Vec<&str> = alias_parameter_names(c, row);
    // Provisional covariance: a cycle re-entering this row reads
    // all-covariant — `_visit`'s recovery — before the real values
    // overwrite them below.
    variances.insert(row, vec![AliasVariance::Covariant; parameters.len()]);
    let computed: Vec<AliasVariance> = parameters
        .iter()
        .copied()
        .map(|name| alias_parameter_variance(c, names, variances, row, name))
        .collect();
    variances.insert(row, computed.clone());
    computed
}

/// The name -> typedef-row index of the unit (the `Decls` idiom: a
/// `Typedef` row's first `Identifier` child — the written return type is a
/// leaf `Type`, never an `Identifier` sibling).
fn alias_names(c: &Check<'_>) -> rustc_hash::FxHashMap<String, u32> {
    let mut names = rustc_hash::FxHashMap::default();
    let Some(root) = c.path.first().copied() else {
        return names;
    };
    for top in c.collect_direct(root) {
        for inner in c.collect_direct(top) {
            if c.kind(inner) != NodeKind::Typedef {
                continue;
            }
            if let Some(name) = c
                .collect_direct(inner)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Identifier)
            {
                names.insert(c.lexeme(c.head_token(name)).to_owned(), inner);
            }
        }
    }
    names
}

/// The declared type-parameter names of the typedef `row`, in order.
fn alias_parameter_names<'a>(c: &'a Check<'_>, row: u32) -> Vec<&'a str> {
    let type_variables = c
        .collect_direct(row)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::TypeVariables);
    let Some(type_variables) = type_variables else {
        return Vec::new();
    };
    // The type variable's name is the `Identifier` sibling before its
    // `TypeVariable` row (the row holds only the bound, not the name).
    let parameters = c.collect_direct(type_variables);
    parameters
        .iter()
        .enumerate()
        .filter(|&(_, &kid)| c.kind(kid) == NodeKind::TypeVariable)
        .filter_map(|(i, _)| {
            parameters[..i]
                .iter()
                .rev()
                .copied()
                .find(|&kid| c.kind(kid) == NodeKind::Identifier)
                .map(|name| c.lexeme(c.head_token(name)))
        })
        .collect()
}

/// The variance of one type parameter inside its typedef body: the `=`
/// form computes over the aliased type, the old form over the return type
/// (out) plus each parameter (contravariant) — `_computeFunctionType` with
/// `typeParameters: null` (`_functionTypeAlias`).
fn alias_parameter_variance<'a>(
    c: &'a Check<'_>,
    names: &rustc_hash::FxHashMap<String, u32>,
    variances: &mut rustc_hash::FxHashMap<u32, Vec<AliasVariance>>,
    alias_row: u32,
    name: &'a str,
) -> AliasVariance {
    if c.field(alias_row, PayloadField::Equals) != Some(crate::rules::NONE_TOKEN) {
        return match first_type_child(c, alias_row) {
            Some(body) => compute_variance(c, names, variances, alias_row, name, body),
            None => AliasVariance::Unrelated,
        };
    }
    let mut result = AliasVariance::Unrelated;
    if let Some(return_type) = first_type_child(c, alias_row) {
        result = result.meet(compute_variance(
            c,
            names,
            variances,
            alias_row,
            name,
            return_type,
        ));
    }
    let parameters = c
        .collect_direct(alias_row)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters);
    if let Some(parameters) = parameters {
        for parameter in c.collect_direct(parameters) {
            result = result.meet(AliasVariance::Contravariant.combine(compute_variance(
                c, names, variances, alias_row, name, parameter,
            )));
        }
    }
    result
}

/// `_compute` over one written row: a named type folds its arguments
/// through the named declaration's per-parameter variances, a function
/// type folds return (out), bounds (any occurrence -> invariant) and
/// parameters (contravariant), a record folds every field.
fn compute_variance<'a>(
    c: &'a Check<'_>,
    names: &rustc_hash::FxHashMap<String, u32>,
    variances: &mut rustc_hash::FxHashMap<u32, Vec<AliasVariance>>,
    alias_row: u32,
    name: &'a str,
    row: u32,
) -> AliasVariance {
    match c.kind(row) {
        NodeKind::Type => match first_type_child(c, row) {
            // The alias body wraps a function/record type in a `Type` row;
            // a named type's only child family is `TypeArguments`.
            Some(inner)
                if matches!(c.kind(inner), NodeKind::FunctionType | NodeKind::RecordType) =>
            {
                compute_variance(c, names, variances, alias_row, name, inner)
            }
            _ => compute_named(c, names, variances, alias_row, name, row),
        },
        NodeKind::FunctionType => compute_function(c, names, variances, alias_row, name, row),
        NodeKind::RecordType => {
            let mut result = AliasVariance::Unrelated;
            for field in record_type_fields(c, row) {
                result = result.meet(compute_variance(
                    c, names, variances, alias_row, name, field,
                ));
            }
            result
        }
        NodeKind::FormalParameter => match first_type_child(c, row) {
            Some(written) => compute_variance(c, names, variances, alias_row, name, written),
            None => AliasVariance::Unrelated, // implicitly typed
        },
        NodeKind::FunctionTypedFormalParameter => {
            compute_function(c, names, variances, alias_row, name, row)
        }
        _ => AliasVariance::Unrelated, // `void`, absent types
    }
}

/// `_computeFunctionType`: the return type at the out position, any bound
/// occurrence forces `invariant`, each parameter at `contravariant`.
fn compute_function<'a>(
    c: &'a Check<'_>,
    names: &rustc_hash::FxHashMap<String, u32>,
    variances: &mut rustc_hash::FxHashMap<u32, Vec<AliasVariance>>,
    alias_row: u32,
    name: &'a str,
    row: u32,
) -> AliasVariance {
    let mut result = AliasVariance::Unrelated;
    if let Some(return_type) = first_type_child(c, row) {
        result = result.meet(compute_variance(
            c,
            names,
            variances,
            alias_row,
            name,
            return_type,
        ));
    }
    let type_variables = c
        .collect_direct(row)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::TypeVariables);
    if let Some(type_variables) = type_variables {
        for parameter in c.collect_direct(type_variables) {
            if c.kind(parameter) != NodeKind::TypeVariable {
                continue;
            }
            if let Some(bound) = first_type_child(c, parameter)
                && compute_variance(c, names, variances, alias_row, name, bound)
                    != AliasVariance::Unrelated
            {
                result = AliasVariance::Invariant;
            }
        }
    }
    let parameters = c
        .collect_direct(row)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters);
    if let Some(parameters) = parameters {
        for parameter in c.collect_direct(parameters) {
            result = result.meet(AliasVariance::Contravariant.combine(compute_variance(
                c, names, variances, alias_row, name, parameter,
            )));
        }
    }
    result
}

/// `_compute` over a named type: a free occurrence of the parameter is
/// `covariant`; otherwise the arguments fold through the named
/// declaration's per-parameter variances — same-file typedef (computed) or
/// class-like (covariant; production parses carry no variance keywords, so
/// every class-like parameter is legacy covariant). An unresolvable name
/// is `unrelated` with its arguments ignored (`_compute`'s fall-through);
/// an arity mismatch folds the common prefix.
fn compute_named<'a>(
    c: &'a Check<'_>,
    names: &rustc_hash::FxHashMap<String, u32>,
    variances: &mut rustc_hash::FxHashMap<u32, Vec<AliasVariance>>,
    alias_row: u32,
    name: &'a str,
    row: u32,
) -> AliasVariance {
    let head = c.head_token(row);
    if !prefixed_type(c, head)
        && c.lexeme(head) == name
        && enclosing_type_parameter(c, row, name).is_none_or(|(owner, _)| owner == alias_row)
    {
        return AliasVariance::Covariant;
    }
    let Some(args_row) = type_arguments_row(c, row) else {
        return AliasVariance::Unrelated;
    };
    let arguments = type_argument_rows(c, args_row);
    if arguments.is_empty() {
        return AliasVariance::Unrelated;
    }
    let mapping: Option<Vec<AliasVariance>> = if prefixed_type(c, head) {
        None
    } else if let Some(alias) = names.get(c.lexeme(head)) {
        Some(alias_variances_uncached(c, names, variances, *alias))
    } else if let Some(decl) = crate::rules::same_file_declaration(c, c.lexeme(head)) {
        let parameters = c
            .collect_direct(decl)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::TypeVariables);
        parameters.map(|type_variables| {
            c.collect_direct(type_variables)
                .into_iter()
                .filter(|&kid| c.kind(kid) == NodeKind::TypeVariable)
                .map(|_| AliasVariance::Covariant)
                .collect()
        })
    } else {
        None
    };
    let Some(mapping) = mapping else {
        return AliasVariance::Unrelated;
    };
    let mut result = AliasVariance::Unrelated;
    for (parameter, &argument) in mapping.iter().zip(arguments.iter()) {
        result = result.meet(parameter.combine(compute_variance(
            c, names, variances, alias_row, name, argument,
        )));
    }
    result
}

/// The written field types of a record type row, positional then named.
fn record_type_fields(c: &Check<'_>, row: u32) -> Vec<u32> {
    let mut fields = Vec::new();
    for kid in c.collect_direct(row) {
        match c.kind(kid) {
            NodeKind::RecordTypeEntry => {
                if let Some(field) = first_type_child(c, kid) {
                    fields.push(field);
                }
            }
            NodeKind::RecordTypeNamedFields => {
                for entry in c.collect_direct(kid) {
                    if c.kind(entry) == NodeKind::RecordTypeEntry
                        && let Some(field) = first_type_child(c, entry)
                    {
                        fields.push(field);
                    }
                }
            }
            _ => {}
        }
    }
    fields
}

/// The nearest enclosing declaration row that declares `name` as a type
/// parameter, walking syntactic enclosure from `from`: `Some((declaring
/// row, the TypeVariable row))`. The innermost declaring declaration
/// wins — the shadowing-aware binding of the resolution face.
fn enclosing_type_parameter(c: &Check<'_>, from: u32, name: &str) -> Option<(u32, u32)> {
    let mut row = from;
    loop {
        row = c.unit.ast.parent(row)?;
        for parameter in c.collect_direct(row) {
            if c.kind(parameter) != NodeKind::TypeVariables {
                continue;
            }
            let parameters = c.collect_direct(parameter);
            for (i, &candidate) in parameters.iter().enumerate() {
                if c.kind(candidate) != NodeKind::TypeVariable {
                    continue;
                }
                // The type variable's name is the `Identifier` sibling before
                // its `TypeVariable` row under `TypeVariables` (the row itself
                // holds only the bound/`NoType`, not the name).
                let declared = parameters[..i]
                    .iter()
                    .rev()
                    .copied()
                    .find(|&kid| c.kind(kid) == NodeKind::Identifier);
                if declared.is_some_and(|declared| c.lexeme(c.head_token(declared)) == name) {
                    return Some((row, candidate));
                }
            }
        }
    }
}

pub(crate) mod unsafe_variance {
    use super::{Check, NodeKind, RuleSpec, first_type_child, type_sibling_before};

    /// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/unsafe_variance.dart`
    /// — a type parameter of a class, mixin, or enum occurring in a
    /// non-covariant position of an instance member's written type reports
    /// at the type. The registered faces are method declarations (the
    /// return type at `out`, the method's own type-parameter bounds at
    /// `inout`; formal parameters are NOT checked) and variable declaration
    /// lists (the written type at `out`). Static members skip: the method
    /// face by `node.isStatic`, the field face by the same modifier (the
    /// upstream lint relies on resolution there — a static field cannot
    /// reference a class type parameter, so the rule is never reached).
    ///
    /// dartq face: the walk is `variance_checker.dart` computed
    /// lints-locally over the written type tree ([`super::walk`]);
    /// generic instantiations map through the same-file declaration's
    /// per-parameter variances — a typedef's variances are computed exactly
    /// as `variance_builder.dart` does ([`super::AliasVariances`]) — with
    /// the all-covariant backup for unresolvable or arity-mismatched
    /// names. Eligibility walks the written enclosure: only a
    /// class/mixin/enum-declared, unmarked parameter reports.
    ///
    /// Gating: the official rule consults no `isFeatureEnabled` face —
    /// `RuleState.experimental()` is registry metadata, not an enable gate —
    /// so the port registers unconditionally and enables by name only.
    /// Message: `diagnostic.g.dart` `unsafeVariance`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unsafe_variance",
    &[NodeKind::Method, NodeKind::VariablesDeclaration],
    check,
);

    /// The static `ModSet` bit (bridge `mod_bits`; the member flags share the
    /// `STATIC` position with the field face).
    const STATIC_BIT: u16 = 1 << 3;

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        match c.kind(node) {
            NodeKind::Method => {
                // `if (node.isStatic) return;` — the static modifier rides
                // the header flag bits, not the `DeclarationKind` payload
                // word (the payload records the enclosing declaration, not
                // the member's own staticity).
                if dartq_ast::mod_flags(c.unit.ast, node) & STATIC_BIT != 0 {
                    return;
                }
                if let Some(return_type) = first_type_child(c, node) {
                    super::walk(c, super::Direction::Out, return_type);
                }
                // `node.typeParameters?.typeParameters.forEach(checker.checkBound);`
                // The method's *own* type parameters tile between the name
                // and the `FormalParameters` row — a `TypeVariables` child
                // later in the window is a body closure's (`=> <Y>() => x`),
                // not the method's signature.
                let direct = c.collect_direct(node);
                let params_start = direct
                    .iter()
                    .copied()
                    .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
                    .map(|params| c.byte_span(params).start);
                let type_variables = direct.iter().copied().find(|&kid| {
                    c.kind(kid) == NodeKind::TypeVariables
                        && params_start.is_none_or(|start| c.byte_span(kid).start < start)
                });
                if let Some(type_variables) = type_variables {
                    for parameter in c.collect_direct(type_variables) {
                        if c.kind(parameter) != NodeKind::TypeVariable {
                            continue;
                        }
                        if let Some(bound) = first_type_child(c, parameter) {
                            super::walk(c, super::Direction::InOut, bound);
                        }
                    }
                }
            }
            NodeKind::VariablesDeclaration => {
                // The static-field face stays silent: upstream never
                // reaches it (resolution blocks the illegal reference).
                if let Some(fields) = c.unit.ast.parent(node)
                    && c.kind(fields) == NodeKind::Fields
                    && dartq_ast::mod_flags(c.unit.ast, fields) & STATIC_BIT != 0
                {
                    return;
                }
                // `visitVariableDeclarationList`'s `node.type`: the written
                // type rides the declaration row's left sibling under the
                // owning `Fields`/`TopLevelFields`/statement wrapper.
                let written =
                    type_sibling_before(c, node).filter(|&prev| c.kind(prev) != NodeKind::NoType);
                if let Some(written) = written {
                    super::walk(c, super::Direction::Out, written);
                }
            }
            _ => {}
        }
    }
}

/// The checker direction (`variance_checker.dart` `Variance`):
/// `out`/`in`/`inout` with its inverse.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Out,
    In,
    InOut,
}

impl Direction {
    fn inverse(self) -> Self {
        match self {
            Self::Out => Self::In,
            Self::In => Self::Out,
            Self::InOut => Self::InOut,
        }
    }
}

/// The per-argument mapping entry of a generic instantiation: identity for
/// a covariant parameter, the inverse for a contravariant one, `inout` for
/// everything else.
#[derive(Clone, Copy)]
pub(crate) enum Mapping {
    Same,
    Inverse,
    InOut,
}

impl Mapping {
    fn apply(self, direction: Direction) -> Direction {
        match self {
            Self::Same => direction,
            Self::Inverse => direction.inverse(),
            Self::InOut => Direction::InOut,
        }
    }
}

/// The checker walk (`VarianceChecker.check`): named-type arguments first
/// (the hook after), function types at return/bounds/parameters, records
/// across every field.
fn walk(c: &mut Check<'_>, direction: Direction, row: u32) {
    match c.kind(row) {
        NodeKind::Type => {
            // A `Type` row wrapping a function/record type walks that type.
            match first_type_child(c, row) {
                Some(inner)
                    if matches!(c.kind(inner), NodeKind::FunctionType | NodeKind::RecordType) =>
                {
                    walk(c, direction, inner);
                }
                _ => walk_named(c, direction, row),
            }
        }
        NodeKind::FunctionType => walk_function(c, direction, row),
        NodeKind::RecordType => {
            for field in record_type_fields(c, row) {
                walk(c, direction, field);
            }
        }
        _ => {}
    }
}

/// The function-type walk: return at the variance, own bounds at `inout`,
/// each explicitly-typed parameter at the inverse
/// (`GenericFunctionType` case + `checkFormalParameter`).
fn walk_function(c: &mut Check<'_>, direction: Direction, row: u32) {
    if let Some(return_type) = first_type_child(c, row) {
        walk(c, direction, return_type);
    }
    let type_variables = c
        .collect_direct(row)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::TypeVariables);
    if let Some(type_variables) = type_variables {
        for parameter in c.collect_direct(type_variables) {
            if c.kind(parameter) != NodeKind::TypeVariable {
                continue;
            }
            if let Some(bound) = first_type_child(c, parameter) {
                walk(c, Direction::InOut, bound);
            }
        }
    }
    let parameters = c
        .collect_direct(row)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters);
    if let Some(parameters) = parameters {
        walk_formal_sections(c, direction.inverse(), &c.collect_direct(parameters));
    }
}

/// The `FormalParameters` children: the required group, then the `[]`/`{}`
/// section — `OptionalFormalParameters` unwraps one level.
fn walk_formal_sections(c: &mut Check<'_>, direction: Direction, sections: &[u32]) {
    for &section in sections {
        if c.kind(section) == NodeKind::OptionalFormalParameters {
            let inner = c.collect_direct(section);
            walk_formal_sections(c, direction, &inner);
        } else {
            walk_formal(c, direction, section);
        }
    }
}

/// `checkFormalParameter`: only explicitly-typed parameters check; the
/// function-typed suffix face recurses like a function type.
fn walk_formal(c: &mut Check<'_>, direction: Direction, row: u32) {
    match c.kind(row) {
        NodeKind::FormalParameter => {
            if let Some(written) = first_type_child(c, row) {
                walk(c, direction, written);
            }
        }
        NodeKind::FunctionTypedFormalParameter => walk_function(c, direction, row),
        _ => {}
    }
}

/// The named-type walk (`VarianceChecker.check` `NamedType` case):
/// arguments first, mapped through the named declaration's per-parameter
/// variances with the all-covariant backup, then the hook.
fn walk_named(c: &mut Check<'_>, direction: Direction, row: u32) {
    if let Some(args_row) = type_arguments_row(c, row) {
        let arguments = type_argument_rows(c, args_row);
        if !arguments.is_empty() {
            if let Some(mapping) = argument_mapping(c, row, arguments.len()) {
                for (position, &argument) in mapping.iter().zip(arguments.iter()) {
                    walk(c, position.apply(direction), argument);
                }
            } else {
                // The backup strategy: code with errors assumes every
                // parameter covariant.
                for argument in arguments {
                    walk(c, direction, argument);
                }
            }
        }
    }
    check_named_type(c, direction, row);
}

/// The hook (`_UnsafeVarianceChecker.checkNamedType`): a bare type
/// parameter of a same-file class/mixin/enum whose parameter is legacy
/// (unmarked) reports when its position is not `out`. Method, local,
/// extension, extension-type, and typedef owners never report (the
/// `owningDeclarationSupportsVariance` element walk, read off the written
/// enclosure).
fn check_named_type(c: &mut Check<'_>, direction: Direction, row: u32) {
    if direction == Direction::Out || type_arguments_row(c, row).is_some() {
        return;
    }
    let head = c.head_token(row);
    if prefixed_type(c, head) {
        return;
    }
    let name = c.lexeme(head);
    let Some((owner, parameter)) = enclosing_type_parameter(c, row, name) else {
        return;
    };
    // The enclosing type-parameter owner is the production
    // `TopLevelDeclaration` wrapper — the class-like itself is its child
    // (the `TypeVariables` row is the wrapper's, tiling before the
    // declaration row). A method-owned parameter resolves to the method
    // row and never reports.
    let owner_kind = if c.kind(owner) == NodeKind::TopLevelDeclaration {
        c.collect_direct(owner)
            .into_iter()
            .map(|kid| c.kind(kid))
            .find(|kind| {
                matches!(
                    kind,
                    NodeKind::ClassDeclaration
                        | NodeKind::MixinDeclaration
                        | NodeKind::EnumDeclaration
                        | NodeKind::ExtensionDeclaration
                        | NodeKind::ExtensionTypeDeclaration
                )
            })
    } else {
        Some(c.kind(owner))
    };
    if !matches!(
        owner_kind,
        Some(NodeKind::ClassDeclaration | NodeKind::MixinDeclaration | NodeKind::EnumDeclaration)
    ) {
        return;
    }
    if c.field(parameter, PayloadField::Variance) != Some(crate::rules::NONE_TOKEN) {
        return; // variance-marked: not legacy covariant
    }
    c.report(c.utf16_span(row), &[]);
}

/// The per-parameter position mapping of the instantiation at `row`;
/// `None` is the unresolvable/arity-mismatch backup
/// (`variance_checker.dart`).
fn argument_mapping(c: &Check<'_>, row: u32, count: usize) -> Option<Vec<Mapping>> {
    let head = c.head_token(row);
    if prefixed_type(c, head) {
        return None;
    }
    let name = c.lexeme(head);
    let alias = {
        let mut cache = c.caches.alias_variances.borrow_mut();
        let AliasVariances { names, .. } = &mut *cache;
        names
            .get_or_insert_with(|| alias_names(c))
            .get(name)
            .copied()
    };
    if let Some(alias) = alias {
        // The typedef mapping reads the computed variances
        // (`parameterElement.variance`).
        let variances = alias_variances(c, alias);
        if variances.len() != count {
            return None;
        }
        return Some(
            variances
                .iter()
                .map(|&variance| match variance {
                    AliasVariance::Covariant => Mapping::Same,
                    AliasVariance::Contravariant => Mapping::Inverse,
                    _ => Mapping::InOut,
                })
                .collect(),
        );
    }
    let declaration = crate::rules::same_file_declaration(c, name)?;
    let type_variables = c
        .collect_direct(declaration)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::TypeVariables)?;
    let parameters: Vec<u32> = c
        .collect_direct(type_variables)
        .into_iter()
        .filter(|&kid| c.kind(kid) == NodeKind::TypeVariable)
        .collect();
    if parameters.len() != count {
        return None;
    }
    Some(
        parameters
            .iter()
            .map(|&parameter| match c.field(parameter, PayloadField::Variance) {
                Some(token) if c.lexeme(token) == "in" => Mapping::Inverse,
                Some(token) if c.lexeme(token) == "inout" => Mapping::InOut,
                _ => Mapping::Same,
            })
            .collect(),
    )
}

#[cfg(test)]
mod unsafe_variance_tests {
    use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

    const MESSAGE: &str =
        "This type is unsafe: a type parameter occurs in a non-covariant position.";

    /// `test_class_method_return` — the type parameter in the function
    /// type's parameter position reports; the `out`-position occurrence
    /// stays silent.
    #[test]
    fn method_return_parameter_position_reports() {
        let f = Fixture::parse("class A<X> {\n  X Function(X) m1() => (X x) => x;\n}\n");
        let lints = f.run(&one_rule("unsafe_variance"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, MESSAGE);
        // `class0 A1 <2 X3 >4 {5 X6 Function7 (8 X9 ...` — the parameter X.
        let x = tok(&f.source, &f.tokens, "X", 2);
        assert_eq!(lints[0].range.start, utf16_at(&f, x));
    }

    /// `test_class_method_parameter` — method formal parameters are NOT
    /// checked, at any nesting depth.
    #[test]
    fn method_parameter_positions_pass() {
        let f = Fixture::parse(
            "class A<X> {\n  void m3(void Function(void Function(X)) _) {}\n  void m4(X _) {}\n}\n",
        );
        assert!(f.run(&one_rule("unsafe_variance")).is_empty());
    }

    /// `test_class_method_bound` — the bound of the method's own type
    /// parameter reports at `inout`; the closure body is not a signature
    /// and never walks.
    #[test]
    fn method_own_bound_reports() {
        let f = Fixture::parse(
            "class A<X> {\n  int Function<Y extends X>() m2() => <Y extends X>() => 2;\n}\n",
        );
        let lints = f.run(&one_rule("unsafe_variance"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        // `... extends10 X11 ...` — the bound X, not the class parameter.
        let x = tok(&f.source, &f.tokens, "X", 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, x));
    }

    /// `test_class_variable` — the variable-declaration face walks the
    /// written type.
    #[test]
    fn field_parameter_position_reports() {
        let f = Fixture::parse("class A<X> {\n  void Function(X) func;\n}\n");
        let lints = f.run(&one_rule("unsafe_variance"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let x = tok(&f.source, &f.tokens, "X", 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, x));
    }

    /// `test_class_method_return_typedef` — the alias's computed invariant
    /// parameter maps the argument to `inout` (`variance_builder.dart`:
    /// `typedef Func<X> = X Function(X)` makes `X` invariant).
    #[test]
    fn typedef_invariant_argument_reports() {
        let f = Fixture::parse(
            "class A<X> {\n  Func<X> m1() => (X x) => x;\n}\ntypedef Func<X> = X Function(X);\n",
        );
        let lints = f.run(&one_rule("unsafe_variance"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        // `Func6 <7 X8 >9` — the argument X.
        let x = tok(&f.source, &f.tokens, "X", 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, x));
    }

    /// A computed-contravariant alias parameter maps the argument to the
    /// inverse; the `X Function` shape stays silent (`X` only occurs at
    /// covariant positions of the alias body).
    #[test]
    fn typedef_covariant_argument_passes() {
        let f = Fixture::parse(
            "class A<X> {\n  Func<X> m1() => (x) => x;\n}\ntypedef Func<X> = X Function();\n",
        );
        assert!(f.run(&one_rule("unsafe_variance")).is_empty());
    }

    /// `test_class_getter` — the getter face walks its written return
    /// type, optional-parameter position included.
    #[test]
    fn getter_parameter_position_reports() {
        let f = Fixture::parse("class A<X> {\n  void Function([X])? get func2 => null;\n}\n");
        let lints = f.run(&one_rule("unsafe_variance"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let x = tok(&f.source, &f.tokens, "X", 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, x));
    }

    /// `test_enum_getter` / `test_mixin_getter` — enum and mixin owners
    /// report like classes.
    #[test]
    fn enum_and_mixin_owners_report() {
        let f = Fixture::parse("enum E<X> {\n  e;\n  void Function([X])? get func2 => null;\n}\n");
        assert_eq!(f.run(&one_rule("unsafe_variance")).len(), 1);

        let f = Fixture::parse("mixin A<X> {\n  void Function([X])? get func2;\n}\n");
        assert_eq!(f.run(&one_rule("unsafe_variance")).len(), 1);
    }

    /// `test_extension_getter` / `test_extension_type_getter` — extension
    /// and extension-type owners never report
    /// (`owningDeclarationSupportsVariance`).
    #[test]
    fn extension_owners_pass() {
        let f = Fixture::parse(
            "extension E<X> on List<X> {\n  void Function([X])? get func2 => null;\n}\n",
        );
        assert!(f.run(&one_rule("unsafe_variance")).is_empty());

        let f = Fixture::parse(
            "extension type A<X>(X x) {\n  void Function([X])? get func2 => null;\n}\n",
        );
        assert!(f.run(&one_rule("unsafe_variance")).is_empty());
    }

    /// `test_static_class_member` (+ the static-field face) — static
    /// members skip on both faces.
    #[test]
    fn static_members_pass() {
        let f = Fixture::parse(
            "class A<X> {\n  static void Function(X)? get func => null;\n  static void Function(X)? m<X>() => null;\n  static void Function(X)? f;\n}\n",
        );
        assert!(f.run(&one_rule("unsafe_variance")).is_empty());
    }

    /// Covariant positions never report: the bare return type, type
    /// arguments of imported generics, and the written field type.
    #[test]
    fn covariant_positions_pass() {
        let f = Fixture::parse(
            "class A<X> {\n  X? m() => null;\n  List<X> xs;\n  Map<String, X> m2() => {};\n}\n",
        );
        assert!(f.run(&one_rule("unsafe_variance")).is_empty());
    }

    /// The report span covers the nullable suffix of the bare parameter
    /// type (`T?` reports across both tokens).
    #[test]
    fn nullable_bare_type_span_covers_question() {
        let f = Fixture::parse("class A<X> {\n  void Function(X?)? f;\n}\n");
        let lints = f.run(&one_rule("unsafe_variance"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let x = tok(&f.source, &f.tokens, "X", 1);
        let start = utf16_at(&f, x);
        assert_eq!(lints[0].range, start..start + 2);
    }

    /// A record field at a contravariant position reports
    /// (`RecordTypeAnnotation` case).
    #[test]
    fn record_field_reports() {
        let f = Fixture::parse("class A<X> {\n  void Function((int, X))? f;\n}\n");
        let lints = f.run(&one_rule("unsafe_variance"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let x = tok(&f.source, &f.tokens, "X", 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, x));
    }

    /// An inner type-parameter declaration shadows the class parameter:
    /// the bound `X` inside the function type is the class parameter, but
    /// the locally declared `X` (the function type's own parameter) is a
    /// method-owned parameter and never reports.
    #[test]
    fn locally_shadowed_parameter_passes() {
        let f = Fixture::parse("class A<X> {\n  void Function<X>(X)? f;\n}\n");
        assert!(f.run(&one_rule("unsafe_variance")).is_empty());
    }
}
