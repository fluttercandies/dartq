use super::{Check, NodeKind, PayloadField, RuleSpec, same_file, slice};
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
///   an instance-qualified target whose member name is private
///   (`c._x = 42` — `writeElement` is the field whatever the receiver),
///   `++`/`--` operands (the bare name completes as a send group), and
///   cascade-section write targets (`.._x = 1` — a section assignment is
///   an `AssignmentExpression` upstream and its `writeElement` is the
///   field). A constructor initializer-list write (`C() : _x = 7`) is
///   NOT a mutation. A write captured by a nearer
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
/// - Candidates: every private non-`final` field declaration of any
///   class-like owner (`visitFieldDeclaration`, :107-123). Enum fields
///   are excluded whole (:109); an extension-type instance field is the
///   front-end's error and never candidates (`isInvalidExtensionTypeField`,
///   :108), while a static one passes — the constructor-set faces
///   (:69-88) run only when the owner `is ClassDeclaration`, so mixin,
///   extension and extension-type fields answer by the initializer
///   alone. The primary-constructor declaring formals
///   (`visitPrimaryConstructorDeclaration`, :140-159) are in: a plain
///   private non-`final` formal of a class or mixin owner declares a
///   field and reports at the formal itself; the enum and extension-type
///   owners are skipped (:142-145).
/// - Private-ness is the leading underscore. The setter-override
///   exemption (`_overridesField`, :147-158) reads the written
///   extends/with/implements chain — the same-file ancestor walk, then
///   the semantic bridge's inheritance arm under the `name=` setter
///   spelling; unresolved chains over-report. A same-class `set _x` is
///   no arm: the official `getOverridden` is supertype-only, and a
///   non-final field beside a same-name setter is the front-end's
///   setter-conflicts-with-declaration error, so no valid shape needs
///   it.
/// - The cross-unit half of the official library scan is out of the
///   substrate: the mutated set folds one unit's writes, so a write in
///   a sibling part unit is invisible to the declaring unit's walk (and
///   vice versa). Closing it needs the driver-mounted library-wide face
///   precedent (`library_type_uses`), not a rule-side change.
pub(crate) static SPEC: RuleSpec =
    crate::spec!("prefer_final_fields", &[NodeKind::CompilationUnit], check,);

/// One candidate: the reported row (the field's name-through-initializer
/// identifier, or a primary-constructor declaring formal), its name,
/// whether the declaration is its own setting, the enclosing class-like
/// row (the setter-override walk anchors on it), and whether the
/// constructor-set faces apply.
struct Candidate<'a> {
    declaration: u32,
    name: &'a str,
    has_initializer: bool,
    class_row: u32,
    /// The constructor-set faces (:69-88) run only when the official's
    /// `classDeclaration is ClassDeclaration` holds — a class owner. A
    /// declaring formal carries `has_initializer` instead: it IS the
    /// setting (:94-98), reported unconditionally when unmutated.
    constructor_faces: bool,
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
        if candidate.constructor_faces {
            // The constructors of the field's own class (:62-71). The
            // class subtree occupies the contiguous row range ending at
            // the class row, so the constructor set reads from that
            // range under every tiling layout. Factories are
            // `ConstructorDeclaration`s in the official and count for
            // the "set in every constructor" question; they never set.
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
        } else if !candidate.has_initializer {
            // A non-class owner (mixin, extension, extension type) or a
            // declaring formal: no constructor set — the initializer (or
            // the formal itself) is the only setting face (:89-91,
            // :94-98).
            continue;
        }
        // The report extent is the official VariableDeclaration — the
        // name through the initializer's end. The declaration row's
        // registered tail can regress behind the initializer's last row
        // (a trailing bracketed literal completes rows whose tail
        // adoption stops short: `var _x = []` registers `_x = `), so the
        // extent also reads the completion window's last row end — the
        // subtree the initializer actually built.
        let range = c.utf16_span(candidate.declaration);
        let end = c
            .children(candidate.declaration)
            .fold(range.end, |end, row| end.max(c.utf16_span(row).end));
        c.report(range.start..end, &[candidate.name]);
    }
}

/// The one-pass unit walk: candidates from `Fields` declarations and
/// primary-constructor declaring formals, and mutations from bare-name
/// writes. `owner` carries the enclosing class-like row and its kind
/// (the extension-type static gate and the constructor-faces gate read
/// the kind). `path` carries the enclosing nodes of the visited row (the
/// shadow gate reads it). Enum bodies are skipped whole (the official
/// excludes every enum field, :109); top-level declarations are not
/// fields and never match.
fn scan<'a>(
    c: &Check<'a>,
    node: u32,
    owner: Option<(u32, NodeKind)>,
    candidates: &mut Vec<Candidate<'a>>,
    mutated: &mut Vec<&'a str>,
    path: &mut Vec<u32>,
) {
    match c.kind(node) {
        NodeKind::EnumDeclaration => return,
        NodeKind::ClassDeclaration
        | NodeKind::MixinDeclaration
        | NodeKind::ExtensionDeclaration
        | NodeKind::ExtensionTypeDeclaration => {
            // Member discovery anchors on the owner row: its completion
            // window opens at the head keyword and covers every member
            // row, while the body container's window can open later and
            // the derived parent pointer can link a field row to either
            // neighbor. Walking the owner row's window is correct under
            // both layouts.
            let kind = c.kind(node);
            for kid in c.collect_direct(node) {
                path.push(kid);
                scan(c, kid, Some((node, kind)), candidates, mutated, path);
                path.pop();
            }
            // `visitPrimaryConstructorDeclaration` (:140-159): the
            // declaring formals of the owner's primary constructor —
            // the enum and extension-type owners are excluded
            // (:142-145).
            if matches!(kind, NodeKind::ClassDeclaration | NodeKind::MixinDeclaration) {
                declaring_formals(c, node, candidates);
            }
            return;
        }
        NodeKind::DeclarationBody => {
            // A body container between the owner row and its members
            // owns rows in either its completion window or its
            // derived-child set depending on layout; both faces are
            // cheap and the candidate set dedupes by declaration.
            let mut kids = c.collect_direct(node);
            kids.extend(crate::rules::rows_with_parent(c, node));
            for kid in kids {
                path.push(kid);
                scan(c, kid, owner, candidates, mutated, path);
                path.pop();
            }
            return;
        }
        NodeKind::Fields => {
            // An extension-type instance field is the front-end's error
            // (`isInvalidExtensionTypeField`, :108) and never
            // candidates; the valid static face passes. The static
            // modifier rides the `Fields` member row. The walk below
            // still descends — an initializer's own writes count (the
            // official visits assignments wherever they sit).
            if let Some((class_row, owner_kind)) = owner
                && (owner_kind != NodeKind::ExtensionTypeDeclaration
                    || dartq_ast::mod_flags(c.unit.ast, node) & same_file::STATIC_BIT != 0)
            {
                for declaration in c.collect_direct(node) {
                    if c.kind(declaration) != NodeKind::VariablesDeclaration {
                        continue;
                    }
                    if let Some(kw) = crate::rules::variables_declaration_keyword(c, declaration)
                        && matches!(c.lexeme(kw), "final" | "const")
                    {
                        continue;
                    }
                    for var in c.collect_direct(declaration) {
                        if c.kind(var) != NodeKind::InitializedIdentifier {
                            continue;
                        }
                        // The production row is width-0 at the name, so the
                        // name is the span head; hand-built tables keep the
                        // same head (see
                        // [`crate::rules::initialized_identifier_name`]).
                        let Some(name_token) =
                            crate::rules::initialized_identifier_name(c, var)
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
                        let has_initializer =
                            c.head_token(var) != crate::rules::tail_token(c, var);
                        candidates.push(Candidate {
                            declaration: var,
                            name,
                            has_initializer,
                            class_row,
                            constructor_faces: owner_kind == NodeKind::ClassDeclaration,
                        });
                    }
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
            } else if !in_initializer_list(c, path) {
                // A bare-name write (`_x = 2`): the left operand run is
                // the owner pieces left of the operator — a
                // single-identifier chain. The direct-child face
                // covers the hand-built compact trees. A constructor
                // initializer-list write (`C() : _x = 7`) is not a
                // mutation — the official rule reports a field written
                // only there.
                let lhs = match c.collect_direct(node).first() {
                    Some(&row) if c.kind(row) == NodeKind::Identifier => Some(row),
                    _ => slice::binary_operands_at(c, node).and_then(|(left, _)| {
                        match slice::access_run(c, &left)[..] {
                            [only] if c.kind(only) == NodeKind::Identifier => Some(only),
                            _ => None,
                        }
                    }),
                };
                match lhs {
                    Some(lhs) => push_write(c, path, mutated, lhs),
                    // An instance-qualified write (`c._x = 42`): the left
                    // operand's member row names the written field — the
                    // official `writeElement` is the field whatever the
                    // receiver — so a private name counts directly (only
                    // this library's fields carry one).
                    None => {
                        if let Some((left, _)) = slice::binary_operands_at(c, node)
                            && let Some(&member) = slice::access_run(c, &left).last()
                            && matches!(
                                c.kind(member),
                                NodeKind::DotAccess | NodeKind::Qualified | NodeKind::Identifier
                            )
                        {
                            let name = c.lexeme(c.head_token(member));
                            if name.starts_with('_') {
                                mutated.push(name);
                            }
                        }
                    }
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
                // The bare-name operand sits on either side of the
                // operator leaf: the bridge stamps the leaf AFTER the
                // operand completes (`--_x`, `_x++` alike — the leaf's own
                // head token is the operator), while a hand-built tree
                // follows the official child order. Both neighbours are
                // candidates and the name row decides.
                let operand = [pos.checked_sub(1), Some(pos + 1)]
                    .into_iter()
                    .flatten()
                    .filter_map(|i| siblings.get(i).copied())
                    .find(|&row| matches!(c.kind(row), NodeKind::Identifier | NodeKind::Send));
                if let Some(operand) = operand {
                    // A bare name completes as a send group: the `Send`
                    // row's head token is the name (`_x++` reports the
                    // write at `_x`, not at the marker).
                    push_write(c, path, mutated, operand);
                }
            }
        }
        _ => {}
    }
    for kid in c.collect_direct(node) {
        path.push(kid);
        scan(c, kid, owner, candidates, mutated, path);
        path.pop();
    }
}

/// The primary-constructor declaring formals of `class_row`
/// (`visitPrimaryConstructorDeclaration`, :140-159): a plain formal
/// declares a field (`isDeclaring`) — a `this.`/`super.` receiver
/// formal binds an explicit field the `Fields` face already carries —
/// and a private non-`final` one candidates with the formal itself as
/// the reported node (`[!var int _x!]`, the whole formal span). No
/// constructor faces: the formal IS the setting (:94-98).
fn declaring_formals<'a>(c: &Check<'a>, class_row: u32, candidates: &mut Vec<Candidate<'a>>) {
    for primary in c
        .collect_direct(class_row)
        .into_iter()
        .filter(|&kid| c.kind(kid) == NodeKind::PrimaryConstructor)
    {
        let Some(params) = c
            .collect_direct(primary)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
        else {
            continue;
        };
        for param in c.collect_direct(params) {
            if c.kind(param) != NodeKind::FormalParameter {
                continue;
            }
            if c.field(param, PayloadField::Receiver).unwrap_or(NONE_TOKEN) != NONE_TOKEN {
                continue;
            }
            let Some(name_token) = c
                .field(param, PayloadField::Name)
                .filter(|&token| token != NONE_TOKEN)
            else {
                continue;
            };
            let name = c.lexeme(name_token);
            if !name.starts_with('_') || formal_is_final(c, param, name_token) {
                continue;
            }
            candidates.push(Candidate {
                declaration: param,
                name,
                has_initializer: true,
                class_row,
                constructor_faces: false,
            });
        }
    }
}

/// Whether a formal carries `final` (`!field.isFinal`, :154): the
/// keyword sits between the formal's span head (a modifier starts the
/// span) and its name token.
fn formal_is_final(c: &Check<'_>, param: u32, name_token: u32) -> bool {
    (c.head_token(param)..name_token).any(|token| c.lexeme(token) == "final")
}

/// Whether the DFS path crosses a constructor initializer list: a write
/// there (`C() : _x = 7`) is not a mutation for this rule — the official
/// `writeElement` face never sees it, so a field written only in an
/// initializer list reports. A constructor *body* write does count and
/// sits under a `BlockBody`, not under these rows.
fn in_initializer_list(c: &Check<'_>, path: &[u32]) -> bool {
    path.iter().any(|&row| {
        matches!(
            c.kind(row),
            NodeKind::FieldInitializer | NodeKind::Initializer
        )
    })
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
        NodeKind::Method | NodeKind::Constructor | NodeKind::Factory | NodeKind::TopLevelMethod => {
            c.collect_direct(node)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
                .is_some_and(|params| parameters_bind(c, params, name))
        }
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
                    && c.field(kid, PayloadField::Variable)
                        .is_some_and(|v| v != NONE_TOKEN && c.lexeme(v) == name)
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

/// Whether a pattern subtree declares `name` (the
/// `DeclaredVariablePattern` row's `Variable` word carries the variable
/// token).
fn pattern_binds(c: &Check<'_>, pattern: u32, name: &str) -> bool {
    c.children(pattern).any(|descendant| {
        c.kind(descendant) == NodeKind::DeclaredVariablePattern
            && c.field(descendant, PayloadField::Variable)
                .is_some_and(|v| v != NONE_TOKEN && c.lexeme(v) == name)
    })
}

/// Whether a formal-parameter list declares a plain `name` formal: the
/// name is payload word 1, and a `this.x`/`super.x` receiver formal
/// (the `Receiver` word) names the field itself — it never captures a bare
/// write (a constructor body's `_x = 1` beside `C(this._x)` writes the
/// field, whose `writeElement` upstream is the field).
fn parameters_bind(c: &Check<'_>, params: u32, name: &str) -> bool {
    c.collect_direct(params).into_iter().any(|param| {
        c.kind(param) == NodeKind::FormalParameter
            && c.field(param, PayloadField::Receiver).unwrap_or(NONE_TOKEN) == NONE_TOKEN
            && c.field(param, PayloadField::Name)
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
        .and_then(|parts| c.field(parts, PayloadField::InKeyword))
        .filter(|&token| token != NONE_TOKEN)
    else {
        return false;
    };
    kids.into_iter().any(|kid| match c.kind(kid) {
        NodeKind::DeclaredVariablePattern => c
            .field(kid, PayloadField::Variable)
            .is_some_and(|v| v < in_token && c.lexeme(v) == name),
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
        .field(kids[cond_pos], PayloadField::CaseKeyword)
        .is_some_and(|case| case != NONE_TOKEN);
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
            let receiver = c.field(id, PayloadField::Receiver).unwrap_or(NONE_TOKEN);
            receiver != NONE_TOKEN
                && c.lexeme(receiver) == "this"
                && c.field(id, PayloadField::Name)
                    .is_some_and(|name_token| c.lexeme(name_token) == name)
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

#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule};

    /// The whole upstream oracle corpus — `PreferFinalFieldsTest` (30
    /// cases, `pkg/linter/test/rules/prefer_final_fields_test.dart`) plus
    /// the `PreferFinalFieldsExtensionTypesTest` trio and the eight
    /// `messages.yaml` doc cases: 41 total, `conformance/status.tsv`
    /// `lint prefer_final_fields 41 28 13`. Each entry is the exact
    /// materialized case source (the test-file cases carry a leading
    /// blank line, the doc files do not) and the oracle's
    /// `(line, column, length)` for every `prefer_final_fields` row
    /// (`expected.tsv`). Only the rule's own rows are pinned — the
    /// co-reported `unused_field`/ERROR rows are other faces.
    ///
    /// The two cross-unit cases (`test_assignedInPart`,
    /// `test_declaredInPart`) are not representable in a single-source
    /// fixture: the oracle answers `-` because the field's write (or
    /// declaration) lives in the sibling part unit, which this per-unit
    /// walk cannot see (the documented cross-unit residual — the fix is
    /// the driver-mounted library-wide mutation face, not a rule-side
    /// change).
    ///
    /// Every mismatch is collected before the single panic so one run
    /// shows the whole deviation set.
    #[test]
    fn prefer_final_fields_oracle_corpus_pins() {
        // (case, source, oracle rows as (line, column, length)).
        let cases: &[(&str, &str, &[(u32, u32, u32)])] = &[
            (
                "assignedInConstructorInitializer",
                "\nclass C {\n  int _x;\n  C() : _x = 7;\n}\n",
                &[(3, 7, 2)],
            ),
            (
                "assignedInConstructorInitializer_butNotAll",
                "\nclass C {\n  var _x;\n  C(this._x);\n  C.named();\n}\n",
                &[],
            ),
            (
                "assignedInTopLevelFunction",
                "\nclass C {\n  int _x = 0;\n}\n\nvoid f() {\n  var c = C();\n  c._x = 42;\n}\n",
                &[],
            ),
            (
                "assignment_plusEquals",
                "\nclass C {\n  var _x = 1;\n  void f() {\n    _x += 2;\n  }\n}\n",
                &[],
            ),
            (
                "enum",
                "\nenum A {\n  a,b,c;\n  int _x = 0;\n  int get x => _x;\n}\n",
                &[],
            ),
            ("final_multiple", "\nclass C {\n  final _x = 1, _y = 2;\n}\n", &[]),
            (
                "final_public_multiple",
                "\nclass C {\n  final x = 1, y = 2;\n}\n",
                &[],
            ),
            (
                "indexAssignment",
                "\nclass C {\n  var _x = [];\n\n  void f() {\n    _x[0] = 3;\n  }\n}\n",
                &[(3, 7, 7)],
            ),
            (
                "overrideField_extends",
                "\nclass A {\n  bool _a = false;\n  void m() {\n    _a = true;\n    print(_a);\n  }\n}\n\nclass B extends A {\n  @override\n  bool _a = false;\n\n  @override\n  void m() {\n    print(_a);\n  }\n}\n",
                &[],
            ),
            (
                "overrideField_implements",
                "\nclass A {\n  bool _a = false;\n  void m() {\n    _a = true;\n    print(_a);\n  }\n}\n\nclass B implements A {\n  @override\n  bool _a = false;\n\n  @override\n  void m() {\n    print(_a);\n  }\n}\n",
                &[],
            ),
            (
                "overrideSetter_extends",
                "\nclass A {\n  set _a(bool a) {}\n  void m() {\n    _a = true;\n  }\n}\n\nclass B extends A {\n  @override\n  bool _a = false;\n\n  @override\n  void m() {\n    print(_a);\n  }\n}\n",
                &[],
            ),
            (
                "overrideSetter_implements",
                "\nclass A {\n  set _a(bool a) {}\n  void m() {\n    _a = true;\n  }\n}\n\nclass B implements A {\n  @override\n  bool _a = false;\n\n  @override\n  void m() {\n    print(_a);\n  }\n}\n",
                &[],
            ),
            (
                "postfixExpression_decrement",
                "\nclass C {\n  int _x = 1;\n  void f() {\n    _x--;\n  }\n}\n",
                &[],
            ),
            (
                "postfixExpression_increment",
                "\nclass C {\n  int _x = 1;\n  void f() {\n    _x++;\n  }\n}\n",
                &[],
            ),
            (
                "prefixExpression_decrement",
                "\nclass C {\n  int _x = 1;\n  void f() {\n    --_x;\n  }\n}\n",
                &[],
            ),
            (
                "prefixExpression_increment",
                "\nclass C {\n  int _x = 1;\n  void f() {\n    ++_x;\n  }\n}\n",
                &[],
            ),
            (
                "prefixExpression_not",
                "\nclass C {\n  bool _x = false;\n  void f() {\n    !_x;\n  }\n}\n",
                &[(3, 8, 10)],
            ),
            (
                "prefixExpression_tilde",
                "\nclass C {\n  int _x = 0xffff;\n  void f() {\n    ~_x;\n  }\n}\n",
                &[(3, 7, 11)],
            ),
            (
                "propertyAccess",
                "\nclass C {\n  int _x = 1;\n  void f() {\n    _x.isEven;\n  }\n}\n",
                &[(3, 7, 6)],
            ),
            (
                "readInInstanceMethod",
                "\nclass C {\n  int _x = 0;\n\n  void f() {\n    var a = _x;\n  }\n}\n",
                &[(3, 7, 6)],
            ),
            (
                "reassigned",
                "\nclass C {\n  var _x = 1;\n  void f() {\n    _x = 2;\n  }\n}\n",
                &[],
            ),
            (
                "referencedInFieldFormalParameters",
                "\nclass C {\n  int _x;\n  C(this._x);\n  C.named(this._x);\n}\n",
                &[(3, 7, 2)],
            ),
            (
                "subclassOnGenericClass",
                "\nabstract class C<T> {\n  int _x = 0;\n}\n\nclass D extends C<int> {\n  void f() {\n    _x = 1;\n  }\n}\n",
                &[],
            ),
            ("unused", "\nclass C {\n  var _x = 1;\n}\n", &[(3, 7, 6)]),
            (
                "unused_fromDeclaringParameter",
                "\nclass C(var int _x) {\n  void m() {\n    print(_x);\n  }\n}\n",
                &[(2, 9, 10)],
            ),
            (
                "unused_multiple",
                "\nclass C {\n  var _x = 1, _y = 2;\n  void f() {\n    _x = 2;\n  }\n}\n",
                &[(3, 15, 6)],
            ),
            ("unused_public", "\nclass C {\n  var x = 1;\n}\n", &[]),
            (
                "unused_uninitialized",
                "\nclass C {\n  int? _x;\n}\n",
                &[],
            ),
            (
                "ext_field_instance",
                "\nextension type E(Object o) {\n  int _i = 0;\n}\n",
                &[],
            ),
            (
                "ext_field_static",
                "\nextension type E(Object o) {\n  static int _i = 0;\n}\n",
                &[(3, 14, 6)],
            ),
            (
                "ext_field_static_writtenInConstructor",
                "\nextension type E(Object o) {\n  static Object _o = 0;\n  E.e(this.o) {\n    _o = o;\n  }\n}\n",
                &[],
            ),
            (
                "doc_bad_0",
                "class C {\n  int _f = 1;\n\n  int get f => _f;\n}\n",
                &[(2, 7, 6)],
            ),
            (
                "doc_bad_1",
                "class BadImmutable {\n  var _label = 'hola mundo! BadImmutable'; // LINT\n  var label = 'hola mundo! BadImmutable'; // OK\n}\n",
                &[(2, 7, 35)],
            ),
            (
                "doc_bad_2",
                "class MultipleMutable {\n  var _label = 'hola mundo! GoodMutable', _offender = 'mumble mumble!'; // LINT\n  var _someOther; // LINT\n\n  MultipleMutable() : _someOther = 5;\n\n  MultipleMutable(this._someOther);\n\n  void changeLabel() {\n    _label= 'hello world! GoodMutable';\n  }\n}\n",
                &[(2, 43, 28), (3, 7, 10)],
            ),
            (
                "doc_bad_3",
                "class AssignedInAllConstructors {\n  var _label; // LINT\n  AssignedInAllConstructors(this._label);\n  AssignedInAllConstructors.withDefault() : _label = 'Hello';\n}\n",
                &[(2, 7, 6)],
            ),
            (
                "doc_good_0",
                "class C {\n  final int _f = 1;\n\n  int get f => _f;\n}\n",
                &[],
            ),
            (
                "doc_good_1",
                "class GoodImmutable {\n  final label = 'hola mundo! BadImmutable', bla = 5; // OK\n  final _label = 'hola mundo! BadImmutable', _bla = 5; // OK\n}\n",
                &[],
            ),
            (
                "doc_good_2",
                "class GoodMutable {\n  var _label = 'hola mundo! GoodMutable';\n\n  void changeLabel() {\n    _label = 'hello world! GoodMutable';\n  }\n}\n",
                &[],
            ),
            (
                "doc_good_3",
                "class NotAssignedInAllConstructors {\n  var _label; // OK\n  NotAssignedInAllConstructors();\n  NotAssignedInAllConstructors.withDefault() : _label = 'Hello';\n}\n",
                &[],
            ),
        ];
        // The UTF-16 offset of a 1-based `(line, column)` in an ASCII source.
        fn offset(source: &str, line: u32, column: u32) -> u32 {
            let line_start: usize = source
                .split_inclusive('\n')
                .take(line as usize - 1)
                .map(str::len)
                .sum();
            (line_start + column as usize - 1) as u32
        }
        let mut mismatches = Vec::new();
        for (case, source, oracle) in cases {
            let f = Fixture::parse(source);
            let mut lints = f.run(&one_rule("prefer_final_fields"));
            lints.sort_by_key(|lint| lint.range.start);
            let got: Vec<(u32, u32)> = lints
                .iter()
                .map(|lint| (lint.range.start, lint.range.end))
                .collect();
            let want: Vec<(u32, u32)> = oracle
                .iter()
                .map(|&(line, column, length)| {
                    let start = offset(source, line, column);
                    (start, start + length)
                })
                .collect();
            if got != want {
                mismatches.push(format!("{case}: want {want:?} got {got:?}"));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }
}
