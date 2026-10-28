//! The collection-literal family: `prefer_inlined_adds`,
//! `prefer_spread_collections`, `prefer_collection_literals`,
//! `unnecessary_to_list_in_spreads`, the standalone
//! `prefer_foreach` and `prefer_if_elements_to_conditional_expressions`,
//! and `prefer_const_literals_to_create_immutables`.
//!
//! Cascade shape (production tiling, `rules::slice`): a [`NodeKind::Cascade`]
//! opens at the `..` before its first section parses, so the section's
//! invocation pieces tile flat inside the cascade window; the
//! [`NodeKind::CascadeAccess`] marker seals after the first section's send.
//! A cascaded section send's owner is therefore the cascade itself, and the
//! first section is the piece run before the marker.

use dartq_ast::NodeKind;

use crate::engine::Check;
use crate::registry::RuleSpec;
use crate::rules::TypeFace;
use crate::rules::slice;

/// The domain's incoming registrations: a new standalone rule appends its
/// `SPEC` reference here (name order) and joins the registry through the
/// aggregated tail — adding a rule touches only this file. Batch close
/// promotes the block into the registry's frozen skeleton together with
/// the pin-test wave list (`tests/registry_pin.rs`).
pub(crate) static INCOMING_RULES: &[&RuleSpec] = &[];

/// The written type arguments of the rows preceding the `limit` row within
/// `rows` (the creation/literal head's explicit argument list). Two tilings
/// carry them: inside the `TypeArguments` wrapper (the constructor
/// reference head), or as sibling `Type` rows beside the wrapper (the
/// literal head).
fn type_argument_faces<'a>(c: &'a Check<'_>, rows: &[u32], limit: u32) -> Vec<(&'a str, bool)> {
    let mut faces = Vec::new();
    let mut taken: Vec<u32> = Vec::new();
    let mut push = |arg: u32, taken: &mut Vec<u32>| {
        let head = c.head_token(arg);
        let nullable = c.byte_span(arg).end > crate::rules::byte_span_of_token(c, head).end;
        faces.push((c.lexeme(head), nullable));
        taken.push(arg);
    };
    for &row in rows {
        if row == limit {
            break;
        }
        match c.kind(row) {
            // The wrapper's own children (the creation-head tiling) — rows
            // the sibling pass below may already have carried (the literal
            // head tiles them beside the wrapper).
            NodeKind::TypeArguments | NodeKind::NoTypeArguments => {
                for arg in c.collect_direct(row) {
                    if c.kind(arg) == NodeKind::Type && !taken.contains(&arg) {
                        push(arg, &mut taken);
                    }
                }
            }
            // The literal-head tiling: the argument rows tile beside the
            // wrapper — take the ones its children did not already carry.
            NodeKind::Type if !taken.contains(&row) => {
                push(row, &mut taken);
            }
            _ => {}
        }
    }
    faces
}

/// The decided face of an untyped local's initializer, with the
/// user-container bit: `true` when the creation head resolves to an
/// indexed container — the user's own class, whose members never ride the
/// core tables (the caller bails where the official's resolution does).
struct InferredFace<'a> {
    face: TypeFace<'a>,
    user_container: bool,
}

/// The non-creation decided face (a literal root).
fn plain_face<'a>(name: &'a str, type_arguments: Vec<(&'a str, bool)>) -> InferredFace<'a> {
    InferredFace {
        face: TypeFace {
            name: Some(name),
            nullable: false,
            is_void: false,
            is_future_or_void: false,
            is_function_type: false,
            type_arguments,
        },
        user_container: false,
    }
}

/// The type face of the innermost untyped (`var`/`final`/`const` local)
/// binding of `name` at the use row, decided by its initializer root: an
/// instance creation carries the head spelling with its written type
/// arguments (flagged when the head resolves to an indexed container); a
/// typed collection literal carries its written type arguments (an
/// untyped `[]` answers the raw `List`/`Map` face — the official's
/// inferred `dynamic` suppresses every verdict through the callers'
/// empty-argument silences); a string literal carries `String`. `None`
/// for every other initializer shape (calls, chains — the callers keep
/// their documented postures) and without the semantic run.
fn inferred_binding_type_face<'a>(
    c: &'a Check<'_>,
    use_row: u32,
    name: &str,
) -> Option<InferredFace<'a>> {
    let use_start = c.byte_span(use_row).start;
    let analysis = c.semantics()?;
    for i in (0..c.path.len().saturating_sub(1)).rev() {
        let ancestor = c.path[i];
        if !matches!(c.kind(ancestor), NodeKind::Block | NodeKind::BlockBody) {
            continue;
        }
        for kid in c.collect_direct(ancestor) {
            if c.kind(kid) != NodeKind::VariablesDeclaration {
                continue;
            }
            for var in c
                .collect_direct(kid)
                .into_iter()
                .filter(|&var| c.kind(var) == NodeKind::InitializedIdentifier)
            {
                let binds = crate::rules::initialized_identifier_name(c, var)
                    .is_some_and(|tok| c.lexeme(tok) == name)
                    && c.byte_span(var).start < use_start;
                if !binds {
                    continue;
                }
                // The innermost binding decides; outer shadowed bindings
                // never answer past it (no initializer means no face). The
                // initializer extent runs from the `=` to the declarator's
                // last token (the row ends at a multi-declarator comma), and
                // the rows read the declaration's own completion window —
                // the production tiling nests the initializer pieces under
                // the declaration, not the block.
                let tokens = c.unit.ast.token_range(var);
                let Some(eq) = (tokens.start..tokens.end).find(|&t| c.lexeme(t) == "=") else {
                    return None;
                };
                let lo = crate::rules::byte_span_of_token(c, eq + 1).start;
                let hi = crate::rules::byte_span_of_token(c, tokens.end - 1).end;
                let rows: Vec<u32> = c
                    .children(var)
                    .filter(|&row| {
                        let span = c.byte_span(row);
                        span.start >= lo && span.end <= hi
                    })
                    .collect();
                let Some(root) = rows
                    .iter()
                    .copied()
                    .max_by_key(|&row| c.byte_span(row).end)
                    .or_else(|| rows.first().copied())
                else {
                    return None;
                };
                return match c.kind(root) {
                    NodeKind::ListLiteral => {
                        Some(plain_face("List", type_argument_faces(c, &rows, root)))
                    }
                    NodeKind::SetOrMapLiteral => {
                        let arguments = type_argument_faces(c, &rows, root);
                        let map = slice::literal_faces(c, root) == Some(slice::MAP)
                            || arguments.len() == 2;
                        Some(plain_face(if map { "Map" } else { "Set" }, arguments))
                    }
                    NodeKind::StringLiteral => Some(plain_face("String", Vec::new())),
                    // The creation root: the head identifier starts the run
                    // (a `new` keyword carries no row); a head resolving to
                    // an indexed container is the user's own class.
                    NodeKind::Send | NodeKind::NewExpression | NodeKind::ImplicitCreation
                        if rows
                            .first()
                            .is_some_and(|&head| c.kind(head) == NodeKind::Identifier) =>
                    {
                        let head = rows.first().copied()?;
                        let user_container = analysis
                            .resolve_constructor(
                                crate::rules::expressions::query_site(c, head),
                                None,
                            )
                            .is_some();
                        Some(InferredFace {
                            face: TypeFace {
                                name: Some(c.lexeme(c.head_token(head))),
                                nullable: false,
                                is_void: false,
                                is_future_or_void: false,
                                is_function_type: false,
                                type_arguments: type_argument_faces(c, &rows, root),
                            },
                            user_container,
                        })
                    }
                    _ => None,
                };
            }
        }
    }
    None
}

/// The direct sibling immediately before `id` (the cascade target slot).
fn sibling_before(c: &Check<'_>, id: u32) -> Option<u32> {
    // `id` is always on the current ancestor path (a cascade above the
    // dispatch node); its parent is the path entry right above it.
    let pos = c.path.iter().position(|&n| n == id)?;
    let parent = *c.path.get(pos.checked_sub(1)?)?;
    let kids = c.collect_direct(parent);
    let kid_pos = kids.iter().position(|&kid| kid == id)?;
    kid_pos.checked_sub(1).map(|prev| kids[prev])
}

/// The `(method name node, arguments node)` of a cascaded section's
/// invocation over the production tiling: the section's pieces tile flat
/// into the [`NodeKind::Cascade`] window (it opens at the `..` before the
/// section parses), with the [`NodeKind::CascadeAccess`] marker sealing
/// after the section's send. Rebuilt over the owner slice — the send reads
/// its parts through [`slice::send_parts`] (`rules::slice`).
fn invocation_parts(c: &Check<'_>, kids: &[u32], pos: usize) -> Option<(u32, u32)> {
    let (head, args) = slice::send_parts(c, kids, pos)?;
    let args = args?;
    if c.kind(kids[head]) != NodeKind::Identifier {
        return None;
    }
    Some((kids[head], args))
}

/// Whether the cascade section containing the send at `pos` opens the
/// cascade (`sections.first == invocation`, prefer_inlined_adds.dart:47-52):
/// the first section's pieces precede its [`NodeKind::CascadeAccess`]
/// marker; later sections follow it (or a DotAccess-led index section with
/// no marker at all, which is never a send-led first section).
fn is_first_section(c: &Check<'_>, kids: &[u32], pos: usize) -> bool {
    match kids
        .iter()
        .position(|&kid| c.kind(kid) == NodeKind::CascadeAccess)
    {
        Some(marker) => pos < marker,
        None => false,
    }
}

/// The official const-context walk (`constantContext`, analyzer
/// `ast.dart:17859`): climb the parent chain — a `VariableDeclarationList`
/// with the `const` keyword decides, const-carrying literal/creation rows
/// decide, plain expression parents pass through, anything else stops. The
/// production tiling relocates declaration rows past the statement
/// container (a cascade initializer's pieces tile at the container and the
/// declaration leaks beside them), so an exhausted path falls back to the
/// container's sibling declaration whose span contains the expression.
fn in_const_context(c: &Check<'_>, above: usize) -> bool {
    let send_span = c.byte_span(c.node());
    let const_kw = |row: u32| {
        crate::rules::variables_declaration_keyword(c, row)
            .is_some_and(|kw| c.lexeme(kw) == "const")
    };
    for depth in 0..above {
        match c.ancestor(depth) {
            Some(ancestor) => match c.kind(ancestor) {
                NodeKind::ConstExpression | NodeKind::ConstLiteral => return true,
                NodeKind::VariablesDeclaration => return const_kw(ancestor),
                NodeKind::BlockBody | NodeKind::Block => {
                    return c.collect_direct(ancestor).into_iter().any(|row| {
                        c.kind(row) == NodeKind::VariablesDeclaration && const_kw(row) && {
                            let span = c.byte_span(row);
                            span.start <= send_span.start && send_span.end <= span.end
                        }
                    });
                }
                _ => {}
            },
            None => return false,
        }
    }
    false
}

/// The shared gate: a first cascaded `add`/`addAll` section with one
/// argument on a list-literal target. Returns the method-name node and the
/// argument list node. The production tiling puts the section's pieces
/// flat inside the cascade window, so the send reads the owner slice and
/// `isCascaded` is the parent being the cascade.
fn cascaded_add(c: &Check<'_>, name: &str) -> Option<(u32, u32)> {
    let (kids, pos) = slice::owner_slice(c)?;
    // `invocation.isCascaded` (:27-29): a section send's owner is the
    // cascade itself.
    if c.kind(c.ancestor(1)?) != NodeKind::Cascade {
        return None;
    }
    let (method, args) = invocation_parts(c, &kids, pos)?;
    if c.lexeme(c.head_token(method)) != name {
        return None;
    }
    if !is_first_section(c, &kids, pos) {
        return None;
    }
    // The cascade target must be a list literal (:42-52 / :44-50).
    if !matches!(sibling_before(c, c.ancestor(1)?), Some(target) if c.kind(target) == NodeKind::ListLiteral)
    {
        return None;
    }
    // Exactly one argument (:28-29).
    let count = slice::arguments(c, &c.collect_direct(args)).len();
    if count != 1 {
        return None;
    }
    Some((method, args))
}

/// Whether the first argument is a list literal: the argument's subtree
/// root trails its piece group's leaked interior rows (separator-less
/// argument lists; completion order puts every strict container after the
/// rows it contains), with a leading type-argument wrapper possibly before
/// them — the root is the OUTERMOST run piece whose span contains the head
/// piece's span, i.e. the last match in completion order (a reverse walk):
/// an inner match would be an interior wrapper (`(…)` around the literal,
/// an index access on the literal's element), not the argument itself. The
/// head piece matches only itself, so an empty walk falls back to it
/// (root-leading tiling). A named argument is a `NamedExpression` — never
/// a literal (the official `arguments.first is! ListLiteral`).
fn first_argument_list_literal(c: &Check<'_>, args: u32) -> bool {
    let pieces = c.collect_direct(args);
    let groups = slice::arguments(c, &pieces);
    let Some(first) = groups.first() else {
        return false;
    };
    let group = &pieces[first.clone()];
    let rest = match group {
        [tyargs, rest @ ..]
            if matches!(
                c.kind(*tyargs),
                NodeKind::NoTypeArguments | NodeKind::TypeArguments
            ) =>
        {
            rest
        }
        _ => group,
    };
    let Some(&run_head) = rest.first() else {
        return false;
    };
    let head_span = c.byte_span(run_head);
    let head = rest
        .iter()
        .copied()
        .rev()
        .find(|&piece| {
            let span = c.byte_span(piece);
            span.start <= head_span.start && head_span.end <= span.end
        })
        .unwrap_or(run_head);
    c.kind(head) == NodeKind::ListLiteral
}

pub(crate) mod prefer_inlined_adds {
    use super::{Check, NodeKind, RuleSpec, cascaded_add, first_argument_list_literal};

    /// Official: `pkg/linter/lib/src/rules/prefer_inlined_adds.dart:22-58` —
    /// the first cascaded `add`/`addAll` on a list literal with one argument
    /// reports at the method name; `addAll` carries the multiple-elements
    /// template (only when its argument is a list literal — a spread argument
    /// belongs to `prefer_spread_collections`, :53-55). Message templates:
    /// `diagnostic.g.dart` `preferInlinedAddsMultiple`/`preferInlinedAddsSingle`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_inlined_adds",
    &[NodeKind::Send],
    check,
);

    fn check(c: &mut Check<'_>) {
        let Some((method, args)) = cascaded_add(c, "add").or_else(|| cascaded_add(c, "addAll"))
        else {
            return;
        };
        let add_all = c.lexeme(c.head_token(method)) == "addAll";
        if add_all && !first_argument_list_literal(c, args) {
            // A list-literal argument is inlined here; anything else is
            // the spread rule's finding (:53-55).
            return;
        }
        let range = c.utf16_span(method);
        if add_all {
            c.report_texts(
                range,
                "The addition of multiple list items could be inlined.",
                "Try adding the items to the list literal directly.",
                &[],
            );
        } else {
            c.report_texts(
                range,
                "The addition of a list item could be inlined.",
                "Try adding the item to the list literal directly.",
                &[],
            );
        }
    }
}

pub(crate) mod prefer_spread_collections {
    use super::{Check, NodeKind, RuleSpec, cascaded_add, first_argument_list_literal, in_const_context};

    /// Official: `pkg/linter/lib/src/rules/prefer_spread_collections.dart:22-55`
    /// — the first cascaded `addAll` on a non-constant list literal whose
    /// single argument is NOT a list literal (that form is
    /// `prefer_inlined_adds`'s) reports at the method name. Message:
    /// `diagnostic.g.dart` `preferSpreadCollections`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_spread_collections",
    &[NodeKind::Send],
    check,
);

    fn check(c: &mut Check<'_>) {
        let Some((method, args)) = cascaded_add(c, "addAll") else {
            return;
        };
        // `target.inConstantContext` (:46-49) — the official
        // `constantContext` walk, mirrored in [`in_const_context`].
        if in_const_context(c, c.path.len()) {
            return;
        }
        if first_argument_list_literal(c, args) {
            // Handled by prefer_inlined_adds (:50-53).
            return;
        }
        let range = c.utf16_span(method);
        c.report(range, &[]);
    }
}

pub(crate) mod prefer_collection_literals {
    use super::{Check, NodeKind, RuleSpec, slice};
    use crate::rules::expressions::same_file;

    const PROBLEM: &str = "Unnecessary constructor invocation.";
    const CORRECTION: &str = "Try using a collection literal.";

    /// Official: `pkg/linter/lib/src/rules/prefer_collection_literals.dart`
    /// :41-95 — no-arg `Map()`/`Set()` creations, `Set.from([…])`/`Set.of([…])`
    /// with one list literal (:75-81), and a `.toSet()` on a list literal
    /// (:87-95). The official pins the types through the element model
    /// (`staticType.isDartCoreMap`/`.isDartCoreSet`, :99-105; the
    /// `isHashMap`/`isHashSet` faces match the `Linked*` forms, :28-32
    /// `isSameAs('LinkedHashMap', 'dart.collection')`) — so a written
    /// `HashMap()`/`HashSet()`, whose static type is the unrelated
    /// dart:collection class, never reports, and the `Linked*` faces exempt
    /// a matching context type (:85-88/:101-104).
    ///
    /// Element approximation: a unit-local declaration of the name (a
    /// typedef, a user class — the `TypeAliasElement` exemption :44-46 and
    /// user shadows) bails through the same-file index, and a `Map`/`Set`
    /// spelling resolving to another library's unit-declared container
    /// bails through the semantic-lane probe (a core spelling resolves to
    /// no unit). The `Linked*`
    /// context exemption reads the declared type around the creation
    /// (declaration type-before, method return type); an assignment or
    /// argument position bails — the context type is not derivable there
    /// (the conservative family). Prefixed callees (`core.Map()`) never
    /// match. Message: `diagnostic.g.dart` `preferCollectionLiterals`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_collection_literals",
    &[
            NodeKind::Send,
            NodeKind::NewExpression,
            NodeKind::ImplicitCreation,
        ],
    check,
);

    /// The unnamed faces (`isMap` :99-100, `isHashMap` :28-32, `isSet`,
    /// `isHashSet`): the written names whose static type is the pinned core
    /// type. `HashMap`/`HashSet` are absent — their static type is the
    /// unrelated dart:collection class.
    const UNNAMED: &[&str] = &["Map", "LinkedHashMap", "Set", "LinkedHashSet"];

    /// The named `from`/`of` faces (:75-81) ride the exact core set type.
    const NAMED_SET: &[&str] = &["Set", "LinkedHashSet"];

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let report = match c.kind(node) {
            // The explicit-new shape owns its callee and argument list.
            NodeKind::NewExpression | NodeKind::ImplicitCreation => {
                let kids = c.collect_direct(node);
                let Some(args) = kids
                    .iter()
                    .copied()
                    .find(|&kid| c.kind(kid) == NodeKind::Arguments)
                else {
                    return;
                };
                owned_creation(c, &kids, args)
            }
            // A plain send owns nothing — rebuild from the owner slice.
            NodeKind::Send => match slice::owner_slice(c) {
                Some((kids, pos)) => send_face(c, &kids, pos),
                None => false,
            },
            _ => false,
        };
        if !report {
            return;
        }
        let range = c.utf16_span(node);
        c.report_texts(range, PROBLEM, CORRECTION, &[]);
    }

    /// The unnamed-creation face over an owned child list (`new Map()`):
    /// the callee is the identifier before the argument list — the
    /// explicit-new shape carries a `ConstructorReference` marker owning
    /// the name.
    fn owned_creation(c: &Check<'_>, kids: &[u32], args: u32) -> bool {
        let Some(args_pos) = kids.iter().position(|&kid| kid == args) else {
            return false;
        };
        let Some((callee, name)) =
            kids[..args_pos]
                .iter()
                .rev()
                .find_map(|&kid| match c.kind(kid) {
                    NodeKind::Identifier => Some((kid, c.lexeme(c.head_token(kid)))),
                    NodeKind::ConstructorReference => c
                        .collect_direct(kid)
                        .into_iter()
                        .find(|&k| c.kind(k) == NodeKind::Identifier)
                        .map(|k| (k, c.lexeme(c.head_token(k)))),
                    _ => None,
                })
        else {
            return false;
        };
        unnamed_face(c, callee, name, args)
    }

    /// The unnamed faces (`constructorName == null && arguments.isEmpty`,
    /// :57-59/:68-71): written empty parentheses only — a `NoArguments`
    /// send is a tear-off, no creation. The `Linked*` faces add the
    /// context-type exemption (:85-88/:101-104).
    fn unnamed_face(c: &Check<'_>, callee: u32, name: &str, args: u32) -> bool {
        if !UNNAMED.contains(&name) || c.kind(args) != NodeKind::Arguments {
            return false;
        }
        // No arguments beyond the callee's type-argument wrapper.
        if c.collect_direct(args).iter().any(|&piece| {
            !matches!(
                c.kind(piece),
                NodeKind::NoTypeArguments | NodeKind::TypeArguments
            )
        }) {
            return false;
        }
        if same_file::declares_name(c, name) || cross_file_collection_head(c, callee) {
            return false;
        }
        if name == "LinkedHashMap" || name == "LinkedHashSet" {
            return context_reports(c, name);
        }
        true
    }

    /// The send face over the owner slice with the invocation send at
    /// `pos`: the argument list sits immediately before it and the callee
    /// head before that (skipping the type-argument wrapper). A trailing
    /// same-span `DotAccess` marker switches to the member faces. A
    /// `NoArguments` send is a tear-off — no invocation, no face.
    fn send_face(c: &Check<'_>, kids: &[u32], pos: usize) -> bool {
        let Some((head, args)) = slice::send_parts(c, kids, pos) else {
            return false;
        };
        if c.kind(kids[head]) != NodeKind::Identifier {
            return false;
        }
        let selector = c.lexeme(c.head_token(kids[head]));
        let access = slice::access_marker(c, kids, pos, NodeKind::DotAccess);
        match (access, args) {
            (None, Some(args)) => unnamed_face(c, kids[head], selector, args),
            (Some(_), Some(args)) if selector == "from" || selector == "of" => {
                named_face(c, kids, head, args)
            }
            (Some(access), Some(_)) if selector == "toSet" => to_set_face(c, kids, head, access),
            _ => false,
        }
    }

    /// The `Set.from([…])`/`Set.of([…])` faces (:75-81): the receiver is
    /// the bare-identifier tear-off group ending at the selector, and the
    /// single argument is a list literal.
    fn named_face(c: &Check<'_>, kids: &[u32], sel: usize, args: u32) -> bool {
        let Some(group_start) = sel.checked_sub(4) else {
            return false;
        };
        if !slice::is_tear_off_group(c, kids, group_start) {
            return false;
        }
        let written = c.lexeme(c.head_token(kids[group_start]));
        if !NAMED_SET.contains(&written)
            || same_file::declares_name(c, written)
            || cross_file_collection_head(c, kids[group_start])
        {
            return false;
        }
        single_list_literal(c, args)
    }

    /// Whether the creation head's spelling resolves to a unit-declared
    /// container — another library's `class Map`/`class Set` is never the
    /// pinned core type (`staticType.isDartCoreMap`/`.isDartCoreSet`),
    /// so the official stays silent on its creations. The probe is the
    /// semantic lane's own resolution faces on the callee row's span: the
    /// creation-type use (`resolve_constructor`, any constructor) or the
    /// bare reference's binding (`BindingKind::Decl`) — a workspace
    /// declaration only, the synthetic SDK face (`is_sdk_binding`) being
    /// the core symbol itself and never a bail. Without the semantic run
    /// the plain-run posture holds — no bail.
    fn cross_file_collection_head(c: &Check<'_>, callee: u32) -> bool {
        let Some(analysis) = c.semantics() else {
            return false;
        };
        let site = crate::rules::expressions::query_site(c, callee);
        analysis.resolve_constructor(site, None).is_some()
            || analysis.binding_of(site).is_some_and(|binding| {
                !analysis.is_sdk_binding(&binding)
                    && binding.kind == dartq_semantic::BindingKind::Decl
            })
    }

    /// `['a'].toSet()` (:87-95): a list literal ends right before the dot —
    /// the literal's span ends one code unit before the selector (the dot
    /// between), so the pieces of a foreign expression never bind.
    fn to_set_face(c: &Check<'_>, kids: &[u32], sel: usize, _access: u32) -> bool {
        let selector_start = c.utf16_span(kids[sel]).start;
        kids.iter().any(|&piece| {
            c.kind(piece) == NodeKind::ListLiteral && c.utf16_span(piece).end + 1 == selector_start
        })
    }

    /// The single-list-literal test over the flattened argument pieces: a
    /// literal's contents tile out beside it, so pieces the literal's span
    /// covers fold into it (:76-78).
    fn single_list_literal(c: &Check<'_>, args: u32) -> bool {
        let pieces = c.collect_direct(args);
        let mut count = 0usize;
        for (i, &piece) in pieces.iter().enumerate() {
            if i == 0
                && matches!(
                    c.kind(piece),
                    NodeKind::NoTypeArguments | NodeKind::TypeArguments
                )
            {
                continue;
            }
            if pieces.iter().any(|&other| {
                other != piece
                    && c.kind(other) == NodeKind::ListLiteral
                    && c.utf16_span(other).start <= c.utf16_span(piece).start
                    && c.utf16_span(piece).end <= c.utf16_span(other).end
            }) {
                continue;
            }
            if c.kind(piece) != NodeKind::ListLiteral {
                return false;
            }
            count += 1;
        }
        count == 1
    }

    /// The `Linked*` context exemption (:85-88/:101-104): the context type
    /// decides — a declaration whose written type matches the creation is
    /// exempt, an untyped (`var`/`final`) declaration is the official's
    /// invalid context, any other declared type reports; an assignment or
    /// argument position bails (the context type is not derivable — the
    /// conservative family).
    fn context_reports(c: &Check<'_>, name: &str) -> bool {
        let len = c.path.len();
        for i in (0..len.saturating_sub(1)).rev() {
            match c.kind(c.path[i]) {
                NodeKind::VariablesDeclaration => {
                    // The declared type tiles in the owner slices above the
                    // declaration (block / fields): the binding type is the
                    // last one ending within the declaration — a later
                    // declaration's type ends past it.
                    let decl = c.path[i];
                    let decl_end = c.byte_span(decl).end;
                    let written = c.path[..=i]
                        .iter()
                        .rev()
                        .flat_map(|owner| c.collect_direct(*owner))
                        .filter(|&kid| {
                            c.kind(kid) == NodeKind::Type && c.byte_span(kid).end <= decl_end
                        })
                        .max_by_key(|&kid| c.byte_span(kid).start)
                        .map(|t| c.lexeme(c.head_token(t)));
                    return match written {
                        // Matching or invalid (`var`) context: exempt.
                        None => false,
                        Some(t) => t != name,
                    };
                }
                NodeKind::Assignment | NodeKind::Arguments => return false,
                NodeKind::Method | NodeKind::TopLevelMethod => {
                    let written = c
                        .collect_direct(c.path[i])
                        .into_iter()
                        .find(|&kid| c.kind(kid) == NodeKind::Type);
                    return match written.map(|t| c.lexeme(c.head_token(t))) {
                        None => true,
                        Some(t) => t != name,
                    };
                }
                _ => {}
            }
        }
        true
    }
}

pub(crate) mod unnecessary_to_list_in_spreads {
    use super::{Check, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/unnecessary_to_list_in_spreads.dart:37-47`
    /// — a spread of `x.toList()` reports at the method name.
    ///
    /// Approximation (documented): the official requires the receiver to
    /// implement `Iterable` (`implementsInterface('Iterable', 'dart.core')`,
    /// :45) — a resolution fact. A plain-identifier receiver resolves its
    /// same-file written type and must land in the Iterable family; an
    /// untyped binding decides through its initializer
    /// ([`inferred_binding_type_face`] — a user-class creation or a map
    /// literal stays silent like the official's non-Iterable read); any
    /// other receiver shape (chains, call initializers, untyped literals)
    /// carries no verdict and keeps the presumption that valid spread code
    /// only spreads Iterables. Message:
    /// `diagnostic.g.dart` `unnecessaryToListInSpreads`.
    ///
    /// Tree shape: the bridge seals `Spread` as a childless leaf on the
    /// `...` token and the operand tiles flat beside it — receiver,
    /// selector, arguments, then the `Send` leaf and the trailing access
    /// marker (the production send tiling `send_parts` reads).
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_to_list_in_spreads",
    &[NodeKind::Spread],
    check,
);

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let Some(parent) = c.unit.ast.parent(node) else {
            return;
        };
        let siblings = c.collect_direct(parent);
        let Some(spread_pos) = siblings.iter().position(|&sibling| sibling == node) else {
            return;
        };
        // The operand run: production tiles the operand's pieces BEFORE the
        // spread leaf (the element cluster from the previous spread marker
        // or the statement start — the same piece run every statement-level
        // walk reads); hand-built trees nest it after. Both segments face
        // the same invocation scan.
        let mut start = siblings[..spread_pos]
            .iter()
            .rposition(|&sibling| crate::rules::is_statement_row(c, sibling))
            .map_or(0, |i| i + 1);
        if let Some(i) = siblings[start..spread_pos]
            .iter()
            .rposition(|&sibling| c.kind(sibling) == NodeKind::Spread)
        {
            start += i + 1;
        }
        let before = &siblings[start..spread_pos];
        let after = siblings[spread_pos + 1..]
            .split(|&sibling| c.kind(sibling) == NodeKind::Spread)
            .next()
            .unwrap_or(&[]);
        for run in [before, after] {
            if scan(c, run) {
                return;
            }
        }
    }

    /// The invocation scan over one operand segment: the root is the LAST
    /// send (a member access chain's inner accessor sends tile before the
    /// invocation's own), the argument list rides before it, and the
    /// selector sits behind the type-argument wrapper. Returns `true` when
    /// the segment settled the spread (reported or bailed) — `false` hands
    /// the decision to the next segment.
    fn scan(c: &mut Check<'_>, run: &[u32]) -> bool {
        // `expression is MethodInvocation` — the invocation root; a bare
        // spread or a parenthesized/composite operand has none.
        let Some(send_pos) = run.iter().rposition(|&row| c.kind(row) == NodeKind::Send) else {
            return false;
        };
        // The argument list rides before the send; a tear-off (`x.toList`)
        // is no MethodInvocation.
        let Some(arguments) = send_pos.checked_sub(1).and_then(|i| run.get(i)) else {
            return false;
        };
        if c.kind(*arguments) != NodeKind::Arguments {
            return false;
        }
        // `methodName.name == 'toList'` (:45), behind optional
        // type-argument wrappers.
        let Some(mut head) = send_pos.checked_sub(2) else {
            return false;
        };
        while matches!(
            c.kind(run[head]),
            NodeKind::NoTypeArguments | NodeKind::TypeArguments
        ) {
            let Some(prev) = head.checked_sub(1) else {
                return false;
            };
            head = prev;
        }
        if c.kind(run[head]) != NodeKind::Identifier
            || c.lexeme(c.head_token(run[head])) != "toList"
        {
            return false;
        }
        let report = |c: &mut Check<'_>| {
            let range = c.utf16_span(run[head]);
            c.report(range, &[]);
            true
        };
        // `target != null` (:45): the receiver tile precedes the selector.
        // Hand-built nests the bare name directly; the production member
        // access carries the accessor cluster (`NoTypeArguments('.')`,
        // `NoArguments`, `Send('x.')`) between the target and the selector.
        let (receiver, receiver_pos) = if head > 0 && c.kind(run[head - 1]) == NodeKind::Identifier
        {
            (run[head - 1], head - 1)
        } else if head >= 4
            && matches!(
                c.kind(run[head - 3]),
                NodeKind::NoTypeArguments | NodeKind::TypeArguments
            )
            && c.kind(run[head - 2]) == NodeKind::NoArguments
            && c.kind(run[head - 1]) == NodeKind::Send
        {
            (run[head - 4], head - 4)
        } else {
            // `target == null` (:45): a receiverless `toList()` is no
            // member invocation — the official bail.
            return true;
        };
        // The gate reads bare-name receivers only — a chain's last spelling
        // (`a.b.toList()` — the cluster target is `b`) carries the chain's
        // static type, not the spelling's, and keeps the presumption. The
        // bare target must open the operand (an enclosing literal's opener
        // marker may precede it).
        let boundary = match run.first() {
            Some(&first)
                if matches!(
                    c.kind(first),
                    NodeKind::NoTypeArguments | NodeKind::TypeArguments
                ) =>
            {
                1 // the enclosing literal's opener marker
            }
            _ => 0,
        };
        if receiver_pos != boundary {
            return report(c);
        }
        let name = c.lexeme(c.head_token(receiver));
        if let Some(written) = crate::rules::expressions::same_file::written_type(c, receiver, name)
        {
            // A same-file type owning the spelling is not the core type,
            // and a written non-family type is provably outside Iterable.
            if crate::rules::expressions::same_file::declares_type(c, written)
                || !matches!(written, "Iterable" | "List" | "Set" | "Queue")
            {
                return true;
            }
        } else if let Some(inferred) = super::inferred_binding_type_face(c, receiver, name)
            && (inferred.user_container
                || !matches!(
                    inferred.face.name,
                    Some("Iterable" | "List" | "Set" | "Queue")
                ))
        {
            // The initializer decided a non-Iterable face (a user class or
            // a map literal): the official's static-type read stays silent.
            return true;
        }
        report(c)
    }
}

pub(crate) mod prefer_iterable_where_type {
    use super::{Check, NodeKind, RuleSpec};
    use crate::rules::{NONE_TOKEN, slice};

    /// Official: `pkg/linter/lib/src/rules/prefer_iterable_whereType.dart:31-83`
    /// — `x.where((e) => e is T)` (or the single-return block form) reports
    /// at the method name.
    ///
    /// The receiver type check (`target.staticType.implementsInterface
    /// ('Iterable')` with a `target == null` bail, :39-42) reads through
    /// the shared resolution-backed face ([`slice::resolved_receiver_family`])
    /// — collection literals decide, a bare-identifier receiver resolves
    /// its written type and must land in the `Iterable` family, anything
    /// else stays silent like the official's unresolvable-target bail. A
    /// written family name resolving to another library's unit-declared
    /// container is the user's static type, not the core `Iterable` — the
    /// semantic-lane probe ([`cross_file_family_receiver`]) bails it. The
    /// closure shape is the official's own lexical test (:50-71): one
    /// parameter, an expression body (or single return) whose
    /// unparenthesized expression is `e is T` (no `is!`) with the
    /// parameter as the tested expression. Message: `diagnostic.g.dart`
    /// `preferIterableWheretype`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_iterable_wheretype",
    &[NodeKind::Send],
    check,
);

    fn check(c: &mut Check<'_>) {
        let Some((kids, pos)) = slice::owner_slice(c) else {
            return;
        };
        // The member face proves a target (a bare `where(f)` has none,
        // :47-49): the send parts' selector identifier carries the check
        // (a statement-position call may tile without the marker row).
        let _ = slice::access_marker(c, &kids, pos, NodeKind::DotAccess);
        let Some((head, arguments)) = slice::send_parts(c, &kids, pos) else {
            return;
        };
        let name = kids[head];
        // `node.methodName.name != 'where'` bails (:47-48).
        if c.kind(name) != NodeKind::Identifier || c.lexeme(c.head_token(name)) != "where" {
            return;
        }
        // The receiver type check (:39-42): a receiver without a same-file
        // verdict stays silent (the official `target == null` face).
        if slice::resolved_receiver_family(c, &kids, head, slice::ITERABLE) != Some(true) {
            return;
        }
        // The cross-file shadow face: a written family name that resolves
        // to a unit-declared type (another library's `class List`) carries
        // the user's static type, and the official
        // `implementsInterface('Iterable', 'dart.core')` read stays silent
        // on it. The semantic-lane probe keeps the plain run's posture.
        if cross_file_family_receiver(c, &kids, head) {
            return;
        }
        // `args.length != 1` bails (:50-51); `arg is FunctionExpression`
        // (:52) — the argument list's subtree roots lead with the call's
        // type-argument wrapper (an empty `NoTypeVariables` row on the
        // common shape), so the sole argument is the single root after it.
        let Some(arguments) = arguments else {
            return;
        };
        let mut args = c.collect_direct(arguments);
        args.retain(|&root| {
            !matches!(
                c.kind(root),
                NodeKind::NoTypeVariables | NodeKind::TypeVariables
            )
        });
        if args.len() != 1 || c.kind(args[0]) != NodeKind::FunctionExpression {
            return;
        }
        let closure = args[0];
        if !is_type_test_closure(c, closure) {
            return;
        }
        let range = c.utf16_span(name);
        c.report(range, &[]);
    }

    /// Whether the bare receiver's written family name (`Iterable`, `List`,
    /// `Set`, `Queue`) resolves to a unit-declared type — another library's
    /// `class List` shadows the core spelling, and the official
    /// `target.staticType.implementsInterface('Iterable', 'dart.core')`
    /// stays silent on it. The probe is the semantic-lane constructor
    /// resolution on the written type's own span (the same-file shadow
    /// gate has already run); a core spelling resolves to no unit and
    /// keeps the report. Without the semantic run (`c.semantics()` is
    /// `None`) the plain-run posture holds — no bail.
    fn cross_file_family_receiver(c: &Check<'_>, pieces: &[u32], name: usize) -> bool {
        let Some(analysis) = c.semantics() else {
            return false;
        };
        let Some(row) = slice::bare_receiver(c, pieces, name) else {
            return false;
        };
        if !matches!(
            c.lexeme(c.head_token(row)),
            "Iterable" | "List" | "Set" | "Queue"
        ) {
            return false;
        }
        let Some((_, Some(type_row))) =
            crate::rules::expressions::same_file::binding(c, row, c.lexeme(c.head_token(row)))
        else {
            return false;
        };
        analysis
            .resolve_constructor(crate::rules::expressions::query_site(c, type_row), None)
            .is_some()
    }

    /// The closure shape (:53-81): one parameter, an expression body (or a
    /// single-return block body) whose unparenthesized expression is `e is T`
    /// (no `is!`) with the parameter as the tested expression. The container
    /// windows read as raw row ranges (descendants included): the
    /// `IsExpression` window opens at the operator, so the tested expression
    /// tiles BEFORE it and the type sits inside — the tested rows are the
    /// container rows ending at the `is` token.
    fn is_type_test_closure(c: &Check<'_>, closure: u32) -> bool {
        let kids = c.collect_direct(closure);
        let Some(&params) = kids
            .iter()
            .find(|&&kid| c.kind(kid) == NodeKind::FormalParameters)
        else {
            return false;
        };
        // One parameter: exactly one `FormalParameter` container row in the
        // flattened parameter window.
        let parameters: Vec<u32> = c
            .collect_direct(params)
            .into_iter()
            .filter(|&kid| c.kind(kid) == NodeKind::FormalParameter)
            .collect();
        if parameters.len() != 1 {
            return false;
        }
        let Some(param_name) = parameter_name(c, parameters[0]) else {
            return false;
        };
        // `hasSingleExpressionBody`: an expression body, or one `return`
        // statement. The body rows tile under the closure between the
        // parameter subtree and the body root — the `ExpressionBody` row's
        // own window is empty (it pins the `=>` token) and a statement row
        // is a childless span leaf, so neither carries the pieces.
        let Some(body_root) = kids
            .iter()
            .copied()
            .find(|&kid| matches!(c.kind(kid), NodeKind::ExpressionBody | NodeKind::BlockBody))
        else {
            return false;
        };
        let mut rows: Vec<u32> = (params + 1..body_root).collect();
        if c.kind(body_root) == NodeKind::BlockBody {
            // The sole statement is the block slice's tail: the return's
            // statement leaf completes last (`statements.length != 1`
            // bails, :56 — an earlier statement's leaf rows count too).
            match rows.last() {
                Some(&last) if c.kind(last) == NodeKind::ReturnStatement => {}
                _ => return false,
            }
            if rows
                .iter()
                .filter(|&row| crate::rules::is_statement_row(c, *row))
                .count()
                != 1
            {
                return false;
            }
            rows.pop();
        }
        // `(e) => e is int` / `(e) { return e is int; }`: the body
        // expression's ROOT completes last in the scan window — the
        // official reads the body expression itself (`expression is
        // IsExpression` after `unParenthesized`, :64-65), so an is-test
        // nested deeper (a binary arm, a conditional branch, a nested
        // closure's body) is not the body and never fires. Each
        // `(...)` wrapper completes right after its content, so the
        // unparenthesized root is the last row with one step back per
        // wrapper layer.
        let Some(mut root) = rows.len().checked_sub(1) else {
            return false;
        };
        while c.kind(rows[root]) == NodeKind::ParenthesizedExpression {
            let Some(prev) = root.checked_sub(1) else {
                return false;
            };
            root = prev;
        }
        if c.kind(rows[root]) != NodeKind::IsExpression {
            return false;
        }
        let expression = rows[root];
        // `expression.notOperator == null` (:71).
        if c.payload(expression).first().copied().unwrap_or(0) != 0 {
            return false;
        }
        // The tested expression tiles before the `is` token (the type rows
        // start after it); a single trailing `(...)` wrapper unwraps.
        let is_byte = c.byte_span(expression).start;
        let tested_end = rows.partition_point(|&row| c.byte_span(row).end <= is_byte);
        let mut tested = &rows[..tested_end];
        if let Some(&last) = tested.last()
            && c.kind(last) == NodeKind::ParenthesizedExpression
        {
            tested = &tested[..tested.len() - 1];
        }
        tested.len() == 1
            && c.kind(tested[0]) == NodeKind::Identifier
            && c.lexeme(c.head_token(tested[0])) == param_name
    }

    /// A simple formal parameter's name (`FormalParameter` payload word 1).
    fn parameter_name<'a>(c: &'a Check<'a>, parameter: u32) -> Option<&'a str> {
        if c.kind(parameter) != NodeKind::FormalParameter {
            return None;
        }
        let name = c.payload(parameter).get(1).copied()?;
        (name != NONE_TOKEN).then(|| c.lexeme(name))
    }
}

pub(crate) mod prefer_for_elements_to_map_from_iterable {
    use super::{Check, NodeKind, RuleSpec};
    use crate::rules::{first_child, slice};

    /// Official: `pkg/linter/lib/src/rules/prefer_for_elements_to_map_fromIterable.dart:31-90`
    /// — a `Map.fromIterable(a, key: …, value: …)` creation whose key and
    /// value arguments are single-parameter single-expression closures
    /// reports at the creation.
    ///
    /// Constructor element pin (`element.enclosingElement !=
    /// typeProvider.mapElement`, :43-45) over the same-file channel: a
    /// same-file declaration owning the `Map` name (class, typedef, value)
    /// resolves the creation to user code and stays silent, and so does a
    /// `Map` spelling resolving to another library's unit-declared
    /// container (the semantic-lane probe — a core `Map` resolves to no
    /// unit). Closure shape
    /// (:71-88): a named argument whose expression (no parens) is a
    /// function expression with `parameters.length == 1 &&
    /// parameters.first.isRequired` — a lone direct positional row is
    /// required, an optional/named group only when its member carries the
    /// `required` keyword — and a single expression body (or one return).
    /// Message: `diagnostic.g.dart` `preferForElementsToMapFromiterable`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_for_elements_to_map_fromiterable",
    &[NodeKind::Send, NodeKind::ImplicitCreation],
    check,
);

    /// The `required` modifier bit of a `FormalParameter` row (bridge
    /// `mod_bits`; the parser lane owns the values).
    const REQUIRED_BIT: u16 = 1 << 7;

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // The invocation run: the production bridge keeps the send a
        // childless leaf, so its pieces tile in the owner slice (the
        // statement run before it) — `slice::send_parts` reads the argument
        // list and the selector; hand-built trees nest [Qualified,
        // Arguments] under the send row.
        let nested = c.collect_direct(node);
        let mut map_row = None;
        let (kids, args_pos, matched): (Vec<u32>, usize, bool) =
            if nested.iter().any(|&kid| c.kind(kid) == NodeKind::Arguments) {
                let Some(args_pos) = nested
                    .iter()
                    .position(|&kid| c.kind(kid) == NodeKind::Arguments)
                else {
                    return;
                };
                let matched = match args_pos.checked_sub(1).and_then(|i| nested.get(i)).copied() {
                    Some(callee) if c.kind(callee) == NodeKind::Qualified => {
                        let qualified = c.collect_direct(callee);
                        let (Some(type_name), Some(ctor)) =
                            (qualified.first().copied(), qualified.last().copied())
                        else {
                            return;
                        };
                        map_row = Some(type_name);
                        c.kind(type_name) == NodeKind::Identifier
                            && c.kind(ctor) == NodeKind::Identifier
                            && c.lexeme(c.head_token(type_name)) == "Map"
                            && c.lexeme(c.head_token(ctor)) == "fromIterable"
                    }
                    // The production typed-creation face: the explicit type
                    // arguments make the head a constructor reference
                    // (`Map<int, int>` tiles under it — the head spelling
                    // first, the selector last).
                    Some(callee) if c.kind(callee) == NodeKind::ConstructorReference => {
                        let reference = c.collect_direct(callee);
                        let (Some(type_name), Some(ctor)) =
                            (reference.first().copied(), reference.last().copied())
                        else {
                            return;
                        };
                        map_row = Some(type_name);
                        c.kind(type_name) == NodeKind::Identifier
                            && c.kind(ctor) == NodeKind::Identifier
                            && c.lexeme(c.head_token(type_name)) == "Map"
                            && c.lexeme(c.head_token(ctor)) == "fromIterable"
                    }
                    _ => false,
                };
                (nested, args_pos, matched)
            } else {
                let Some((owner, pos)) = slice::owner_slice(c) else {
                    return;
                };
                let start = owner[..pos]
                    .iter()
                    .rposition(|&row| crate::rules::is_statement_row(c, row))
                    .map_or(0, |i| i + 1);
                let mut owner = owner;
                owner.drain(..start);
                let pos = pos - start;
                if pos == 0 || c.kind(owner[pos - 1]) != NodeKind::Arguments {
                    return;
                }
                let Some((head, Some(_))) = slice::send_parts(c, &owner, pos) else {
                    return;
                };
                // `Map.fromIterable` through the production pieces: the
                // selector identifier over the bare `Map` receiver — the
                // receiver run's trailing type-argument wrappers strip
                // first (`Map<int, int>.fromIterable` resolves to the same
                // map element).
                let access = slice::access_run(c, &owner[..head]);
                let mut heads: &[u32] = access.as_slice();
                while let Some((last, rest)) = heads.split_last()
                    && matches!(
                        c.kind(*last),
                        NodeKind::NoTypeArguments | NodeKind::TypeArguments
                    )
                {
                    heads = rest;
                }
                let matched = c.lexeme(c.head_token(owner[head])) == "fromIterable"
                    && matches!(
                        heads,
                        [only]
                            if c.kind(*only) == NodeKind::Identifier
                                && c.lexeme(c.head_token(*only)) == "Map"
                    );
                map_row = access.first().copied();
                (owner, pos - 1, matched)
            };
        if !matched {
            return;
        }
        // The constructor element pin: a same-file `Map` declaration is
        // not typeProvider's map element — and neither is another
        // library's `class Map` (the semantic-lane probe; a core `Map`
        // resolves to no unit).
        if crate::rules::expressions::same_file::declares_name(c, "Map") {
            return;
        }
        if let Some(map_row) = map_row
            && cross_file_map_head(c, map_row)
        {
            return;
        }
        // `arguments.length != 3` bails (:57-59). The argument list's
        // children are the arguments' subtree-end rows (a named argument is
        // the `name :` marker row plus its value sibling), so the count and
        // the two named markers read through the grouping helper.
        let pieces = c.collect_direct(kids[args_pos]);
        let groups = slice::arguments(c, &pieces);
        if groups.len() != 3 {
            return;
        }
        // The key/value closure extraction (:61-67, the TODO's named-args-
        // anywhere face kept positional).
        let second = pieces[groups[1].end - 1];
        let third = pieces[groups[2].end - 1];
        let key = extract_closure(c, "key", second).or_else(|| extract_closure(c, "key", third));
        let value =
            extract_closure(c, "value", third).or_else(|| extract_closure(c, "value", second));
        if key.is_none() || value.is_none() {
            return;
        }
        let range = c.utf16_span(node);
        c.report(range, &[]);
    }

    /// Whether the creation head's `Map` spelling resolves to a
    /// unit-declared container — another library's `class Map` is never
    /// `typeProvider.mapElement`, so the official's element pin (:43-45)
    /// stays silent on it. The probe is the semantic lane's own resolution
    /// faces on the head row's span: the creation-type use
    /// (`resolve_constructor`, any constructor — the enclosing-element
    /// check keys on the container, not the member) or the static-call
    /// receiver's bare binding (`BindingKind::Decl`) — a workspace
    /// declaration only, the synthetic SDK face (`is_sdk_binding`) being
    /// the core `Map` itself and never a bail. Without the semantic run
    /// the plain-run posture holds — no bail.
    fn cross_file_map_head(c: &Check<'_>, map_row: u32) -> bool {
        let Some(analysis) = c.semantics() else {
            return false;
        };
        let site = crate::rules::expressions::query_site(c, map_row);
        analysis.resolve_constructor(site, None).is_some()
            || analysis.binding_of(site).is_some_and(|binding| {
                !analysis.is_sdk_binding(&binding)
                    && binding.kind == dartq_semantic::BindingKind::Decl
            })
    }

    /// `_extractClosure` (:71-88): a named argument whose expression (no
    /// parens) is a one-required-parameter function expression with a single
    /// expression body. The closure expression: hand-built trees nest it
    /// under the named-argument row; production anchors the childless
    /// `name :` marker at the value head and the value trails as the
    /// previous sibling inside the argument list.
    fn extract_closure(c: &Check<'_>, name: &str, argument: u32) -> Option<()> {
        if c.kind(argument) != NodeKind::NamedArgument {
            return None;
        }
        let nested = c.collect_direct(argument);
        let mut expression = if nested.first().is_some_and(|&label| {
            c.kind(label) == NodeKind::Identifier && c.lexeme(c.head_token(label)) == name
        }) {
            *nested.get(1)?
        } else {
            let parent = c.unit.ast.parent(argument)?;
            let pieces = c.collect_direct(parent);
            let i = pieces.iter().position(|&p| p == argument)?;
            let name_pos = slice::named_arg_name(c, &pieces, i)?;
            if c.lexeme(c.head_token(pieces[name_pos])) != name {
                return None;
            }
            *pieces.get(i.checked_sub(1)?)?
        };
        while c.kind(expression) == NodeKind::ParenthesizedExpression {
            expression = first_child(c, expression)?;
        }
        if c.kind(expression) != NodeKind::FunctionExpression {
            return None;
        }
        let kids = c.collect_direct(expression);
        // `parameters.length == 1 && parameters.first.isRequired` (:77-80).
        let params = kids
            .iter()
            .find(|&&kid| c.kind(kid) == NodeKind::FormalParameters)
            .copied()?;
        if !one_required_parameter(c, params) {
            return None;
        }
        // `hasSingleExpressionBody`: an expression body, or one return.
        let has_expression_body = kids
            .iter()
            .any(|&kid| c.kind(kid) == NodeKind::ExpressionBody)
            || kids.iter().any(|&kid| c.kind(kid) == NodeKind::BlockBody) && {
                let body = kids
                    .iter()
                    .find(|&&kid| c.kind(kid) == NodeKind::BlockBody)
                    .copied()?;
                // The statements live under the body's `Block` child.
                let block = c
                    .collect_direct(body)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::Block)?;
                let statements = c.collect_direct(block);
                statements.len() == 1 && c.kind(statements[0]) == NodeKind::ReturnStatement
            };
        has_expression_body.then_some(())
    }

    /// The single-required-parameter gate (:77-80): exactly one formal
    /// parameter, and it is required. Direct rows are positional (always
    /// required — their leaked interior rows are other kinds); an
    /// optional/named group member is required only with the `required`
    /// keyword, so a bare `[e]`/`{e}` member bails like the official.
    fn one_required_parameter(c: &Check<'_>, params: u32) -> bool {
        let is_param = |row: u32| {
            matches!(
                c.kind(row),
                NodeKind::FormalParameter | NodeKind::FunctionTypedFormalParameter
            )
        };
        let mut count = 0usize;
        let mut required = 0usize;
        for kid in c.collect_direct(params) {
            if is_param(kid) {
                count += 1;
                required += 1;
            } else if c.kind(kid) == NodeKind::OptionalFormalParameters {
                for member in c.collect_direct(kid).into_iter().filter(|&m| is_param(m)) {
                    count += 1;
                    if dartq_ast::mod_flags(c.unit.ast, member) & REQUIRED_BIT != 0 {
                        required += 1;
                    }
                }
            }
        }
        count == 1 && required == 1
    }
}

pub(crate) mod collection_methods_unrelated_type {
    use super::{Check, NodeKind, RuleSpec};
    use crate::rules::{TypeFace, slice, type_face_of_field, written_type_face};

    /// Official: `pkg/linter/lib/src/rules/collection_methods_unrelated_type.dart`
    /// — a single-argument invocation of a collection method whose argument
    /// type is unrelated to the receiver's type argument reports at the
    /// argument, with `(argument type, expected type)` (:243-265). The
    /// method table (:117-177): `Iterable.contains` → `E`,
    /// `List.remove`/`Set.remove`/`Queue.remove` → `E`, `Set.lookup` →
    /// `E`, `Map.[]` → `K`, `Map.containsKey`/`Map.remove` → `K`,
    /// `Map.containsValue` → `V`.
    ///
    /// Receiver resolution, strongest face first. The typed-literal
    /// receiver — `<num>[].contains('x')`, the shape every official test
    /// block pins — is its own static type: the outermost collection or
    /// string literal of the receiver run reads as the pinned core
    /// `List`/`Set`/`Map`/`String` with the literal's written type
    /// arguments (an untyped literal carries none — the official reads the
    /// inferred element, and an inferred `dynamic` suppresses every
    /// comparison, so the silence matches the raw-written-type silence).
    /// Otherwise: a `this.<field>` receiver reads the same-class field's
    /// written type ([`type_face_of_field`] — `this` is unshadowable); a
    /// bare-name receiver resolves the innermost binding
    /// ([`written_type_face`] — a local/parameter shadows the field, the
    /// official `target.staticType` face). The written type must carry the
    /// row's name unshadowed ([`crate::rules::expressions::same_file::
    /// declares_name`] — a same-file `class List` is not the core type, the
    /// official `collectionTypeFor` miss), and the written type arguments
    /// stand in for the resolved ones (an omitted argument list — raw
    /// `List` — carries no verdict). The dart:collection implementations
    /// join the table under their core supertype's slots
    /// (`HashMap`/`LinkedHashMap`/`SplayTreeMap` as `Map`,
    /// `HashSet`/`LinkedHashSet`/`SplayTreeSet` as `Set`/`Iterable`,
    /// `ListQueue`/`DoubleLinkedQueue` as `Queue`/`Iterable` — their
    /// resolved members declare on the core containers, which is what
    /// `_checkMethod` keys on). `String.contains` rides the
    /// `Iterable<String>` face: `asInstanceOf(iterableElement)` resolves
    /// on `String` (never on `Map` — `Map` is no `Iterable`), so the
    /// expected element is `String` itself.
    ///
    /// The argument type is the scalar-literal face (`int`, `double`,
    /// `String`, `bool`, `null`); unknown arguments stay silent. The
    /// relation (`typesAreUnrelated`) ports as: equal names, `num` against
    /// `int`/`double`, `Object`/`dynamic` against everything,
    /// nullable-`?` against `Null`, and `Comparable`/`Pattern` (the core
    /// supertypes the scalars implement) are comparable — everything else
    /// reports.
    ///
    /// The index face (:180-200) dispatches on the `Indexed` handle: the
    /// receiver reads through the same resolution, the key argument is the
    /// single piece inside the `[...]` extent, and both the read (`m[k]`)
    /// and the write (`m[k] = v` — the write's `Assignment` container
    /// adopts the `Indexed` row as its first child, so the piece run reads
    /// at the assignment's owner) check the key against `K`.
    ///
    /// Known silences (each the documented fallback of the same-file
    /// channel): a receiver whose written type leaves the file (a member
    /// chain, an import) — the official's unresolvable-or-imported faces;
    /// an untyped binding decides only through its initializer
    /// ([`inferred_binding_type_face`] — a user-container creation head or
    /// any other shape keeps the silence); a prefixed collection
    /// spelling (`collection.Queue`) — the `Type` row's head is the prefix,
    /// no slot matches; a user-declared collection subtype (same-file
    /// `class MyList extends List<int>` or a cross-file one) — the
    /// official reads the resolved supertype's substituted type arguments,
    /// a substitution the written type does not carry; a bare-`this` or
    /// implicit receiver (the official `_getTargetType` member walk); and
    /// the inferred element type of an untyped literal (the official's
    /// inference, `[]`'s `dynamic` included). Message:
    /// `diagnostic.g.dart` `collectionMethodsUnrelatedType`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "collection_methods_unrelated_type",
    &[NodeKind::Send, NodeKind::Indexed],
    check,
);

    fn check(c: &mut Check<'_>) {
        if c.kind(c.node()) == NodeKind::Indexed {
            check_index(c);
        } else {
            check_send(c);
        }
    }

    /// The method-invocation face (`visitMethodInvocation`, :203-238).
    fn check_send(c: &mut Check<'_>) {
        let Some((kids, pos)) = slice::owner_slice(c) else {
            return;
        };
        // The statement's own run: expression rows tile flat in the
        // enclosing block (the span-only statement leaf trails them), so
        // the send's owner slice can span earlier statements — the
        // receiver read starts after the last one.
        let start = kids[..pos]
            .iter()
            .rposition(|&row| crate::rules::is_statement_row(c, row))
            .map_or(0, |i| i + 1);
        let kids = &kids[start..];
        let pos = pos - start;
        let Some((head, Some(arguments))) = slice::send_parts(c, kids, pos) else {
            return;
        };
        let name = c.lexeme(c.head_token(kids[head]));
        // The receiver run: the pieces before the selector, read through
        // the access-run form (the markers and the bare-name send group
        // tile beside the chain rows).
        let mut end = head;
        while end > 0
            && matches!(
                c.kind(kids[end - 1]),
                NodeKind::NoTypeArguments | NodeKind::TypeArguments
            )
        {
            end -= 1;
        }
        let trimmed = slice::access_run(c, &kids[..end]);
        // The single scalar-literal argument (:203-205): the argument is the
        // `Arguments` container's lone child row.
        let args = c.collect_direct(arguments);
        let [arg] = args.as_slice() else {
            return;
        };
        // The typed-literal receiver (`<num>[].contains('x')`): the
        // literal is its own static type, ahead of the written-type faces.
        if let Some(literal) = literal_receiver_face(c, &kids[..end]) {
            if let Some(pinned) = literal.name {
                if let Some((arg_type, expected)) =
                    verdict_on(c, name, pinned, &literal.type_arguments, *arg)
                {
                    let range = c.utf16_span(*arg);
                    c.report(range, &[&arg_type, &expected]);
                }
            }
            return;
        }
        let (face, inferred_user) = match receiver_face(c, &trimmed) {
            Some(face) => (face, false),
            None => match inferred_receiver_face(c, &trimmed) {
                Some((face, user)) => (face, user),
                None => return,
            },
        };
        let bare = match trimmed.as_slice() {
            [only] if c.kind(*only) == NodeKind::Identifier => Some(*only),
            _ => None,
        };
        if !inferred_user && let Some((arg_type, expected)) = verdict(c, name, &face, bare, *arg) {
            let range = c.utf16_span(*arg);
            c.report(range, &[&arg_type, &expected]);
        }
    }

    /// The index-operator face (`visitIndexExpression`, :180-200): the
    /// receiver is the access run ending at the `[`, the key argument the
    /// single piece inside the extent. A `[]=` write adopts the `Indexed`
    /// row as the `Assignment` container's first child while its receiver
    /// and key pieces stay owner-level siblings below the container — the
    /// piece run then reads at the assignment's owner (the statement run
    /// before it). The receiver matches the run's trailing pieces, so an
    /// assignment's left-hand spill (`x = m[k]`) stays out of the face.
    fn check_index(c: &mut Check<'_>) {
        let node = c.node();
        let Some(parent) = c.ancestor(1) else {
            return;
        };
        let write = c.kind(parent) == NodeKind::Assignment && c.children(parent).start == node;
        let (host, dispatched) = if write {
            let Some(grand) = c.ancestor(2) else {
                return;
            };
            (grand, parent)
        } else {
            (parent, node)
        };
        let kids = c.collect_direct(host);
        let Some(pos) = kids.iter().position(|&kid| kid == dispatched) else {
            return;
        };
        // The statement's own run: expression rows tile flat in the
        // enclosing block (the span-only statement leaf trails them).
        let start = kids[..pos]
            .iter()
            .rposition(|&row| crate::rules::is_statement_row(c, row))
            .map_or(0, |i| i + 1);
        let kids = &kids[start..pos];
        // The extent: the span head is the `?` of `a?[k]` (payload word 0)
        // or the `[`; the `]` is the span's last token. The byte offsets
        // bound the key scan.
        let range = c.unit.ast.token_range(node);
        let open = if c.payload(node).first().copied() == Some(crate::rules::NONE_TOKEN) {
            range.start
        } else {
            range.start + 1
        };
        let close = range.end - 1;
        let open_byte = crate::rules::byte_span_of_token(c, open).start;
        let close_byte = crate::rules::byte_span_of_token(c, close).end;
        // The key: every piece before the host whose span sits inside the
        // brackets; the scalar face needs exactly one (a nested expression
        // tiles several and stays unknown). The receiver's bare-name send
        // group leaks a width-0 `NoArguments` marker anchored at the key
        // head (parse_send emits the group for every bare name), so the
        // group's marker pieces stay out of the count.
        let mut key = None;
        let mut key_pos = 0;
        let mut key_count = 0;
        for (i, &kid) in kids.iter().enumerate() {
            if slice::is_send_group_piece(c, kid) {
                continue;
            }
            let span = c.byte_span(kid);
            if span.start > open_byte && span.end <= close_byte {
                key = Some(kid);
                key_pos = i;
                key_count += 1;
            }
        }
        if key_count != 1 {
            return;
        }
        // The receiver: the access run before the key piece, matched on its
        // trailing `[ThisExpression, name]` / `[name]` face. The typed-
        // literal receiver (`<int, String>{}['k']`) reads first.
        if let Some(literal) = literal_receiver_face(c, &kids[..key_pos]) {
            if let Some(pinned) = literal.name {
                if let Some((arg_type, expected)) = verdict_on(
                    c,
                    "[]",
                    pinned,
                    &literal.type_arguments,
                    key.expect("counted above"),
                ) {
                    let range = c.utf16_span(key.expect("counted above"));
                    c.report(range, &[&arg_type, &expected]);
                }
            }
            return;
        }
        let trimmed = slice::access_run(c, &kids[..key_pos]);
        let tail = trimmed.len().saturating_sub(2);
        let end = trimmed.len();
        let (face, inferred_user) = match receiver_face(c, &trimmed[tail..]) {
            Some(face) => (face, false),
            None => match inferred_receiver_face(c, &trimmed[tail..end]) {
                Some((face, user)) => (face, user),
                None => return,
            },
        };
        let bare = match &trimmed[tail..end] {
            [only] if c.kind(*only) == NodeKind::Identifier => Some(*only),
            _ => None,
        };
        if !inferred_user
            && let Some((arg_type, expected)) =
                verdict(c, "[]", &face, bare, key.expect("counted above"))
        {
            let range = c.utf16_span(key.expect("counted above"));
            c.report(range, &[&arg_type, &expected]);
        }
    }

    /// The resolved receiver's written type: `this.<field>` reads the
    /// same-class field (unshadowable), a bare name the innermost binding
    /// (the official `target.staticType`; `_getTargetType`'s implicit-
    /// receiver walk, :284-306, over the same-file channel).
    fn receiver_face<'a>(c: &'a Check<'_>, trimmed: &[u32]) -> Option<TypeFace<'a>> {
        match trimmed {
            [target, field] if c.kind(*target) == NodeKind::ThisExpression => {
                type_face_of_field(c, c.lexeme(c.head_token(*field)))
            }
            [field] if c.kind(*field) == NodeKind::Identifier => {
                let name = c.lexeme(c.head_token(*field));
                written_type_face(c, *field, name)
            }
            _ => None,
        }
    }

    /// The untyped-local face ([`inferred_binding_type_face`]) over a bare
    /// one-piece receiver run — the official's resolved static type where
    /// the written surface stays silent (`var q = Queue<int>()`). The
    /// boolean is the user-container bit: a creation head resolving to an
    /// indexed container is the user's own class, whose members never ride
    /// the core table.
    fn inferred_receiver_face<'a>(
        c: &'a Check<'_>,
        trimmed: &[u32],
    ) -> Option<(TypeFace<'a>, bool)> {
        match trimmed {
            [field] if c.kind(*field) == NodeKind::Identifier => {
                super::inferred_binding_type_face(c, *field, c.lexeme(c.head_token(*field)))
                    .map(|inferred| (inferred.face, inferred.user_container))
            }
            _ => None,
        }
    }

    /// Whether the receiver's written type resolves to an indexed
    /// container — another library's `class Queue` (any cross-file user
    /// type sharing a core spelling) carries the user's members, and the
    /// official `collectionTypeFor` read (`asInstanceOf` onto the pinned
    /// element) never lands on it. The probe reads the receiver's own
    /// declaration's written-type row (the same-file shadow gate has
    /// already run) and answers the semantic lane's container resolution
    /// on its span; a core spelling resolves to no unit. Without the
    /// semantic run the plain-run posture holds — no bail.
    fn cross_file_container(c: &Check<'_>, receiver: u32) -> bool {
        let Some(analysis) = c.semantics() else {
            return false;
        };
        let Some((_, Some(type_row))) = crate::rules::expressions::same_file::binding(
            c,
            receiver,
            c.lexeme(c.head_token(receiver)),
        ) else {
            return false;
        };
        analysis
            .resolve_constructor(crate::rules::expressions::query_site(c, type_row), None)
            .is_some()
    }

    /// The verdict over a written-type receiver: the same-file shadow gate,
    /// the cross-file user-container gate, then the shared verdict body.
    /// `receiver` is the bare receiver row when the face came off a bare
    /// name (`None` for `this.<field>` and inferred-initializer faces — the
    /// latter gate their own creation heads).
    fn verdict(
        c: &Check<'_>,
        method: &str,
        face: &TypeFace<'_>,
        receiver: Option<u32>,
        argument: u32,
    ) -> Option<(String, String)> {
        let written = face.name?;
        // A same-file declaration owns the written name — the
        // `collectionTypeFor` lookup misses the core type.
        if crate::rules::expressions::same_file::declares_name(c, written) {
            return None;
        }
        // The table check first: the semantic probe pays only for tabled
        // methods.
        expected_slot(method, written)?;
        if let Some(receiver) = receiver
            && cross_file_container(c, receiver)
        {
            return None;
        }
        verdict_on(c, method, written, &face.type_arguments, argument)
    }

    /// The verdict body (:243-265) over an already-core written name — the
    /// written-type face and the typed-literal face share it (a literal's
    /// static type is the core collection whatever the file declares, so
    /// the shadow gate does not apply). `(argument type, expected type)`
    /// when the report fires, `None` otherwise.
    fn verdict_on(
        c: &Check<'_>,
        method: &str,
        written: &str,
        type_arguments: &[(&str, bool)],
        argument: u32,
    ) -> Option<(String, String)> {
        // `String.contains` rides the `Iterable<String>` face —
        // `asInstanceOf(iterableElement)` resolves on the unparameterized
        // `String` (never on `Map`: `Map` is no `Iterable`, so a map
        // receiver's `contains` never reaches the table) — and the
        // expected element is `String` itself.
        if method == "contains" && written == "String" {
            let arg_type = literal_type_name(c, argument)?;
            return if comparable("String", false, arg_type) {
                None
            } else {
                Some((arg_type.to_owned(), "String".to_owned()))
            };
        }
        let (index, ()) = expected_slot(method, written)?;
        if type_arguments.len() <= index {
            return None; // raw or argument-less written type: no verdict
        }
        let arg_type = literal_type_name(c, argument)?;
        let (element, element_nullable) = type_arguments[index];
        if comparable(element, element_nullable, arg_type) {
            return None;
        }
        let mut expected = element.to_owned();
        if element_nullable {
            expected.push('?');
        }
        Some((arg_type.to_owned(), expected))
    }

    /// The method's expected slot: `(type-argument index, declaring
    /// containers)` — `None` when the method is outside the table.
    /// `String.contains` is the special face on the verdict body, not a
    /// slot; `Queue.remove` declares on dart:collection's `Queue`
    /// (:158-164), and the dart:collection implementations key through
    /// their core supertype's slots.
    fn expected_slot(method: &str, written: &str) -> Option<(usize, ())> {
        match (method, written) {
            ("containsValue", "Map" | "HashMap" | "LinkedHashMap" | "SplayTreeMap") => {
                Some((1, ()))
            }
            (
                "containsKey" | "[]" | "remove",
                "Map" | "HashMap" | "LinkedHashMap" | "SplayTreeMap",
            ) => Some((0, ())),
            (
                "contains" | "remove",
                "List" | "Set" | "HashSet" | "LinkedHashSet" | "SplayTreeSet" | "Queue"
                | "ListQueue" | "DoubleLinkedQueue" | "Iterable",
            ) => Some((0, ())),
            ("lookup", "Set" | "HashSet" | "LinkedHashSet" | "SplayTreeSet") => Some((0, ())),
            _ => None,
        }
    }

    /// The typed-literal receiver's core face — `<num>[].contains('x')`,
    /// the shape the official List/Set/Map/Queue test blocks pin: the
    /// receiver run's outermost collection or string literal (the piece
    /// whose span ends last — interior element rows tile beside their
    /// root, under the literal's own span) read as the pinned core
    /// `List`/`Set`/`Map`/`String` with the literal's written type
    /// arguments (a set-or-map literal decides through its entries; an
    /// empty `{}` reads as a set unless two written type arguments say
    /// `K, V` — the context-free map reading). An untyped literal answers
    /// with no arguments, and the
    /// verdict body's slot check silences it: the official reads the
    /// inferred element, and an inferred `dynamic` suppresses every
    /// comparison. `None` when the run's latest-ending piece is no
    /// literal — the receiver is not a literal (the piece run is trimmed
    /// to the statement, so earlier expressions' literals stay out unless
    /// they genuinely end after the receiver's, which only the receiver's
    /// own run can produce adjacent to the selector).
    fn literal_receiver_face<'a>(c: &'a Check<'_>, run: &[u32]) -> Option<TypeFace<'a>> {
        let literal = run
            .iter()
            .copied()
            .filter(|&row| {
                matches!(
                    c.kind(row),
                    NodeKind::ListLiteral | NodeKind::SetOrMapLiteral | NodeKind::StringLiteral
                )
            })
            .max_by_key(|&row| c.byte_span(row).end)?;
        let mut type_arguments = Vec::new();
        for &piece in run {
            if c.kind(piece) != NodeKind::TypeArguments {
                continue;
            }
            if c.byte_span(piece).end <= c.byte_span(literal).start {
                type_arguments = c
                    .collect_direct(piece)
                    .into_iter()
                    .filter(|&arg| c.kind(arg) == NodeKind::Type)
                    .map(|arg| {
                        let head = c.head_token(arg);
                        let nullable =
                            c.byte_span(arg).end > crate::rules::byte_span_of_token(c, head).end;
                        (c.lexeme(head), nullable)
                    })
                    .collect();
            }
        }
        let pinned: &'static str = match c.kind(literal) {
            NodeKind::ListLiteral => "List",
            NodeKind::StringLiteral => "String",
            NodeKind::SetOrMapLiteral => match slice::literal_faces(c, literal) {
                Some(slice::MAP) => "Map",
                // The empty `{}` reads as a set ([`slice::literal_faces`]'s
                // documented corner — the official resolves it through
                // context); two written type arguments are the map's
                // `K, V`, the context-free reading.
                _ if type_arguments.len() == 2 => "Map",
                _ => "Set",
            },
            _ => return None,
        };
        Some(TypeFace {
            name: Some(pinned),
            nullable: false,
            is_void: false,
            is_future_or_void: false,
            is_function_type: false,
            type_arguments,
        })
    }

    /// The scalar-literal type face; `null` answers `"Null"`.
    fn literal_type_name(c: &Check<'_>, row: u32) -> Option<&'static str> {
        match c.kind(row) {
            NodeKind::LiteralInt => Some("int"),
            NodeKind::LiteralDouble => Some("double"),
            NodeKind::StringLiteral => Some("String"),
            NodeKind::LiteralBool => Some("bool"),
            NodeKind::LiteralNull => Some("Null"),
            _ => None,
        }
    }

    /// `typesAreUnrelated` on the ported faces (documented on the SPEC).
    fn comparable(element: &str, element_nullable: bool, arg: &str) -> bool {
        element == arg
            || matches!(
                element,
                "Object" | "dynamic" | "void" | "Comparable" | "Pattern"
            )
            || (element == "num" && matches!(arg, "int" | "double"))
            || (element_nullable && arg == "Null")
    }
}

pub(crate) mod prefer_foreach {
    use dartq_ast::NodeKind;

    use crate::engine::Check;
    use crate::registry::RuleSpec;
    use crate::rules::slice;
    use crate::rules::{initialized_identifier_name, is_statement_row};

    /// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/prefer_foreach.dart`
    /// — a for-each loop (`ForEachPartsWithDeclaration`, :57-64) whose body is
    /// a single expression statement — through single-statement blocks
    /// (:43-47) and transparent parentheses (:87-89) — invoking a function
    /// with exactly one argument that is the loop variable itself reports at
    /// the whole for statement (:70-84). A method invocation's target must
    /// not reference the loop variable (`_ReferenceFinder`, :81/:92-113); a
    /// function-expression invocation has no target and is never checked
    /// (:68-73). The identifier form (`for (e in xs)`) is no declaration and
    /// never fires (:57).
    ///
    /// dartq faces: the loop variable reads the header's lone
    /// `VariablesDeclaration` (the declaration form; the pattern form's rows
    /// never match). The single-argument check reads the invocation's
    /// argument list over the production tiling — the argument is the bare
    /// loop-variable name (`arguments.first.canonicalElement == element`
    /// through the lexical face: at the body's top level, outside every
    /// nested scope, a bare name equal to the loop variable resolves only to
    /// it). The target check scans the target's tokens: a name equal to the
    /// loop variable that is neither a member (`.name`) nor a named-argument
    /// label (`name:`) is a reference — shadowing-aware, because a nested
    /// function literal binding the name (its own parameter list, or a local
    /// declared inside it) re-resolves every lexeme inside its extent to
    /// that binder, never to the loop variable (`_ReferenceFinder`'s
    /// `canonicalElement == element` reads the resolution the scope walk
    /// stands in for). A function-typed formal's inner parameter naming the
    /// loop variable is that inner binder's own declaration — the extent
    /// carve-out reads its inner parameter list too. Message:
    /// `diagnostic.g.dart` `preferForeach`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_foreach",
    &[NodeKind::Send],
    check,
);

    fn check(c: &mut Check<'_>) {
        // The body walk inverted: the statement-level invocation climbs to
        // its for-in through transparent parentheses (:87-89) and
        // single-statement blocks (:43-47); anything else in between (a
        // wrapper expression, a second statement) is not the walked shape.
        let mut cur = c.node();
        let forin = loop {
            let Some(parent) = c.unit.ast.parent(cur) else {
                return;
            };
            match c.kind(parent) {
                NodeKind::ParenthesizedExpression => cur = parent,
                NodeKind::Block => {
                    let kids = c.collect_direct(parent);
                    let Some(pos) = kids.iter().position(|&kid| kid == cur) else {
                        return;
                    };
                    // The statement row trails its expression pieces (a
                    // member-access marker tiles between the send and it),
                    // and it is the first statement row after them; the
                    // walked face is the expression-statement face only.
                    let Some(row) = kids[pos + 1..]
                        .iter()
                        .find(|&&row| is_statement_row(c, row))
                    else {
                        return;
                    };
                    if c.kind(*row) != NodeKind::ExpressionStatement {
                        return;
                    }
                    if statement_row_count(c, &kids) != 1 {
                        return;
                    }
                    // The walked send is the statement's ROOT invocation:
                    // the last piece before the statement row, trailing
                    // access markers skipped. A nested fragment send (a
                    // `list.indexOf(d)` call inside the target) carries an
                    // argument list too, but it is not the statement's
                    // expression — only the root's argument list is the
                    // official's `arguments` read.
                    let Some(stmt) = kids.iter().position(|&kid| kid == *row) else {
                        return;
                    };
                    let mut root = stmt;
                    while root > 0 && c.kind(kids[root - 1]) == NodeKind::DotAccess {
                        root -= 1;
                    }
                    if root == 0 || kids[root - 1] != cur {
                        return;
                    }
                    cur = parent;
                }
                NodeKind::ForIn => break parent,
                _ => return,
            }
        };
        // `loopParts is ForEachPartsWithDeclaration` (:57): the header's lone
        // variable declaration names the loop variable; the identifier and
        // pattern forms carry no such row.
        let Some(loop_var) = loop_variable(c, forin) else {
            return;
        };
        // The invocation face over the send's own pieces (the owner slice —
        // the send's pieces tile flat in the enclosing container): a
        // `NoArguments` send is a tear-off, no invocation. The
        // callee-parenthesized invocation (`((e) => …)(x)`) has no selector
        // identifier for the walk to name — its callee is the parenthesized
        // wrapper row before the argument list.
        let Some((kids, pos)) = slice::owner_slice(c) else {
            return;
        };
        let Some((head, Some(args))) = slice::send_parts(c, &kids, pos).or_else(|| {
            let args_pos = pos.checked_sub(1)?;
            matches!(c.kind(kids[args_pos]), NodeKind::Arguments).then_some(())?;
            // The nearest parenthesized wrapper before the argument list is
            // the callee.
            let callee = kids[..args_pos]
                .iter()
                .rev()
                .copied()
                .find(|&row| c.kind(row) == NodeKind::ParenthesizedExpression)?;
            Some((
                kids.iter().position(|&kid| kid == callee)?,
                Some(kids[args_pos]),
            ))
        }) else {
            return;
        };
        if !single_bare_loop_variable(c, args, loop_var) {
            return;
        }
        // The reference scan's bound: the member invocation's selector. A
        // callee-parenthesized invocation (`((e) => …)(x)`) walks to no
        // selector identifier — its callee is the parenthesized wrapper row
        // before the argument list, and the bound is the wrapper's end: the
        // walked identifier (an inner binder's spelling) sits INSIDE the
        // callee extent, and a selector-token bound would push the binder's
        // carve-out out of the scan.
        let Some(args_pos) = kids.iter().position(|&kid| kid == args) else {
            return;
        };
        let selector_head = kids[head..args_pos]
            .iter()
            .filter(|&&row| c.kind(row) == NodeKind::ParenthesizedExpression)
            .next_back()
            .map_or_else(
                || c.head_token(kids[head]),
                |&paren| c.unit.ast.token_range(paren).end,
            );
        // The target check (:81): a method invocation whose target references
        // the loop variable stays silent; a function-expression invocation
        // has no target and is never checked (:68-73).
        if references_in_target(c, &kids[..head], selector_head, loop_var, forin) {
            return;
        }
        c.report(c.utf16_span(forin), &[]);
    }

    /// The statement rows of a block window (`node.statements.length`, :44).
    fn statement_row_count(c: &Check<'_>, kids: &[u32]) -> usize {
        kids.iter().filter(|&&row| is_statement_row(c, row)).count()
    }

    /// The loop variable's name: the header's lone `VariablesDeclaration`
    /// with exactly one `InitializedIdentifier` (:57-64 the declaration
    /// form), or `None`.
    fn loop_variable<'a>(c: &'a Check<'a>, forin: u32) -> Option<&'a str> {
        let mut declarations = c
            .collect_direct(forin)
            .into_iter()
            .filter(|&row| c.kind(row) == NodeKind::VariablesDeclaration);
        let declaration = declarations.next()?;
        if declarations.next().is_some() {
            return None;
        }
        let variables: Vec<u32> = c
            .collect_direct(declaration)
            .into_iter()
            .filter(|&var| c.kind(var) == NodeKind::InitializedIdentifier)
            .collect();
        if variables.len() != 1 {
            return None;
        }
        let name = initialized_identifier_name(c, variables[0])?;
        Some(c.lexeme(name))
    }

    /// `arguments.length == 1 && arguments.first == element` (:70/:79): the
    /// sole argument is the bare loop-variable name — a one-piece
    /// `Identifier`, or the production bare-name group
    /// (`Identifier, type arguments, NoArguments, Send`).
    fn single_bare_loop_variable(c: &Check<'_>, args: u32, loop_var: &str) -> bool {
        let pieces = c.collect_direct(args);
        let groups = slice::arguments(c, &pieces);
        let Some(group) = groups.first() else {
            return false;
        };
        let bare = match group.len() {
            1 => c.kind(pieces[group.start]) == NodeKind::Identifier,
            4 => slice::is_tear_off_group(c, &pieces, group.start),
            _ => false,
        };
        bare && c.lexeme(c.head_token(pieces[group.start])) == loop_var
    }

    /// `_ReferenceFinder.references(target)` (:97-113): a scope-resolved name
    /// equal to the loop variable inside the target's token extent. The
    /// target is the statement's own piece run before the selector — empty
    /// for a bare call (no target, never checked). Member names
    /// (`.name`) and named-argument labels (`name:`) are not scope-resolved.
    /// A lexeme inside a nested function literal that binds the name is the
    /// binder's, not the loop variable's — [`shadow_spans`] carves those
    /// extents out.
    fn references_in_target(
        c: &Check<'_>,
        run: &[u32],
        selector_head: u32,
        loop_var: &str,
        forin: u32,
    ) -> bool {
        // The statement's own run: expression rows tile flat in the enclosing
        // container (earlier statements trail their own statement rows).
        let run = slice::statement_run(c, run);
        let Some((&first, _)) = run.split_first() else {
            return false; // no target
        };
        let start = c.head_token(first);
        let mut shadows: Option<Vec<core::ops::Range<u32>>> = None;
        for token in start..selector_head {
            if c.lexeme(token) != loop_var {
                continue;
            }
            if token > 0 && c.lexeme(token - 1) == "." {
                continue; // member name
            }
            // `token + 1` stays in the table: the scan stops before the
            // selector.
            if c.lexeme(token + 1) == ":" {
                continue; // named-argument label
            }
            // Binder extents pay for themselves on the first candidate
            // lexeme only.
            if shadows
                .get_or_insert_with(|| shadow_spans(c, forin, start, selector_head, loop_var))
                .iter()
                .any(|span| span.contains(&token))
            {
                continue; // a nested binder's own declaration or read
            }
            return true;
        }
        false
    }

    /// The token-extent spans of the target's nested function literals that
    /// bind `loop_var` — a parameter of the literal's own list, or a local
    /// declared inside it. Every `loop_var` lexeme inside such an extent
    /// resolves to the shadowing declaration (`_ReferenceFinder` reads the
    /// same through `canonicalElement`, :104-112), never to the loop
    /// variable. Candidates come off the loop's completion window (every
    /// node constructed inside the for-in, the flat tiling included),
    /// clipped to the target extent — a bounded, recursion-free pass.
    fn shadow_spans(
        c: &Check<'_>,
        forin: u32,
        start: u32,
        selector_head: u32,
        loop_var: &str,
    ) -> Vec<core::ops::Range<u32>> {
        let mut spans = Vec::new();
        for id in c.children(forin) {
            if !matches!(
                c.kind(id),
                NodeKind::FunctionExpression | NodeKind::NamedFunctionExpression
            ) {
                continue;
            }
            // The literal's lexical extent: the production tiling ends the
            // row at `=>` (the body's pieces tile after it), so the true
            // end is the window's max token end — the body's rows complete
            // inside the literal.
            let tokens = c.unit.ast.token_range(id);
            let end = c
                .unit
                .ast
                .children(id)
                .map(|row| c.unit.ast.token_range(row).end)
                .max()
                .unwrap_or(tokens.end)
                .max(tokens.end);
            let extent = tokens.start..end;
            if extent.start < start || extent.end > selector_head {
                continue;
            }
            if function_literal_binds(c, id, extent.clone(), loop_var) {
                spans.push(extent);
            }
        }
        spans
    }

    /// Whether the function literal binds `loop_var`: a formal of its own
    /// parameter list (the plain and function-typed rows of the direct
    /// [`NodeKind::FormalParameters`], the validated parameter tiling), a
    /// function-typed formal's INNER parameter list (`(void cb(int e)) =>
    /// …` — its parameter names are declarations, every lexeme inside the
    /// inner list resolving to the inner binder, never to the loop
    /// variable), or a local declared anywhere inside its extent (one
    /// binder deciding is all the lexical walk needs — innermost shadowing
    /// only renames the winner).
    fn function_literal_binds(
        c: &Check<'_>,
        literal: u32,
        extent: core::ops::Range<u32>,
        loop_var: &str,
    ) -> bool {
        let (extent_start, extent_end) = (extent.start, extent.end);
        if let Some(params) = c
            .collect_direct(literal)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
            && slice::parameter_assignments::parameter_rows(c, params)
                .iter()
                .any(|&(row, _)| {
                    slice::parameter_assignments::parameter_name(c, row) == Some(loop_var)
                })
        {
            return true;
        }
        // The completion window: node ids constructed inside the literal
        // (the flat tiling included). A function-typed formal's inner
        // parameter list is a window row inside the formal's own extent.
        let window = c.children(literal);
        let in_extent = |row: u32| {
            let span = c.unit.ast.token_range(row);
            span.start >= extent_start && span.end <= extent_end
        };
        for formal in window
            .clone()
            .filter(|&row| c.kind(row) == NodeKind::FunctionTypedFormalParameter)
            .filter(|&row| in_extent(row))
        {
            let span = c.unit.ast.token_range(formal);
            let binds = window.clone().find(|&row| {
                c.kind(row) == NodeKind::FormalParameters && {
                    let inner = c.unit.ast.token_range(row);
                    inner.start >= span.start && inner.end <= span.end
                }
            });
            if let Some(params) = binds
                && slice::parameter_assignments::parameter_rows(c, params)
                    .iter()
                    .any(|&(row, _)| {
                        slice::parameter_assignments::parameter_name(c, row) == Some(loop_var)
                    })
            {
                return true;
            }
        }
        c.children(literal).any(|kid| {
            c.kind(kid) == NodeKind::VariablesDeclaration
                && c.unit.ast.token_range(kid).start >= extent_start
                && c.unit.ast.token_range(kid).end <= extent_end
                && c.collect_direct(kid).into_iter().any(|var| {
                    c.kind(var) == NodeKind::InitializedIdentifier
                        && initialized_identifier_name(c, var)
                            .is_some_and(|token| c.lexeme(token) == loop_var)
                })
        })
    }
}

#[cfg(test)]
mod tests {
    use dartq_ast::NodeKind;

    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_resolution::{Combinators, ImportEdge, LibraryClassModifiers, LibraryGraph, LibraryGraphBuilder, Namespaces};
    use dartq_semantic::{ElementIndex, MicroKind, StrictModes, analyze_unit, collect};
    use dartq_source::{FileTable, StringTable};

    use crate::engine::Lint;
    use crate::testsupport::{Fixture, NONE, identifier, one_rule, tok, unit_of, utf16_at};

    /// Runs `rule` over `source` with the semantic face mounted: the source
    /// folded as the sole analyzed file of `package:t` (the driver's
    /// per-file build shape). `dependency` analyzes a second `kit.dart` and
    /// imports it unprefixed — the cross-file container faces. The context
    /// tables leak (`'static`); the tests are one-shot processes.
    fn run_semantic(source: &str, rule: &str, dependency: Option<&str>) -> Vec<Lint> {
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
        if let Some(dependency) = dependency {
            let dep_file = files.intern(std::path::Path::new("kit.dart"));
            let scanned_dep = dartq_scanner::scan(dependency, crate::testsupport::CONFIG, dep_file);
            assert!(
                scanned_dep.diagnostics.is_empty(),
                "scanner diagnostics: {dependency:?}"
            );
            let dep_elements = collect(dependency, &scanned_dep.tokens, features, strings);
            let dep_lib = builder.intern("package:t/kit.dart");
            for (name, declaration) in dep_elements.declarations() {
                if declaration.kind == MicroKind::TypeParam {
                    continue;
                }
                builder.declare(dep_lib, name, dep_file);
            }
            index.insert(dep_file, dep_elements);
            builder.import(
                lib,
                ImportEdge {
                    target: Some(dep_lib),
                    prefix: None,
                    deferred: false,
                    combinators: Combinators::unfiltered(),
                },
            );
        }
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

    /// `[]..addAll([1, 2]);` — a cascaded list-literal addAll reports with
    /// the multiple template (`test/rules/prefer_inlined_adds_test.dart`),
    /// through the production bridge: the section's pieces tile flat inside
    /// the cascade window and the method name reads off the owner slice.
    #[test]
    fn inlined_adds_reports_addall_of_list_literal() {
        let f = Fixture::parse("void f() { []..addAll([1, 2]); }\n");
        let lints = f.run(&one_rule("prefer_inlined_adds"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The addition of multiple list items could be inlined."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 7));
        assert!(f.run(&one_rule("prefer_spread_collections")).is_empty());
    }

    /// `[]..add(1);` — the single template.
    #[test]
    fn inlined_adds_reports_add() {
        let f = Fixture::parse("void f() { []..add(1); }\n");
        let lints = f.run(&one_rule("prefer_inlined_adds"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The addition of a list item could be inlined."
        );
        // The report anchors at the method name (`add` — token 7; the
        // scanner emits the `[]` operator as one token, so token 6 is the
        // cascade `..`).
        assert_eq!(lints[0].range.start, utf16_at(&f, 7));
    }

    /// A non-first section is skipped (`sections.first == invocation`).
    #[test]
    fn inlined_adds_second_section_passes() {
        let f = Fixture::parse("void f() { []..addAll(1, 2)..add(3); }\n");
        assert!(f.run(&one_rule("prefer_inlined_adds")).is_empty());
        assert!(f.run(&one_rule("prefer_spread_collections")).is_empty());
    }

    /// A non-list target passes.
    #[test]
    fn inlined_adds_non_list_target_passes() {
        let f = Fixture::parse("void f() { ts..add(1); }\n");
        assert!(f.run(&one_rule("prefer_inlined_adds")).is_empty());
    }

    /// `[]..addAll(b);` — spread argument: the spread rule reports, the
    /// inlined rule stays silent (list-literal gate, :53-55).
    #[test]
    fn spread_reports_addall_of_expression() {
        let f = Fixture::parse("void f() { []..addAll(b); }\n");
        let spread = f.run(&one_rule("prefer_spread_collections"));
        assert_eq!(spread.len(), 1);
        assert_eq!(
            spread[0].message,
            "The addition of multiple elements could be inlined."
        );
        assert_eq!(spread[0].range.start, utf16_at(&f, 7));
        assert!(f.run(&one_rule("prefer_inlined_adds")).is_empty());
    }

    /// A `const` declaration ancestor passes the spread rule (the syntactic
    /// `inConstantContext` approximation).
    #[test]
    fn spread_const_target_passes() {
        let f = Fixture::parse("void f() { const a = []..addAll(b); }\n");
        assert!(f.run(&one_rule("prefer_spread_collections")).is_empty());
    }

    // —— prefer_collection_literals ———————————————————————————————————————

    /// `test/rules/prefer_collection_literals_test.dart` `test_map` — the
    /// no-arg `Map()`/`Set()` invocations report; `HashMap()`/`HashSet()`
    /// never report (their static type is the unrelated dart:collection
    /// class — :28-32/:99-105 pin `Map`/`Set`/`Linked*` only).
    #[test]
    fn collection_literals_reports_map_and_set() {
        let f = Fixture::parse("void f() { Map(); }");
        let lints = f.run(&one_rule("prefer_collection_literals"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Unnecessary constructor invocation.");
        assert_eq!(
            lints[0].correction,
            Some("Try using a collection literal.".to_owned())
        );

        let f = Fixture::parse("void f() { Set(); }");
        assert_eq!(f.run(&one_rule("prefer_collection_literals")).len(), 1);

        let f = Fixture::parse("void f() { LinkedHashMap(); LinkedHashSet(); }");
        assert_eq!(f.run(&one_rule("prefer_collection_literals")).len(), 2);

        let f = Fixture::parse("void f() { HashMap(); HashSet(); }");
        assert!(f.run(&one_rule("prefer_collection_literals")).is_empty());

        // Arguments block the unnamed face and a tear-off reference is no
        // creation; type arguments are not arguments (`Set<int>()` still
        // reports — the static type stays the core set).
        let f = Fixture::parse("void f() { Map(1); g(Map); }");
        assert!(f.run(&one_rule("prefer_collection_literals")).is_empty());

        let f = Fixture::parse("void f() { Set<int>(); }");
        assert_eq!(f.run(&one_rule("prefer_collection_literals")).len(), 1);
    }

    /// `Set.from([1])` reports; `LinkedHashSet.from` rides the `isHashSet`
    /// face; a named constructor on the map family, a non-list argument,
    /// and two arguments pass.
    #[test]
    fn collection_literals_from_of_and_negatives() {
        let f = Fixture::parse("void f() { Set.from([1]); }");
        assert_eq!(f.run(&one_rule("prefer_collection_literals")).len(), 1);

        let f = Fixture::parse("void f() { Set.of([1]); LinkedHashSet.from([1]); }");
        assert_eq!(f.run(&one_rule("prefer_collection_literals")).len(), 2);

        let f = Fixture::parse("void f() { HashMap.identity(); Map.from([1]); }");
        assert!(f.run(&one_rule("prefer_collection_literals")).is_empty());

        let f = Fixture::parse("void f() { Set.from(1); Set.of([1], 2); }");
        assert!(f.run(&one_rule("prefer_collection_literals")).is_empty());
    }

    /// The same-file element gate (:44-46): a unit-local declaration of
    /// the name — a user class or a typedef — shadows the core type and
    /// passes.
    #[test]
    fn collection_literals_same_file_shadows_pass() {
        let f = Fixture::parse("class Set {}\nvoid f() { Set(); Set.from([1]); }");
        assert!(f.run(&one_rule("prefer_collection_literals")).is_empty());

        let f = Fixture::parse("typedef Map = List;\nvoid f() { Map(); }");
        assert!(f.run(&one_rule("prefer_collection_literals")).is_empty());

        // An extension type named like the core collection constructs
        // through its own representation — the element-identity face
        // resolves to user code, never the core type.
        let f = Fixture::parse("extension type Map(int i) {}\nvoid f() { Map(1); }");
        assert!(f.run(&one_rule("prefer_collection_literals")).is_empty());
    }

    /// The `Linked*` context exemption (:85-88/:101-104): a declaration
    /// typed with the same creation is exempt, an untyped (`var`)
    /// declaration is the official's invalid context, and any other
    /// declared type reports.
    #[test]
    fn collection_literals_context_types() {
        let f = Fixture::parse(
            "void f() { LinkedHashMap<int, int> m = LinkedHashMap(); Set<int> s = LinkedHashSet(); }",
        );
        // The matching-type creation is exempt; `LinkedHashSet()` in a
        // `Set<int>` context is a non-exempt context and reports.
        assert_eq!(f.run(&one_rule("prefer_collection_literals")).len(), 1);

        let f = Fixture::parse("void f() { var m = LinkedHashMap(); final s = LinkedHashSet(); }");
        assert!(f.run(&one_rule("prefer_collection_literals")).is_empty());

        let f = Fixture::parse("void f() { Map<int, int> m = LinkedHashMap(); }");
        assert_eq!(f.run(&one_rule("prefer_collection_literals")).len(), 1);
    }

    /// `['a'].toSet()` reports; `x.toSet()` on a non-literal target and an
    /// index target pass (:87-95).
    #[test]
    fn collection_literals_to_set_on_list_literal() {
        let f = Fixture::parse("void f() { ['a'].toSet(); }");
        assert_eq!(f.run(&one_rule("prefer_collection_literals")).len(), 1);

        let f = Fixture::parse("void f(List<int> x) { x.toSet(); x[0].toSet(); }");
        assert!(f.run(&one_rule("prefer_collection_literals")).is_empty());
    }

    // —— unnecessary_to_list_in_spreads ————————————————————————————————————

    /// `test/rules/unnecessary_to_list_in_spreads_test.dart` `test_toList` —
    /// `...x.toList()` reports at the method name, on the production bridge
    /// shape (the operand tiles flat beside the `Spread` leaf). The written
    /// `List<int>` parameter lands in the Iterable family.
    #[test]
    fn to_list_spread_reports() {
        let f = Fixture::parse("void f(List<int> list) { [...list.toList()]; }");
        let lints = f.run(&one_rule("unnecessary_to_list_in_spreads"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Unnecessary use of 'toList' in a spread.");
        assert_eq!(
            lints[0].correction,
            Some("Try removing the invocation of 'toList'.".to_owned())
        );
        let name = tok(&f.source, &f.tokens, "toList", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, name));
        assert_eq!(lints[0].range.end, utf16_at(&f, name) + 6);
    }

    /// The corpus face: a spread in a list literal initializer over a
    /// written-`List<int>` parameter. Oracle row: at `toList`, len 6.
    #[test]
    fn to_list_spread_corpus_face_reports() {
        let f = Fixture::parse(
            "void copyUser(List<int> xs) {\n  final copied = [...xs.toList()];\n}\n",
        );
        let lints = f.run(&one_rule("unnecessary_to_list_in_spreads"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let name = tok(&f.source, &f.tokens, "toList", 0);
        assert_eq!(lints[0].range, utf16_at(&f, name)..utf16_at(&f, name) + 6);
    }

    /// The receiver gate: a written non-Iterable receiver (a compile error
    /// upstream) stays silent; an unresolvable receiver keeps the documented
    /// presumption; a plain spread element and a bare `toList()` call pass.
    #[test]
    fn to_list_spread_receiver_and_shape_faces() {
        let f = Fixture::parse("void f(String s) { [...s.toList()]; }");
        assert!(
            f.run(&one_rule("unnecessary_to_list_in_spreads"))
                .is_empty(),
            "a written String is provably outside the Iterable family"
        );

        let f = Fixture::parse("void f(x) { [...x.toList()]; }");
        assert_eq!(
            f.run(&one_rule("unnecessary_to_list_in_spreads")).len(),
            1,
            "an untyped receiver keeps the presumption"
        );

        let f = Fixture::parse("void f(List<int> list) { [...list]; }");
        assert!(
            f.run(&one_rule("unnecessary_to_list_in_spreads"))
                .is_empty()
        );

        let f = Fixture::parse("void f() { [...toList()]; }");
        assert!(
            f.run(&one_rule("unnecessary_to_list_in_spreads"))
                .is_empty()
        );

        // A tear-off (`x.toList`) is no MethodInvocation.
        let f = Fixture::parse("void f(List<int> list) { var g = [...list.toList]; }");
        assert!(
            f.run(&one_rule("unnecessary_to_list_in_spreads"))
                .is_empty()
        );
    }

    /// The inferred-initializer receiver gate (the ledgered dartq_only
    /// face: a user `toList` on a non-Iterable over-reported). An untyped
    /// binding decides through its initializer: a user-class creation (the
    /// official's non-Iterable static type — the spread is a compile error
    /// upstream) and a map literal stay silent; a core `Queue<int>`
    /// creation and a typed list literal stay Iterable and report.
    #[test]
    fn to_list_spread_inferred_initializer_faces() {
        let silent = run_semantic(
            "class Box {\n  List<int> toList() => [];\n}\nvoid f() {\n  var b = Box();\n  [...b.toList()];\n}\n",
            "unnecessary_to_list_in_spreads",
            None,
        );
        assert!(
            silent.is_empty(),
            "a user creation is no Iterable: {silent:?}"
        );

        let silent = run_semantic(
            "void f() {\n  var m = <int, int>{};\n  [...m.toList()];\n}\n",
            "unnecessary_to_list_in_spreads",
            None,
        );
        assert!(silent.is_empty(), "a map is no Iterable: {silent:?}");

        let reported = run_semantic(
            "void f() {\n  var q = Queue<int>();\n  [...q.toList()];\n}\n",
            "unnecessary_to_list_in_spreads",
            None,
        );
        assert_eq!(reported.len(), 1, "Queue<int> is an Iterable: {reported:?}");

        let reported = run_semantic(
            "void f() {\n  var xs = <int>[];\n  [...xs.toList()];\n}\n",
            "unnecessary_to_list_in_spreads",
            None,
        );
        assert_eq!(
            reported.len(),
            1,
            "a typed list literal is an Iterable: {reported:?}"
        );
    }

    // —— prefer_iterable_whereType —————————————————————————————————————————

    /// `test/rules/prefer_iterable_whereType_test.dart` `test_whereType` —
    /// `x.where((e) => e is int)` reports at the method name.
    /// Production-bridge fixture.
    #[test]
    fn where_type_reports_a_type_test_closure() {
        let f = Fixture::parse("void f(List<int> xs) { xs.where((e) => e is int); }");
        let lints = f.run(&one_rule("prefer_iterable_wheretype"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use 'whereType' to select elements of a given type."
        );
        let name = tok(&f.source, &f.tokens, "where", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, name));
        assert_eq!(lints[0].range.end, utf16_at(&f, name) + 5);
    }

    /// An `is!` closure and a foreign-parameter test pass (:71-79).
    #[test]
    fn where_type_not_is_and_foreign_param_pass() {
        let f = Fixture::parse("void f(List<int> xs) { xs.where((e) => e is! int); }");
        assert!(f.run(&one_rule("prefer_iterable_wheretype")).is_empty());

        let f = Fixture::parse("void f(List<int> xs) { xs.where((e) => f(e) is int); }");
        assert!(f.run(&one_rule("prefer_iterable_wheretype")).is_empty());
    }

    /// Two arguments and a bare untargeted `where` pass (:47-51), and the
    /// restored receiver face keeps a map literal silent like the official
    /// `Iterable`-only gate (:48-49).
    #[test]
    fn where_type_argument_and_receiver_faces_pass() {
        let f = Fixture::parse("void f(List<int> xs) { xs.where((e) => e is int, 1); }");
        assert!(f.run(&one_rule("prefer_iterable_wheretype")).is_empty());

        let f = Fixture::parse("void f() { var x = ({'k': 1}).where((e) => e is int); }");
        assert!(
            f.run(&one_rule("prefer_iterable_wheretype")).is_empty(),
            "a map literal is outside Iterable"
        );
    }

    /// The body-ROOT gate (the ledgered dartq_only face: the closure-
    /// parameter identity presumed for an is-test nested deeper). The
    /// official reads the body expression itself (`expression is
    /// IsExpression` after `unParenthesized`), so a binary arm, a
    /// conditional branch, a nested closure's body, and a second statement
    /// keep the rule silent; the parenthesized body and the single-return
    /// block body still report.
    #[test]
    fn where_type_body_root_faces() {
        // An is-test inside a binary expression is not the body.
        let f = Fixture::parse(
            "void f(List<int> xs, bool flag) { xs.where((e) => e is int && flag); }",
        );
        assert!(
            f.run(&one_rule("prefer_iterable_wheretype")).is_empty(),
            "a binary body is no is-test"
        );

        // A conditional body is not the is-test either.
        let f = Fixture::parse(
            "void f(List<int> xs, bool flag) { xs.where((e) => flag ? e is int : false); }",
        );
        assert!(f.run(&one_rule("prefer_iterable_wheretype")).is_empty());

        // An is-test inside a nested closure's body stays silent.
        let f = Fixture::parse(
            "void f(List<int> xs) { xs.where((e) { return xs.map((x) => x is int).length == 1; }); }",
        );
        assert!(f.run(&one_rule("prefer_iterable_wheretype")).is_empty());

        // A second statement bails (`statements.length != 1`).
        let f = Fixture::parse("void f(List<int> xs) { xs.where((e) { g(); return e is int; }); }");
        assert!(f.run(&one_rule("prefer_iterable_wheretype")).is_empty());

        // The parenthesized expression body and the single-return block
        // body are the is-test themselves.
        let f = Fixture::parse("void f(List<int> xs) { xs.where((e) => (e is int)); }");
        assert_eq!(f.run(&one_rule("prefer_iterable_wheretype")).len(), 1);

        let f = Fixture::parse("void f(List<int> xs) { xs.where((e) { return e is int; }); }");
        assert_eq!(f.run(&one_rule("prefer_iterable_wheretype")).len(), 1);
    }

    // —— collection_methods_unrelated_type: the A6 burn faces ——————————————

    /// The `Map[]=` index-assignment row: the key checks against `K` on
    /// both the read and the write (official `visitIndexExpression`,
    /// the `[]` method definition).
    #[test]
    fn index_read_and_write_faces_check_the_key() {
        let f = Fixture::parse("void f(Map<int, String> m) { m['k']; }");
        let lints = f.run(&one_rule("collection_methods_unrelated_type"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The argument type 'String' isn't related to 'int'."
        );

        let f = Fixture::parse("void f(Map<int, String> m) { m['k'] = 'v'; }");
        let lints = f.run(&one_rule("collection_methods_unrelated_type"));
        assert_eq!(lints.len(), 1, "the write face checks the key: {lints:?}");

        let f = Fixture::parse("void f(Map<int, String> m) { m[0] = 'v'; }");
        assert!(
            f.run(&one_rule("collection_methods_unrelated_type"))
                .is_empty()
        );
    }

    /// The `Queue.remove` row (`_MethodDefinitionForName('dart.collection',
    /// 'Queue', 'remove')`): the written receiver face decides without the
    /// element model — `Queue` never declares in the file.
    #[test]
    fn queue_remove_written_receiver_reports() {
        let f = Fixture::parse("void f(Queue<int> q) { q.remove('x'); }");
        let lints = f.run(&one_rule("collection_methods_unrelated_type"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The argument type 'String' isn't related to 'int'."
        );

        let f = Fixture::parse("void f(Queue<int> q) { q.remove(1); }");
        assert!(
            f.run(&one_rule("collection_methods_unrelated_type"))
                .is_empty()
        );
    }

    /// The inferred receiver face: an untyped local decides through its
    /// initializer — a core `Queue<int>` creation rides the table, a
    /// user-container creation (`Box`) and a map literal never do.
    #[test]
    fn collection_methods_inferred_initializer_receivers() {
        let reported = run_semantic(
            "import 'dart:collection';\nvoid f() {\n  var q = Queue<int>();\n  q.remove('x');\n}\n",
            "collection_methods_unrelated_type",
            None,
        );
        assert_eq!(reported.len(), 1, "Queue<int>.remove -> E: {reported:?}");
        assert_eq!(
            reported[0].message,
            "The argument type 'String' isn't related to 'int'."
        );

        let reported = run_semantic(
            "void f() {\n  var m = <int, int>{};\n  m.containsKey('k');\n}\n",
            "collection_methods_unrelated_type",
            None,
        );
        assert_eq!(reported.len(), 1, "Map K=int vs String: {reported:?}");

        let silent = run_semantic(
            "class Box {\n  void remove(String x) {}\n}\nvoid f() {\n  var b = Box();\n  b.remove('x');\n}\n",
            "collection_methods_unrelated_type",
            None,
        );
        assert!(
            silent.is_empty(),
            "a user container rides no slot: {silent:?}"
        );
    }

    /// The opaque-receiver over-report face: a cross-file user class
    /// sharing a core spelling carries the user's members, and the
    /// official `collectionTypeFor` read never lands on it.
    #[test]
    fn collection_methods_cross_file_user_container_stays_silent() {
        let lints = run_semantic(
            "import 'kit.dart';\nvoid f(Queue<int> q) {\n  q.remove('x');\n}\n",
            "collection_methods_unrelated_type",
            Some("class Queue<T> {\n  void remove(T x) {}\n}\n"),
        );
        assert!(
            lints.is_empty(),
            "the user's own Queue is not dart:collection's: {lints:?}"
        );
    }

    // —— prefer_for_elements_to_map_fromIterable ———————————————————————————

    /// `Map<int, int>.fromIterable(...)` — the receiver run's explicit
    /// type-argument wrappers strip before the head match: the creation
    /// resolves to the same map element and reports like the bare
    /// spelling (production bridge).
    #[test]
    fn map_from_iterable_explicit_type_arguments_report() {
        let f = Fixture::parse(
            "void f(Iterable<int> i) { Map<int, int>.fromIterable(i, key: (e) => e, value: (e) => e); }",
        );
        assert_eq!(
            f.run(&one_rule("prefer_for_elements_to_map_fromiterable"))
                .len(),
            1
        );
    }

    /// `Map.fromIterable(x, key: …, value: …)` with two single-expression
    /// closures reports at the creation.
    #[test]
    fn map_from_iterable_reports_with_both_closures() {
        let source = "Map.fromIterable(x, key: (e) => e, value: (e) => e);";
        // Map0 .1 fromIterable2 (3 x4 ,5 key6 :7 (8 e9 )10 =>11 e12 ,13 value14 :15 (16 e17 )18 =>19 e20 )21 ;22
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Send, 0);
                b.begin(NodeKind::Qualified, 0);
                b.patch(0, 1);
                identifier(b, 0, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::Qualified, 3);
                b.begin(NodeKind::Arguments, 3);
                identifier(b, 4, 5);
                b.begin(NodeKind::NamedArgument, 5);
                b.patch(0, 7);
                identifier(b, 6, 7);
                b.begin(NodeKind::FunctionExpression, 8);
                b.begin(NodeKind::FormalParameters, 8);
                b.begin(NodeKind::FormalParameter, 8);
                b.patch(0, NONE);
                b.patch(1, 9);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                identifier(b, 9, 10);
                b.end(NodeKind::FormalParameter, 11);
                b.end(NodeKind::FormalParameters, 11);
                b.begin(NodeKind::ExpressionBody, 11);
                b.patch(0, 11);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                identifier(b, 12, 13);
                b.end(NodeKind::ExpressionBody, 13);
                b.end(NodeKind::FunctionExpression, 13);
                b.end(NodeKind::NamedArgument, 13);
                b.begin(NodeKind::NamedArgument, 13);
                b.patch(0, 15);
                identifier(b, 14, 15);
                b.begin(NodeKind::FunctionExpression, 16);
                b.begin(NodeKind::FormalParameters, 16);
                b.begin(NodeKind::FormalParameter, 16);
                b.patch(0, NONE);
                b.patch(1, 17);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                identifier(b, 17, 18);
                b.end(NodeKind::FormalParameter, 19);
                b.end(NodeKind::FormalParameters, 19);
                b.begin(NodeKind::ExpressionBody, 19);
                b.patch(0, 19);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                identifier(b, 20, 21);
                b.end(NodeKind::ExpressionBody, 22);
                b.end(NodeKind::FunctionExpression, 22);
                b.end(NodeKind::NamedArgument, 22);
                b.end(NodeKind::Arguments, 22);
                b.end(NodeKind::Send, 23);
                b.end(NodeKind::ExpressionStatement, 23);
            }),
        );
        let lints = f.run(&one_rule("prefer_for_elements_to_map_fromiterable"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use 'for' elements when building maps from iterables."
        );
    }

    /// A missing value closure (two arguments) passes (:57-69).
    #[test]
    fn map_from_iterable_without_value_closure_passes() {
        let source = "Map.fromIterable(x, key: (e) => e);";
        // Map0 .1 fromIterable2 (3 x4 ,5 key6 :7 (8 e9 )10 =>11 e12 )13 ;14
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Send, 0);
                b.begin(NodeKind::Qualified, 0);
                b.patch(0, 1);
                identifier(b, 0, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::Qualified, 3);
                b.begin(NodeKind::Arguments, 3);
                identifier(b, 4, 5);
                b.begin(NodeKind::NamedArgument, 5);
                b.patch(0, 7);
                identifier(b, 6, 7);
                b.begin(NodeKind::FunctionExpression, 8);
                b.begin(NodeKind::FormalParameters, 8);
                b.begin(NodeKind::FormalParameter, 8);
                b.patch(0, NONE);
                b.patch(1, 9);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                identifier(b, 9, 10);
                b.end(NodeKind::FormalParameter, 11);
                b.end(NodeKind::FormalParameters, 11);
                b.begin(NodeKind::ExpressionBody, 11);
                b.patch(0, 11);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                identifier(b, 12, 13);
                b.end(NodeKind::ExpressionBody, 14);
                b.end(NodeKind::FunctionExpression, 14);
                b.end(NodeKind::NamedArgument, 14);
                b.end(NodeKind::Arguments, 14);
                b.end(NodeKind::Send, 15);
                b.end(NodeKind::ExpressionStatement, 15);
            }),
        );
        assert!(
            f.run(&one_rule("prefer_for_elements_to_map_fromiterable"))
                .is_empty()
        );
    }
    // ---- #586: collection_methods_unrelated_type (production bridge) ----

    /// A scalar-literal argument unrelated to the receiver's written type
    /// argument reports at the argument; comparable arguments and methods
    /// outside the table stay silent.
    #[test]
    fn collection_methods_unrelated_type_reports_mismatched_argument() {
        let f = Fixture::parse(
            "class C {\n  List<int> list = [];\n  Set<String> set = {};\n  Map<int, String> map = {};\n  void m() {\n    this.list.contains('x');\n    this.set.remove(1);\n    this.map.containsKey('k');\n    this.map.containsValue(1);\n    this.list.contains(1);\n    this.list.length;\n  }\n}",
        );
        let lints = f.run(&one_rule("collection_methods_unrelated_type"));
        assert_eq!(lints.len(), 4, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The argument type 'String' isn't related to 'int'."
        );
        assert_eq!(
            lints[1].message,
            "The argument type 'int' isn't related to 'String'."
        );
        assert_eq!(
            lints[2].message,
            "The argument type 'String' isn't related to 'int'."
        );
        assert_eq!(
            lints[3].message,
            "The argument type 'int' isn't related to 'String'."
        );
    }

    /// `num` accepts `int`/`double` arguments; a raw written type carries
    /// no verdict.
    #[test]
    fn collection_methods_unrelated_type_num_and_raw_pass() {
        let f = Fixture::parse(
            "class C {\n  List<num> nums = [];\n  List raw = [];\n  void m() {\n    this.nums.contains(1);\n    this.nums.contains(0.5);\n    this.raw.contains('x');\n  }\n}",
        );
        assert!(
            f.run(&one_rule("collection_methods_unrelated_type"))
                .is_empty()
        );
    }

    // —— the converted audit faces (exemption-reaudit F2/F6/F7): the
    // production-bridge pins for the resolution-backed decisions ——

    /// The `Map.[]` index row (:180-200): both the read and the `[]=`
    /// write check the key against `K` — the oracle probe reports
    /// `this.map['k']` and `this.map['k'] = 'v'`, and stays silent on the
    /// related key.
    #[test]
    fn collection_methods_index_faces() {
        let f = Fixture::parse(
            r"
class C {
  Map<int, String> map = <int, String>{};

  void m() {
    this.map[1];
    this.map['k'];
    this.map['k'] = 'v';
  }
}
",
        );
        let lints = f.run(&one_rule("collection_methods_unrelated_type"));
        assert_eq!(lints.len(), 2, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The argument type 'String' isn't related to 'int'."
        );
        assert_eq!(
            lints[1].message,
            "The argument type 'String' isn't related to 'int'."
        );
    }

    /// The `Queue.remove` row (:158-164): the unshadowed written
    /// dart:collection spelling keys the table.
    #[test]
    fn collection_methods_queue_remove_row() {
        let f = Fixture::parse(
            r"
class C {
  Queue<String> queue = Queue<String>();

  void m() {
    this.queue.remove(1);
    this.queue.remove('x');
  }
}
",
        );
        let lints = f.run(&one_rule("collection_methods_unrelated_type"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The argument type 'int' isn't related to 'String'."
        );
    }

    /// The typed-literal receiver (`<num>[].contains('x')` — the shape the
    /// official ListTest/SetTest/MapTest blocks pin): the literal is its
    /// own static type. The untyped literal stays silent — the inferred
    /// `dynamic` element suppresses every comparison
    /// (`test_contains_related_ListOfDynamic`) — `num` accepts `int`, and
    /// a `Map` receiver's `contains` never reaches the table (`Map` is no
    /// `Iterable`, the official `asInstanceOf(iterableElement)` miss).
    #[test]
    fn collection_methods_typed_list_literal_receiver() {
        let f = Fixture::parse(
            "void f() {\n  <num>[].contains('x');\n  [].contains('x');\n  <num>[].contains(1);\n  <int, String>{}.contains('x');\n}",
        );
        let lints = f.run(&one_rule("collection_methods_unrelated_type"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The argument type 'String' isn't related to 'num'."
        );
        // The report anchors at the argument (`'x'` of the first
        // statement).
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "'x'", 0))
        );
    }

    /// The typed map/set literal faces: `containsValue` reads `V`, a set
    /// literal's `remove` reads `E`, and the index read and the `[]=` write
    /// both check the key against `K`.
    #[test]
    fn collection_methods_typed_map_and_set_literal_faces() {
        let f = Fixture::parse(
            "void f() {\n  <int, String>{}.containsValue(1);\n  <String>{'a', 'b'}.remove(1);\n  <int, String>{}['k'];\n  <int, String>{}['k'] = 1;\n}",
        );
        let lints = f.run(&one_rule("collection_methods_unrelated_type"));
        assert_eq!(lints.len(), 4, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The argument type 'int' isn't related to 'String'."
        );
        assert_eq!(
            lints[1].message,
            "The argument type 'int' isn't related to 'String'."
        );
        assert_eq!(
            lints[2].message,
            "The argument type 'String' isn't related to 'int'."
        );
        assert_eq!(
            lints[3].message,
            "The argument type 'String' isn't related to 'int'."
        );
    }

    /// `String.contains` rides the `Iterable<String>` face — the expected
    /// element is `String` itself, so a `String` argument is related and
    /// `int`/`null` report (`typesAreUnrelated(Null, String)`: the right
    /// type is `Null` against a non-nullable left).
    #[test]
    fn collection_methods_string_literal_contains() {
        let f = Fixture::parse(
            "void f() {\n  'abc'.contains(0);\n  'abc'.contains('a');\n  'abc'.contains(null);\n}",
        );
        let lints = f.run(&one_rule("collection_methods_unrelated_type"));
        assert_eq!(lints.len(), 2, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The argument type 'int' isn't related to 'String'."
        );
        assert_eq!(
            lints[1].message,
            "The argument type 'Null' isn't related to 'String'."
        );
    }

    /// The dart:collection implementations key through their core
    /// supertype's slots (`HashMap` as `Map`, `HashSet` as
    /// `Set`/`Iterable`, `ListQueue` as `Queue`/`Iterable` — their
    /// resolved members declare on the core containers, which is what the
    /// official `_checkMethod` keys on); a raw spelling carries no type
    /// arguments and no verdict.
    #[test]
    fn collection_methods_identity_spelling_receivers() {
        let f = Fixture::parse(
            r"
class C {
  HashMap<String, int> map = HashMap<String, int>();
  HashSet<int> set = HashSet<int>();
  ListQueue<int> queue = ListQueue<int>();
  HashMap raw = HashMap();

  void m() {
    map.containsKey(1);
    map.remove(1);
    map.containsValue('x');
    set.contains('x');
    set.lookup('x');
    queue.remove('x');
    raw.remove('x');
  }
}
",
        );
        let lints = f.run(&one_rule("collection_methods_unrelated_type"));
        assert_eq!(lints.len(), 6, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The argument type 'int' isn't related to 'String'."
        );
        assert_eq!(
            lints[1].message,
            "The argument type 'int' isn't related to 'String'."
        );
        for lint in &lints[2..] {
            assert_eq!(
                lint.message,
                "The argument type 'String' isn't related to 'int'."
            );
        }
    }

    /// The receiver resolution: a bare name resolves through the
    /// innermost binding (a local shadows the field — the local's type
    /// decides), and a same-file class owning the written collection name
    /// stays silent (the `collectionTypeFor` miss, the oracle probe's
    /// shadow face).
    #[test]
    fn collection_methods_receiver_resolution_faces() {
        // A local `List<int>` shadows the `List<String>` field — the
        // element is int, so `'x'` reports against int.
        let f = Fixture::parse(
            r"
class C {
  List<String> list = <String>[];

  void m() {
    List<int> list = <int>[];
    list.remove('x');
  }
}
",
        );
        let lints = f.run(&one_rule("collection_methods_unrelated_type"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The argument type 'String' isn't related to 'int'."
        );

        // A same-file `class List` owns the written name — silent.
        let f = Fixture::parse(
            r"
class List {
  void remove(Object? x) {}
}

class D {
  List<int> list = <int>[];

  void m() {
    list.remove('x');
  }
}
",
        );
        assert!(
            f.run(&one_rule("collection_methods_unrelated_type"))
                .is_empty()
        );
    }

    /// The typed-receiver gate on `where` (:39-42): an untyped parameter
    /// and a user-class receiver carry no same-file Iterable verdict and
    /// stay silent like the official unresolvable-target bail; the typed
    /// `List` parameter (the corpus fixture face) still reports.
    #[test]
    fn where_type_unresolved_receivers_pass() {
        let f = Fixture::parse("void f(xs) { xs.where((e) => e is int); }");
        assert!(f.run(&one_rule("prefer_iterable_wheretype")).is_empty());

        let f = Fixture::parse(
            r"
class User {
  void where(bool Function(dynamic) test) {}
}

void f(User u) {
  u.where((e) => e is int);
}
",
        );
        assert!(f.run(&one_rule("prefer_iterable_wheretype")).is_empty());

        let f = Fixture::parse("void f(List<int> xs) { xs.where((e) => e is int); }");
        assert_eq!(f.run(&one_rule("prefer_iterable_wheretype")).len(), 1);
    }

    /// The closure requiredness gate (:77-80): an optional positional or
    /// a bare named parameter bails; `{required e}` reports; a same-file
    /// `Map` declaration stays silent (the constructor element pin).
    #[test]
    fn map_from_iterable_requiredness_and_shadow_faces() {
        let f =
            Fixture::parse("void f(x) { Map.fromIterable(x, key: ([e]) => e, value: (e) => e); }");
        assert!(
            f.run(&one_rule("prefer_for_elements_to_map_fromiterable"))
                .is_empty()
        );

        let f =
            Fixture::parse("void f(x) { Map.fromIterable(x, key: ({e}) => e, value: (e) => e); }");
        assert!(
            f.run(&one_rule("prefer_for_elements_to_map_fromiterable"))
                .is_empty()
        );

        let f = Fixture::parse(
            "void f(x) { Map.fromIterable(x, key: ({required e}) => e, value: (e) => e); }",
        );
        assert_eq!(
            f.run(&one_rule("prefer_for_elements_to_map_fromiterable"))
                .len(),
            1
        );

        let f = Fixture::parse(
            r"
class Map {}

void f(x) {
  Map.fromIterable(x, key: (e) => e, value: (e) => e);
}
",
        );
        assert!(
            f.run(&one_rule("prefer_for_elements_to_map_fromiterable"))
                .is_empty()
        );
    }
    // —— prefer_foreach ——————————————————————————————————————————————————

    /// A function-typed formal's inner parameter naming the loop variable
    /// is the inner binder's own declaration, never a target reference —
    /// the official resolves it to the inner parameter and reports; the
    /// shadow scan carves the inner parameter list too.
    #[test]
    fn foreach_function_typed_formal_inner_name_is_no_reference() {
        let f = Fixture::parse(
            "void t(List<int> xs) {\n  for (var e in xs) {\n    ((void cb(int e)) => cb(0))(e);\n  }\n}\n",
        );
        assert_eq!(
            f.run(&one_rule("prefer_foreach")).len(),
            1,
            "the inner `e` binds itself, the target is clean"
        );
    }

    /// `test/rules/prefer_foreach_test.dart`
    /// `test_blockBody_singleStatement_functionCall` — the single-statement
    /// body invoking a function with the loop variable reports at the whole
    /// for statement.
    #[test]
    fn foreach_single_statement_body_reports() {
        let f = Fixture::parse(
            "void f(List<int> list, void Function(int) fn) {\n  for (final a in list) {\n    fn(a);\n  }\n}\n",
        );
        let lints = f.run(&one_rule("prefer_foreach"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Use 'forEach' and a tear-off rather than a 'for' loop to apply a function to every element."
        );
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try using 'forEach' and a tear-off rather than a 'for' loop.")
        );
        let for_kw = tok(&f.source, &f.tokens, "for", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, for_kw));
    }

    /// `test_blockBody_multipleStatements` — a two-statement body passes.
    #[test]
    fn foreach_multiple_statements_pass() {
        let f = Fixture::parse(
            "void f(List<int> list, void Function(int) fn) {\n  for (final a in list) {\n    fn(a);\n    fn(a);\n  }\n}\n",
        );
        assert!(f.run(&one_rule("prefer_foreach")).is_empty());
    }

    /// `test_blockBody_singleStatement_methodCall_explicitTarget` — a target
    /// without a loop-variable reference still reports.
    #[test]
    fn foreach_explicit_target_reports() {
        let f = Fixture::parse(
            "void f(D d, List<int> list) {\n  for (final a in list) {\n    d.f(a);\n  }\n}\nclass D {\n  void f(int a) {}\n}\n",
        );
        assert_eq!(f.run(&one_rule("prefer_foreach")).len(), 1);
    }

    /// `test_blockBody_singleStatement_methodCall_forVariableIsTarget` — the
    /// loop variable as the target keeps the rule silent (:81).
    #[test]
    fn foreach_loop_variable_target_passes() {
        let f = Fixture::parse(
            "void f(List<D> list) {\n  for (final d in list) {\n    d.f(d);\n  }\n}\nclass D {\n  void f(D d) {}\n}\n",
        );
        assert!(f.run(&one_rule("prefer_foreach")).is_empty());
    }

    /// `test_blockBody_singleStatement_methodCall_forVariableIsInTarget` —
    /// the loop variable nested inside the target (an index argument)
    /// references it (:97-113).
    #[test]
    fn foreach_loop_variable_nested_in_target_passes() {
        let f = Fixture::parse(
            "void f(List<D> list) {\n  for (final d in list) {\n    list[list.indexOf(d)].f(d);\n  }\n}\nclass D {\n  void f(D d) {}\n}\n",
        );
        assert!(f.run(&one_rule("prefer_foreach")).is_empty());
    }

    /// `test_blockBody_singleStatement_parenthesizedFunctionCall` — the
    /// parenthesized face reports (:87-89).
    #[test]
    fn foreach_parenthesized_body_reports() {
        let f = Fixture::parse(
            "void f(List<int> list, void Function(int) fn) {\n  for (final a in list) {\n    (fn(a));\n  }\n}\n",
        );
        assert_eq!(f.run(&one_rule("prefer_foreach")).len(), 1);
    }

    /// `test_nonBlockBody_singleStatement` — the non-block body reports.
    #[test]
    fn foreach_non_block_body_reports() {
        let f = Fixture::parse(
            "void f(List<int> list, void Function(int) fn) {\n  for (final a in list) fn(a);\n}\n",
        );
        assert_eq!(f.run(&one_rule("prefer_foreach")).len(), 1);
    }

    /// `test_blockBody_singleStatement_functionTypedExpressionCall` — the
    /// function-expression-invocation face has no target check (:68-73).
    #[test]
    fn foreach_function_expression_invocation_reports() {
        let f = Fixture::parse(
            "void Function(int) fn() => (int a) {};\n\nvoid f(List<int> list) {\n  for (final a in list) {\n    fn()(a);\n  }\n}\n",
        );
        assert_eq!(f.run(&one_rule("prefer_foreach")).len(), 1);
    }

    /// The identifier form (`for (e in xs)`) is no
    /// `ForEachPartsWithDeclaration` and never fires (:57); a wrapped
    /// statement expression (a variable declaration) is not the walked face.
    #[test]
    fn foreach_identifier_form_and_declaration_body_pass() {
        let f = Fixture::parse(
            "void f(List<int> list, void Function(int) fn) {\n  for (a in list) {\n    fn(a);\n  }\n}\n",
        );
        assert!(f.run(&one_rule("prefer_foreach")).is_empty());

        let f = Fixture::parse(
            "void f(List<int> list, void Function(int) fn) {\n  for (final a in list) {\n    var b = fn(a);\n  }\n}\n",
        );
        assert!(f.run(&one_rule("prefer_foreach")).is_empty());
    }

    /// The shadowing face: a nested function literal's own parameter binds
    /// the loop variable's name, so the lexemes inside its extent are the
    /// binder's (`_ReferenceFinder`'s `canonicalElement` reads the
    /// resolution) — the target does not reference the loop variable and
    /// the rule reports where the pre-shadow scan stayed silent.
    #[test]
    fn foreach_shadowing_closure_in_target_reports() {
        let f = Fixture::parse(
            "void f(List<int> list) {\n  for (final element in list) {\n    list.map((element) => element).first.f(element);\n  }\n}\n",
        );
        assert_eq!(f.run(&one_rule("prefer_foreach")).len(), 1);
    }

    /// The capture face stays silent: a closure without a binder of that
    /// name captures the loop variable, and the official's reference walk
    /// (:97-113) finds it in the target.
    #[test]
    fn foreach_capturing_closure_in_target_passes() {
        let f = Fixture::parse(
            "void f(List<int> list) {\n  for (final element in list) {\n    list.map(() => element).first.f(element);\n  }\n}\n",
        );
        assert!(f.run(&one_rule("prefer_foreach")).is_empty());
    }
}

pub(crate) mod prefer_if_elements_to_conditional_expressions {
    use super::{Check, NodeKind, RuleSpec};

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/prefer_if_elements_to_conditional_expressions.dart`
    /// — a conditional expression used directly as a collection element
    /// reports (`_shouldReport`): an element of a list literal, an element
    /// of a set literal, the then/else branch of an if-element, or the body
    /// of a for-element. The reported node climbs the parenthesized
    /// wrappers, so the span covers the whole parenthesized form. Message:
    /// `diagnostic.g.dart` `preferIfElementsToConditionalExpressions`.
    ///
    /// dartq faces: the production tiling makes a spread (`...`/`...?`) or
    /// null-aware (`?`) element a marker leaf preceding its expression, so
    /// the official parent there is the `SpreadElement`/`NullAwareElement`
    /// and no arm matches — a marker sibling keeps the rule silent. The set
    /// face reads the parser's `hasSetEntry` bit (payload word 1; upstream
    /// `_fe_analyzer_shared` parser, parser_impl.dart:8593-8598 — the first
    /// simple element's colon decides) in place of the resolver-computed
    /// `isSet`: the two agree on every valid literal, and diverge only on
    /// mixed entry/element literals, which are compile-time errors.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_if_elements_to_conditional_expressions",
    &[NodeKind::Conditional],
    check,
);

    /// The innermost row strictly containing `row`'s span: post-order places
    /// enclosing rows after their contents, so the first later row whose
    /// span covers `row` is the tightest one. Collection elements (and the
    /// literals themselves) leak as childless siblings into the enclosing
    /// statement run, so the real parent chain never names the collection —
    /// span containment does.
    fn enclosing(c: &Check<'_>, row: u32) -> Option<u32> {
        let span = c.byte_span(row);
        let len = c.unit.ast.len() as u32;
        (row + 1..len).find(|&candidate| {
            let s = c.byte_span(candidate);
            s.start <= span.start && s.end >= span.end
        })
    }

    fn check(c: &mut Check<'_>) {
        // `errorNode` climbs the parenthesized wrappers (`errorNode = parent`
        // in the official walk) and `parent` lands on the first non-paren
        // ancestor — over enclosing spans here, since the element rows sit
        // outside the reported node's parent chain.
        let mut error_node = c.node();
        let parent = loop {
            let Some(container) = enclosing(c, error_node) else {
                return;
            };
            if c.kind(container) != NodeKind::ParenthesizedExpression {
                break container;
            }
            error_node = container;
        };
        let err_span = c.byte_span(error_node);
        // The element lead-in: a spread (`...`) or null-aware (`?`) marker
        // leaf owns the element expression (the official parent is the
        // wrapper, so no arm matches). The marker seals right after the
        // element's rows — in post-order it sits between `error_node` and
        // the container's own seal.
        let marker_after = (error_node + 1..parent).any(|r| {
            matches!(c.kind(r), NodeKind::Spread | NodeKind::NullAwareElement)
                && c.byte_span(r).start < err_span.start
        });
        if marker_after {
            return;
        }
        let window = c.children(parent);
        let reports = match c.kind(parent) {
            // A list element always reports (:40-41).
            NodeKind::ListLiteral => true,
            // The set face: `parent.isSet` through the parser shape bit.
            NodeKind::SetOrMapLiteral => c.payload(parent).get(1).copied().unwrap_or(0) != 0,
            // The then/else branches report; the condition position (ahead
            // of the then marker) never does (:42-46). `in_condition`
            // stays as a recovery-tiling guard: a conditional inside the
            // `ParenthesizedCondition` span is the condition itself — over
            // well-formed tilings it is what keeps `if (c ? a : b) x`
            // silent — and markers seal as leaf rows inside the
            // if-element's window.
            NodeKind::IfControlFlow => {
                let in_condition = (window.start..parent).any(|r| {
                    c.kind(r) == NodeKind::ParenthesizedCondition
                        && c.byte_span(r).start <= err_span.start
                        && c.byte_span(r).end >= err_span.end
                });
                !in_condition
                    && (window.start..error_node).any(|r| {
                        matches!(
                            c.kind(r),
                            NodeKind::ThenControlFlow | NodeKind::ElseControlFlow
                        )
                    })
            }
            // The body reports; a conditional inside the loop parts (e.g.
            // the iterable) does not (:47-50). The parts row is the member
            // of the window whose span still covers the node.
            NodeKind::ForControlFlow => !(window.start..parent).any(|r| {
                matches!(c.kind(r), NodeKind::ForParts | NodeKind::ForInParts)
                    && c.byte_span(r).start <= err_span.start
                    && c.byte_span(r).end >= err_span.end
            }),
            _ => false,
        };
        if reports {
            let range = c.utf16_span(error_node);
            c.report(range, &[]);
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule};

        /// `test/rules/prefer_if_elements_to_conditional_expressions_test.dart`
        /// `test_conditionalInList`.
        #[test]
        fn reports_a_conditional_in_a_list_literal() {
            let f = Fixture::parse("List<String> f(bool b) { return ['a', b ? 'c' : 'd', 'e']; }");
            let lints = f.run(&one_rule("prefer_if_elements_to_conditional_expressions"));
            assert_eq!(lints.len(), 1);
            assert_eq!(
                lints[0].message,
                "Use an 'if' element to conditionally add elements."
            );
            assert_eq!(
                lints[0].correction.as_deref(),
                Some("Try using an 'if' element rather than a conditional expression.")
            );
        }

        /// `test_conditionalInSet` — all bare elements, `hasSetEntry` true.
        #[test]
        fn reports_a_conditional_in_a_set_literal() {
            let f = Fixture::parse("Set<String> f(bool b) { return {'a', b ? 'c' : 'd', 'e'}; }");
            assert_eq!(
                f.run(&one_rule("prefer_if_elements_to_conditional_expressions"))
                    .len(),
                1
            );
        }

        /// `test_conditionalInIfElement_then`.
        #[test]
        fn reports_an_if_element_then_branch() {
            let f = Fixture::parse(
                "List<String> f(bool b, bool c) { return ['a', if (b) c ? 'd' : 'e', 'f']; }",
            );
            assert_eq!(
                f.run(&one_rule("prefer_if_elements_to_conditional_expressions"))
                    .len(),
                1
            );
        }

        /// The else branch arms like the then branch.
        #[test]
        fn reports_an_if_element_else_branch() {
            let f = Fixture::parse(
                "List<String> f(bool b, bool c) { return ['a', if (b) 'c' else c ? 'd' : 'e']; }",
            );
            assert_eq!(
                f.run(&one_rule("prefer_if_elements_to_conditional_expressions"))
                    .len(),
                1
            );
        }

        /// `test_conditionalInForElement`.
        #[test]
        fn reports_a_for_element_body() {
            let f = Fixture::parse(
                "List<String> f(List<bool> blist) { return ['a', for (var b in blist) b ? 'c' : 'd', 'e']; }",
            );
            assert_eq!(
                f.run(&one_rule("prefer_if_elements_to_conditional_expressions"))
                    .len(),
                1
            );
        }

        /// `test_conditionalInIfElement_condition` — the condition is the
        /// if-element's first child, not a branch.
        #[test]
        fn ignores_an_if_element_condition() {
            let f = Fixture::parse(
                "List<String> f(bool b, bool c) { return ['a', if (c ? false : true) 'd', 'e']; }",
            );
            assert!(
                f.run(&one_rule("prefer_if_elements_to_conditional_expressions"))
                    .is_empty()
            );
        }

        /// `test_conditionalInMap` — the conditional parses as an entry key:
        /// the literal is a map (`hasSetEntry` false) and never fires.
        #[test]
        fn ignores_a_map_entry_key() {
            let f = Fixture::parse(
                "Map<String, int> f(bool b) { return {'a': 1, b ? 'c' : 'd' : 2, 'e': 3}; }",
            );
            assert!(
                f.run(&one_rule("prefer_if_elements_to_conditional_expressions"))
                    .is_empty()
            );
        }

        /// `test_conditionalInList_parenthesized` and
        /// `test_conditionalInSet_parenthesizedTwice` — the report covers
        /// the whole parenthesized form and stays one report.
        #[test]
        fn reports_over_parenthesized_conditionals() {
            let f =
                Fixture::parse("List<String> f(bool b) { return ['a', (b ? 'c' : 'd'), 'e']; }");
            assert_eq!(
                f.run(&one_rule("prefer_if_elements_to_conditional_expressions"))
                    .len(),
                1
            );
            let f =
                Fixture::parse("Set<String> f(bool b) { return {'a', ((b ? 'c' : 'd')), 'e'}; }");
            assert_eq!(
                f.run(&one_rule("prefer_if_elements_to_conditional_expressions"))
                    .len(),
                1
            );
        }

        /// A conditional outside a collection element position never fires.
        #[test]
        fn ignores_a_conditional_outside_collections() {
            let f = Fixture::parse("void f(bool b) { var x = b ? 1 : 2; g(b ? 1 : 2); }");
            assert!(
                f.run(&one_rule("prefer_if_elements_to_conditional_expressions"))
                    .is_empty()
            );
        }

        /// A spread element owns its expression — the official parent is the
        /// `SpreadElement` and no arm matches.
        #[test]
        fn ignores_a_conditional_under_a_spread_element() {
            let f = Fixture::parse(
                "List<String> f(bool b) { return ['a', ...(b ? ['c'] : ['d']), 'e']; }",
            );
            assert!(
                f.run(&one_rule("prefer_if_elements_to_conditional_expressions"))
                    .is_empty()
            );
        }
    }
}

/// The null-aware-element family: an if-element null check the `?` marker
/// spells shorter.
pub(crate) mod use_null_aware_elements {
    use dartq_ast::NodeKind;

    use super::{Check, RuleSpec, slice};
    use crate::rules::{MemberQuery, utf16_token_span};

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/use_null_aware_elements.dart` — an
    /// if-element whose condition null-checks a target and whose
    /// then-element is exactly that target reports at the `if` keyword:
    /// `[if (x != null) x]` → `[?x]`, `{if (x != null) x: v}` →
    /// `{?x: v}`, and the getter face spells the `!` (`...x!`) — with a
    /// `case var y?` null-check pattern (no `when`) matching on the
    /// declared name (:44-135). An `else` segment never reports (:44).
    ///
    /// Gate note: the official registration bails without the
    /// `null_aware_elements` feature (:35, positive polarity — the rule
    /// runs only with it, `experimental_features.yaml` `enabledIn:
    /// 3.8.0`); the check-entry gate reads the
    /// [`crate::engine::Features`] `null_aware_elements` bit (shipped,
    /// default on).
    ///
    /// Approximation (documented): the official target/reference match
    /// runs over canonical elements; the face here is name-level — a
    /// target with a same-file value binding is promotable (the bare
    /// then-form), a same-file getter takes the `!` form, and an
    /// unresolvable name (a cross-file getter) stays silent (the
    /// official non-promotable posture, `test_nonPromotable_nullCheck`
    /// list). Reference rows read the `expression`/`expressionContinuation`
    /// identifier contexts only, so named-argument labels and selectors
    /// never match. A member-access operand (`a.b`, `this.b`) names its
    /// rightmost selector and carries the getter face — same-file getters
    /// resolve, cross-file members under-report. The member lookup walks
    /// the enclosing class chain and the top-level getter index only, so a
    /// member reached through a DIFFERENT same-file class's instance
    /// (`class C { int? get b; }` + `class D { m(C a) => [if (a.b != null)
    /// b!]; }`) under-reports: resolving `a`'s type to C's member needs
    /// element identity the name-level face does not carry.
    /// Message: `diagnostic.g.dart` `useNullAwareElements`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "use_null_aware_elements",
    &[NodeKind::IfControlFlow],
    check,
);

    /// The then-form the target's resolution demands.
    #[derive(Clone, Copy)]
    enum Target<'a> {
        /// A promotable (value-bound) or pattern-declared name: the bare
        /// then-form reports.
        Bare(&'a str),
        /// A getter: only the `!` then-form reports (the official
        /// `GetterElement` arms, :86-98).
        Getter(&'a str),
    }

    fn check(c: &mut Check<'_>) {
        if !c.unit.features.null_aware_elements {
            return;
        }
        let node = c.node();
        let kids = c.collect_direct(node);
        // The then segment runs to the end; an else segment never reports
        // (:44 — `elseKeyword: null`).
        let Some(then_pos) = kids
            .iter()
            .position(|&kid| c.kind(kid) == NodeKind::ThenControlFlow)
        else {
            return;
        };
        if kids[then_pos..]
            .iter()
            .any(|&kid| c.kind(kid) == NodeKind::ElseControlFlow)
        {
            return;
        }
        let Some(target) = condition_target(c, &kids[..then_pos]) else {
            return;
        };
        let then_rows: Vec<u32> = kids[then_pos + 1..].to_vec();
        if then_reports(c, &then_rows, target) {
            c.report(utf16_token_span(c, c.head_token(node)), &[]);
        }
    }

    /// The null-check target of the condition segment: the `x != null`
    /// face over the condition's binary expression, or the `x case var y?`
    /// face (a case clause with a null-check pattern and no `when`,
    /// :61-67).
    fn condition_target<'a>(c: &'a Check<'a>, condition: &[u32]) -> Option<Target<'a>> {
        for &container in condition {
            if c.kind(container) != NodeKind::ParenthesizedCondition {
                continue;
            }
            // The `when` guard kills the pattern face (:65).
            let words = c.payload(container);
            if words
                .get(1)
                .is_some_and(|&when| when != crate::rules::NONE_TOKEN)
            {
                continue;
            }
            let case = words.first().copied().unwrap_or(crate::rules::NONE_TOKEN);
            if case != crate::rules::NONE_TOKEN {
                // `x case var y?` — the declared variable is the target
                // (:61-67), definitionally promotable. Upstream requires
                // the guarded pattern itself to be
                // `NullCheckPattern(pattern: DeclaredVariablePattern)`; the
                // container adopts exactly the top-level pattern row, so
                // the shape is: the adopted child is the `?` marker leaf
                // and the row sealed right before it is the declared
                // variable it wraps. A composite pattern (record, list,
                // `&&`) adopts its own root instead — a `?` nested inside
                // never qualifies.
                let adopted = c.collect_direct(container);
                let [null_check] = adopted.as_slice() else {
                    return None;
                };
                if c.kind(*null_check) != NodeKind::NullCheckPattern {
                    return None;
                }
                let Some(declared) = null_check.checked_sub(1) else {
                    return None;
                };
                if c.kind(declared) != NodeKind::DeclaredVariablePattern {
                    return None;
                }
                return Some(Target::Bare(
                    c.lexeme(c.payload(declared).first().copied()?),
                ));
            }
            let rows: Vec<u32> = c.children(container).collect();
            // `x != null` / `null != x` — the non-null operand is the
            // target (:46-60). The operator is `!=` only (:52 —
            // `operator.lexeme == '!='`); `==` would invert the `?x`
            // rewrite's semantics.
            for &row in &rows {
                if c.kind(row) != NodeKind::BinaryExpression {
                    continue;
                }
                let operator = c.lexeme(c.payload(row).first().copied()?);
                if operator != "!=" {
                    continue;
                }
                let operands = c.collect_direct(row);
                let (Some(first), Some(second)) = (operands.first(), operands.get(1)) else {
                    continue;
                };
                let operand = if c.kind(*second) == NodeKind::LiteralNull {
                    *first
                } else if c.kind(*first) == NodeKind::LiteralNull {
                    *second
                } else {
                    continue;
                };
                // `canonicalElement` reads `self.unParenthesized`
                // (extensions.dart :196-204): transparent `(...)` wrappers
                // around the operand never change the target.
                let target_row =
                    crate::rules::expressions::static_type_faces::unparenthesized(c, operand);
                // The operand must be a null-checkable target upstream: a
                // bare name (`x` — the tear-off send group
                // `[Identifier, TypeArguments?, NoArguments, Send]`) or a
                // member chain (`a.b` — a `DotAccess` sealing a tear-off
                // selector). A group closing in `Arguments` is an
                // invocation (`x()`/`a.b()`) — the callee's element is
                // neither promotable nor a getter (:75-98), so the
                // condition can never report. The send group's pieces seal
                // as the rows right before its `Send`/`DotAccess`.
                let name = match c.kind(target_row) {
                    NodeKind::Identifier => c.lexeme(c.head_token(target_row)),
                    NodeKind::Send => {
                        let Some(start) = target_row.checked_sub(3) else {
                            continue;
                        };
                        let group = [start, start + 1, start + 2, target_row];
                        if !slice::is_tear_off_group(c, &group, 0) {
                            continue;
                        }
                        c.lexeme(c.head_token(start))
                    }
                    NodeKind::DotAccess => {
                        // The rightmost selector must be a tear-off: its
                        // send group is the four rows right before the
                        // access marker. The canonical element of `a.b` is
                        // the accessed member — only the getter `!`
                        // then-form can report (:86-98).
                        let Some(start) = target_row.checked_sub(4) else {
                            continue;
                        };
                        let group = [start, start + 1, start + 2, target_row - 1];
                        if !slice::is_tear_off_group(c, &group, 0) {
                            continue;
                        }
                        let name = c.lexeme(c.head_token(target_row));
                        // A member access resolves to the accessor element —
                        // never a promotable variable — so only the
                        // same-file getter face can answer.
                        if is_same_file_getter(c, name) {
                            return Some(Target::Getter(name));
                        }
                        return None;
                    }
                    _ => continue,
                };
                // The promotable split (upstream Dart SDK source,
                // `pkg/analyzer/lib/src/dart/element/element.dart`
                // :3981/:8421/:9947): only locals and parameters are
                // `PromotableElementImpl` — the bare then-form. A field
                // (instance or static) or a top-level variable read
                // resolves to a `GetterElement` — the `!` then-form.
                use crate::rules::expressions::same_file::BindingScope;
                if let Some((scope, _)) =
                    crate::rules::expressions::same_file::binding(c, target_row, name)
                {
                    return Some(match scope {
                        BindingScope::Local | BindingScope::Parameter => Target::Bare(name),
                        BindingScope::Field
                        | BindingScope::StaticField
                        | BindingScope::TopLevel => Target::Getter(name),
                    });
                }
                // No value binding: a getter target takes the `!` form
                // only when it resolves to a same-file getter.
                if is_same_file_getter(c, name) {
                    return Some(Target::Getter(name));
                }
                return None;
            }
        }
        None
    }

    /// Whether the name is a getter visible to the use: an enclosing
    /// class-like chain, or a same-file top-level getter (the official
    /// `GetterElement` targets, :86-98).
    fn is_same_file_getter(c: &Check<'_>, name: &str) -> bool {
        if let Some(class_row) = c.path.iter().rev().find_map(|&a| {
            matches!(
                c.kind(a),
                NodeKind::ClassDeclaration
                    | NodeKind::MixinDeclaration
                    | NodeKind::EnumDeclaration
                    | NodeKind::ExtensionDeclaration
            )
            .then_some(a)
        }) && class_chain_has_getter(c, class_row, name)
        {
            return true;
        }
        // The same-file top-level getter face (`List<int>? get x => null;`).
        let Some(root) = c.unit.ast.root() else {
            return false;
        };
        for top in c.collect_direct(root) {
            if c.kind(top) != NodeKind::TopLevelDeclaration {
                continue;
            }
            for decl in c.collect_direct(top) {
                if c.kind(decl) != NodeKind::TopLevelMethod
                    || crate::rules::get_set_token(c, decl) != Some("get")
                {
                    continue;
                }
                if c.collect_direct(decl).into_iter().any(|kid| {
                    c.kind(kid) == NodeKind::Identifier
                        && c.payload(kid).first()
                            == Some(
                                &(dartq_parser::IdentifierContext::TopLevelFunctionDeclaration
                                    as u32),
                            )
                        && c.lexeme(c.head_token(kid)) == name
                }) {
                    return true;
                }
            }
        }
        false
    }

    /// The bounded class-chain getter walk (extends/with/implements,
    /// same-file links only).
    fn class_chain_has_getter(c: &Check<'_>, class_row: u32, name: &str) -> bool {
        let mut seen = 0usize;
        let mut current = Some(class_row);
        while let Some(row) = current {
            seen += 1;
            if seen > 8 {
                return false;
            }
            for member in crate::rules::body_members(c, row) {
                if !crate::rules::member_matches(c, member, MemberQuery::Getter, name).is_empty() {
                    return true;
                }
            }
            current = None;
            for (supertype, _) in crate::rules::supertypes(c, row) {
                if let Some(row) = crate::rules::same_file_declaration(c, supertype) {
                    current = Some(row);
                    break;
                }
            }
        }
        false
    }

    /// The then-element face over the segment's rows: the element must BE
    /// the target (:70-134) — the bare-name send group, optionally
    /// `!`-marked (getter targets, :86-98) and optionally under a spread
    /// marker (`...x`/`...?x`, :77-81/:91-97); or a `MapEntry` whose key or
    /// value side is exactly that name (:106-133). A larger expression
    /// merely containing the name (`g(x)`, `x + 1`, `[x]`) never matches —
    /// its rows break the shape.
    fn then_reports(c: &Check<'_>, rows: &[u32], target: Target<'_>) -> bool {
        let name = match target {
            Target::Bare(name) | Target::Getter(name) => name,
        };
        if let Some((&marker, sides)) = rows.split_last()
            && c.kind(marker) == NodeKind::MapEntry
        {
            // MapEntry span contract (bridge.rs `handle_literal_map_entry`):
            // the marker's token_range is [value-head .. follower-inclusive)
            // — the entry event fires after the value parses and anchors at
            // the last completed node's head, so `token_range(marker).start`
            // is NEVER the key head and never the entry's source start. The
            // key and value tile in source order ahead of the marker as
            // preceding siblings, split exactly at the colon (payload[0] —
            // the upstream arms read `key`/`value` as whole expressions,
            // :107-133): the key side's rows all end at or before the token
            // right after the colon (a send group's trailing markers absorb
            // the colon and may sit a row past it), and every value row
            // covers a token past the colon. The colon is the ONLY reliable
            // key/value boundary. A name merely contained in a larger side —
            // a member chain's receiver prefix, a binary's operand — is
            // never the side itself.
            let colon = c.payload(marker).first().copied().unwrap_or(0);
            let split = sides.partition_point(|&row| c.unit.ast.token_range(row).end <= colon + 1);
            return element_reports(c, &sides[..split], target, name, false)
                || element_reports(c, &sides[split..], target, name, false);
        }
        element_reports(c, rows, target, name, true)
    }

    /// Whether `seg` is exactly the bare-name face — `[Identifier,
    /// NoTypeArguments, NoArguments, Send]` — plus the optional trailing
    /// markers: `NonNullAssert` (`x!`, the getter form) and `Spread`
    /// (`...x`; element positions only — the map-entry arms admit no
    /// spread side). The name must sit in a value position and the `!`
    /// marker must be exactly what the target's resolution demands.
    fn element_reports(
        c: &Check<'_>,
        seg: &[u32],
        target: Target<'_>,
        name: &str,
        spread: bool,
    ) -> bool {
        let mut seg = seg;
        if spread && seg.last().is_some_and(|&r| c.kind(r) == NodeKind::Spread) {
            seg = &seg[..seg.len() - 1];
        }
        let bang = seg
            .last()
            .is_some_and(|&r| c.kind(r) == NodeKind::NonNullAssert);
        if bang {
            seg = &seg[..seg.len() - 1];
        }
        let [head, tyargs, no_args, send] = seg else {
            return false;
        };
        if c.kind(*head) != NodeKind::Identifier
            || c.kind(*tyargs) != NodeKind::NoTypeArguments
            || c.kind(*no_args) != NodeKind::NoArguments
            || c.kind(*send) != NodeKind::Send
        {
            return false;
        }
        // Value positions only: a named-argument label or a member-access
        // selector is never the then-element.
        let context = c.payload(*head).first().copied();
        if context != Some(dartq_parser::IdentifierContext::Expression as u32)
            && context != Some(dartq_parser::IdentifierContext::ExpressionContinuation as u32)
        {
            return false;
        }
        c.lexeme(c.head_token(*head)) == name && bang == matches!(target, Target::Getter(_))
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

        fn run(source: &str) -> Vec<crate::engine::Lint> {
            Fixture::parse(source).run(&one_rule("use_null_aware_elements"))
        }

        /// `test_nullCheck_list` — both operand orders report at the `if`.
        #[test]
        fn null_check_list_reports_both_orders() {
            let source =
                "List<int> f(int? x) {\n  return [if (x != null) x, if (null != x) x];\n}\n";
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("use_null_aware_elements"));
            assert_eq!(lints.len(), 2, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Use the null-aware marker '?' rather than a null check via an 'if'."
            );
            assert_eq!(lints[0].correction.as_deref(), Some("Try using '?'."));
            assert_eq!(
                lints[0].range.start,
                utf16_at(&f, tok(source, &f.tokens, "if", 0))
            );
        }

        /// `test_nullCheck_mapKey` / `test_nullCheck_mapValue` /
        /// `test_nullCheck_mapKeyAndValue`.
        #[test]
        fn null_check_map_entry_positions_report() {
            assert_eq!(
                run("Map<int, String> f(int? x) { return {if (x != null) x: \"\"}; }").len(),
                1
            );
            assert_eq!(
                run("Map<String, int> f(int? x) { return {if (x != null) \"k\": x}; }").len(),
                1
            );
            assert_eq!(
                run("Map<int, int> f(int? x) { return {if (x != null) x: x}; }").len(),
                1
            );
        }

        /// `test_nullCheck_set`.
        #[test]
        fn null_check_set_reports() {
            assert_eq!(
                run("Set<int> f(int? x) { return {if (x != null) x}; }").len(),
                1
            );
        }

        /// `test_spread_list` — the spread form reports.
        #[test]
        fn spread_form_reports() {
            assert_eq!(
                run("List<int> f(List<int>? x) => [if (x != null) ...x];").len(),
                1
            );
        }

        /// `test_spread_getter_list` — a getter target needs the `!`.
        #[test]
        fn getter_target_takes_the_bang_form() {
            assert_eq!(
                run("List<int>? get x => null;\nList<int> f() => [if (x != null) ...x!];").len(),
                1
            );
            // The bare getter form is not promotable — silent.
            assert!(
                run("List<int>? get x => null;\nList<int> f() => [if (x != null) ...x];")
                    .is_empty()
            );
        }

        /// `test_nullCheckPattern_list` — the `case var y?` face.
        #[test]
        fn null_check_pattern_reports() {
            assert_eq!(
                run("List<int> f(int? x) { return [if (x case var y?) y]; }").len(),
                1
            );
            assert_eq!(
                run("Map<String, int> f(int? x) { return {if (x case var y?) \"k\": y}; }").len(),
                1
            );
        }

        /// `test_nullCheckPattern_whenClause_list` — a `when` guard kills
        /// the pattern face.
        #[test]
        fn pattern_with_when_clause_is_silent() {
            assert!(
                run("List<int> f(int? x) { return [if (x case var y? when y.isEven) y]; }")
                    .is_empty()
            );
        }

        /// An `else` segment never reports (:44).
        #[test]
        fn else_segment_is_silent() {
            assert!(run("List<int> f(int? x) { return [if (x != null) x else 1]; }").is_empty());
        }

        /// A mismatched then-element never reports.
        #[test]
        fn mismatched_then_element_is_silent() {
            assert!(run("List<int> f(int? x) { return [if (x != null) 1]; }").is_empty());
            assert!(run("List<int> f(int? x, int? y) { return [if (x != null) y]; }").is_empty());
        }

        /// The report pins the `if` keyword.
        #[test]
        fn report_pins_the_if_keyword() {
            let source = "List<int> f(int? x) => [if (x != null) x];";
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("use_null_aware_elements"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].range.start,
                utf16_at(&f, tok(source, &f.tokens, "if", 0))
            );
        }

        /// Upstream fires only on `!=` (:52 — `operator.lexeme == '!='`);
        /// `==` inverts the suggested `?x` semantics.
        #[test]
        fn equal_operator_is_silent() {
            assert!(run("List<int> f(int? x) { return [if (x == null) x]; }").is_empty());
            assert!(run("List<int> f(int? x) { return [if (x == null) x else 2]; }").is_empty());
        }

        /// The then-element must BE the target (:70-134 — SimpleIdentifier /
        /// SpreadElement(identifier) / PostfixExpression(identifier!)) — a
        /// name merely appearing inside a larger expression never matches.
        #[test]
        fn non_bare_then_elements_are_silent() {
            assert!(
                run("List<int> f(int? x, int Function(int) g) { return [if (x != null) g(x)]; }")
                    .is_empty()
            );
            assert!(run("List<int> f(int? x) { return [if (x != null) x + 1]; }").is_empty());
            assert!(run("List<List<int>> f(int? x) { return [if (x != null) [x]]; }").is_empty());
            assert!(run("List<int> f(int? x) { return [if (x != null) x as int]; }").is_empty());
            assert!(run("List<int> f(int? x) { return [if (x != null) -x]; }").is_empty());
            assert!(run("List<int> f(int? x) { return [if (x != null) (x)]; }").is_empty());
        }

        /// The map-entry arm shares the shape requirement (:106-133 — the
        /// key or value itself is the name): a call wrapping the name in
        /// either position stays silent.
        #[test]
        fn map_entry_wrapped_target_sides_are_silent() {
            assert!(
                run("Map<int,int> f(int? x, int Function(int) g) { return {if (x != null) g(x): 1}; }")
                    .is_empty()
            );
            assert!(
                run("Map<int,int> f(int? x, int Function(int) g) { return {if (x != null) 1: g(x)}; }")
                    .is_empty()
            );
        }

        /// The colon token (`MapEntry` payload[0]) is the only key/value
        /// boundary: the upstream arms read `key`/`value` as whole
        /// expressions (:106-133), so a side merely CONTAINING the name —
        /// a member chain's receiver prefix or a binary's operand — never
        /// matches.
        #[test]
        fn map_entry_colon_boundary_split_is_silent() {
            // `a.b` as the key: the `a` tear-off prefix is not the key.
            assert!(
                run("class C { int? get b => null; }\nMap<int,int> f(C? a) { return {if (a != null) a.b: 1}; }")
                    .is_empty()
            );
            // `x + 1` as the key: the `x` tear-off prefix is not the key.
            assert!(run("Map<int,int> f(int? x) { return {if (x != null) x + 1: 2}; }").is_empty());
            // `x + y` as the value: the `y` tear-off suffix is not the value.
            assert!(
                run("Map<int,int> f(int? y, int x) { return {if (y != null) 1: x + y}; }")
                    .is_empty()
            );
            // Whole-side matches still report on either position.
            assert_eq!(
                run("Map<int,int> f(int? x, int v) { return {if (x != null) x: v}; }").len(),
                1
            );
            assert_eq!(
                run("Map<int,int> f(int? x, int k) { return {if (x != null) k: x}; }").len(),
                1
            );
        }

        /// The condition's operand unwraps transparent parens before the
        /// shape dispatch (`extensions.dart` `canonicalElement` reads
        /// `self.unParenthesized`, :196-204): `(x) != null` and
        /// `null != (x)` reach the bare-name arm; `(a.b) != null` reaches
        /// the member face — the `a.b` element is the getter, so the `!`
        /// then-form reports and the bare form stays silent.
        #[test]
        fn parenthesized_condition_operand_unwraps() {
            assert_eq!(
                run("List<int> f(int? x) { return [if ((x) != null) x]; }").len(),
                1
            );
            assert_eq!(
                run("List<int> f(int? x) { return [if (null != (x)) x]; }").len(),
                1
            );
            // Nested parens read through to the same target.
            assert_eq!(
                run("List<int> f(int? x) { return [if (((x)) != null) x]; }").len(),
                1
            );
            // The member face under parens: `b` resolves to the enclosing
            // class getter, so only the `!` then-form reports.
            assert_eq!(
                run("class C { int? get b => null; void m() { var l = [if ((this.b) != null) b!]; } }")
                    .len(),
                1
            );
            assert!(
                run("class C { int? get b => null; void m() { var l = [if ((this.b) != null) b]; } }")
                    .is_empty()
            );
        }

        /// `x case var y?` is a top-level `NullCheckPattern` wrapping the
        /// declared variable (:61-67): a `?` inside a composite pattern
        /// binds nothing here (the record/list top level is not a null
        /// check), and a `&&` pattern's top level is the binary pattern.
        #[test]
        fn composite_null_check_patterns_are_silent() {
            assert!(
                run("List<int> f((int, int) x) { return [if (x case (var a, var b?)) a]; }")
                    .is_empty()
            );
            assert!(
                run("List<int> f((int, int) x) { return [if (x case (var a, var b?)) b]; }")
                    .is_empty()
            );
            assert!(
                run("List<int> f(List<int>? x) { return [if (x case [var a, var b?]) a]; }")
                    .is_empty()
            );
            assert!(
                run("List<int> f(List<int>? x) { return [if (x case [var a, var b?]) b]; }")
                    .is_empty()
            );
            assert!(
                run("List<int> f(int? x) { return [if (x case var y? && true) y]; }").is_empty()
            );
        }

        /// A member-access null check targets the accessed member's element
        /// (:57-59 `canonicalElement` of `a.b` — the getter, so only the `!`
        /// then-form can report, :86-98); a bare receiver or the full chain
        /// in the then position never matches. An invocation operand's
        /// target is the callee's element — neither arm reaches it.
        #[test]
        fn member_and_invocation_condition_shapes_are_silent() {
            // The bare receiver is not the target.
            assert!(
                run("class C { int? get b => null; }\nList<int> f(C a) { return [if (a.b != null) a]; }")
                    .is_empty()
            );
            // `a.b` is a PropertyAccess, not SimpleIdentifier/PostfixExpression.
            assert!(
                run("class C { int? get b => null; }\nList<int> f(C a) { return [if (a.b != null) a.b]; }")
                    .is_empty()
            );
            // `x()`'s canonical element is the callee, never promotable.
            assert!(
                run("List<int> f(int? Function() x) { return [if (x() != null) x]; }").is_empty()
            );
            assert!(
                run("List<int> f(int? Function() x) { return [if (x() != null) x()!]; }")
                    .is_empty()
            );
        }

        /// `this.b`/`a.b` targets keep the getter `!` form: the selector
        /// name resolves inside the declaring class and `b!` reports
        /// (:86-98 — the `a.b` and implicit `this.b` reads share the getter).
        #[test]
        fn member_getter_bang_form_reports() {
            assert_eq!(
                run(
                    "class C { int? get b => null; void m() { var l = [if (this.b != null) b!]; } }"
                )
                .len(),
                1
            );
            assert_eq!(
                run(
                    "class C { int? get b => null; void m(C a) { var l = [if (a.b != null) b!]; } }"
                )
                .len(),
                1
            );
            // The bare then-form never matches a getter target.
            assert!(
                run(
                    "class C { int? get b => null; void m() { var l = [if (this.b != null) b]; } }"
                )
                .is_empty()
            );
        }

        /// Name-level approximation pin (documented in `SPEC`): upstream
        /// resolves `a.b` to the getter C.b through `a`'s static type and
        /// reports `b!` (:86-98); the name-level face reads only the
        /// enclosing chain and the top-level index, so a member on a
        /// DIFFERENT same-file class under-reports. Element-identity
        /// resolution is deliberately out of scope for the syntactic face.
        #[test]
        fn member_getter_through_other_class_is_silent() {
            assert!(
                run(
                    "class C { int? get b => null; }\nclass D { void m(C a) { var l = [if (a.b != null) b!]; } }"
                )
                .is_empty()
            );
        }

        /// The promotable split (upstream Dart SDK source,
        /// `pkg/analyzer/lib/src/dart/element/element.dart`
        /// :3981/:8421/:9947 — only locals and parameters are
        /// `PromotableElementImpl`; a field or top-level variable read
        /// resolves to a `GetterElement`): the bare then-form fires only on
        /// a local/parameter target, and a field — instance or static — or
        /// a top-level variable takes the getter `!` then-form (:86-98).
        #[test]
        fn binding_scope_decides_the_then_form() {
            // Instance field: `x` reads the field's getter — bare silent,
            // `x!` reports.
            assert!(run("class C { int? x; List<int> m() => [if (x != null) x]; }").is_empty());
            assert_eq!(
                run("class C { int? x; List<int> m() => [if (x != null) x!]; }").len(),
                1
            );
            // Static field: the same getter face — the innermost static
            // binding shadows any outer name.
            assert!(
                run("class C { static int? x; List<int> m() => [if (x != null) x]; }").is_empty()
            );
            assert_eq!(
                run("class C { static int? x; List<int> m() => [if (x != null) x!]; }").len(),
                1
            );
            // Top-level variable: the same getter face.
            assert!(run("int? x;\nList<int> f() => [if (x != null) x];").is_empty());
            assert_eq!(
                run("int? x;\nList<int> f() => [if (x != null) x!];").len(),
                1
            );
            // Local and parameter targets stay promotable: the bare
            // then-form reports and the `!` form never matches
            // (`PromotableElementImpl` admits no PostfixExpression arm,
            // :75-81).
            assert_eq!(
                run("List<int> f(int? p) { int? x = p; return [if (x != null) x]; }").len(),
                1
            );
            assert!(
                run("List<int> f(int? p) { int? x = p; return [if (x != null) x!]; }").is_empty()
            );
            assert_eq!(run("List<int> f(int? x) => [if (x != null) x];").len(), 1);
            assert!(run("List<int> f(int? x) => [if (x != null) x!];").is_empty());
        }
    }
}

pub(crate) mod prefer_const_literals_to_create_immutables {
    use super::{Check, NodeKind, RuleSpec, slice};
    use crate::rules::declarations::{annotation_names, top_level_annotations};
    use crate::rules::{SupertypeClause, const_ability, const_keyword, in_const_context, same_file_declaration, supertypes};

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/prefer_const_literals_to_create_immutables.dart`
    /// — a non-const list/set/map literal under an instance creation whose
    /// class carries `@immutable` (the class or any superclass chain link)
    /// that could be written `const` reports at the literal
    /// (`reportAtNode(literal)`, :56-57). The parent walk (:43-53) climbs
    /// the transparent set — parenthesized expressions, argument lists,
    /// outer literals, map entries, named arguments — and answers only at
    /// an instance creation; `literal.isConst` skips (:41) and
    /// `literal.canBeConst` gates (:56).
    ///
    /// Approximation (documented): the analyzer's resolution rewrites an
    /// unresolved invocation into `InstanceCreationExpression`; here the
    /// pre-resolution shapes stand in — a bare-spelling invocation (no
    /// member chain, the head identifier is the type) plus the explicit
    /// `new`/implicit-creation rows. The class resolves same-file (the
    /// class/mixin/enum index plus an extension-type supplement —
    /// `test_extensionType`); a prefixed or unresolvable spelling and a
    /// plain function callee stay silent (the upstream unresolved-type
    /// face, :102-105). `hasImmutable` reads the written annotations by
    /// spelling (`immutable` / `Immutable` — the `package:meta` getter and
    /// class faces) over the class and its extends chain; a chain that
    /// leaves the file under-reports. The analyzer resolves the annotation
    /// only over the declaring-library gate (`_isPackageMetaGetter`/
    /// `_isPackageMetaConstructor`, `element.library.name == 'meta'`), so
    /// the spelling read answers only over a `library meta;` unit — an
    /// unnamed vendored meta never recognizes it and the rule stays
    /// silent (the live oracle's own face over a differential corpus).
    /// `canBeConst` is the syntactic `const_ability` face. Message:
    /// `diagnostic.g.dart` `preferConstLiteralsToCreateImmutables`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_const_literals_to_create_immutables",
    &[NodeKind::ListLiteral, NodeKind::SetOrMapLiteral],
    check,
);

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `literal.isConst` (:41): the own keyword or an enclosing const
        // context.
        if const_keyword(c, node).is_some() || in_const_context(c) {
            return;
        }
        // The creation ancestor and the created type spelling (:43-53).
        let Some((_creation, name)) = creation_ancestor(c, node) else {
            return;
        };
        // `_hasImmutableAnnotation(node.staticType)` (:101-119).
        if !spelling_has_immutable(c, name) {
            return;
        }
        // `literal.canBeConst` (:56).
        if const_ability::is_const_able(c.unit, node) {
            c.report(c.utf16_span(node), &[]);
        }
    }

    /// The parent walk (:43-53): the transparent set climbs to the
    /// creation face — a `new`/implicit creation row, or the argument
    /// list's send group (the analyzer's resolution rewrites a bare
    /// invocation into the instance creation; the bare-spelling group
    /// stands in for it) — with the created type's spelling; anything
    /// else (and the unit root) answers `None`. The send group tiles
    /// flat, so an argument list's owner holds the group pieces and the
    /// send marker seals directly after its `Arguments` row.
    fn creation_ancestor<'a>(c: &'a Check<'_>, literal: u32) -> Option<(u32, &'a str)> {
        let mut current = literal;
        loop {
            let parent = c.unit.ast.parent(current)?;
            match c.kind(parent) {
                NodeKind::ParenthesizedExpression
                | NodeKind::ListLiteral
                | NodeKind::SetOrMapLiteral
                | NodeKind::MapEntry
                | NodeKind::NamedArgument => current = parent,
                NodeKind::NewExpression | NodeKind::ImplicitCreation => {
                    return creation_head_name(c, parent).map(|name| (parent, name));
                }
                NodeKind::Arguments => {
                    let owner = c.unit.ast.parent(parent)?;
                    // An explicit `new`/implicit creation owns the argument
                    // list directly — no send group seals it.
                    if matches!(
                        c.kind(owner),
                        NodeKind::NewExpression | NodeKind::ImplicitCreation
                    ) {
                        return creation_head_name(c, owner).map(|name| (owner, name));
                    }
                    let kids = c.collect_direct(owner);
                    let pos = kids.iter().position(|&kid| kid == parent)?;
                    let marker = *kids
                        .get(pos + 1)
                        .filter(|&kid| c.kind(*kid) == NodeKind::Send)?;
                    // A member-chain callee (`a.B()`) is no creation
                    // spelling.
                    if slice::access_marker(c, &kids, pos + 1, NodeKind::DotAccess).is_some() {
                        return None;
                    }
                    let (head, _arguments) = slice::send_parts(c, &kids, pos + 1)?;
                    let head_row = kids[head];
                    let name = (c.kind(head_row) == NodeKind::Identifier)
                        .then(|| c.lexeme(c.head_token(head_row)))?;
                    return Some((marker, name));
                }
                _ => return None,
            }
        }
    }

    /// The created type spelling of a `new`/implicit-creation row: the
    /// identifier before the argument list (a prefixed or `Foo.new` shape
    /// carries no bare identifier and under-reports). The bare head tiles
    /// as a `ConstructorReference` whose own identifier is head-borrowed —
    /// it never lands in the creation's direct kids, so read the head
    /// identifier off the reference row's subtree.
    fn creation_head_name<'a>(c: &'a Check<'_>, creation: u32) -> Option<&'a str> {
        let kids = c.collect_direct(creation);
        let args = kids
            .iter()
            .position(|&kid| c.kind(kid) == NodeKind::Arguments)?;
        kids[..args].iter().rev().find_map(|&kid| {
            if c.kind(kid) == NodeKind::Identifier {
                return Some(c.lexeme(c.head_token(kid)));
            }
            if c.kind(kid) == NodeKind::ConstructorReference {
                return c.children(kid).find_map(|inner| {
                    (c.kind(inner) == NodeKind::Identifier).then(|| c.lexeme(c.head_token(inner)))
                });
            }
            None
        })
    }

    /// `_hasImmutableAnnotation` (:101-119) over the same-file substrate:
    /// the resolved class-like and its extends chain, each read for a
    /// written `immutable`/`Immutable` annotation; an unresolvable link
    /// ends the chain (the under-report).
    fn spelling_has_immutable(c: &Check<'_>, name: &str) -> bool {
        // The declaring-library gate first: the annotation resolves only
        // out of a library named `meta` (the doc block) — the mounted
        // fold's resolution face, the written-directive scan as the
        // plain-run fallback.
        if !const_ability::spells_meta_library(c.unit, c.semantics()) {
            return false;
        }
        let Some(first) = same_file_class_like(c, name) else {
            return false;
        };
        let mut seen = vec![first];
        let mut current = Some(first);
        while let Some(row) = current {
            if top_level_annotations(c, row).into_iter().any(|meta| {
                annotation_names(c, meta, "immutable") || annotation_names(c, meta, "Immutable")
            }) {
                return true;
            }
            // `current.superclass` (:110-113): the extends chain only.
            let next = supertypes(c, row)
                .into_iter()
                .find(|&(_, clause)| clause == SupertypeClause::Extends)
                .and_then(|(name, _)| same_file_class_like(c, name));
            current = next.filter(|next| !seen.contains(next));
            if let Some(next) = current {
                seen.push(next);
            }
        }
        false
    }

    /// The same-file class-like row of a type spelling: the class/mixin/
    /// enum index plus the extension-type supplement (the representation
    /// creation face, `test_extensionType`).
    fn same_file_class_like(c: &Check<'_>, name: &str) -> Option<u32> {
        same_file_declaration(c, name).or_else(|| extension_type_row(c, name))
    }

    /// The same-file extension type named `name` (the row index skips
    /// extension types): the first wrapper whose declaration row spells
    /// the name two tokens past the `extension` keyword.
    fn extension_type_row(c: &Check<'_>, name: &str) -> Option<u32> {
        let root = c.unit.ast.root()?;
        c.collect_direct(root)
            .into_iter()
            .filter(|&top| c.kind(top) == NodeKind::TopLevelDeclaration)
            .find_map(|top| {
                c.collect_direct(top).into_iter().find(|&kid| {
                    c.kind(kid) == NodeKind::ExtensionTypeDeclaration
                        && c.lexeme(c.head_token(kid) + 2) == name
                })
            })
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

        use super::SPEC;

        /// `test_listLiteral_noConst` — a mutable-position list literal
        /// under an `@immutable` creation reports at the literal. The
        /// annotation resolves only over a `library meta;` unit (the
        /// declaring-library gate), so every firing fixture declares it.
        #[test]
        fn list_literal_under_immutable_creation_reports() {
            let source = concat!(
                "library meta;\n",
                "@immutable\n",
                "class C {\n",
                "  const C(List<Object> p);\n",
                "}\n",
                "var x = C([]);\n",
            );
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule(SPEC.name));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Use 'const' literals as arguments to constructors of '@immutable' classes."
            );
            let open = tok(source, &f.tokens, "[]", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, open));
        }

        /// The corpus's own face: the unnamed vendored meta (no `library
        /// meta;` in the importing unit) never resolves the annotation,
        /// and no row reports — the live oracle emits zero rows for the
        /// rule over that corpus.
        #[test]
        fn unnamed_meta_library_stays_silent() {
            let source = "@immutable\nclass C {\n  const C(List<Object> p);\n}\nvar x = C([]);\n";
            let f = Fixture::parse(source);
            assert!(f.run(&one_rule(SPEC.name)).is_empty());
        }

        /// `test_listLiteral_nested_noConst` — a nested literal reports
        /// once per level; `test_listLiteral_const` and the nested-const
        /// shape stay silent.
        #[test]
        fn nested_and_const_faces() {
            let source = "library meta;\n@immutable\nclass C {\n  const C(List<Object> p);\n}\nvar x = C([\n  [],\n]);\n";
            let f = Fixture::parse(source);
            assert_eq!(f.run(&one_rule(SPEC.name)).len(), 2);

            let source = "library meta;\n@immutable\nclass C {\n  const C(List<Object> p);\n}\nvar x = C(const []);\n";
            let f = Fixture::parse(source);
            assert!(f.run(&one_rule(SPEC.name)).is_empty());

            let source = "library meta;\n@immutable\nclass C {\n  const C(List<Object> p);\n}\nvar x = C(const [\n  const [],\n]);\n";
            let f = Fixture::parse(source);
            assert!(f.run(&one_rule(SPEC.name)).is_empty());
        }

        /// `@Immutable('')` — the class spelling — answers too
        /// (`test_listLiteral_noConst_instantiationAnnotation`), and an
        /// inherited annotation over the extends chain answers.
        #[test]
        fn immutable_spellings_and_inheritance() {
            let source = "library meta;\n@Immutable('')\nclass C {\n  const C(List<Object> p);\n}\nvar x = C([]);\n";
            assert_eq!(Fixture::parse(source).run(&one_rule(SPEC.name)).len(), 1);

            let source = concat!(
                "library meta;\n",
                "@immutable\n",
                "class B {\n",
                "  const B();\n",
                "}\n",
                "class C extends B {}\n",
                "var maker = C;\n",
                "var x = C.new([]);\n",
            );
            // The `.new` member-chain callee is no bare spelling — silent.
            let f = Fixture::parse(source);
            assert!(f.run(&one_rule(SPEC.name)).is_empty());

            let source = concat!(
                "library meta;\n",
                "@immutable\n",
                "class B {\n",
                "  const B(List<Object> p);\n",
                "}\n",
                "class C extends B {}\n",
                "var x = C([]);\n",
            );
            let f = Fixture::parse(source);
            assert_eq!(f.run(&one_rule(SPEC.name)).len(), 1);
        }

        /// `test_extensionType` — the representation creation over an
        /// `@immutable` extension type reports.
        #[test]
        fn extension_type_creation_reports() {
            let source =
                "library meta;\n@immutable\nextension type E(List<int> i) {}\nvar e = E([1]);\n";
            let f = Fixture::parse(source);
            assert_eq!(
                f.run(&one_rule(SPEC.name)).len(),
                1,
                "{:?}",
                f.run(&one_rule(SPEC.name))
            );
        }

        /// A non-immutable class, a plain function callee, an unresolvable
        /// spelling, and a literal that cannot be const all stay silent
        /// (`test_listLiteral_notConstable_noConst`).
        #[test]
        fn negative_faces() {
            let sources = [
                // No annotation.
                "class C {\n  const C(List<Object> p);\n}\nvar x = C([]);\n",
                // A function callee is no creation.
                "void g(List<Object> p) {}\nvar x = g([]);\n",
                // An undeclared spelling (the unresolved-type face).
                "var x = Unknown([]);\n",
                // A member-chain callee.
                "class C {\n  const C(List<Object> p);\n}\nvar c = C.named;\nvar x = c([]);\n",
                // `canBeConst` gate: a non-const-able element.
                "@immutable\nclass C {\n  const C(List<Object> p);\n}\nint e() => 7;\nvar x = C([e()]);\n",
            ];
            for source in sources {
                let f = Fixture::parse(source);
                let lints = f.run(&one_rule(SPEC.name));
                assert!(lints.is_empty(), "{source:?} -> {lints:?}");
            }
        }

        /// The explicit `new` face and the transparent named-argument and
        /// map-entry climbs.
        #[test]
        fn new_expression_and_transparent_parents() {
            let source = "library meta;\n@immutable\nclass C {\n  const C(List<Object> p);\n}\nvar x = new C([]);\n";
            assert_eq!(Fixture::parse(source).run(&one_rule(SPEC.name)).len(), 1);

            let source = "library meta;\n@immutable\nclass C {\n  const C({List<Object>? p});\n}\nvar x = C(p: []);\n";
            assert_eq!(Fixture::parse(source).run(&one_rule(SPEC.name)).len(), 1);

            let source = "library meta;\n@immutable\nclass C {\n  const C(Map<Object, Object> p);\n}\nvar x = C({'k': [1]});\n";
            assert_eq!(Fixture::parse(source).run(&one_rule(SPEC.name)).len(), 2);
        }
    }
}
