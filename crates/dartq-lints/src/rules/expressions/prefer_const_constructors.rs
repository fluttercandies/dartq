use super::{Check, Group, NodeKind, RuleSpec, slice, static_type_faces};
use crate::rules::{body_members, in_const_context, member_annotations, same_file_declaration};

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/prefer_const_constructors.dart` — a
/// non-const instance creation whose constructor is `const` and whose
/// every argument could be const reports at the creation (:57/:99
/// `node.canBeConst` → `reportAtNode`). The visitor's skip gates in
/// order (:39-60 dot-shorthand, :63-102 instance creation): the
/// written `const` (`node.isConst`), a deferred constructor type
/// (:65), a non-const constructor (:46/:69), a `@literal` constructor
/// (:49/:72 — the analyzer's own warning covers it), the dart:core
/// `Object` unnamed constructor (:52-55/:75-78 — `new Object()` makes
/// identity sentinels), and the generic-context-type skip (:80-97 —
/// a generic class created without written type arguments whose
/// context type carries type parameters, dart-lang/linter#4531).
///
/// dartq faces over the production tiling: the dispatch rows are the
/// three creation shapes — the `new`-keyword row, the generic implicit
/// creation, and the invocation send of a bare/member callee. The
/// const gates fold into the shared const-ability face
/// ([`crate::rules::const_ability::is_const_able`]: the callee
/// resolves to a same-file `const` constructor and the arguments are
/// const-able — the `canBeConst` core) and
/// [`crate::rules::in_const_context`] (the written-`const` and
/// constant-context faces — a creation inside `const [...]`, an
/// annotation, an enum element, or a constant pattern is already
/// const). Approximations (documented): the deferred-import gate is
/// subsumed (a prefixed callee never resolves same-file, so the
/// const-ability face keeps it silent), the `Object` skip likewise
/// (dart:core `Object` never resolves here — a same-file `Object`
/// class is the user's and correctly reports), and the generic
/// context-type read is unreachable without inference, so the whole
/// generic-without-written-type-arguments face bails (the
/// conservative direction — suggesting `const` there can change
/// inference, #4531; the downward-inference face that does report
/// upstream under-reports). The `@literal` gate reads the resolved
/// same-file constructor's own metadata over the analyzer's
/// declaring-library gate (`library meta;` — the unnamed vendored
/// meta of a differential corpus never recognizes it, and the
/// creation reports; the written-type-argument face of `canBeConst`
/// — a type variable in `C<T>()` — lives in the shared const-ability
/// face). Message: `diagnostic.g.dart`
/// `preferConstConstructors`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "prefer_const_constructors",
    group: Group::Standalone,
    problem: "Use 'const' with the constructor to improve performance.",
    correction: "Try adding the 'const' keyword to the constructor invocation.",
    kinds: &[
        NodeKind::NewExpression,
        NodeKind::ImplicitCreation,
        NodeKind::Send,
    ],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `node.isConst` and every constant-context face (:42/:64): a
    // const wrapper, an annotation, an enum element, a constant
    // pattern, or a const collection's contents — already const.
    if in_const_context(c) {
        return;
    }
    // The dot-shorthand creation resolves through the context type, so
    // `is_const_able` (a same-file-name face) cannot see it — handle it
    // before the shared const-ability early-return.
    if c.kind(node) == NodeKind::Send && shorthand_send_face(c) {
        return;
    }
    // `element.isConst` (:46/:69) + `node.canBeConst` (:57/:99) — the
    // same-file const-ability face.
    if !crate::rules::const_ability::is_const_able(c.unit, node) {
        return;
    }
    match c.kind(node) {
        NodeKind::NewExpression | NodeKind::ImplicitCreation => {
            // `new` is the row's span head for the keyword shape; the
            // implicit creation begins at the class name itself.
            let from = if c.kind(node) == NodeKind::NewExpression {
                c.head_token(node) + 1
            } else {
                c.head_token(node)
            };
            let Some((class_name, selector, written_type_arguments)) =
                creation_header_tokens(c, from)
            else {
                return;
            };
            if gate_generic(c, node, class_name, written_type_arguments)
                || ctor_has_literal(c, class_name, selector)
            {
                return;
            }
            // The whole creation (`new A(...)` — the row's span is the
            // `new` keyword alone; the args close it).
            let end = creation_args_end(c, node);
            let start = c.utf16_span(node).start;
            c.report(start..end.unwrap_or(c.utf16_span(node).end), &[]);
        }
        _ => send_face(c),
    }
}

/// The dot-shorthand creation face (`A a = .new()`, `.tightFor(…)`,
/// `C get f => .new(1)`): the dispatch `Send` carries the `new (…)` /
/// `selector (…)` invocation, the `DotShorthandHead` leaf for the `.`
/// tiles immediately before its callee head, and the class resolves
/// through the enclosing context's written type. The report spans the
/// `.` through the argument list (`upstream Dart SDK source,
/// pkg/linter/lib/src/rules/prefer_const_constructors.dart` :39-60
/// `visitDotShorthandInvocation` — `canBeConst` gates the same way).
/// Returns `true` when the node was the shorthand send (reported or
/// resolved-silent), `false` to fall through to the generic faces.
fn shorthand_send_face(c: &mut Check<'_>) -> bool {
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return false;
    };
    let Some((head, arguments)) = slice::send_parts(c, &kids, pos) else {
        return false;
    };
    let Some(arguments) = arguments else {
        return false;
    };
    let selector_row = kids[head];
    // The `.` head leaf seals immediately after the send in the owner
    // run, its span butting the callee selector's start (`.new` — the
    // leaf owns the `.`, the send owns `new (…)`).
    let Some(&head_row) = kids.get(pos + 1) else {
        return false;
    };
    if c.kind(head_row) != NodeKind::DotShorthandHead
        || c.unit.ast.token_range(head_row).end != c.head_token(selector_row)
    {
        return false;
    }
    let selector = c.lexeme(c.head_token(selector_row));
    let Some(class_name) = shorthand_context_type(c) else {
        return true;
    };
    if !crate::rules::const_ability::send_shorthand_able(
        c.unit, class_name, selector, arguments,
    ) || ctor_has_literal(c, class_name, (selector != "new").then_some(selector))
    {
        return true;
    }
    let start = c.utf16_span(head_row).start;
    c.report(start..c.utf16_span(arguments).end, &[]);
    true
}

/// The class the shorthand's `.new`/`.named` resolves against — the
/// enclosing `VariablesDeclaration`'s written type (`A a = .new()`),
/// else the enclosing function/getter's return type (`C get f =>
/// .new(1)`). `None` when no context type is written (under-report).
fn shorthand_context_type<'a>(c: &'a Check<'_>) -> Option<&'a str> {
    for &ancestor in c.path[..c.path.len() - 1].iter().rev() {
        let ty = match c.kind(ancestor) {
            NodeKind::VariablesDeclaration | NodeKind::InitializedIdentifier => {
                declared_type_left_sibling(c, ancestor)
            }
            NodeKind::TopLevelMethod
            | NodeKind::Method
            | NodeKind::LocalFunctionDeclaration => c
                .collect_direct(ancestor)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Type),
            _ => None,
        };
        if let Some(ty) = ty {
            return Some(c.lexeme(c.head_token(ty)));
        }
    }
    None
}

/// The invocation-send face: the callee resolves through the same
/// shared face, the member shape reads the single-name receiver, and
/// the report spans the whole creation (`A(...)` / `A.named(...)`).
fn send_face(c: &mut Check<'_>) {
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
    let Some((head, arguments)) = slice::send_parts(c, &kids, pos) else {
        return;
    };
    // A tear-off send carries no invocation (:46 `element is!
    // ConstructorElement` face).
    let Some(arguments) = arguments else {
        return;
    };
    let selector_row = kids[head];
    let (start, selector) =
        if slice::access_marker(c, &kids, pos, NodeKind::DotAccess).is_some() {
            // `C.named(...)`: the class is the single-name receiver.
            let selector = c.head_token(selector_row);
            let Some(receiver) = static_type_faces::single_receiver(c, &kids[..head], selector)
            else {
                return;
            };
            (c.head_token(receiver), Some(c.lexeme(selector)))
        } else {
            // `C(...)`: the callee head is the class.
            let head_token = c.head_token(selector_row);
            (head_token, None)
        };
    let Some((class_name, written_type_arguments)) =
        creation_header_names(c, start, c.head_token(arguments))
    else {
        return;
    };
    if gate_generic(c, c.node(), class_name, written_type_arguments)
        || ctor_has_literal(c, class_name, selector)
    {
        return;
    }
    let end = c.utf16_span(arguments).end;
    c.report(crate::rules::utf16_token_span(c, start).start..end, &[]);
}

/// The creation's header tokens from `from` up to the arguments' `(`:
/// the first identifier names the class, an identifier after the last
/// `.` is the named-constructor selector, and a `<` proves written
/// type arguments (`C<int>.named(` — the whole header is a handful of
/// tokens). Returns `(class name, written type arguments)`.
fn creation_header_names<'a>(
    c: &'a Check<'_>,
    from: u32,
    args_start: u32,
) -> Option<(&'a str, bool)> {
    let mut class_name = None;
    let mut type_arguments = false;
    let mut token = from;
    while token < args_start {
        let lexeme = c.lexeme(token);
        match lexeme {
            "<" => type_arguments = true,
            "." | "?" | "new" => {}
            _ if is_identifier_lexeme(c, token) && class_name.is_none() => {
                class_name = Some(lexeme);
            }
            _ => {}
        }
        token += 1;
    }
    Some((class_name?, type_arguments))
}

/// The same walk for the structured rows: `from` skips the `new`
/// keyword; the named-constructor selector rides the same tokens.
fn creation_header_tokens<'a>(
    c: &'a Check<'_>,
    from: u32,
) -> Option<(&'a str, Option<&'a str>, bool)> {
    let node = c.node();
    let args = creation_args_row(c, node)?;
    let (class_name, type_arguments) = creation_header_names(c, from, c.head_token(args))?;
    // The selector is the identifier right after the header's last
    // `.`; `new` never names one (the unnamed face).
    let mut selector = None;
    let mut token = from;
    while token < c.head_token(args) {
        if c.lexeme(token) == "." {
            let next = token + 1;
            if next < c.head_token(args) && is_identifier_lexeme(c, next) {
                selector = Some(c.lexeme(next));
            }
        }
        token += 1;
    }
    Some((class_name, selector, type_arguments))
}

/// The arguments row of a structured creation: the content rows
/// completed inside the row's own window (the begin/end brackets wrap
/// them).
fn creation_args_row(c: &Check<'_>, node: u32) -> Option<u32> {
    c.children(node)
        .rev()
        .find(|&row| c.kind(row) == NodeKind::Arguments)
}

/// The creation's last UTF-16 offset: the arguments row's end when the
/// window carries one.
fn creation_args_end(c: &Check<'_>, node: u32) -> Option<u32> {
    Some(c.utf16_span(creation_args_row(c, node)?).end)
}

/// The generic-context-type skip (:80-97): a same-file class that
/// declares type parameters, created without written type arguments,
/// whose CONTEXT type's written type arguments name an enclosing type
/// parameter (`TypeParameterType`, dart-lang/linter#4531) — suggesting
/// `const` there changes inference. A generic creation without written
/// arguments whose context type is concrete (or absent) still reports.
/// The header's `TypeVariables` tiles beside the `ClassDeclaration`
/// row under the top-level wrapper; the context type read is the
/// enclosing declarator's declared type or the enclosing function's
/// return type.
fn gate_generic(
    c: &Check<'_>,
    node: u32,
    class_name: &str,
    written_type_arguments: bool,
) -> bool {
    if written_type_arguments {
        return false;
    }
    let generic = same_file_declaration(c, class_name).is_some_and(|class_row| {
        c.unit.ast.parent(class_row).is_some_and(|top| {
            c.collect_direct(top)
                .iter()
                .any(|&kid| c.kind(kid) == NodeKind::TypeVariables)
        })
    });
    generic && context_mentions_type_variable(c, node)
}

/// Whether the creation's context type's written type arguments name
/// an in-scope type parameter — the upstream `TypeParameterType` face
/// (:85-97). Reads the enclosing `VariablesDeclaration` declared type
/// or the enclosing function's return type, then tests each type
/// argument's head name against the type variables in scope. `false`
/// (the conservative under-report keep) when no context type is
/// written or none of its arguments is a type parameter.
fn context_mentions_type_variable(c: &Check<'_>, node: u32) -> bool {
    // The in-scope type variables: the enclosing generic declaration's
    // `TypeVariables` heads (class or function).
    let mut names = Vec::new();
    let mut current = Some(node);
    while let Some(row) = current {
        for kid in c.collect_direct(row) {
            if c.kind(kid) == NodeKind::TypeVariables {
                names.extend(type_variable_names(c, kid));
            }
        }
        current = c.unit.ast.parent(row);
    }
    if names.is_empty() {
        return false;
    }
    // The context type's written type arguments: the enclosing
    // declarator's declared type, or the enclosing function's return
    // type — each a `Type` row whose `TypeArguments` carry the args.
    let mut current = Some(node);
    while let Some(row) = current {
        let ty = match c.kind(row) {
            NodeKind::VariablesDeclaration | NodeKind::InitializedIdentifier => {
                declared_type_left_sibling(c, row)
            }
            // The function faces: the return type is the method's /
            // function's own `Type` child (`G<int> f() => …`).
            NodeKind::TopLevelMethod
            | NodeKind::Method
            | NodeKind::LocalFunctionDeclaration => c
                .collect_direct(row)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Type),
            _ => None,
        };
        if let Some(ty) = ty {
            return type_args_name_type_variables(c, ty, &names);
        }
        current = c.unit.ast.parent(row);
    }
    false
}

/// The left-sibling `Type` of a `VariablesDeclaration` — the declared
/// type's row tiles before the declaration (which heads at the name).
fn declared_type_left_sibling(c: &Check<'_>, decl: u32) -> Option<u32> {
    let parent = c.unit.ast.parent(decl)?;
    let siblings = c.collect_direct(parent);
    let pos = siblings.iter().position(|&kid| kid == decl)?;
    siblings[..pos]
        .iter()
        .rev()
        .find(|&&kid| c.kind(kid) == NodeKind::Type)
        .copied()
}

/// The names a `TypeVariables` row declares (`<T, S>` → `T`, `S`).
fn type_variable_names<'a>(c: &'a Check<'_>, vars: u32) -> Vec<&'a str> {
    c.collect_direct(vars)
        .into_iter()
        .filter(|&kid| c.kind(kid) == NodeKind::TypeVariable)
        .map(|var| c.lexeme(c.head_token(var)))
        .collect()
}

/// Whether the `Type` row's written type arguments' head names hit any
/// of `vars` — the `AImplicit<T>` / `ADownward<int>` split. The
/// `TypeArguments` tiles as the `Type`'s right sibling in the owner run
/// (production tiling keeps them flat, not nested).
fn type_args_name_type_variables(c: &Check<'_>, ty: u32, vars: &[&str]) -> bool {
    let Some(parent) = c.unit.ast.parent(ty) else {
        return false;
    };
    let args = c
        .collect_direct(parent)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::TypeArguments);
    let Some(args) = args else {
        return false;
    };
    c.collect_direct(args).iter().any(|&arg| {
        c.kind(arg) == NodeKind::Type && vars.contains(&c.lexeme(c.head_token(arg)))
    })
}

/// `element.metadata.hasLiteral` (:49/:72): the resolved constructor's
/// own metadata names `literal`. The analyzer recognizes the annotation
/// only over the declaring-library gate — `_isPackageMetaGetter`
/// requires `element.library.name == 'meta'` — so the same-file
/// spelling read answers only when the library is meta (the
/// [`crate::rules::const_ability::spells_meta_library`] gate: the
/// mounted fold's resolution face, the written-directive scan as the
/// plain-run fallback). A corpus importing an unnamed vendored meta —
/// the live oracle's own face — never recognizes the annotation, and
/// the creation reports like any other.
fn ctor_has_literal(c: &Check<'_>, class_name: &str, selector: Option<&str>) -> bool {
    if !crate::rules::const_ability::spells_meta_library(c.unit, c.semantics()) {
        return false;
    }
    let Some(class_row) = same_file_declaration(c, class_name) else {
        return false;
    };
    let unnamed = selector.is_none() || selector == Some("new");
    let body = c
        .collect_direct(class_row)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::DeclarationBody);
    for member in body_members(c, class_row) {
        if c.kind(member) != NodeKind::Constructor {
            continue;
        }
        // The row anchors at the class name; a named constructor's
        // selector rides the `MethodDeclarationContinuation`
        // identifier (declarations.rs `constructor_name_part` idiom).
        if c.lexeme(c.head_token(member)) != class_name {
            continue;
        }
        let part = c.collect_direct(member).into_iter().find(|&kid| {
            c.kind(kid) == NodeKind::Identifier
                && c.payload(kid).first()
                    == Some(
                        &(dartq_parser::IdentifierContext::MethodDeclarationContinuation
                            as u32),
                    )
        });
        let matches = match (unnamed, part.map(|p| c.lexeme(c.head_token(p)))) {
            (true, None | Some("new")) => true,
            (false, Some(name)) => Some(name) == selector,
            _ => false,
        };
        if !matches {
            continue;
        }
        let Some(body) = body else { return false };
        return member_annotations(c, body, member)
            .iter()
            .any(|&meta| annotation_names_literal(c, meta));
    }
    false
}

/// Whether the metadata names `literal` (any segment of a prefix
/// chain — the `@literal` spelling).
fn annotation_names_literal(c: &Check<'_>, meta: u32) -> bool {
    c.collect_direct(meta).iter().any(|&piece| {
        c.kind(piece) == NodeKind::Identifier && c.lexeme(c.head_token(piece)) == "literal"
    })
}

/// Whether the token is a plain identifier (keywords excluded — `new`
/// and `const` never name a class).
fn is_identifier_lexeme(c: &Check<'_>, token: u32) -> bool {
    c.unit
        .tokens
        .get(token as usize)
        .is_some_and(|t| matches!(t.kind, dartq_scanner::TokenKind::Identifier))
}
