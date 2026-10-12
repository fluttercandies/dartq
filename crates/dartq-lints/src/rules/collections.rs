//! The collection-literal family: `prefer_inlined_adds`,
//! `prefer_spread_collections`, `prefer_collection_literals`,
//! `unnecessary_to_list_in_spreads`.
//!
//! Cascade shape (production tiling, `rules::slice`): a [`NodeKind::Cascade`]
//! opens at the `..` before its first section parses, so the section's
//! invocation pieces tile flat inside the cascade window; the
//! [`NodeKind::CascadeAccess`] marker seals after the first section's send.
//! A cascaded section send's owner is therefore the cascade itself, and the
//! first section is the piece run before the marker.

use dartq_ast::NodeKind;

use crate::engine::Check;
use crate::registry::{Group, RuleSpec};
use crate::rules::slice;

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
    use super::{Check, Group, NodeKind, RuleSpec, cascaded_add, first_argument_list_literal};

    /// Official: `pkg/linter/lib/src/rules/prefer_inlined_adds.dart:22-58` —
    /// the first cascaded `add`/`addAll` on a list literal with one argument
    /// reports at the method name; `addAll` carries the multiple-elements
    /// template (only when its argument is a list literal — a spread argument
    /// belongs to `prefer_spread_collections`, :53-55). Message templates:
    /// `diagnostic.g.dart` `preferInlinedAddsMultiple`/`preferInlinedAddsSingle`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_inlined_adds",
        group: Group::Recommended,
        problem: "The addition of multiple list items could be inlined.",
        correction: "Try adding the items to the list literal directly.",
        kinds: &[NodeKind::Send],
        check,
    };

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
    use super::{
        Check, Group, NodeKind, RuleSpec, cascaded_add, first_argument_list_literal,
        in_const_context,
    };

    /// Official: `pkg/linter/lib/src/rules/prefer_spread_collections.dart:22-55`
    /// — the first cascaded `addAll` on a non-constant list literal whose
    /// single argument is NOT a list literal (that form is
    /// `prefer_inlined_adds`'s) reports at the method name. Message:
    /// `diagnostic.g.dart` `preferSpreadCollections`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_spread_collections",
        group: Group::Recommended,
        problem: "The addition of multiple elements could be inlined.",
        correction: "Try using the spread operator ('...') to inline the addition.",
        kinds: &[NodeKind::Send],
        check,
    };

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
    use super::{Check, Group, NodeKind, RuleSpec, slice};
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
    /// user shadows) bails through the same-file index. The `Linked*`
    /// context exemption reads the declared type around the creation
    /// (declaration type-before, method return type); an assignment or
    /// argument position bails — the context type is not derivable there
    /// (the conservative family). Prefixed callees (`core.Map()`) never
    /// match. Message: `diagnostic.g.dart` `preferCollectionLiterals`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_collection_literals",
        group: Group::Recommended,
        problem: PROBLEM,
        correction: CORRECTION,
        kinds: &[
            NodeKind::Send,
            NodeKind::NewExpression,
            NodeKind::ImplicitCreation,
        ],
        check,
    };

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
        let Some(name) = kids[..args_pos]
            .iter()
            .rev()
            .find_map(|&kid| match c.kind(kid) {
                NodeKind::Identifier => Some(c.lexeme(c.head_token(kid))),
                NodeKind::ConstructorReference => c
                    .collect_direct(kid)
                    .into_iter()
                    .find(|&k| c.kind(k) == NodeKind::Identifier)
                    .map(|k| c.lexeme(c.head_token(k))),
                _ => None,
            })
        else {
            return false;
        };
        unnamed_face(c, name, args)
    }

    /// The unnamed faces (`constructorName == null && arguments.isEmpty`,
    /// :57-59/:68-71): written empty parentheses only — a `NoArguments`
    /// send is a tear-off, no creation. The `Linked*` faces add the
    /// context-type exemption (:85-88/:101-104).
    fn unnamed_face(c: &Check<'_>, name: &str, args: u32) -> bool {
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
        if same_file::declares_name(c, name) {
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
            (None, Some(args)) => unnamed_face(c, selector, args),
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
        if !NAMED_SET.contains(&written) || same_file::declares_name(c, written) {
            return false;
        }
        single_list_literal(c, args)
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
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/unnecessary_to_list_in_spreads.dart:37-47`
    /// — a spread of `x.toList()` reports at the method name.
    ///
    /// Approximation (documented): the official requires the receiver to
    /// implement `Iterable` (`implementsInterface('Iterable', 'dart.core')`,
    /// :45) — a resolution fact; a spread expression's type is checked by
    /// the compiler, so the lexical name check matches on all valid code and
    /// a user `toList` on a non-Iterable (already a compile error in a
    /// spread) over-reports. Message: `diagnostic.g.dart`
    /// `unnecessaryToListInSpreads`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "unnecessary_to_list_in_spreads",
        group: Group::Recommended,
        problem: "Unnecessary use of 'toList' in a spread.",
        correction: "Try removing the invocation of 'toList'.",
        kinds: &[NodeKind::Spread],
        check,
    };

    fn check(c: &mut Check<'_>) {
        // `node.expression` is the spread's operand child (collapse map
        // §3.5 SpreadExpressionHandle).
        let Some(expression) = c
            .collect_direct(c.node())
            .into_iter()
            .next()
            .filter(|&kid| c.kind(kid) == NodeKind::Send)
        else {
            return;
        };
        let kids = c.collect_direct(expression);
        let Some(args_pos) = kids
            .iter()
            .position(|&kid| matches!(c.kind(kid), NodeKind::Arguments))
        else {
            return;
        };
        let Some(&callee) = args_pos.checked_sub(1).and_then(|i| kids.get(i)) else {
            return;
        };
        // `expression.methodName.name == 'toList'` with a target
        // (:43-45): the callee is `x.toList` — a `DotAccess` whose last
        // child names the method.
        if c.kind(callee) != NodeKind::DotAccess {
            return;
        }
        let access = c.collect_direct(callee);
        let Some(name) = access.last().copied() else {
            return;
        };
        if c.kind(name) != NodeKind::Identifier
            || c.lexeme(c.head_token(name)) != "toList"
            || access.len() < 2
        {
            return;
        }
        let range = c.utf16_span(name);
        c.report(range, &[]);
    }
}

pub(crate) mod prefer_iterable_where_type {
    use super::{Check, Group, NodeKind, RuleSpec};
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
    /// else stays silent like the official's unresolvable-target bail. The
    /// closure shape is the official's own lexical test (:50-71): one
    /// parameter, an expression body (or single return) whose
    /// unparenthesized expression is `e is T` (no `is!`) with the
    /// parameter as the tested expression. Message: `diagnostic.g.dart`
    /// `preferIterableWheretype`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_iterable_whereType",
        group: Group::Core,
        problem: "Use 'whereType' to filter elements by type.",
        correction: "Try using 'whereType' rather than 'where'.",
        kinds: &[NodeKind::Send],
        check,
    };

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
            // statement leaf completes last.
            match rows.last() {
                Some(&last) if c.kind(last) == NodeKind::ReturnStatement => {}
                _ => return false,
            }
            rows.pop();
        }
        // `(e) => e is int` / `(e) { return e is int; }`: the `IsExpression`
        // closes last in the scan window (a `(...)` wrapper around it just
        // follows).
        let Some(&expression) = rows
            .iter()
            .rev()
            .find(|&&row| c.kind(row) == NodeKind::IsExpression)
        else {
            return false;
        };
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

    /// A simple formal parameter's name (`FormalParameter` payload word 1,
    /// collapse map §3.3).
    fn parameter_name<'a>(c: &'a Check<'a>, parameter: u32) -> Option<&'a str> {
        if c.kind(parameter) != NodeKind::FormalParameter {
            return None;
        }
        let name = c.payload(parameter).get(1).copied()?;
        (name != NONE_TOKEN).then(|| c.lexeme(name))
    }
}

pub(crate) mod prefer_for_elements_to_map_from_iterable {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::{first_child, slice};

    /// Official: `pkg/linter/lib/src/rules/prefer_for_elements_to_map_fromIterable.dart:31-90`
    /// — a `Map.fromIterable(a, key: …, value: …)` creation whose key and
    /// value arguments are single-parameter single-expression closures
    /// reports at the creation.
    ///
    /// Constructor element pin (`element.enclosingElement !=
    /// typeProvider.mapElement`, :43-45) over the same-file channel: a
    /// same-file declaration owning the `Map` name (class, typedef, value)
    /// resolves the creation to user code and stays silent. Closure shape
    /// (:71-88): a named argument whose expression (no parens) is a
    /// function expression with `parameters.length == 1 &&
    /// parameters.first.isRequired` — a lone direct positional row is
    /// required, an optional/named group only when its member carries the
    /// `required` keyword — and a single expression body (or one return).
    /// Message: `diagnostic.g.dart` `preferForElementsToMapFromiterable`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_for_elements_to_map_fromIterable",
        group: Group::Recommended,
        problem: "Use 'for' elements when building maps from iterables.",
        correction: "Try using a collection literal with a 'for' element.",
        kinds: &[NodeKind::Send],
        check,
    };

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
                // selector identifier over the bare `Map` receiver.
                let matched = c.lexeme(c.head_token(owner[head])) == "fromIterable"
                    && matches!(
                        slice::access_run(c, &owner[..head]).as_slice(),
                        [only]
                            if c.kind(*only) == NodeKind::Identifier
                                && c.lexeme(c.head_token(*only)) == "Map"
                    );
                (owner, pos - 1, matched)
            };
        if !matched {
            return;
        }
        // The constructor element pin: a same-file `Map` declaration is
        // not typeProvider's map element.
        if crate::rules::expressions::same_file::declares_name(c, "Map") {
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
    use super::{Check, Group, NodeKind, RuleSpec};
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
    /// Receiver resolution over the same-file channel: a `this.<field>`
    /// receiver reads the same-class field's written type
    /// ([`type_face_of_field`] — `this` is unshadowable); a bare-name
    /// receiver resolves the innermost binding
    /// ([`written_type_face`] — a local/parameter shadows the field, the
    /// official `target.staticType` face). The written type must carry the
    /// row's name unshadowed ([`crate::rules::expressions::same_file::
    /// declares_name`] — a same-file `class List` is not the core type, the
    /// official `collectionTypeFor` miss), and the written type arguments
    /// stand in for the resolved ones (an omitted argument list — raw
    /// `List` — carries no verdict). The argument type is the
    /// scalar-literal face (`int`, `double`, `String`, `bool`, `null`);
    /// unknown arguments stay silent. The relation (`typesAreUnrelated`)
    /// ports as: equal names, `num` against `int`/`double`,
    /// `Object`/`dynamic` against everything, nullable-`?` against `Null`,
    /// and `Comparable`/`Pattern` (the core supertypes the scalars
    /// implement) are comparable — everything else reports.
    ///
    /// The index face (:180-200) dispatches on the `Indexed` handle: the
    /// receiver reads through the same resolution, the key argument is the
    /// single piece inside the `[...]` extent, and both the read (`m[k]`)
    /// and the write (`m[k] = v` — the write's `Assignment` container
    /// adopts the `Indexed` row as its first child, so the piece run reads
    /// at the assignment's owner) check the key against `K`.
    ///
    /// Known silences (each the documented fallback of the same-file
    /// channel): a receiver whose written type leaves the file (an
    /// untyped/inferred binding, a member chain, an import) — the
    /// official's unresolvable-or-imported faces; a prefixed collection
    /// spelling (`collection.Queue`) — the `Type` row's head is the prefix,
    /// no slot matches; a same-file supertype of a collection
    /// (`class MyList extends List<int>`) — the supertype's own type
    /// arguments are a substitution the written type does not carry.
    /// Message: `diagnostic.g.dart` `collectionMethodsUnrelatedType`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "collection_methods_unrelated_type",
        group: Group::Core,
        problem: "The argument type '{0}' isn't related to '{1}'.",
        correction: "Try changing the argument or element type to match.",
        kinds: &[NodeKind::Send, NodeKind::Indexed],
        check,
    };

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
        let Some(face) = receiver_face(c, &trimmed) else {
            return;
        };
        // The single scalar-literal argument (:203-205): the argument is the
        // `Arguments` container's lone child row.
        let args = c.collect_direct(arguments);
        let [arg] = args.as_slice() else {
            return;
        };
        if let Some((arg_type, expected)) = verdict(c, name, &face, *arg) {
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
        // trailing `[ThisExpression, name]` / `[name]` face.
        let trimmed = slice::access_run(c, &kids[..key_pos]);
        let tail = trimmed.len().saturating_sub(2);
        let Some(face) = receiver_face(c, &trimmed[tail..]) else {
            return;
        };
        if let Some((arg_type, expected)) = verdict(c, "[]", &face, key.expect("counted above")) {
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

    /// The verdict body (:243-265): the method's expected slot keyed on
    /// the written type name, the single scalar-literal argument, the
    /// unrelated-type relation — `(argument type, expected type)` when the
    /// report fires, `None` otherwise.
    fn verdict(
        c: &Check<'_>,
        method: &str,
        face: &TypeFace<'_>,
        argument: u32,
    ) -> Option<(String, String)> {
        let written = face.name?;
        // A same-file declaration owns the written name — the
        // `collectionTypeFor` lookup misses the core type.
        if crate::rules::expressions::same_file::declares_name(c, written) {
            return None;
        }
        let (index, ()) = expected_slot(method, written)?;
        if face.type_arguments.len() <= index {
            return None; // raw or argument-less written type: no verdict
        }
        let arg_type = literal_type_name(c, argument)?;
        let (element, element_nullable) = face.type_arguments[index];
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
    /// `String` joins `contains` through its `Iterable<String>` face;
    /// `Queue.remove` declares on dart:collection's `Queue` (:158-164).
    fn expected_slot(method: &str, written: &str) -> Option<(usize, ())> {
        match (method, written) {
            ("containsValue", "Map") => Some((1, ())),
            ("contains", "List" | "Set" | "Iterable" | "String" | "Queue")
            | ("remove", "List" | "Set" | "Queue" | "Map")
            | ("lookup", "Set")
            | ("containsKey" | "[]", "Map") => Some((0, ())),
            _ => None,
        }
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
#[cfg(test)]
mod tests {
    use dartq_ast::NodeKind;

    use crate::testsupport::{Fixture, NONE, identifier, one_rule, tok, unit_of, utf16_at};

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

    /// `test/rules/unnecessary_to_list_in_spreads_test.dart` — `...x.toList()`
    /// reports at the method name.
    #[test]
    fn to_list_spread_reports() {
        let source = "[...list.toList()];";
        // [0 ...1 list2 .3 toList4 (5 )6 ]7 ;8
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::ListLiteral, 0);
                b.patch(0, NONE);
                b.begin(NodeKind::Spread, 1);
                b.begin(NodeKind::Send, 2);
                b.begin(NodeKind::DotAccess, 2);
                b.patch(0, 3);
                b.patch(1, 0);
                identifier(b, 2, 3);
                identifier(b, 4, 5);
                b.end(NodeKind::DotAccess, 7);
                b.begin(NodeKind::Arguments, 5);
                b.end(NodeKind::Arguments, 8);
                b.end(NodeKind::Send, 8);
                b.end(NodeKind::Spread, 8);
                b.end(NodeKind::ListLiteral, 9);
                b.end(NodeKind::ExpressionStatement, 9);
            }),
        );
        let lints = f.run(&one_rule("unnecessary_to_list_in_spreads"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Unnecessary use of 'toList' in a spread.");
        assert_eq!(lints[0].range.start, utf16_at(&f, 4));
    }

    /// A plain spread element and a bare `toList()` call pass.
    #[test]
    fn plain_spread_and_bare_to_list_pass() {
        let plain = "[...list];";
        // [0 ...1 list2 ]3 ;4
        let f = Fixture::build(
            plain,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::ListLiteral, 0);
                b.patch(0, NONE);
                b.begin(NodeKind::Spread, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::Spread, 4);
                b.end(NodeKind::ListLiteral, 5);
                b.end(NodeKind::ExpressionStatement, 5);
            }),
        );
        assert!(
            f.run(&one_rule("unnecessary_to_list_in_spreads"))
                .is_empty()
        );

        let bare = "[...toList()];";
        // [0 ...1 toList2 (3 )4 ]5 ;6
        let f = Fixture::build(
            bare,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::ListLiteral, 0);
                b.patch(0, NONE);
                b.begin(NodeKind::Spread, 1);
                b.begin(NodeKind::Send, 2);
                identifier(b, 2, 3);
                b.begin(NodeKind::Arguments, 3);
                b.end(NodeKind::Arguments, 6);
                b.end(NodeKind::Send, 6);
                b.end(NodeKind::Spread, 6);
                b.end(NodeKind::ListLiteral, 7);
                b.end(NodeKind::ExpressionStatement, 7);
            }),
        );
        assert!(
            f.run(&one_rule("unnecessary_to_list_in_spreads"))
                .is_empty()
        );
    }

    // —— prefer_iterable_whereType —————————————————————————————————————————

    /// `test/rules/prefer_iterable_whereType_test.dart` `test_whereType` —
    /// `x.where((e) => e is int)` reports at the method name.
    /// Production-bridge fixture.
    #[test]
    fn where_type_reports_a_type_test_closure() {
        let f = Fixture::parse("void f(List<int> xs) { xs.where((e) => e is int); }");
        let lints = f.run(&one_rule("prefer_iterable_whereType"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use 'whereType' to filter elements by type."
        );
        let name = tok(&f.source, &f.tokens, "where", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, name));
        assert_eq!(lints[0].range.end, utf16_at(&f, name) + 5);
    }

    /// An `is!` closure and a foreign-parameter test pass (:71-79).
    #[test]
    fn where_type_not_is_and_foreign_param_pass() {
        let f = Fixture::parse("void f(List<int> xs) { xs.where((e) => e is! int); }");
        assert!(f.run(&one_rule("prefer_iterable_whereType")).is_empty());

        let f = Fixture::parse("void f(List<int> xs) { xs.where((e) => f(e) is int); }");
        assert!(f.run(&one_rule("prefer_iterable_whereType")).is_empty());
    }

    /// Two arguments and a bare untargeted `where` pass (:47-51), and the
    /// restored receiver face keeps a map literal silent like the official
    /// `Iterable`-only gate (:48-49).
    #[test]
    fn where_type_argument_and_receiver_faces_pass() {
        let f = Fixture::parse("void f(List<int> xs) { xs.where((e) => e is int, 1); }");
        assert!(f.run(&one_rule("prefer_iterable_whereType")).is_empty());

        let f = Fixture::parse("void f() { var x = ({'k': 1}).where((e) => e is int); }");
        assert!(
            f.run(&one_rule("prefer_iterable_whereType")).is_empty(),
            "a map literal is outside Iterable"
        );
    }

    // —— prefer_for_elements_to_map_fromIterable ———————————————————————————

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
        let lints = f.run(&one_rule("prefer_for_elements_to_map_fromIterable"));
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
            f.run(&one_rule("prefer_for_elements_to_map_fromIterable"))
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
            r#"
class C {
  Map<int, String> map = <int, String>{};

  void m() {
    this.map[1];
    this.map['k'];
    this.map['k'] = 'v';
  }
}
"#,
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
            r#"
class C {
  Queue<String> queue = Queue<String>();

  void m() {
    this.queue.remove(1);
    this.queue.remove('x');
  }
}
"#,
        );
        let lints = f.run(&one_rule("collection_methods_unrelated_type"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The argument type 'int' isn't related to 'String'."
        );
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
            r#"
class C {
  List<String> list = <String>[];

  void m() {
    List<int> list = <int>[];
    list.remove('x');
  }
}
"#,
        );
        let lints = f.run(&one_rule("collection_methods_unrelated_type"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The argument type 'String' isn't related to 'int'."
        );

        // A same-file `class List` owns the written name — silent.
        let f = Fixture::parse(
            r#"
class List {
  void remove(Object? x) {}
}

class D {
  List<int> list = <int>[];

  void m() {
    list.remove('x');
  }
}
"#,
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
        assert!(f.run(&one_rule("prefer_iterable_whereType")).is_empty());

        let f = Fixture::parse(
            r#"
class User {
  void where(bool Function(dynamic) test) {}
}

void f(User u) {
  u.where((e) => e is int);
}
"#,
        );
        assert!(f.run(&one_rule("prefer_iterable_whereType")).is_empty());

        let f = Fixture::parse("void f(List<int> xs) { xs.where((e) => e is int); }");
        assert_eq!(f.run(&one_rule("prefer_iterable_whereType")).len(), 1);
    }

    /// The closure requiredness gate (:77-80): an optional positional or
    /// a bare named parameter bails; `{required e}` reports; a same-file
    /// `Map` declaration stays silent (the constructor element pin).
    #[test]
    fn map_from_iterable_requiredness_and_shadow_faces() {
        let f =
            Fixture::parse("void f(x) { Map.fromIterable(x, key: ([e]) => e, value: (e) => e); }");
        assert!(
            f.run(&one_rule("prefer_for_elements_to_map_fromIterable"))
                .is_empty()
        );

        let f =
            Fixture::parse("void f(x) { Map.fromIterable(x, key: ({e}) => e, value: (e) => e); }");
        assert!(
            f.run(&one_rule("prefer_for_elements_to_map_fromIterable"))
                .is_empty()
        );

        let f = Fixture::parse(
            "void f(x) { Map.fromIterable(x, key: ({required e}) => e, value: (e) => e); }",
        );
        assert_eq!(
            f.run(&one_rule("prefer_for_elements_to_map_fromIterable"))
                .len(),
            1
        );

        let f = Fixture::parse(
            r#"
class Map {}

void f(x) {
  Map.fromIterable(x, key: (e) => e, value: (e) => e);
}
"#,
        );
        assert!(
            f.run(&one_rule("prefer_for_elements_to_map_fromIterable"))
                .is_empty()
        );
    }
}
