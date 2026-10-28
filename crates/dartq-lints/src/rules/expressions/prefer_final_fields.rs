use super::{Check, NodeKind, RuleSpec, same_file, slice};
use crate::rules::{MemberQuery, NONE_TOKEN, body_members, member_matches, same_file_ancestors};

/// Official: `pkg/linter/lib/src/rules/prefer_final_fields.dart` — a
/// private non-`final` field never mutated reports when it is
/// initialized at its declaration or set by every constructor of its
/// class (field formals and field initializers count as setting;
/// :52-84). The mutated set is library-wide and collected before the
/// decision (`afterLibrary`, :85-112); the report sits on the variable
/// declaration. Message: `diagnostic.g.dart` `preferFinalFields`.
///
/// Approximations (documented — the official runs on resolved elements
/// across the whole library; this is one unit, name-level):
/// - Mutations are bare-name writes: assignment targets (any operator),
///   `++`/`--` operands, and cascade-section write targets (`.._x = 1`
///   — a section assignment is an `AssignmentExpression` upstream and
///   its `writeElement` is the field). A write captured by a nearer
///   same-file binding — a local, a parameter, a catch/for/case binder
///   (the shadow-check face shared with the declarations.rs write
///   scans) — does not count. Remaining element splits fold into the
///   bare name: a `this._x` target, a same-named field of another
///   class, and a cascade-section postfix (`o.._x++` — the postfix row
///   wraps the whole cascade and reads no bare head) under-report.
///   Index writes (`_x[0] = 1`) do not count — matching the official,
///   whose `writeElement` for them is the `[]=` method, not the field.
///   Pattern assignments (`(_x,) = ...`, `var (...) = ...`) do not
///   count either — the official registers no pattern-assignment
///   visitor (:41-48), so a field mutated only there reports, exactly
///   as upstream.
/// - Private-ness is the leading underscore. The setter-override
///   exemption (`_overridesField`, :147-158) reads the written
///   extends/with/implements chain — the same-file ancestor walk, then
///   the semantic bridge's inheritance arm under the `name=` setter
///   spelling; unresolved chains over-report. A same-class `set _x` is
///   no arm: the official `getOverridden` is supertype-only, and a
///   non-final field beside a same-name setter is the front-end's
///   setter-conflicts-with-declaration error, so no valid shape needs
///   it.
/// - The cross-unit half of the official library scan
///   (`_fieldsFromParameters` — primary-constructor declaring formals,
///   :114-131) is out of the substrate and under-reports.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_final_fields",
    &[NodeKind::CompilationUnit],
    check,
);

/// One candidate: the declaration node, its name, whether the
/// declaration carries an initializer, and the enclosing class row (the
/// constructor set faces scan its completion window).
struct Candidate<'a> {
    declaration: u32,
    name: &'a str,
    has_initializer: bool,
    class_row: u32,
}

fn check(c: &mut Check<'_>) {
    let Some(root) = c.unit.ast.root() else {
        return;
    };
    let mut candidates: Vec<Candidate<'_>> = Vec::new();
    let mut mutated: Vec<&str> = Vec::new();
    let mut path: Vec<u32> = Vec::new();
    scan(c, root, None, &mut candidates, &mut mutated, &mut path);
    // A field row reachable from both the class row and a body
    // container yields the candidate twice; keep one.
    candidates.sort_by_key(|candidate| candidate.declaration);
    candidates.dedup_by_key(|candidate| candidate.declaration);
    for candidate in &candidates {
        if mutated.contains(&candidate.name) {
            continue;
        }
        // `_overridesField` (:147-158): a field whose written
        // supertype chain carries a setter of the same name never
        // candidates.
        if overrides_setter(c, candidate.class_row, candidate.name) {
            continue;
        }
        // The constructors of the field's own class (:62-71). The class
        // subtree occupies the contiguous row range ending at the class
        // row, so the constructor set reads from that range under every
        // tiling layout. Factories are `ConstructorDeclaration`s in the
        // official and count for the "set in every constructor"
        // question; they never set.
        let members = c.children(candidate.class_row);
        let constructors: Vec<u32> = (members.start..=candidate.class_row)
            .filter(|&id| matches!(c.kind(id), NodeKind::Constructor | NodeKind::Factory))
            .collect();
        let set = |ctor: u32| sets_field(c, ctor, candidate.name);
        let set_any = constructors.iter().copied().any(set);
        let set_all = constructors.iter().copied().all(set);
        if !(candidate.has_initializer || set_any && set_all) {
            continue;
        }
        let range = c.utf16_span(candidate.declaration);
        c.report(range, &[candidate.name]);
    }
}

/// The one-pass unit walk: candidates from `Fields` declarations and
/// mutations from bare-name writes. `path` carries the enclosing nodes
/// of the visited row (the shadow gate reads it). Enum and
/// extension-type bodies are skipped whole (the official excludes
/// their fields, :98/:118-121); top-level declarations are not fields
/// and never match.
fn scan<'a>(
    c: &Check<'a>,
    node: u32,
    class_row: Option<u32>,
    candidates: &mut Vec<Candidate<'a>>,
    mutated: &mut Vec<&'a str>,
    path: &mut Vec<u32>,
) {
    match c.kind(node) {
        NodeKind::EnumDeclaration | NodeKind::ExtensionTypeDeclaration => return,
        NodeKind::ClassDeclaration => {
            // Member discovery anchors on the class row: its completion
            // window opens at the head keyword and covers every member
            // row, while the body container's window can open later and
            // the derived parent pointer can link a field row to either
            // neighbor. Walking the class row's window is correct under
            // both layouts.
            for kid in c.collect_direct(node) {
                path.push(kid);
                scan(c, kid, Some(node), candidates, mutated, path);
                path.pop();
            }
            return;
        }
        NodeKind::DeclarationBody => {
            // A body container between the class row and its members
            // owns rows in either its completion window or its
            // derived-child set depending on layout; both faces are
            // cheap and the candidate set dedupes by declaration.
            let mut kids = c.collect_direct(node);
            kids.extend(crate::rules::rows_with_parent(c, node));
            for kid in kids {
                path.push(kid);
                scan(c, kid, class_row, candidates, mutated, path);
                path.pop();
            }
            return;
        }
        NodeKind::Fields => {
            for declaration in c.collect_direct(node) {
                if c.kind(declaration) != NodeKind::VariablesDeclaration {
                    continue;
                }
                if let Some(kw) = crate::rules::variables_declaration_keyword(c, declaration)
                    && matches!(c.lexeme(kw), "final" | "const")
                {
                    continue;
                }
                let Some(class_row) = class_row else { continue };
                for var in c.collect_direct(declaration) {
                    if c.kind(var) != NodeKind::InitializedIdentifier {
                        continue;
                    }
                    // The production row is width-0 at the name, so the
                    // name is the span head; hand-built tables keep the
                    // same head (see
                    // [`crate::rules::initialized_identifier_name`]).
                    let Some(name_token) = crate::rules::initialized_identifier_name(c, var)
                    else {
                        continue;
                    };
                    let name = c.lexeme(name_token);
                    if !name.starts_with('_') {
                        continue;
                    }
                    // The row ends past its head exactly when an
                    // initializer followed (`end_last` adopts the
                    // initializer's value end; a bare name ends at the
                    // name itself).
                    let has_initializer = c.head_token(var) != crate::rules::tail_token(c, var);
                    candidates.push(Candidate {
                        declaration: var,
                        name,
                        has_initializer,
                        class_row,
                    });
                }
            }
        }
        NodeKind::Assignment => {
            // A cascade-section write (`.._x = 1`): the section's send
            // group and `CascadeAccess` marker tile inside the
            // enclosing `Cascade` window as flat siblings before the
            // assignment row (hand-built trees may instead adopt the
            // marker as the assignment's first child). The write
            // target is the section's bare send head — it selects the
            // cascade target's member, not a scope-resolved name, so
            // the shadow gate does not apply (the official
            // `writeElement` is the field under any local namesake).
            let marker = c
                .unit
                .ast
                .parent(node)
                .filter(|&parent| c.kind(parent) == NodeKind::Cascade)
                .and_then(|parent| {
                    let siblings = c.collect_direct(parent);
                    let pos = siblings.iter().position(|&sibling| sibling == node)?;
                    siblings[..pos]
                        .iter()
                        .copied()
                        .find(|&row| c.kind(row) == NodeKind::CascadeAccess)
                })
                .or_else(|| {
                    c.collect_direct(node)
                        .into_iter()
                        .find(|&row| c.kind(row) == NodeKind::CascadeAccess)
                });
            if let Some(marker) = marker {
                if let Some(head) = cascade_section_head(c, marker) {
                    mutated.push(head);
                }
            } else {
                // A bare-name write (`_x = 2`): the left operand run is
                // the owner pieces left of the operator — a
                // single-identifier chain. The direct-child face
                // covers the hand-built compact trees.
                let lhs = match c.collect_direct(node).first() {
                    Some(&row) if c.kind(row) == NodeKind::Identifier => Some(row),
                    _ => slice::binary_operands_at(c, node).and_then(|(left, _)| {
                        match slice::access_run(c, &left)[..] {
                            [only] if c.kind(only) == NodeKind::Identifier => Some(only),
                            _ => None,
                        }
                    }),
                };
                if let Some(lhs) = lhs {
                    push_write(c, path, mutated, lhs);
                }
            }
        }
        NodeKind::UnaryPrefix | NodeKind::UnaryPostfix => {
            let op = c.lexeme(c.head_token(node));
            if op == "++" || op == "--" {
                let Some(parent) = c.unit.ast.parent(node) else {
                    return;
                };
                let siblings = c.collect_direct(parent);
                let Some(pos) = siblings.iter().position(|&sibling| sibling == node) else {
                    return;
                };
                // The prefix operator parses before its operand, the
                // postfix operator after it.
                let operand = match c.kind(node) {
                    NodeKind::UnaryPrefix => siblings.get(pos + 1),
                    _ => pos.checked_sub(1).and_then(|i| siblings.get(i)),
                };
                if let Some(&operand) = operand {
                    match c.kind(operand) {
                        NodeKind::Identifier => push_write(c, path, mutated, operand),
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }
    for kid in c.collect_direct(node) {
        path.push(kid);
        scan(c, kid, class_row, candidates, mutated, path);
        path.pop();
    }
}

/// Records the bare-name write bound at `row` unless a nearer
/// same-file binding captures the name there (the official scan is
/// element-based: `writeElement` is the field only when no local
/// shadows it).
fn push_write<'a>(c: &Check<'a>, path: &[u32], mutated: &mut Vec<&'a str>, row: u32) {
    let name = c.lexeme(c.head_token(row));
    if !shadowed(c, path, name, c.byte_span(row).start) {
        mutated.push(name);
    }
}

/// The bare send-head row of a cascade section's write: the section's
/// send group seals right before the `CascadeAccess` marker in the
/// enclosing `Cascade`'s flat children, inside the marker's span. The
/// head is a childless `Send` leaf — an invocation section carries its
/// `Arguments` child, and a selector chain completes its `DotAccess`
/// after the marker, so neither answers the head shape.
fn cascade_section_head<'a>(c: &Check<'a>, marker: u32) -> Option<&'a str> {
    let parent = c.unit.ast.parent(marker)?;
    let siblings = c.collect_direct(parent);
    let pos = siblings.iter().position(|&sibling| sibling == marker)?;
    let head = *siblings.get(pos.checked_sub(1)?)?;
    let (head_span, marker_span) = (c.byte_span(head), c.byte_span(marker));
    (c.kind(head) == NodeKind::Send
        && c.collect_direct(head).is_empty()
        && head_span.start >= marker_span.start
        && head_span.end <= marker_span.end)
        .then(|| c.lexeme(c.head_token(head)))
}

/// `_overridesField` (:147-158): `getOverridden` answers the
/// interface's overridden map — every written supertype clause,
/// abstract members included, so the walks run with the concrete cut
/// off. The same-file arm matches instance setters over the ancestor
/// chain; the cross-file arm anchors the semantic bridge's
/// inheritance walk on the `name=` setter spelling (setters are
/// model-keyed under it). Unresolved chains answer false.
fn overrides_setter(c: &Check<'_>, class_row: u32, name: &str) -> bool {
    let chain = same_file_ancestors(c, class_row, false);
    if chain.iter().any(|&ancestor| {
        body_members(c, ancestor).into_iter().any(|member| {
            c.kind(member) == NodeKind::Method
                && dartq_ast::mod_flags(c.unit.ast, member) & same_file::STATIC_BIT == 0
                && !member_matches(c, member, MemberQuery::Setter, name).is_empty()
        })
    }) {
        return true;
    }
    let spelled = format!("{name}=");
    crate::rules::declarations::bridge_inherited_member(c, class_row, false, &spelled).is_some()
}

/// Whether a nearer same-file binding captures `name` at byte
/// position `at`: any enclosing scope on `path` that binds it —
/// innermost first. The mirror of the shadow-check face in
/// `crate::rules::declarations` (shared there by the
/// `recursive_getters` and `unnecessary_getters_setters` walks): the
/// official checks are element identities, so a same-lexeme target
/// captured by a nearer binding is a different element and must not
/// count. The binders the walk honors: local variables, local/named
/// functions (name and parameters), the enclosing member's own
/// parameters, catch parameters, for-loop variables, switch-case
/// pattern variables, and an if-case's pattern variables (then branch
/// only).
fn shadowed(c: &Check<'_>, path: &[u32], name: &str, at: usize) -> bool {
    path.iter()
        .rev()
        .any(|&scope| scope_binds_at(c, scope, name, at))
}

/// [`scope_binds`] with the if-case precision: the binder family binds
/// the whole span, an if-case only its then branch.
fn scope_binds_at(c: &Check<'_>, node: u32, name: &str, at: usize) -> bool {
    match c.kind(node) {
        NodeKind::IfStatement => if_case_binds(c, node, name) && in_then_branch(c, node, at),
        _ => scope_binds(c, node, name) && c.byte_span(node).contains(&at),
    }
}

/// Whether `node` is one of the scope binders that capture `name` for
/// its whole span.
fn scope_binds(c: &Check<'_>, node: u32, name: &str) -> bool {
    match c.kind(node) {
        NodeKind::Block | NodeKind::BlockBody => c
            .collect_direct(node)
            .into_iter()
            .any(|stmt| statement_binds(c, stmt, name)),
        NodeKind::CatchClause | NodeKind::FunctionExpression => c
            .collect_direct(node)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
            .is_some_and(|params| parameters_bind(c, params, name)),
        // The enclosing member's own parameters: the walk starts at the
        // unit root, so member rows sit on the path — a formal of the
        // same name captures every bare write in the body.
        NodeKind::Method
        | NodeKind::Constructor
        | NodeKind::Factory
        | NodeKind::TopLevelMethod => c
            .collect_direct(node)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
            .is_some_and(|params| parameters_bind(c, params, name)),
        NodeKind::LocalFunctionDeclaration | NodeKind::NamedFunctionExpression => {
            function_name_binds(c, node, name)
                || c.collect_direct(node)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
                    .is_some_and(|params| parameters_bind(c, params, name))
        }
        NodeKind::ForIn | NodeKind::ForControlFlow => for_in_binds(c, node, name),
        NodeKind::ForStatement => c
            .collect_direct(node)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::ForParts)
            .is_some_and(|parts| {
                c.collect_direct(parts)
                    .into_iter()
                    .any(|stmt| statement_binds(c, stmt, name))
            }),
        NodeKind::SwitchCase | NodeKind::SwitchExpressionCase => {
            c.collect_direct(node).into_iter().any(|kid| {
                c.kind(kid) == NodeKind::DeclaredVariablePattern
                    && c.payload(kid)
                        .first()
                        .is_some_and(|&v| v != NONE_TOKEN && c.lexeme(v) == name)
            })
        }
        _ => false,
    }
}

/// Whether `stmt` declares a binding of `name` at its own level (a
/// block sibling, or the initializer of a `for` parts list).
fn statement_binds(c: &Check<'_>, stmt: u32, name: &str) -> bool {
    match c.kind(stmt) {
        // The production `InitializedIdentifier` is width-0 at the
        // name — the name is its span head, not a child row
        // (`initialized_identifier_name` covers both tilings).
        NodeKind::VariablesDeclaration => c.collect_direct(stmt).into_iter().any(|decl| {
            c.kind(decl) == NodeKind::InitializedIdentifier
                && crate::rules::initialized_identifier_name(c, decl)
                    .is_some_and(|n| c.lexeme(n) == name)
        }),
        NodeKind::LocalFunctionDeclaration | NodeKind::NamedFunctionExpression => {
            function_name_binds(c, stmt, name)
        }
        NodeKind::PatternVarDeclStatement => pattern_binds(c, stmt, name),
        NodeKind::LabeledStatement => c
            .collect_direct(stmt)
            .into_iter()
            .any(|kid| c.kind(kid) != NodeKind::Label && statement_binds(c, kid, name)),
        _ => false,
    }
}

/// The name a local/named function binds (the `LocalFunctionDeclaration`-
/// stamped `Identifier`, reached through the `FunctionName` wrapper by
/// `member_declaration_name`).
fn function_name_binds(c: &Check<'_>, node: u32, name: &str) -> bool {
    crate::rules::declarations::member_declaration_name(c, node)
        .is_some_and(|n| c.lexeme(c.head_token(n)) == name)
}

/// Whether a pattern subtree declares `name` (`DeclaredVariablePattern`
/// payload word 0 carries the variable token).
fn pattern_binds(c: &Check<'_>, pattern: u32, name: &str) -> bool {
    c.children(pattern).any(|descendant| {
        c.kind(descendant) == NodeKind::DeclaredVariablePattern
            && c.payload(descendant)
                .first()
                .is_some_and(|&v| v != NONE_TOKEN && c.lexeme(v) == name)
    })
}

/// Whether a formal-parameter list declares a plain `name` formal: the
/// name is payload word 1, and a `this.x`/`super.x` receiver formal
/// (payload word 0) names the field itself — it never captures a bare
/// write (a constructor body's `_x = 1` beside `C(this._x)` writes the
/// field, whose `writeElement` upstream is the field).
fn parameters_bind(c: &Check<'_>, params: u32, name: &str) -> bool {
    c.collect_direct(params).into_iter().any(|param| {
        c.kind(param) == NodeKind::FormalParameter
            && c.payload(param).first().copied().unwrap_or(NONE_TOKEN) == NONE_TOKEN
            && c.payload(param)
                .get(1)
                .copied()
                .is_some_and(|tok| tok != NONE_TOKEN && c.lexeme(tok) == name)
    })
}

/// The for-in loop variable binds inside the loop: the bare form is an
/// `Identifier` child completing before the parts' `in` token, the
/// `var x` form a `DeclaredVariablePattern` child.
fn for_in_binds(c: &Check<'_>, scope: u32, name: &str) -> bool {
    let kids = c.collect_direct(scope);
    let Some(in_token) = kids
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::ForInParts)
        .and_then(|parts| c.payload(parts).get(3).copied())
        .filter(|&token| token != NONE_TOKEN)
    else {
        return false;
    };
    kids.into_iter().any(|kid| match c.kind(kid) {
        NodeKind::DeclaredVariablePattern => c
            .payload(kid)
            .first()
            .is_some_and(|&v| v < in_token && c.lexeme(v) == name),
        NodeKind::Identifier => {
            c.unit.ast.token_range(kid).end <= in_token && c.lexeme(c.head_token(kid)) == name
        }
        _ => false,
    })
}

/// Whether an `IfStatement`'s case clause binds `name`: the `case`
/// keyword rides the `ParenthesizedCondition` payload word 0 and the
/// pattern rows sit before it.
fn if_case_binds(c: &Check<'_>, stmt: u32, name: &str) -> bool {
    let kids = c.collect_direct(stmt);
    let Some(cond_pos) = kids
        .iter()
        .position(|&kid| c.kind(kid) == NodeKind::ParenthesizedCondition)
    else {
        return false;
    };
    let has_case = c
        .payload(kids[cond_pos])
        .first()
        .is_some_and(|&case| case != NONE_TOKEN);
    has_case
        && kids[..=cond_pos]
            .iter()
            .any(|&kid| pattern_binds(c, kid, name))
}

/// Whether the position `at` sits in `stmt`'s then branch — the only
/// branch an if-case pattern binding reaches.
fn in_then_branch(c: &Check<'_>, stmt: u32, at: usize) -> bool {
    let kids = c.collect_direct(stmt);
    let Some(cond_pos) = kids
        .iter()
        .position(|&kid| c.kind(kid) == NodeKind::ParenthesizedCondition)
    else {
        return false;
    };
    kids.get(cond_pos + 1)
        .is_some_and(|then| c.byte_span(*then).contains(&at))
}

/// Whether `ctor` sets `name`: a `this.name` field formal or a field
/// initializer of the name (:129-146).
fn sets_field(c: &Check<'_>, ctor: u32, name: &str) -> bool {
    let window = c.children(ctor);
    (window.start..window.end).any(|id| match c.kind(id) {
        NodeKind::FormalParameter => {
            let receiver = c.payload(id).first().copied().unwrap_or(NONE_TOKEN);
            receiver != NONE_TOKEN
                && c.lexeme(receiver) == "this"
                && c.payload(id)
                    .get(1)
                    .is_some_and(|&name_token| c.lexeme(name_token) == name)
        }
        NodeKind::Initializer => initializer_set_name(c, id) == Some(name),
        _ => false,
    })
}

/// The field name an initializer entry sets: the entry head token is the
/// bare form's name; `this.x = e` names two tokens later.
fn initializer_set_name<'a>(c: &'a Check<'a>, init: u32) -> Option<&'a str> {
    let head = c.head_token(init);
    let name_token = match c.lexeme(head) {
        "this" => head + 2,
        _ => head,
    };
    let token = c.unit.tokens.get(name_token as usize)?;
    (token.kind == dartq_scanner::TokenKind::Identifier).then(|| c.lexeme(name_token))
}
