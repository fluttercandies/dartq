use rustc_hash::{FxHashMap, FxHashSet};

use super::{Check, EXTERNAL_MEMBER_BIT, PayloadField, RuleSpec, container_name_token};
use crate::engine::ReachabilityFace;
use crate::rules::{
    MemberQuery, SupertypeClause, get_set_token, initialized_identifier_name, member_matches,
    member_name, rows_with_parent, same_file_ancestors, supertypes, utf16_token_span,
    variables_declaration_keyword,
};
use dartq_ast::{NodeKind, mod_flags};

/// Official: upstream Dart SDK source,
/// `pkg/linter/lib/src/rules/unreachable_from_main.dart` — in an
/// executable library (one with an entry point), a public declaration
/// no entry point reaches reports at its name
/// (`Unreachable member '{0}' in an executable library.`). Message:
/// `diagnostic.g.dart` `unreachableFromMain`.
///
/// The shell is the thin consumer of the dartq-engine reachability pass
/// (#1199): entry detection (`main`, `@pragma('vm:entry-point')`,
/// `@Preview`), reference extraction, and propagation are the engine
/// face, exposed per declaration through
/// [`crate::engine::Unit::reachability`]. The shell owns the reporting
/// face — the candidate set and the official judgment order:
/// unreachable (a `None` flag — outside the face's universe — never
/// reports) → public → not `@visibleForTesting` → not a public
/// widget-preview member → outermost container wins (a member nested in
/// an unreachable container is not reported; the container is).
///
/// Candidates: every top-level declaration (top-level variables split
/// per variable), plus the public members of a public
/// class/enum/extension/extension-type/mixin — public constructors
/// (an enum's never gather), public fields, public methods. Methods
/// named `test_*`/`solo_test_*`/`setUp`/`tearDown`/`setUpClass`/
/// `tearDownClass` never gather; neither do enum constants (the
/// official gatherer adds an enum body's `members`, never its
/// constants).
///
/// Shell-side suppression faces (approximations of the official's
/// element queries): a member whose name resolves against an ancestor's
/// instance-member table never gathers (the `isOverride` arm —
/// `getOverridden` across the interface, instance members only; the
/// same-file ancestors answer name-level, the cross-file remainder
/// anchors the semantic bridge's inheritance walk
/// ([`super::bridge_inherited_member`], the `prefer_final_fields`
/// override read's twin); extensions and extension types pass
/// `containerElement: null` upstream (:135-149) so their members never
/// take the override skip), a non-static method named `toJson` never
/// gathers (the `dart:convert` dynamic-invocation auto-reach arm
/// :293-303), and a declaration whose doc-comment `[Name]` references
/// match its own name reads as referenced (the comment-reference edge
/// the engine face does not carry).
///
/// The local per-library face: the official walk is per-library
/// (`context.allUnits`, `entryPoints.isEmpty` bail) while the engine
/// graph is workspace-global, so for a single-file library the shell
/// re-derives the official algorithm name-level — the
/// [`ReferenceGraph`] carries the entry seeds (`main`, the
/// `vm:entry-point` pragma through const aliases — `computeConstantValue`
/// — and widget-preview functions) and the entry-reach closure over the
/// same reference keys the official visitor counts (`visitSimpleIdentifier`
/// expression reads; `visitNamedType`'s exception list — typedef aliases,
/// extension types, type arguments; `visitClassDeclaration`'s
/// unconditional clause names; annotations; operators through
/// `visitBinaryExpression`/`visitPrefixExpression`/`visitPostfixExpression`/
/// `visitIndexExpression`; redirects through
/// `visitRedirectingConstructorInvocation`). The closure is
/// AUTHORITATIVE for single-file libraries in both directions — the
/// engine flag's under-reach (a missed edge) and over-reach (a type
/// annotation, a constant pattern, another library's reference, a
/// reference inside an unreachable declaration's body) both fold to
/// the closure's answer — and it answers whether THIS library declares
/// an entry at all (an imported file without one is silent even when
/// the workspace graph marks its rows reachable). A part or part-of
/// directive makes the same-file index incomplete, so a part-carrying
/// library keeps the engine flag with the counted-use rescue.
///
/// Approximations (documented): the bridge-backed override read
/// degrades to the same-file arm alone when the semantic analysis is
/// unmounted (the fixture harness) or the supertype chain leaves the
/// analyzed set — the face family's documented under-reach posture.
/// The reference graph is name-level — an operator use on
/// an unrelated type marks the same-file operator spelling (the
/// official resolves the element), the external-variable
/// `NamedType` exception is not carried (no corpus pin), and entries
/// read annotations by spelling with a flutter `widget_previews.dart`
/// import-surface gate (a unit-declared `Preview` class shadows, the
/// `customPreviewClass` posture). Annotation reads (`@visibleForTesting`)
/// are by spelling. An unnamed extension has no name token to key a
/// face flag with: it never reports, and its members judge on their own
/// flags.
///
/// Constructor verdicts: the face universe is the capture's declaration
/// rows and constructors are not declaration rows (`dartq-index`
/// capture, the constructor exclusion), so a real-pipeline constructor
/// rides its class's offset (the unnamed echo) or answers no verdict
/// (the named face). Where the face gives a constructor no dead verdict,
/// the constructor's own reference face decides (`ctor_self_reachable`):
/// the counting positions (`C()`, `C.named`, `: this.x()`, `= C.x`,
/// `super.x()`) plus the marking arms the counting index cannot spell —
/// `@reflectiveTest` (:220-231), the subclass default-constructor walk
/// (:199-215, :454-465), the walked explicit constructor without a
/// super-initializer (:247-259), and the unnamed `super()` invocation
/// (:400-406). Residual: a constructor referenced only from another
/// unit of the library keeps no counted position here and over-reports
/// where the official's all-units declaration map marks it.
pub(crate) static SPEC: RuleSpec =
    crate::spec!("unreachable_from_main", &[NodeKind::CompilationUnit], check,);

/// The candidate class the widget-preview suppression reads (upstream
/// `hasWidgetPreview`: constructors, top-level functions, and static
/// executables are the suppressible shapes).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// A top-level function, getter, or setter
    /// (`TopLevelFunctionElement`).
    TopLevelFunction,
    /// A constructor or factory (`ConstructorElement`).
    Constructor,
    /// A method or accessor; the flag is the `static` face.
    Method(bool),
    /// Everything else — never widget-preview suppressed.
    Other,
}

fn check(c: &mut Check<'_>) {
    let Some(face) = c.unit.reachability else {
        return;
    };
    // The constructor marking arms (the subclass walk the constructor
    // verdict fallback consults) — built once per unit.
    let arms = ctor_arms(c);
    // The name-level reference graph: the flat counting index, the
    // per-declaration dependency edges, the entry seeds, and the
    // entry-reach closure (the official per-library walk, name-level).
    let graph = ReferenceGraph::build(c);
    // `if (entryPoints.isEmpty) return` — a library without an entry
    // point reports nothing at all. The entry read is local first (the
    // engine's `has_entry` is workspace-global — a same-workspace
    // reference can mark this library's rows reachable without any
    // local entry); a part or part-of directive widens the entry read
    // back to the engine flag (the entry may live in a sibling file of
    // the same library, and the local index does not cross files).
    let multi_file = directive_rows(c, c.node())
        .into_iter()
        .any(|row| matches!(c.kind(row), NodeKind::Part | NodeKind::PartOf));
    if graph.used.is_empty() && !(multi_file && face.has_entry) {
        return;
    }
    // The anti-over-reach override (a `Some(true)` flag the local
    // closure contradicts) applies only where the local index is
    // complete — a single-file library.
    let single_file = !multi_file;
    // The name offsets of the unreachable public containers: a member's
    // enclosing declaration in that set suppresses the member (the
    // outermost-container-wins gate).
    let mut dead: FxHashSet<u32> = FxHashSet::default();
    for top in c.collect_direct(c.node()) {
        if c.kind(top) != NodeKind::TopLevelDeclaration {
            continue;
        }
        for row in declaration_rows(c, top) {
            match c.kind(row) {
                NodeKind::TopLevelMethod => {
                    let Some(name) = name_token_of(c, top, row) else {
                        continue;
                    };
                    judge(
                        c,
                        &face,
                        &graph,
                        &arms,
                        single_file,
                        top,
                        row,
                        Shape::TopLevelFunction,
                        name,
                        "",
                        c.lexeme(name),
                        c.lexeme(name),
                        c.lexeme(name),
                    );
                }
                NodeKind::TopLevelFields => {
                    for variable in c.collect_direct(row) {
                        if c.kind(variable) != NodeKind::VariablesDeclaration {
                            continue;
                        }
                        for var in c.collect_direct(variable) {
                            if c.kind(var) != NodeKind::InitializedIdentifier {
                                continue;
                            }
                            let Some(name) = initialized_identifier_name(c, var) else {
                                continue;
                            };
                            judge(
                                c,
                                &face,
                                &graph,
                                &arms,
                                single_file,
                                top,
                                row,
                                Shape::Other,
                                name,
                                "",
                                c.lexeme(name),
                                c.lexeme(name),
                                c.lexeme(name),
                            );
                        }
                    }
                }
                NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::ExtensionDeclaration
                | NodeKind::ExtensionTypeDeclaration
                | NodeKind::EnumDeclaration => {
                    judge_container(c, &face, &graph, &arms, single_file, top, row, &mut dead);
                }
                NodeKind::Typedef => {
                    let Some(name) = name_token_of(c, top, row) else {
                        continue;
                    };
                    judge(
                        c,
                        &face,
                        &graph,
                        &arms,
                        single_file,
                        top,
                        row,
                        Shape::Other,
                        name,
                        "",
                        c.lexeme(name),
                        c.lexeme(name),
                        c.lexeme(name),
                    );
                }
                _ => {}
            }
        }
    }
}

/// Judges one public container: the container's own report, its
/// dead-container registration, and its members' reports.
fn judge_container(
    c: &mut Check<'_>,
    face: &ReachabilityFace<'_>,
    graph: &ReferenceGraph,
    arms: &CtorArms,
    single_file: bool,
    wrapper: u32,
    row: u32,
    dead: &mut FxHashSet<u32>,
) {
    let name = container_name_token(c, row);
    let name_offset = name.map(|token| utf16_token_span(c, token).start);
    // The dead registration precedes the reporting gates (upstream's
    // `unusedDeclarations` is the pre-gate set): an unreachable public
    // container suppresses its members even when the container itself
    // is `@visibleForTesting`.
    let is_dead = name.is_some_and(|token| {
        effective_verdict(
            c,
            face,
            graph,
            single_file,
            row,
            token,
            utf16_token_span(c, token).start,
        ) == Some(false)
    });
    if let (Some(offset), true) = (name_offset, is_dead) {
        let public = name.is_some_and(|token| !c.lexeme(token).starts_with('_'));
        if public {
            dead.insert(offset);
        }
    }
    if let Some(name) = name {
        judge(
            c,
            &face,
            graph,
            arms,
            single_file,
            wrapper,
            row,
            Shape::Other,
            name,
            "",
            c.lexeme(name),
            c.lexeme(name),
            c.lexeme(name),
        );
    }
    // Members gather only under a public container. A dead one
    // suppresses its executables — methods and constructors report at
    // the container alone (upstream :568-577: the member's enclosing
    // `Declaration` is the dead class-like) — but FIELDS still face
    // their own verdict: the gathered field node is the
    // `VariableDeclaration`, whose enclosing `Declaration` is the
    // `FieldDeclaration` itself (never a gathered declaration, so never
    // in `unusedDeclarations` — the suppression cannot fire upstream).
    // An unnamed extension has no name to judge — its members judge on
    // their own flags.
    let Some(name) = name else {
        return;
    };
    if c.lexeme(name).starts_with('_') {
        return;
    }
    let container = c.lexeme(name).to_owned();
    let is_enum = c.kind(row) == NodeKind::EnumDeclaration;
    // The ancestor chain resolves once per container, flattened to its
    // member rows — every member's `isOverride` read consults the same
    // table (the official gathers `members` per container element; the
    // same-file approximation). Extensions and extension types pass
    // `containerElement: null` upstream (:135-149) — `getOverridden`
    // never answers for their members, so the chain read joins
    // `implements` links only for the class-like containers.
    let joins_implements = !matches!(
        c.kind(row),
        NodeKind::ExtensionDeclaration | NodeKind::ExtensionTypeDeclaration
    );
    let ancestor_members: Vec<u32> = same_file_ancestors(c, row, !joins_implements)
        .iter()
        .flat_map(|&ancestor| member_rows(c, ancestor))
        .collect();
    for member in member_rows(c, row) {
        match c.kind(member) {
            NodeKind::Constructor | NodeKind::Factory => {
                // An enum's constructors never gather (upstream
                // `member.parent?.parent is! EnumDeclaration`), and a
                // dead container's executables suppress into its own
                // report (the enclosing-Declaration gate above).
                if is_enum || is_dead {
                    continue;
                }
                judge_constructor(c, face, graph, arms, single_file, member, &container);
            }
            NodeKind::Fields => {
                for variable in c.collect_direct(member) {
                    if c.kind(variable) != NodeKind::VariablesDeclaration {
                        continue;
                    }
                    for var in c.collect_direct(variable) {
                        if c.kind(var) != NodeKind::InitializedIdentifier {
                            continue;
                        }
                        let Some(name) = initialized_identifier_name(c, var) else {
                            continue;
                        };
                        // The `isOverride` arm (:101-116): a field
                        // whose getter resolves against an ancestor
                        // instance member never gathers. The
                        // getter/setter face reads the field's own
                        // name — upstream keys on `element.getter`,
                        // the accessor the variable carries.
                        if member_is_override(
                            c,
                            row,
                            &ancestor_members,
                            joins_implements,
                            c.lexeme(name),
                            MemberQuery::Getter,
                        ) {
                            continue;
                        }
                        judge(
                            c,
                            face,
                            graph,
                            arms,
                            single_file,
                            member,
                            member,
                            Shape::Other,
                            name,
                            "",
                            c.lexeme(name),
                            c.lexeme(name),
                            c.lexeme(name),
                        );
                    }
                }
            }
            NodeKind::Method => {
                // A dead container's executables suppress into its own
                // report (the enclosing-Declaration gate above).
                if is_dead {
                    continue;
                }
                let Some(name) = member_name(c, member) else {
                    continue;
                };
                // An operator names through its `OperatorName` payload
                // token (the head would hit the `operator` keyword).
                let name = if c.kind(name) == NodeKind::OperatorName {
                    c.field(name, PayloadField::Name)
                        .unwrap_or_else(|| c.head_token(name))
                } else {
                    c.head_token(name)
                };
                let lexeme = c.lexeme(name);
                // The test-harness names never gather
                // (`isTestMethod`).
                if is_test_name(lexeme) {
                    continue;
                }
                let is_static = dartq_ast::mod_flags(c.unit.ast, member) & STATIC_BIT != 0;
                // The `toJson` arm (:293-303): a non-static `toJson`
                // is dynamically invoked by `dart:convert`, so it is
                // automatically reachable — never a candidate.
                if !is_static && lexeme == "toJson" {
                    continue;
                }
                // The `isOverride` arm (:147): the member's own accessor
                // shape queries the table — a getter's element IS the
                // accessor (upstream `getOverridden(Name.forElement)`),
                // so an overriding getter resolves against a base
                // getter, an overriding setter against a base setter.
                let override_query = match get_set_token(c, member) {
                    Some("get") => MemberQuery::Getter,
                    Some(_) => MemberQuery::Setter,
                    None => MemberQuery::Method,
                };
                if member_is_override(
                    c,
                    row,
                    &ancestor_members,
                    joins_implements,
                    lexeme,
                    override_query,
                ) {
                    continue;
                }
                judge(
                    c,
                    face,
                    graph,
                    arms,
                    single_file,
                    member,
                    member,
                    Shape::Method(is_static),
                    name,
                    "",
                    lexeme,
                    lexeme,
                    lexeme,
                );
            }
            _ => {}
        }
    }
}

/// The `isOverride` arm (:101-116, `getOverridden`): whether `name` in
/// the accessor shape of `query` resolves against an ancestor's
/// INSTANCE member table — upstream's interface map carries no static
/// members, so a static ancestor declaration never suppresses. The
/// same-file ancestors answer name-level; the cross-file remainder
/// anchors the semantic bridge's inheritance walk (every written
/// supertype clause, abstract members included — the concrete cut is
/// off), gated on the InterfaceElement containers only (extensions and
/// extension types pass `containerElement: null` upstream). An
/// unmounted bridge or an unresolved chain answers through the
/// same-file arm alone (the documented under-reach posture).
fn member_is_override(
    c: &Check<'_>,
    container_row: u32,
    same_file: &[u32],
    bridge_mounted_container: bool,
    name: &str,
    query: MemberQuery,
) -> bool {
    if same_file.iter().any(|&member| {
        dartq_ast::mod_flags(c.unit.ast, member) & STATIC_BIT == 0
            && !member_matches(c, member, query, name).is_empty()
    }) {
        return true;
    }
    bridge_mounted_container && bridge_is_override(c, container_row, name, query)
}

/// The cross-file half of the override read: the semantic bridge's
/// inheritance walk on the accessor-spelled name (`name=` keys a
/// setter in the model; getters, methods, and field getters keep the
/// bare spelling — upstream `Name.forElement`/`Name.forLibrary`). A
/// static hit is no hit (the instance-only interface map).
fn bridge_is_override(c: &Check<'_>, container_row: u32, name: &str, query: MemberQuery) -> bool {
    let spelled = match query {
        MemberQuery::Setter => format!("{name}="),
        _ => name.to_owned(),
    };
    super::bridge_inherited_member(c, container_row, false, &spelled)
        .is_some_and(|(_, entry)| !entry.is_static)
}

/// Judges one constructor: the display name is `Type.written`, the
/// unnamed face `Type.new` (upstream `nameForError`'s
/// `self.name?.lexeme ?? 'new'`), anchored at the name token. A
/// `new`-prefixed declaration (`new named();`) carries no
/// method-continuation name — its row head IS the written name.
fn judge_constructor(
    c: &mut Check<'_>,
    face: &ReachabilityFace<'_>,
    graph: &ReferenceGraph,
    arms: &CtorArms,
    single_file: bool,
    row: u32,
    container: &str,
) {
    let written = constructor_name_node(c, row);
    let (name_token, member) = match written {
        Some(node) => {
            let lexeme = c.lexeme(c.head_token(node));
            if lexeme == container {
                // The unnamed face: the row anchors at the class-name
                // echo.
                (c.head_token(node), "new")
            } else {
                (c.head_token(node), lexeme)
            }
        }
        None => {
            let head = c.head_token(row);
            let lexeme = c.lexeme(head);
            // The row head is the class-name echo for the unnamed face
            // and the written name for a `new`-prefixed declaration.
            (head, if lexeme == container { "new" } else { lexeme })
        }
    };
    let display = format!("{container}.{member}");
    judge(
        c,
        face,
        graph,
        arms,
        single_file,
        row,
        row,
        Shape::Constructor,
        name_token,
        container,
        member,
        &display,
        &display,
    );
}

/// The five judgment gates, in the official order, then the report.
/// `wrapper` is the candidate's top-level wrapper (top-level rows);
/// `annotation_row` carries the metadata (the declaration row, or the
/// field row a variable rides). `name_token` is the candidate's name
/// TOKEN id — the face flags key at name-token UTF-16 offsets (the
/// capture row's `range`), so every caller anchors at a token (via
/// `head_token` where the name is a node), never at a node id: node
/// ids and token ids are separate id spaces, and an arena row past
/// the token table panics the lookup (#1729).
#[expect(clippy::too_many_arguments)] // mirrors the official five-gate walk's inputs; a struct would only rename
fn judge(
    c: &mut Check<'_>,
    face: &ReachabilityFace<'_>,
    graph: &ReferenceGraph,
    arms: &CtorArms,
    single_file: bool,
    wrapper: u32,
    annotation_row: u32,
    shape: Shape,
    name_token: u32,
    container: &str,
    public: &str,
    counted: &str,
    display: &str,
) {
    // 0. Entry point (`_isEntryPoint`: a function declaration named
    // `main` or carrying the pragma / widget-preview annotation) — an
    // entry is reachable by construction, never a candidate.
    if graph.entries.contains(public) || graph.entries.contains(counted) {
        return;
    }
    // 1. Unreachable (the face's flag; `None` — outside the face's
    // universe — never reports). The local override: a `Some(true)`
    // flag a single-file library's entry-reach closure contradicts
    // reads unreachable (the engine graph is workspace-global and
    // counts every type reference; the official walk is per-library
    // with the `visitNamedType` filter).
    let offset = utf16_token_span(c, name_token).start;
    // 1.5. Counted use (the anti-reach arm): a candidate the face's
    // under-reach flagged unreachable but whose name has a same-file
    // counting position reads as reachable — a plain type reference or
    // a pattern name never counts (upstream's visitor split), an
    // expression or constructor use does. Constructors count under the
    // qualified `C.name` key AND the bare member (a `: this.one()`
    // redirect writes `one`, never `C.one`). Single-file libraries
    // never take this arm — the local closure (the effective verdict)
    // already answers, and a reference inside an unreachable
    // declaration's body must NOT rescue (the official walk propagates
    // only from reachable declarations).
    let counted_count = graph.uses.get(counted).copied().unwrap_or(0)
        + if counted == public {
            0
        } else {
            graph.uses.get(public).copied().unwrap_or(0)
        };
    let verdict = effective_verdict(
        c,
        face,
        graph,
        single_file,
        annotation_row,
        name_token,
        offset,
    );
    match verdict {
        Some(false) if !single_file => {
            if counted_count > 0 {
                return;
            }
        }
        Some(false) => {}
        // The face's universe is the capture's declaration rows, and
        // constructors are not declaration rows (`dartq-index` capture,
        // the constructor exclusion): a real-pipeline constructor rides
        // its class's offset (the unnamed echo) or answers no verdict
        // (the named face) — never its own. The constructor's own
        // reference face decides instead (upstream
        // `unreachable_from_main.dart` marks a constructor exactly when
        // a reference or a marking arm reaches it): a counting position
        // (`C()`, `C.named`, `: this.x()`, `= C.x`, `super.x()`) or one
        // of the marking arms the counting index cannot spell keeps it
        // silent; otherwise it reports.
        _ => {
            if shape != Shape::Constructor
                || ctor_self_reachable(c, face, &graph.uses, arms, container, public)
            {
                return;
            }
        }
    }
    // 2. Public (`element.isPublic` — the written declaration name; a
    // constructor's display carries the `Type.` prefix, so the public
    // test reads the member name itself).
    if public.starts_with('_') {
        return;
    }
    // 5. Outermost container wins — a dead container's executables never
    // reach this far (the caller's member-loop gates); its fields do
    // (upstream's enclosing-Declaration read never suppresses a field).
    // 3. Not `@visibleForTesting` (`element.hasVisibleForTesting`).
    if carries_annotation(c, wrapper, annotation_row, "visibleForTesting") {
        return;
    }
    // 4. Not a public widget-preview member (`hasWidgetPreview`:
    // constructors, top-level functions, and static executables — the
    // annotation must RESOLVE into the flutter widget-previews
    // library; a unit-declared `Preview` class or a bare spelling with
    // no flutter import does not suppress).
    let previewable = match shape {
        Shape::TopLevelFunction | Shape::Constructor => true,
        Shape::Method(is_static) => is_static,
        Shape::Other => false,
    };
    if previewable
        && (carries_annotation(c, wrapper, annotation_row, "Preview") && graph.preview_resolves
            || graph
                .preview_aliases
                .iter()
                .any(|alias| carries_annotation(c, wrapper, annotation_row, alias)))
    {
        return;
    }
    c.report(utf16_token_span(c, name_token), &[display]);
}

/// The reach verdict over one name offset: a single-file library
/// answers through the local entry-reach closure alone — the official
/// walk IS per-library, so the closure is authoritative in both
/// directions (the engine flag's under-reach, a missed edge, and its
/// over-reach — a type annotation, a constant pattern, another
/// library's reference — both fold to the closure's answer). A
/// part-carrying library (the same-file index incomplete) keeps the
/// face flag with the counted-use rescue. `declaration_row` locates
/// the enclosing declaration for the closure's member keys — the
/// container whose members ride the class flag, the member whose own
/// flag the face answers directly.
fn effective_verdict(
    c: &Check<'_>,
    face: &ReachabilityFace<'_>,
    graph: &ReferenceGraph,
    single_file: bool,
    declaration_row: u32,
    name_token: u32,
    offset: u32,
) -> Option<bool> {
    if single_file {
        return Some(locally_used(c, graph, declaration_row, name_token));
    }
    face.is_reachable(offset)
}

/// Whether the entry-reach closure marks the candidate named at
/// `name_token` (in `declaration_row`) used — the candidate's own keys
/// (its name, or a member's bare and container-qualified keys) plus
/// nothing else: a member kept alive only through its container's own
/// reference still reports (the official marks the container, not the
/// member).
fn locally_used(
    c: &Check<'_>,
    graph: &ReferenceGraph,
    declaration_row: u32,
    name_token: u32,
) -> bool {
    let name = c.lexeme(name_token);
    if graph.used.contains(name) {
        return true;
    }
    // A member or constructor: the container-qualified key too — a
    // receiver-resolved continuation (`c.m()` with `var c = C()`)
    // marks `C.m`, a redirect or tear-off `C.new`.
    if matches!(
        c.kind(declaration_row),
        NodeKind::Constructor | NodeKind::Factory | NodeKind::Method | NodeKind::Fields
    ) && let Some(container) = enclosing_container_name(c, declaration_row)
    {
        let member = if name == container.as_str() {
            "new"
        } else {
            name
        };
        return graph.used.contains(member)
            || graph
                .used
                .contains(format!("{container}.{member}").as_str());
    }
    false
}

/// The name of the class-like container enclosing `row`, when one does.
fn enclosing_container_name(c: &Check<'_>, row: u32) -> Option<String> {
    let mut current = row;
    while let Some(parent) = c.unit.ast.parent(current) {
        if matches!(
            c.kind(parent),
            NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::ExtensionDeclaration
                | NodeKind::ExtensionTypeDeclaration
                | NodeKind::EnumDeclaration
        ) {
            return container_name_token(c, parent).map(|token| c.lexeme(token).to_owned());
        }
        current = parent;
    }
    None
}

/// The same-file class table the constructor marking arms walk. The
/// capture universe carries no constructor rows, so the real face never
/// gives a constructor its own verdict — this table supplies the
/// upstream marking arms (`unreachable_from_main.dart`) the counting
/// index cannot spell.
struct CtorArms {
    classes: Vec<CtorClass>,
}

/// One same-file class's constructor-arm facts.
struct CtorClass {
    /// The class row (annotation and member lookups).
    row: u32,
    /// The written class name.
    name: String,
    /// The `extends` clause's supertype name, when written (the upstream
    /// `supertype` — the arms walk `extends` only: `with`/`on` have no
    /// super constructor, an `implements` name is no supertype).
    extends: Option<String>,
    /// Whether the class declares any explicit constructor/factory (the
    /// default-constructor arm's gate).
    has_explicit_ctor: bool,
    /// `@reflectiveTest` (the test-harness instantiation arm
    /// `unreachable_from_main.dart` :220-231 — marks the unnamed
    /// constructor).
    reflective_test: bool,
    /// The explicit constructors: `(member name, unnamed, super shape)`.
    ctors: Vec<(String, bool, Option<SuperInit>)>,
}

/// The super-invocation shape of one explicit constructor's initializer
/// list (`unreachable_from_main.dart` :250-253 — `hasSuperInitializer`
/// keys the implicit-super arm; :400-406 — an unnamed `super()` marks
/// the supertype's unnamed constructor).
#[derive(Clone, Copy, PartialEq, Eq)]
enum SuperInit {
    /// `super.name(...)` — marks the supertype's NAMED constructor (the
    /// counting index covers it through the `name` continuation).
    Named,
    /// `super(...)` — marks the supertype's UNNAMED constructor.
    Unnamed,
}

/// Builds the same-file class table: one pass over the unit's class
/// declarations. Enum/mixin/extension containers carry no constructor
/// arms (the official gatherer adds an enum's constructors never, and
/// mixins/extensions have no super constructor).
fn ctor_arms(c: &Check<'_>) -> CtorArms {
    let mut classes = Vec::new();
    let Some(root) = c.unit.ast.root() else {
        return CtorArms { classes };
    };
    for wrapper in c.children(root) {
        if c.kind(wrapper) != NodeKind::TopLevelDeclaration {
            continue;
        }
        for row in declaration_rows(c, wrapper) {
            if c.kind(row) != NodeKind::ClassDeclaration {
                continue;
            }
            let Some(name_token) = container_name_token(c, row) else {
                continue;
            };
            let extends = supertypes(c, row)
                .into_iter()
                .find(|&(_, clause)| clause == SupertypeClause::Extends)
                .map(|(name, _)| name.to_owned());
            let mut ctors = Vec::new();
            for member in member_rows(c, row) {
                if !matches!(c.kind(member), NodeKind::Constructor | NodeKind::Factory) {
                    continue;
                }
                let written = constructor_name_node(c, member);
                let (member_name, unnamed) = match written {
                    Some(node) => {
                        let lexeme = c.lexeme(c.head_token(node));
                        (lexeme.to_owned(), lexeme == c.lexeme(name_token))
                    }
                    None => ("new".to_owned(), true),
                };
                ctors.push((member_name, unnamed, super_init_shape(c, member)));
            }
            classes.push(CtorClass {
                row,
                name: c.lexeme(name_token).to_owned(),
                extends,
                has_explicit_ctor: !ctors.is_empty(),
                reflective_test: carries_annotation(c, wrapper, row, "reflectiveTest"),
                ctors,
            });
        }
    }
    CtorArms { classes }
}

/// The super-invocation shape of a constructor's initializer list: the
/// row-less `super` token whose follower is `(` names the unnamed face,
/// `.` the named one. The scan rides the `Initializers` row when the
/// declaration carries one — a body's `super.foo()` member read is not
/// an initializer (`unreachable_from_main.dart` :251-253 reads
/// `node.initializers`).
fn super_init_shape(c: &Check<'_>, ctor: u32) -> Option<SuperInit> {
    let initializers = c
        .collect_direct(ctor)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Initializers)?;
    let last = c.unit.tokens.len().saturating_sub(1) as u32;
    let mut named = false;
    for token in c.unit.ast.token_range(initializers) {
        if c.lexeme(token) == "super" {
            match c.lexeme((token + 1).min(last)) {
                "(" => return Some(SuperInit::Unnamed),
                "." => named = true,
                _ => {}
            }
        }
    }
    named.then_some(SuperInit::Named)
}

/// Whether the constructor `{container}.{member}` has its own reach
/// beyond the face verdict — a counting position (`C()`, `C.named`, a
/// `: this.x()` redirect, an `= C.x` factory redirect, a `super.x()`
/// call — the judge's counted keys), the `@reflectiveTest` unnamed
/// arm, or the subclass walk (`marked_unnamed`).
fn ctor_self_reachable(
    c: &Check<'_>,
    face: &ReachabilityFace<'_>,
    uses: &rustc_hash::FxHashMap<String, u32>,
    arms: &CtorArms,
    container: &str,
    member: &str,
) -> bool {
    if ctor_counted(uses, container, member) {
        return true;
    }
    let Some(class) = arms.classes.iter().find(|class| class.name == container) else {
        return false;
    };
    // The reflectiveTest arm marks the UNNAMED constructor; the
    // implicit-super arms mark the unnamed face only (upstream seeds
    // `firstWhereOrNull((e) => e.name == 'new')`). A named constructor
    // has no arm beyond its references.
    if member != "new" {
        return false;
    }
    if class.reflective_test {
        return true;
    }
    marked_unnamed(c, face, uses, arms, container, &mut Vec::new())
}

/// Whether the UNNAMED constructor of `name` is walked by the upstream
/// traversal: a direct reference (`C()`, `C.new`), the reflectiveTest
/// instantiation, or a same-file `extends` subclass whose walked
/// constructor carries an implicit or unnamed-super initial super
/// step (`unreachable_from_main.dart` :199-215 default-constructor
/// arm, :247-259 implicit-super arm, :400-406 unnamed-super arm). The
/// recursion covers default-constructor chains (a subclass walked
/// through its own subclass marks upward), bounded by the same-file
/// class count with a visiting guard (an `extends` cycle is a compile
/// error, the guard is the termination invariant).
fn marked_unnamed(
    c: &Check<'_>,
    face: &ReachabilityFace<'_>,
    uses: &rustc_hash::FxHashMap<String, u32>,
    arms: &CtorArms,
    name: &str,
    visiting: &mut Vec<String>,
) -> bool {
    if ctor_counted(uses, name, "new") {
        return true;
    }
    if visiting.iter().any(|seen| seen == name) {
        return false;
    }
    let Some(class) = arms.classes.iter().find(|class| class.name == name) else {
        return false;
    };
    if class.reflective_test {
        return true;
    }
    visiting.push(name.to_owned());
    let walked = arms.classes.iter().any(|subclass| {
        subclass.extends.as_deref() == Some(name)
            && subclass_walks_super(c, face, uses, arms, subclass, visiting)
    });
    visiting.pop();
    walked
}

/// Whether some constructor walk of `subclass` marks its supertype's
/// unnamed constructor: the class itself walked with no explicit
/// constructor (the default constructor's implicit super-initializer),
/// or a walked explicit constructor carrying no super-initializer (the
/// implicit face) or an unnamed `super()` (the explicit unnamed face).
fn subclass_walks_super(
    c: &Check<'_>,
    face: &ReachabilityFace<'_>,
    uses: &rustc_hash::FxHashMap<String, u32>,
    arms: &CtorArms,
    subclass: &CtorClass,
    visiting: &mut Vec<String>,
) -> bool {
    if !subclass.has_explicit_ctor {
        // The default constructor walks when the class declaration
        // walks: the engine's class flag (any reference marks it — a
        // supertype clause or an instantiation alike), or a counted
        // use.
        let walked_by_flag = container_name_token(c, subclass.row)
            .map(|token| face.is_reachable(utf16_token_span(c, token).start) == Some(true))
            .unwrap_or(false);
        return walked_by_flag || uses.get(&subclass.name).copied().unwrap_or(0) > 0;
    }
    subclass.ctors.iter().any(|(member, unnamed, super_init)| {
        let walked = ctor_counted(uses, &subclass.name, member)
            || (*unnamed && marked_unnamed(c, face, uses, arms, &subclass.name, visiting));
        walked && *super_init != Some(SuperInit::Named)
    })
}

/// The counted positions of one constructor: the qualified
/// `{container}.{member}` key and the bare member key.
fn ctor_counted(uses: &rustc_hash::FxHashMap<String, u32>, container: &str, member: &str) -> bool {
    uses.get(&format!("{container}.{member}"))
        .copied()
        .unwrap_or(0)
        > 0
        || uses.get(member).copied().unwrap_or(0) > 0
}

/// The declaration rows of one top-level wrapper (the name and
/// recovery rows excluded).
fn declaration_rows(c: &Check<'_>, wrapper: u32) -> Vec<u32> {
    c.collect_direct(wrapper)
        .into_iter()
        .filter(|&row| {
            matches!(
                c.kind(row),
                NodeKind::TopLevelMethod
                    | NodeKind::TopLevelFields
                    | NodeKind::ClassDeclaration
                    | NodeKind::MixinDeclaration
                    | NodeKind::ExtensionDeclaration
                    | NodeKind::ExtensionTypeDeclaration
                    | NodeKind::EnumDeclaration
                    | NodeKind::Typedef
            )
        })
        .collect()
}

/// The directive rows of the unit — every `Import`/`Export`/`Part`/
/// `PartOf` row, wherever the bridge tiles it: directives sit behind a
/// `TopLevelDeclaration` wrapper beside their `MetadataStar` sibling
/// (`behind_prelude`'s descent), so both the bare and the wrapped face
/// are admitted.
fn directive_rows(c: &Check<'_>, unit: u32) -> Vec<u32> {
    fn is_directive(kind: NodeKind) -> bool {
        matches!(
            kind,
            NodeKind::Import | NodeKind::Export | NodeKind::Part | NodeKind::PartOf
        )
    }
    let mut rows = Vec::new();
    for kid in c.collect_direct(unit) {
        if is_directive(c.kind(kid)) {
            rows.push(kid);
        } else if c.kind(kid) == NodeKind::TopLevelDeclaration {
            rows.extend(
                c.collect_direct(kid)
                    .into_iter()
                    .filter(|&row| is_directive(c.kind(row))),
            );
        }
    }
    rows
}

/// The member rows of a container (its `DeclarationBody`/`EnumBody`
/// children).
fn member_rows(c: &Check<'_>, container: u32) -> Vec<u32> {
    let body = c
        .collect_direct(container)
        .into_iter()
        .find(|&kid| matches!(c.kind(kid), NodeKind::DeclarationBody | NodeKind::EnumBody));
    match body {
        Some(body) => rows_with_parent(c, body),
        None => Vec::new(),
    }
}

/// The name TOKEN of a declaration: a name-bearing direct child
/// (methods, operators, typedefs — an `OperatorName` names through its
/// payload token, the head would hit the `operator` keyword; the
/// identifier read is context-filtered, so a return-type identifier —
/// `int get g` — never answers for the name), else the production
/// wrapper's name row (the class-like left-sibling placement), else
/// [`container_name_token`].
fn name_token_of(c: &Check<'_>, wrapper: u32, row: u32) -> Option<u32> {
    use dartq_parser::IdentifierContext as Ctx;
    const NAME_CONTEXTS: &[u32] = &[
        Ctx::MethodDeclaration as u32,
        Ctx::TopLevelFunctionDeclaration as u32,
        Ctx::TypedefDeclaration as u32,
        // Hand-built test tables stamp every identifier with the
        // generic `Expression` word (the `member_name` posture).
        Ctx::Expression as u32,
    ];
    if let Some(name) = c
        .collect_direct(row)
        .into_iter()
        .find(|&kid| match c.kind(kid) {
            NodeKind::OperatorName => true,
            NodeKind::Identifier => c
                .field(kid, PayloadField::Context)
                .is_some_and(|ctx| NAME_CONTEXTS.contains(&ctx)),
            _ => false,
        })
    {
        return Some(if c.kind(name) == NodeKind::OperatorName {
            c.field(name, PayloadField::Name)
                .unwrap_or_else(|| c.head_token(name))
        } else {
            c.head_token(name)
        });
    }
    let parent = c.unit.ast.parent(wrapper).unwrap_or(wrapper);
    let siblings = c.collect_direct(parent);
    let pos = siblings.iter().position(|&kid| kid == wrapper)?;
    siblings[..pos]
        .iter()
        .rev()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::Identifier)
        .map(|name| c.head_token(name))
        .or_else(|| container_name_token(c, row))
}

/// The written constructor name: the identifier in the
/// method-continuation context (`unnecessary_constructor_name`'s
/// reading), else the plain method-declaration identifier — a
/// `new`-prefixed declaration (`new named();`) names through its
/// identifier. An unnamed constructor's class-name echo carries the
/// same contexts; the caller compares against the container name.
fn constructor_name_node(c: &Check<'_>, row: u32) -> Option<u32> {
    let cont = dartq_parser::IdentifierContext::MethodDeclarationContinuation as u32;
    let decl = dartq_parser::IdentifierContext::MethodDeclaration as u32;
    let mut fallback = None;
    c.collect_direct(row)
        .into_iter()
        .find(|&kid| {
            if c.kind(kid) != NodeKind::Identifier {
                return false;
            }
            match c.field(kid, PayloadField::Context) {
                Some(ctx) if ctx == cont => true,
                Some(ctx) if ctx == decl && fallback.is_none() => {
                    fallback = Some(kid);
                    false
                }
                _ => false,
            }
        })
        .or(fallback)
}

/// The test-harness method names (`isTestMethod`).
fn is_test_name(name: &str) -> bool {
    name.starts_with("test_")
        || name.starts_with("solo_test_")
        || matches!(name, "setUp" | "tearDown" | "setUpClass" | "tearDownClass")
}

/// Whether the row carries an annotation named `name`, by spelling:
/// the `MetadataStar` left of the annotation row under its parent (the
/// member placement), or the star left of the top-level wrapper (the
/// production top-level placement).
fn carries_annotation(c: &Check<'_>, wrapper: u32, row: u32, name: &str) -> bool {
    let names = |star: u32| {
        c.collect_direct(star).into_iter().any(|meta| {
            c.kind(meta) == NodeKind::Metadata
                && c.collect_direct(meta).into_iter().any(|piece| {
                    c.kind(piece) == NodeKind::Identifier && c.lexeme(c.head_token(piece)) == name
                })
        })
    };
    let mut holders = [Some(row), None];
    if c.kind(wrapper) == NodeKind::TopLevelDeclaration {
        holders[1] = Some(wrapper);
    }
    for holder in holders.into_iter().flatten() {
        let Some(parent) = c.unit.ast.parent(holder) else {
            continue;
        };
        let siblings = c.collect_direct(parent);
        let Some(pos) = siblings.iter().position(|&kid| kid == holder) else {
            continue;
        };
        if let Some(&star) = siblings[..pos]
            .iter()
            .rev()
            .find(|&&kid| c.kind(kid) == NodeKind::MetadataStar)
            && names(star)
        {
            return true;
        }
    }
    false
}

/// The `static` `ModSet` bit (bridge `mod_bits`, member family).
const STATIC_BIT: u16 = 1 << 3;

/// The name-level reference graph of one unit — the official
/// per-library walk re-derived shell-side (upstream Dart SDK source,
/// `pkg/linter/lib/src/rules/unreachable_from_main.dart`):
///
/// * `uses` — the flat name → counting-position index: every reference
///   key the official visitor counts (the `visitSimpleIdentifier`
///   expression reads, `visitNamedType`'s exception list — typedef
///   aliases, extension types, type arguments — `visitClassDeclaration`'s
///   unconditional clause names, `visitAnnotation`, the operator visits,
///   `visitRedirectingConstructorInvocation`, and the doc-comment
///   `[Name]` face). The anti-reach arm (gate 1.5) reads it.
/// * `entries` — the entry seeds (`_isEntryPoint`: functions named
///   `main`, or carrying `@pragma('vm:entry-point')` — const aliases
///   followed through `computeConstantValue` — or a widget-preview
///   annotation that resolves into the flutter library).
/// * `used` — the entry-reach closure over the per-declaration
///   dependency edges (a reference credits its enclosing member and the
///   enclosing container — upstream walks each declaration's subtree, so
///   a class's dep set covers its member bodies — `_addDeclaration`'s
///   enclosing-top-level double marking; the worklist walk folds them
///   into `used` at build time).
struct ReferenceGraph {
    uses: FxHashMap<String, u32>,
    entries: FxHashSet<String>,
    used: FxHashSet<String>,
    /// Whether a `Preview` annotation can resolve into the flutter
    /// widget-previews library here (the `widget_previews.dart` import
    /// surface, unshadowed by a unit-declared `Preview` class).
    preview_resolves: bool,
    /// The const aliases of the resolved flutter `Preview`
    /// (`const preview = Preview();` — `@preview` follows the alias).
    preview_aliases: Vec<String>,
}

impl ReferenceGraph {
    fn build(c: &Check<'_>) -> ReferenceGraph {
        use dartq_parser::IdentifierContext as Ctx;
        const EXPRESSION: u32 = Ctx::Expression as u32;
        const EXPRESSION_CONT: u32 = Ctx::ExpressionContinuation as u32;
        const CTOR: u32 = Ctx::ConstructorReference as u32;
        const CTOR_CONT: u32 = Ctx::ConstructorReferenceContinuation as u32;
        const CTOR_CONT_ARGS: u32 = Ctx::ConstructorReferenceContinuationAfterTypeArguments as u32;
        const PATTERN_FIELD: u32 = Ctx::PatternFieldName as u32;
        const TYPE_REF: u32 = Ctx::TypeReference as u32;
        const META_REF: u32 = Ctx::MetadataReference as u32;
        const META_CONT: u32 = Ctx::MetadataContinuation as u32;
        let last = c.unit.tokens.len().saturating_sub(1) as u32;

        // The declaration pre-scan: containers, the alias-like type
        // names (`visitNamedType` counts every reference to a typedef
        // or an extension type), the member-name → container table
        // (`_addDeclaration`'s enclosing-top-level arm), the flutter
        // import surface, and the const annotation aliases.
        let mut alias_like: FxHashSet<&str> = FxHashSet::default();
        let mut alias_rhs: FxHashMap<String, String> = FxHashMap::default();
        let mut containers: FxHashSet<&str> = FxHashSet::default();
        let mut member_containers: FxHashMap<String, String> = FxHashMap::default();
        let mut import_uris: Vec<&str> = Vec::new();
        let mut declares_preview = false;
        let mut pragma_aliases: Vec<String> = Vec::new();
        let mut preview_aliases: Vec<String> = Vec::new();
        let mut entries: FxHashSet<String> = FxHashSet::default();
        let mut uses: FxHashMap<String, u32> = FxHashMap::default();
        let mut deps: FxHashMap<String, Vec<String>> = FxHashMap::default();
        let Some(root) = c.unit.ast.root() else {
            return ReferenceGraph {
                uses,
                entries,
                used: FxHashSet::default(),
                preview_resolves: false,
                preview_aliases,
            };
        };
        let wrappers: Vec<u32> = c
            .children(root)
            .into_iter()
            .filter(|&row| c.kind(row) == NodeKind::TopLevelDeclaration)
            .collect();
        for wrapper in &wrappers {
            for row in declaration_rows(c, *wrapper) {
                match c.kind(row) {
                    NodeKind::Typedef => {
                        if let Some(name) = name_token_of(c, *wrapper, row) {
                            alias_like.insert(c.lexeme(name));
                            // `typedef A = B;` — an alias invocation
                            // (`A()`, `A.named()`) resolves through to B's
                            // constructor element; the row pass expands the
                            // credited keys through this edge.
                            if let Some(head) = typedef_rhs_class(c, row) {
                                alias_rhs.insert(c.lexeme(name).to_owned(), head.to_owned());
                            }
                        }
                    }
                    NodeKind::ExtensionTypeDeclaration => {
                        if let Some(name) = container_name_token(c, row) {
                            alias_like.insert(c.lexeme(name));
                            containers.insert(c.lexeme(name));
                            let container = c.lexeme(name).to_owned();
                            credit_member_names(c, row, &container, &mut member_containers);
                        }
                    }
                    NodeKind::ClassDeclaration
                    | NodeKind::MixinDeclaration
                    | NodeKind::ExtensionDeclaration
                    | NodeKind::EnumDeclaration => {
                        if let Some(name) = container_name_token(c, row) {
                            if c.lexeme(name) == "Preview" {
                                declares_preview = true;
                            }
                            let container = c.lexeme(name).to_owned();
                            containers.insert(c.lexeme(name));
                            credit_member_names(c, row, &container, &mut member_containers);
                        }
                    }
                    NodeKind::TopLevelMethod => {
                        // `_isEntryPoint`: a function (or accessor) named
                        // `main`, or one carrying an entry annotation —
                        // read after the alias scan below completes.
                        if let Some(name) = member_name(c, row) {
                            if c.lexeme(c.head_token(name)) == "main" {
                                entries.insert("main".to_owned());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        for row in directive_rows(c, root) {
            if c.kind(row) != NodeKind::Import {
                continue;
            }
            for kid in c.collect_direct(row) {
                if c.kind(kid) == NodeKind::StringLiteral {
                    import_uris.push(unquote(c.lexeme(c.head_token(kid))));
                }
            }
        }
        let preview_resolves = import_uris
            .iter()
            .any(|uri| uri.contains("widget_previews"))
            && !declares_preview;
        // The const aliases (`const entryPoint = pragma('vm:entry-point')`,
        // `const preview = Preview()`): `_isExemptingAnnotation` follows
        // them through `computeConstantValue`, so `@entryPoint` reads
        // as the pragma and `@preview` as the widget preview.
        for wrapper in &wrappers {
            for row in declaration_rows(c, *wrapper) {
                if c.kind(row) != NodeKind::TopLevelFields {
                    continue;
                }
                for variable in c.collect_direct(row) {
                    if c.kind(variable) != NodeKind::VariablesDeclaration
                        || variables_declaration_keyword(c, variable)
                            .is_none_or(|kw| c.lexeme(kw) != "const")
                    {
                        continue;
                    }
                    for var in c.collect_direct(variable) {
                        let Some(name) = (c.kind(var) == NodeKind::InitializedIdentifier)
                            .then(|| initialized_identifier_name(c, var))
                            .flatten()
                        else {
                            continue;
                        };
                        let Some(init) = c
                            .collect_direct(var)
                            .into_iter()
                            .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer)
                        else {
                            continue;
                        };
                        let kids = c.collect_direct(init);
                        let Some(head) = kids
                            .iter()
                            .find(|&&kid| c.kind(kid) == NodeKind::Identifier)
                            .map(|&kid| c.head_token(kid))
                        else {
                            continue;
                        };
                        match c.lexeme(head) {
                            "pragma"
                                if first_string_argument(c, &kids) == Some("vm:entry-point") =>
                            {
                                pragma_aliases.push(c.lexeme(name).to_owned());
                            }
                            "Preview" if preview_resolves && has_arguments(c, &kids) => {
                                preview_aliases.push(c.lexeme(name).to_owned());
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        // The entry annotations on functions: the direct pragma, the
        // aliases, and the resolved widget preview.
        for wrapper in &wrappers {
            for row in declaration_rows(c, *wrapper) {
                if c.kind(row) != NodeKind::TopLevelMethod {
                    continue;
                }
                let Some(name) = member_name(c, row) else {
                    continue;
                };
                let name = c.lexeme(c.head_token(name));
                if carries_pragma_entry(c, *wrapper, row)
                    || (carries_annotation(c, *wrapper, row, "Preview") && preview_resolves)
                    || pragma_aliases
                        .iter()
                        .chain(preview_aliases.iter())
                        .any(|alias| carries_annotation(c, *wrapper, row, alias))
                {
                    entries.insert(name.to_owned());
                }
            }
        }
        // The local receiver types: a variable initialized from a
        // same-file creation (`var c = C();`) or annotated with a
        // same-file container type names its container — a member
        // continuation on that receiver (`c.m()`) keys the QUALIFIED
        // member (`C.m`), the official's element resolution; an
        // unknown receiver keeps the bare member key (the conservative
        // name-level fallback).
        let mut receiver_types: FxHashMap<&str, &str> = FxHashMap::default();
        for row in c.children(root) {
            if c.kind(row) != NodeKind::InitializedIdentifier {
                continue;
            }
            let Some(name) = initialized_identifier_name(c, row) else {
                continue;
            };
            let kids = c.collect_direct(row);
            if let Some(&init) = kids
                .iter()
                .find(|&&kid| c.kind(kid) == NodeKind::VariableInitializer)
            {
                // The initializer's head identifier (a token read — the
                // creation's tiling never matters); `C(`, `C.named`,
                // `C<…>` head a same-file container's instance.
                for token in c.unit.ast.token_range(init) {
                    let lexeme = c.lexeme(token);
                    if lexeme == "=" || lexeme == "new" || lexeme == "const" {
                        continue;
                    }
                    if lexeme
                        .chars()
                        .next()
                        .is_none_or(|ch| !(ch.is_alphabetic() || ch == '_'))
                    {
                        break;
                    }
                    let next = (token + 1).min(last);
                    if containers.contains(lexeme)
                        && next <= last
                        && matches!(c.lexeme(next), "(" | "." | "<")
                    {
                        receiver_types.insert(c.lexeme(name), lexeme);
                    }
                    break;
                }
            } else if let Some(&ty) = kids.iter().find(|&&kid| c.kind(kid) == NodeKind::Type) {
                let head = c.lexeme(c.head_token(ty));
                if containers.contains(head) {
                    receiver_types.insert(c.lexeme(name), head);
                }
            }
        }
        // The row pass: every reference key counts into `uses` and
        // credits the enclosing declarations' dependency edges.
        // `credit` folds one reference row: the closure keys into the
        // flat use index and the per-declaration dependency edges (the
        // attribution), `uses_extra` the flat-index-only keys a
        // receiver-resolved continuation carries beside its qualified
        // closure key.
        let mut credit = |keys: &[String], uses_extra: &[String], row: u32| {
            count_keys(&mut uses, keys);
            count_keys(&mut uses, uses_extra);
            let mut attribution = attribution_keys(c, row);
            // `_addDeclaration`'s enclosing-top-level arm: a plain-name
            // reference to a member also marks the member's container.
            for key in keys {
                if !key.contains('.') {
                    if let Some(container) = member_containers.get(key) {
                        attribution.push(container.clone());
                    }
                }
            }
            for source in &attribution {
                deps.entry(source.clone())
                    .or_default()
                    .extend_from_slice(keys);
            }
        };
        let constant_patterns: Vec<core::ops::Range<u32>> = c
            .children(root)
            .into_iter()
            .filter(|&row| c.kind(row) == NodeKind::ConstantPattern)
            .map(|row| c.unit.ast.token_range(row))
            .collect();
        for row in c.children(root) {
            let mut keys: Vec<String> = Vec::new();
            let mut uses_extra: Vec<String> = Vec::new();
            match c.kind(row) {
                NodeKind::Identifier => {
                    let Some(ctx) = c.field(row, PayloadField::Context) else {
                        continue;
                    };
                    // Upstream's `_patternLevel` (:170, :237-243): a
                    // reference inside a `ConstantPattern` subtree
                    // never counts — the pattern-field name is exempt.
                    if ctx != PATTERN_FIELD
                        && inside_constant_pattern(&constant_patterns, c.head_token(row))
                    {
                        continue;
                    }
                    let lexeme = c.lexeme(c.head_token(row));
                    if ctx == EXPRESSION || ctx == EXPRESSION_CONT || ctx == PATTERN_FIELD {
                        // A member continuation on a locally typed
                        // receiver keys the qualified member for the
                        // closure (`c.m()` with `var c = C()` marks
                        // `C.m` — the official resolves the element; a
                        // bare key would mark every same-named member);
                        // the flat use index keeps the bare key too
                        // (`uses_extra` — the multi-file counted rescue
                        // reads it).
                        if ctx == EXPRESSION_CONT {
                            let start = c.unit.ast.token_range(row).start;
                            let resolved = (start >= 2 && c.lexeme(start - 1) == ".")
                                .then(|| receiver_types.get(c.lexeme(start - 2)))
                                .flatten();
                            if let Some(container) = resolved {
                                // The class key too — `_addDeclaration`
                                // marks the member's enclosing top-level.
                                keys.push(container.to_string());
                                keys.push(format!("{container}.{lexeme}"));
                                uses_extra.push(lexeme.to_owned());
                            } else {
                                keys.push(lexeme.to_owned());
                            }
                        } else {
                            keys.push(lexeme.to_owned());
                        }
                        // An invocation head resolves to the constructor
                        // it invokes: `(` counts the unnamed constructor,
                        // `.` the written member (`C()`, `C.named()`).
                        // No angle scan (a comparison's `<` would run it).
                        if ctx == EXPRESSION {
                            let next = c.unit.ast.token_range(row).end.min(last);
                            if next + 1 < c.unit.tokens.len() as u32 {
                                match c.lexeme(next) {
                                    "(" => {
                                        credit_ctor(&mut keys, &alias_rhs, lexeme, "new");
                                    }
                                    "." => credit_ctor(
                                        &mut keys,
                                        &alias_rhs,
                                        lexeme,
                                        c.lexeme(next + 1),
                                    ),
                                    _ => {}
                                }
                            }
                        }
                    } else if ctx == CTOR_CONT || ctx == CTOR_CONT_ARGS {
                        // A constructor-reference continuation counts
                        // its written member (`: this.one()` names `one`;
                        // the qualified key is the head arm's job).
                        keys.push(lexeme.to_owned());
                    } else if ctx == CTOR {
                        // A constructor head counts the class and its
                        // member (`C()` → `C`, `C.new`; `C<T>.two()` →
                        // `C`, `C.two`) — the balanced `<…>` scan walks
                        // the head's own type arguments.
                        keys.push(lexeme.to_owned());
                        let mut dot = c.unit.ast.token_range(row).end;
                        if dot <= last && c.lexeme(dot) == "<" {
                            let mut depth = 1i32;
                            while depth > 0 && dot < last {
                                dot += 1;
                                match c.lexeme(dot) {
                                    "<" | "<<" | "<<<" => depth += 1,
                                    ">" => depth -= 1,
                                    ">>" => depth -= 2,
                                    ">>>" => depth -= 3,
                                    _ => {}
                                }
                            }
                            // Past the closing bracket: the member
                            // continuation (`.two`) follows it.
                            dot = (dot + 1).min(last);
                        }
                        let member = if dot + 1 < c.unit.tokens.len() as u32 && c.lexeme(dot) == "."
                        {
                            c.lexeme(dot + 1).to_owned()
                        } else {
                            "new".to_owned()
                        };
                        credit_ctor(&mut keys, &alias_rhs, lexeme, &member);
                    } else if ctx == META_REF {
                        // `visitAnnotation`: the annotation's element —
                        // the class (`@C`), its constructor (`@C()`,
                        // `@C.named()`), or the const the alias names.
                        keys.push(lexeme.to_owned());
                        let next = c.unit.ast.token_range(row).end.min(last);
                        if next + 1 < c.unit.tokens.len() as u32 {
                            match c.lexeme(next) {
                                "(" => keys.push(format!("{lexeme}.new")),
                                "." => keys.push(format!("{lexeme}.{}", c.lexeme(next + 1))),
                                _ => {}
                            }
                        }
                    } else if ctx == META_CONT {
                        // The named-annotation member (`@C.named`).
                        keys.push(lexeme.to_owned());
                    } else if ctx == TYPE_REF
                        && (alias_like.contains(lexeme)
                            || in_type_argument(c, row)
                            || external_type_position(c, row))
                    {
                        // `visitNamedType`'s exception list: a typedef
                        // alias, an extension type, a type-argument
                        // position, or an external variable type /
                        // external function return type counts; an
                        // ordinary type annotation never does.
                        keys.push(lexeme.to_owned());
                    } else {
                        continue;
                    }
                }
                NodeKind::ObjectPattern => {
                    // The pattern's type name rides the row's name
                    // fields (the head identifier tiles no Identifier
                    // row of its own): `SecondName` when a prefix dot
                    // is present (`p.C(...)` names the type at `C`),
                    // else `FirstName`. `visitNamedType`'s exception
                    // list counts it only when the type is alias-like
                    // — a typedef or an extension type (structural
                    // matching instantiates the representation); a
                    // plain class in a pattern never counts (the
                    // official NamedType filter).
                    let named = |field| {
                        c.field(row, field)
                            .filter(|&token| token != dartq_ast::TOKEN_NONE)
                    };
                    let type_token =
                        named(PayloadField::SecondName).or_else(|| named(PayloadField::FirstName));
                    if let Some(token) = type_token {
                        let lexeme = c.lexeme(token);
                        if alias_like.contains(lexeme) {
                            keys.push(lexeme.to_owned());
                        }
                    }
                }
                NodeKind::BinaryExpression => {
                    // `visitBinaryExpression`: the operator element.
                    if let Some(operator) = c.field(row, PayloadField::Operator) {
                        keys.push(c.lexeme(operator).to_owned());
                    }
                }
                NodeKind::UnaryPrefix => {
                    // `visitPrefixExpression`: `~`/unary `-`/`++`/`--`.
                    match c.lexeme(c.head_token(row)) {
                        "~" | "-" => keys.push(c.lexeme(c.head_token(row)).to_owned()),
                        "++" => keys.push("+".to_owned()),
                        "--" => keys.push("-".to_owned()),
                        _ => {}
                    }
                }
                NodeKind::UnaryPostfix => {
                    // `visitPostfixExpression`: `++`/`--` name `+`/`-`.
                    let range = c.unit.ast.token_range(row);
                    if range.end > 0 {
                        match c.lexeme(range.end - 1) {
                            "++" => keys.push("+".to_owned()),
                            "--" => keys.push("-".to_owned()),
                            _ => {}
                        }
                    }
                }
                NodeKind::Indexed => {
                    // `visitIndexExpression`: `a[i]` reads `[]`; an
                    // assignment target (`a[i] = v`, `a[i] += v`) is
                    // the write element `[]=`.
                    keys.push("[]".to_owned());
                    let next = c.unit.ast.token_range(row).end.min(last);
                    if next <= last {
                        match c.lexeme(next) {
                            "=" | "+=" | "-=" | "*=" | "/=" | "%=" | "<<=" | ">>=" | ">>>="
                            | "&=" | "|=" | "^=" => keys.push("[]=".to_owned()),
                            _ => {}
                        }
                    }
                }
                NodeKind::ThisExpression => {
                    // `visitRedirectingConstructorInvocation`: a
                    // redirect's `: this()` marks the target constructor
                    // and its class; `: this.name()` the named target.
                    let next = c.unit.ast.token_range(row).end.min(last);
                    if next <= last {
                        if let Some(container) = enclosing_container_name(c, row) {
                            match c.lexeme(next) {
                                "(" => {
                                    keys.push(container.clone());
                                    keys.push(format!("{container}.new"));
                                }
                                "." if next + 1 < c.unit.tokens.len() as u32 => {
                                    let member = c.lexeme(next + 1);
                                    keys.push(container.clone());
                                    keys.push(format!("{container}.{member}"));
                                }
                                _ => {}
                            }
                        }
                    }
                }
                _ => continue,
            }
            if !keys.is_empty() {
                credit(&keys, &uses_extra, row);
            }
        }
        // The class clauses (`visitClassDeclaration` /
        // `visitExtensionTypeDeclaration`): an `extends`/`with`/
        // `implements`/`on` name counts unconditionally — the
        // reference visitor's `_addNamedType` bypasses the NamedType
        // filter — credited to the container's declaration.
        for wrapper in &wrappers {
            for row in declaration_rows(c, *wrapper) {
                if !matches!(
                    c.kind(row),
                    NodeKind::ClassDeclaration
                        | NodeKind::MixinDeclaration
                        | NodeKind::ExtensionDeclaration
                        | NodeKind::ExtensionTypeDeclaration
                        | NodeKind::EnumDeclaration
                ) {
                    continue;
                }
                let Some(name) = container_name_token(c, row) else {
                    continue;
                };
                let container = c.lexeme(name).to_owned();
                for (super_name, _) in supertypes(c, row) {
                    count_keys(&mut uses, &[super_name.to_owned()]);
                    deps.entry(container.clone())
                        .or_default()
                        .push(super_name.to_owned());
                }
            }
        }
        // The doc-comment `[Name]` face: a bracketed name in any
        // comment reads as a reference — into the flat use index, and
        // as a dependency edge of the declaration the comment attaches
        // to (the official reads doc-comment references against the
        // element model; the deepest covering row is the doc'd
        // declaration).
        for comment in c.comments.all() {
            let text = &c.unit.source[comment.range.start as usize..comment.range.end as usize];
            let bytes = text.as_bytes();
            let mut names: Vec<String> = Vec::new();
            let mut i = 0usize;
            while i < bytes.len() {
                if bytes[i] == b'[' {
                    let start = i + 1;
                    let mut end = start;
                    while end < bytes.len()
                        && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
                    {
                        end += 1;
                    }
                    if end > start && bytes.get(end) == Some(&b']') {
                        names.push(text[start..end].to_owned());
                        i = end;
                        continue;
                    }
                }
                i += 1;
            }
            if names.is_empty() {
                continue;
            }
            count_keys(&mut uses, &names);
            let candidates: Vec<u32> = wrappers
                .iter()
                .flat_map(|wrapper| declaration_rows(c, *wrapper))
                .collect();
            if let Some(owner) = doc_owner(c, &candidates, comment.token) {
                for source in attribution_keys(c, owner) {
                    deps.entry(source).or_default().extend_from_slice(&names);
                }
            }
        }
        // The entry-reach closure: the worklist walk over the
        // dependency edges (mark-before-push, one enqueue per key).
        let mut used = entries.clone();
        let mut queue: Vec<String> = entries.iter().cloned().collect();
        while let Some(source) = queue.pop() {
            let Some(targets) = deps.get(&source) else {
                continue;
            };
            for target in targets.clone() {
                if used.insert(target.clone()) {
                    queue.push(target);
                }
            }
        }
        ReferenceGraph {
            uses,
            entries,
            used,
            preview_resolves,
            preview_aliases,
        }
    }
}

/// Counts every reference key into the flat use index.
fn count_keys(uses: &mut FxHashMap<String, u32>, keys: &[String]) {
    for key in keys {
        *uses.entry(key.clone()).or_default() += 1;
    }
}

/// Credits every member name of `container` in the member-name →
/// container table (`_addDeclaration`'s enclosing-top-level arm).
fn credit_member_names(
    c: &Check<'_>,
    container_row: u32,
    container: &str,
    member_containers: &mut FxHashMap<String, String>,
) {
    for member in member_rows(c, container_row) {
        match c.kind(member) {
            NodeKind::Method => {
                if let Some(name) = member_name(c, member) {
                    let token = if c.kind(name) == NodeKind::OperatorName {
                        c.field(name, PayloadField::Name)
                            .unwrap_or_else(|| c.head_token(name))
                    } else {
                        c.head_token(name)
                    };
                    member_containers.insert(c.lexeme(token).to_owned(), container.to_owned());
                }
            }
            NodeKind::Fields => {
                for variable in c.collect_direct(member) {
                    if c.kind(variable) != NodeKind::VariablesDeclaration {
                        continue;
                    }
                    for var in c.collect_direct(variable) {
                        if c.kind(var) == NodeKind::InitializedIdentifier
                            && let Some(name) = initialized_identifier_name(c, var)
                        {
                            member_containers
                                .insert(c.lexeme(name).to_owned(), container.to_owned());
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// The declaration keys a row sits in: its nearest member (or
/// top-level function / variable / constructor) key and the enclosing
/// container's key — the official dependency walk's attribution.
/// `MetadataStar` rows sit LEFT of the declaration they annotate
/// (flat-tailed siblings, never ancestors): the walk jumps to the
/// star's right sibling — the annotated declaration.
fn attribution_keys(c: &Check<'_>, row: u32) -> Vec<String> {
    let mut keys = Vec::new();
    let mut node = row;
    loop {
        // `MetadataStar` rows sit LEFT of the declaration they annotate
        // (flat-tailed siblings, never ancestors): when the walk is
        // about to climb into one, jump to the annotated declaration
        // instead. A top-level star's right sibling is the production
        // wrapper — descend to its declaration row.
        if c.unit
            .ast
            .parent(node)
            .is_some_and(|parent| c.kind(parent) == NodeKind::MetadataStar)
        {
            let Some(star) = c.unit.ast.parent(node) else {
                break;
            };
            let Some(star_parent) = c.unit.ast.parent(star) else {
                break;
            };
            let siblings = c.collect_direct(star_parent);
            let Some(pos) = siblings.iter().position(|&kid| kid == star) else {
                break;
            };
            let Some(&next) = siblings.get(pos + 1) else {
                break;
            };
            node = if c.kind(next) == NodeKind::TopLevelDeclaration {
                match declaration_rows(c, next).first() {
                    Some(&decl) => decl,
                    None => break,
                }
            } else {
                next
            };
        }
        match c.kind(node) {
            NodeKind::Constructor | NodeKind::Factory => {
                if let Some(container) = enclosing_container_name(c, node) {
                    let written = constructor_name_node(c, node);
                    let head =
                        written.map_or_else(|| c.head_token(node), |name| c.head_token(name));
                    let head_lexeme = c.lexeme(head);
                    let member = if head_lexeme == container {
                        "new"
                    } else {
                        head_lexeme
                    };
                    keys.push(format!("{container}.{member}"));
                    if member != "new" {
                        keys.push(member.to_owned());
                    }
                }
            }
            NodeKind::Method | NodeKind::TopLevelMethod => {
                if let Some(name) = member_name(c, node) {
                    let token = if c.kind(name) == NodeKind::OperatorName {
                        c.field(name, PayloadField::Name)
                            .unwrap_or_else(|| c.head_token(name))
                    } else {
                        c.head_token(name)
                    };
                    keys.push(c.lexeme(token).to_owned());
                }
            }
            NodeKind::InitializedIdentifier => {
                if let Some(name) = initialized_identifier_name(c, node) {
                    keys.push(c.lexeme(name).to_owned());
                }
            }
            // A fields row attributes its WRITTEN TYPE to every variable
            // it declares (`declarations.addAll(variables.variables)` +
            // the `visitVariableDeclaration` type re-accept — a reference
            // in the type rides each variable's dependency set). The
            // type's rows tile as the fields row's direct children (the
            // `VariablesDeclaration` seals after the names as a sibling),
            // so the walk reaches this arm with NO keys yet; a walk that
            // climbed out of an initializer already carries that one
            // variable's key (the `InitializedIdentifier` arm) and must
            // not widen to its siblings.
            NodeKind::Fields | NodeKind::TopLevelFields if keys.is_empty() => {
                for list in c.collect_direct(node) {
                    if c.kind(list) != NodeKind::VariablesDeclaration {
                        continue;
                    }
                    for var in c.collect_direct(list) {
                        if c.kind(var) == NodeKind::InitializedIdentifier
                            && let Some(name) = initialized_identifier_name(c, var)
                        {
                            keys.push(c.lexeme(name).to_owned());
                        }
                    }
                }
            }
            NodeKind::ClassDeclaration
            | NodeKind::MixinDeclaration
            | NodeKind::ExtensionDeclaration
            | NodeKind::ExtensionTypeDeclaration
            | NodeKind::EnumDeclaration => {
                if let Some(name) = container_name_token(c, node) {
                    keys.push(c.lexeme(name).to_owned());
                }
                break;
            }
            NodeKind::Typedef => {
                if let Some(name) = name_token_of_typedef(c, node) {
                    keys.push(c.lexeme(name).to_owned());
                }
                break;
            }
            _ => {}
        }
        let Some(parent) = c.unit.ast.parent(node) else {
            break;
        };
        node = parent;
    }
    keys
}

/// The deepest declaration row whose token window covers `token` —
/// the declaration a comment attaches to (a member's doc comment
/// credits the member, not its container).
fn doc_owner(c: &Check<'_>, candidates: &[u32], token: u32) -> Option<u32> {
    candidates.iter().copied().find_map(|row| {
        c.unit
            .ast
            .token_range(row)
            .contains(&token)
            .then(|| doc_owner(c, &member_rows(c, row), token).unwrap_or(row))
    })
}

/// The name token of a typedef row (its direct identifier child).
fn name_token_of_typedef(c: &Check<'_>, row: u32) -> Option<u32> {
    c.collect_direct(row)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Identifier)
        .map(|kid| c.head_token(kid))
        .or_else(|| container_name_token(c, row))
}

/// Whether the identifier row sits inside a type-argument list — the
/// token walkback reaches the opening `<` before any non-argument
/// punctuation (a record type's `(C, D)` stops at `(`).
/// Credit an invocation head's constructor key (`C`/`C.new`, `C.named`)
/// — a typedef alias head (`typedef A = B; A()`) resolves through to the
/// target class's element, so the target keys ride along.
fn credit_ctor(
    keys: &mut Vec<String>,
    alias_rhs: &FxHashMap<String, String>,
    lexeme: &str,
    member: &str,
) {
    keys.push(format!("{lexeme}.{member}"));
    if let Some(target) = alias_rhs.get(lexeme) {
        keys.push(target.clone());
        keys.push(format!("{target}.{member}"));
    }
}

/// The `typedef A = B;` RHS head class name — the alias's resolution
/// target (`A()` invokes B's constructor). Only a plain-identifier head
/// names a class (`void Function()` / record RHSes target no class).
fn typedef_rhs_class<'a>(c: &'a Check<'_>, row: u32) -> Option<&'a str> {
    let rhs = c.collect_direct(row).into_iter().find(|&kid| {
        matches!(
            c.kind(kid),
            NodeKind::Type | NodeKind::FunctionType | NodeKind::RecordType
        )
    })?;
    let head = c.head_token(rhs);
    super::is_identifier_token(c, head).then(|| c.lexeme(head))
}

/// `isInExternalVariableTypeOrFunctionReturnType`
/// (unreachable_from_main.dart:740-753): the topmost type annotation's
/// declaration — an external function's RETURN type, or the written
/// type of an external field / top-level variable. `external C? c;`
/// types the variable from outside — the external implementation can
/// instantiate it — so the type counts as reachable.
fn external_type_position(c: &Check<'_>, row: u32) -> bool {
    // Climb to the topmost type-annotation row (the `topmostTypeAnnotation`
    // walk — Type/FunctionType/RecordType/TypeArguments shells).
    let mut top = row;
    while let Some(parent) = c.unit.ast.parent(top) {
        if matches!(
            c.kind(parent),
            NodeKind::Type
                | NodeKind::FunctionType
                | NodeKind::RecordType
                | NodeKind::TypeArguments
        ) {
            top = parent;
        } else {
            break;
        }
    }
    let Some(owner) = c.unit.ast.parent(top) else {
        return false;
    };
    match c.kind(owner) {
        // `external C f();` — a MEMBER method's return type (:744-745
        // matches `MethodDeclaration` only: a top-level or local
        // function's return type never counts). The return type is the
        // child before the parameter group; parameter types sit inside it.
        NodeKind::Method => {
            mod_flags(c.unit.ast, owner) & EXTERNAL_MEMBER_BIT != 0
                && c.collect_direct(owner)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
                    .is_some_and(|params| {
                        c.unit.ast.token_range(top).start < c.unit.ast.token_range(params).start
                    })
        }
        // `external C? c;` — the written type of an external field /
        // top-level variable: the type's name and `Type` rows tile as
        // direct children of the `Fields`/`TopLevelFields` row, and that
        // row carries the `external` modifier bit (the
        // `VariablesDeclaration` seals after the names as a sibling and
        // never owns the type).
        NodeKind::Fields | NodeKind::TopLevelFields => {
            mod_flags(c.unit.ast, owner) & EXTERNAL_MEMBER_BIT != 0
        }
        _ => false,
    }
}

fn in_type_argument(c: &Check<'_>, row: u32) -> bool {
    let mut token = c.unit.ast.token_range(row).start;
    for _ in 0..16 {
        if token == 0 {
            return false;
        }
        token -= 1;
        match c.lexeme(token) {
            "<" => return true,
            // A type-parameter BOUND (`<T extends C>` / `on` constraints)
            // is a declaration position, never a type argument — `extends`
            // cannot occur inside a true argument list, so it settles the
            // walk as "not an argument" (upstream: the parent is a
            // TypeParameter, not a TypeArgumentList).
            "extends" | "on" => return false,
            "," | "." | ">" => continue,
            lexeme => {
                let first = lexeme.chars().next();
                if first.is_some_and(char::is_alphabetic) || lexeme.starts_with('_') {
                    continue;
                }
                return false;
            }
        }
    }
    false
}

/// Whether the row carries a `@pragma('vm:entry-point')` annotation
/// (any string spelling — `_isValidVmEntryPoint`'s const value).
fn carries_pragma_entry(c: &Check<'_>, wrapper: u32, row: u32) -> bool {
    for meta in metadata_of(c, wrapper, row) {
        let kids = c.collect_direct(meta);
        let Some(head) = kids
            .iter()
            .find(|&&kid| c.kind(kid) == NodeKind::Identifier)
            .map(|&kid| c.head_token(kid))
        else {
            continue;
        };
        if c.lexeme(head) == "pragma" && first_string_argument(c, &kids) == Some("vm:entry-point") {
            return true;
        }
    }
    false
}

/// The metadata rows annotating `row` (the `MetadataStar` left of the
/// row under its parent, or of the wrapper).
fn metadata_of(c: &Check<'_>, wrapper: u32, row: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut holders = [Some(row), None];
    if c.kind(wrapper) == NodeKind::TopLevelDeclaration {
        holders[1] = Some(wrapper);
    }
    for holder in holders.into_iter().flatten() {
        let Some(parent) = c.unit.ast.parent(holder) else {
            continue;
        };
        let siblings = c.collect_direct(parent);
        let Some(pos) = siblings.iter().position(|&kid| kid == holder) else {
            continue;
        };
        if let Some(&star) = siblings[..pos]
            .iter()
            .rev()
            .find(|&&kid| c.kind(kid) == NodeKind::MetadataStar)
        {
            out.extend(
                c.collect_direct(star)
                    .into_iter()
                    .filter(|&meta| c.kind(meta) == NodeKind::Metadata),
            );
        }
    }
    out
}

/// The first string-literal argument of the first `Arguments` row among
/// `kids`, unquoted.
fn first_string_argument<'a>(c: &'a Check<'_>, kids: &[u32]) -> Option<&'a str> {
    let arguments = kids
        .iter()
        .find(|&&kid| c.kind(kid) == NodeKind::Arguments)
        .copied()?;
    let literal = c
        .collect_direct(arguments)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::StringLiteral)?;
    Some(unquote(c.lexeme(c.head_token(literal))))
}

/// Whether `kids` carry an `Arguments` row (a creation, not a bare
/// identifier).
fn has_arguments(c: &Check<'_>, kids: &[u32]) -> bool {
    kids.iter().any(|&kid| c.kind(kid) == NodeKind::Arguments)
}

/// A string literal's lexeme unquoted (any quote style, `r` prefix).
fn unquote(lexeme: &str) -> &str {
    let trimmed = lexeme.trim_start_matches('r');
    trimmed.trim_matches(|ch| ch == '\'' || ch == '"')
}

/// Whether `row` sits inside a `ConstantPattern` subtree — the
/// `_patternLevel` arm the use index consults (parent walk; the
/// subtree boundary is the pattern row's own completion window).
/// Whether `row` sits inside a `ConstantPattern` subtree — the
/// `_patternLevel` arm the use index consults. The pattern row BEGINS
/// after its operand pieces (the seal-order tiling — `const C()`'s
/// identifier and arguments are created before the pattern row opens),
/// so the parent walk never sees it; the pattern's TOKEN window is the
/// reliable cover.
fn inside_constant_pattern(ranges: &[core::ops::Range<u32>], token: u32) -> bool {
    ranges.iter().any(|range| range.contains(&token))
}

#[cfg(test)]
mod oracle_corpus_pins {
    use crate::testsupport::{Fixture, one_rule};

    /// The whole upstream oracle corpus for `unreachable_from_main`: `UnreachableFromMainTest` (143 cases) plus two `messages.yaml` doc entries,
    /// `conformance/status.tsv` `lint unreachable_from_main 145 117 28`. Each entry is the exact
    /// materialized case source and the oracle's `(line, column, length)` for
    /// every `unreachable_from_main` row; only the rule's own rows are pinned.
    ///
    /// 138 single-file cases pinned (56 oracle rows).
    /// Not representable in a single-source fixture (aux files or packages):
    /// `UnreachableFromMainTest/test_class_instanceMethod_reachable_overrides_imported#0` (imported a.dart declares the overridden method).
    /// `UnreachableFromMainTest/test_class_reachable_mainInPart#0` (multi_file).
    /// `UnreachableFromMainTest/test_class_unreachable_mainInPart#0` (multi_file).
    /// `UnreachableFromMainTest/test_classInPart_reachable#0` (multi_file).
    /// `UnreachableFromMainTest/test_classInPart_reachable_mainInPart#0` (multi_file).
    /// `UnreachableFromMainTest/test_classInPart_unreachable#0` (multi_file).
    /// `UnreachableFromMainTest/test_classInPart_unreachable_mainInPart#0` (multi_file).
    #[test]
    fn unreachable_from_main_oracle_corpus_pins() {
        // (case, source, oracle rows as (line, column, length)).
        let cases: &[(&str, &str, &[(u32, u32, u32)])] = &[
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_instanceField_reachable_matchedInPattern#0"#,
                r#"
void main() {
  var x = switch (C(1)) {
    C(:final f) => f.round(),
  };
  print(x);
}

class C {
  final int f;
  C(this.f);
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_instanceField_reachable_overrides_local#0"#,
                r#"
void main() {
  B();
}

class A {
  int? f;
}

class B extends A {
  int? f;
}
"#,
                &[(7, 8, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_instanceField_reachable_read#0"#,
                r#"
void main() {
  C().f;
}

class C {
  int? f;
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_instanceField_reachable_write#0"#,
                r#"
void main() {
  C().f = 0;
}

class C {
  int? f;
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_instanceField_unreachable#0"#,
                r#"
void main() {
  C();
}

class C {
  int? f;
}
"#,
                &[(7, 8, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_instanceGetter_reachable_invoked#0"#,
                r#"
void main() {
  C().foo;
}

class C {
  int get foo => 0;
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_instanceGetter_reachable_overrides#0"#,
                r#"
void main() {
  B();
}

class A {
  int get foo => 0;
}

class B extends A {
  int get foo => 0;
}
"#,
                &[(7, 11, 3)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_instanceGetter_unreachable#0"#,
                r#"
void main() {
  C();
}

class C {
  int get foo => 0;
}
"#,
                &[(7, 11, 3)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_instanceMethod_reachable_invoked#0"#,
                r#"
void main() {
  C().foo();
}

class C {
  void foo() {}
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_instanceMethod_reachable_invoked_generic#0"#,
                r#"
void main() {
  C<int>().foo(null);
}

class C<T> {
  void foo(T? _) {}
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_instanceMethod_reachable_overrides_local#0"#,
                r#"
void main() {
  B();
}

class A {
  void foo() {}
}

class B extends A {
  void foo() {}
}
"#,
                &[(7, 8, 3)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_instanceMethod_unreachable#0"#,
                r#"
void main() {
  C();
}

class C {
  void foo() {}
}
"#,
                &[(7, 8, 3)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_instanceSetter_reachable_invoked#0"#,
                r#"
void main() {
  C().foo = 0;
}

class C {
  set foo(int _) {}
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_instanceSetter_reachable_overrides#0"#,
                r#"
void main() {
  B();
}

class A {
  set foo(int _) {}
}

class B extends A {
  set foo(int _) {}
}
"#,
                &[(7, 7, 3)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_instanceSetter_unreachable#0"#,
                r#"
void main() {
  C();
}

class C {
  set foo(int _) {}
}
"#,
                &[(7, 7, 3)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_reachableViaAnnotation#0"#,
                r#"
void main() {
  f();
}

class C {
  const C();
}

@C()
void f() {}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_reachableViaComment#0"#,
                r#"
/// See [C].
void main() {}

class C {}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_reachableViaDefaultValueType#0"#,
                r#"
void main() {
  f();
}

class C {
  const C();
}

void f([Object? p = const C()]) {}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_reachable_referencedDeepInTypeAnnotation_externalMethodDeclaration#0"#,
                r#"
void main() {
  D().f;
}

class C {}

class D {
  external C Function() f();
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_reachable_referencedDeepInTypeArgument#0"#,
                r#"
void main() {
  C<D Function()>();
}

class C<T> {}

class D {}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_reachable_referencedInTypeAnnotation_externalFieldDeclaration#0"#,
                r#"
void main() {
  D().c;
}

class C {}

class D {
  external C? c;
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_reachable_referencedInTypeAnnotation_externalMethodDeclaration#0"#,
                r#"
void main() {
  D().f;
}

class C {}

class D {
  external C f();
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_reachable_referencedInTypeArgument#0"#,
                r#"
void main() {
  C<D>();
}

class C<T> {}

class D {}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_reachable_referencedInTypeArgument_genericGenericType#0"#,
                r#"
class A {}

void main() {
  f();
}

void f([List<List<A>>? l]) {}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_reachable_typeLiteral#0"#,
                r#"
void main() {
  C;
}

class C {}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_referencedInObjectPattern#0"#,
                r#"
class C {}

void main() {
  f();
}

void f([Object? c]) {
  if (c case C()) {}
}
"#,
                &[(2, 7, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_unreachable#0"#,
                r#"
void main() {}

class C {}
"#,
                &[(4, 7, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_unreachable_foundInAsExpression#0"#,
                r#"
class A {}

void main() {
  f();
}

void f([Object? o]) {
  o as A;
}
"#,
                &[(2, 7, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_unreachable_foundInAsPattern#0"#,
                r#"
class A {}

void main() {
  f();
}

void f([(Object, )? l]) {
  var (_ as A, ) = l!;
}
"#,
                &[(2, 7, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_unreachable_foundInIsExpression#0"#,
                r#"
class A {}

void main() {
  f();
}

void f([Object? o]) {
  o is A;
}
"#,
                &[(2, 7, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_unreachable_hasNamedConstructors#0"#,
                r#"
void main() {}

class C {
  C();
  C.named();
}
"#,
                &[(4, 7, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_unreachable_referencedInParameter_externalMethodDeclaration#0"#,
                r#"
void main() {
  D().f;
}

class C {}

class D {
  external f(C c);
}
"#,
                &[(6, 7, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_unreachable_referencedInTypeAnnotation_fieldDeclaration#0"#,
                r#"
void main() {
  D().c;
}

class C {}

class D {
  C? c;
}
"#,
                &[(6, 7, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_unreachable_referencedInTypeAnnotation_parameter#0"#,
                r#"
void main() {
  f();
}

void f([C? c]) {}

class C {}
"#,
                &[(8, 7, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_unreachable_referencedInTypeAnnotation_topLevelVariable#0"#,
                r#"
void main() {
  print(c);
}

C? c;

class C {}
"#,
                &[(8, 7, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_unreachable_referencedInTypeAnnotation_variableDeclaration#0"#,
                r#"
void main() {
  C? c;
}

class C {}
"#,
                &[(6, 7, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_class_unreachable_typeArgumentBound#0"#,
                r#"
void main() {
  f();
}

class C {}

void f<T extends C>() {}
"#,
                &[(6, 7, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_constructor_named_new#0"#,
                r#"
void main() {
  C;
}

class C {
  new named();
}
"#,
                &[(7, 7, 5)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_constructor_named_onEnum#0"#,
                r#"
void main() {
  E.one;
  E.two;
}

enum E {
  one(), two();

  const E();
  const E.named();
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_constructor_named_reachableViaDirectCall#0"#,
                r#"
void main() {
  C.named();
}

class C {
  C.named();
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_constructor_named_reachableViaExplicitSuperCall#0"#,
                r#"
void main() {
  D();
}

class C {
  C.named();
}

class D extends C {
  D() : super.named();
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_constructor_named_reachableViaRedirectedConstructor#0"#,
                r#"
void main() {
  C.two();
}

class C {
  C.named();
  factory C.two() = C.named;
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_constructor_named_reachableViaRedirection#0"#,
                r#"
void main() {
  C.two();
}

class C {
  C.named();

  C.two() : this.named();
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_constructor_named_reachableViaTearoff#0"#,
                r#"
void main() {
  C.named;
}

class C {
  C.named();
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_constructor_named_unreachable#0"#,
                r#"
void main() {
  C;
}

class C {
  C.named();
}
"#,
                &[(7, 5, 5)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_constructor_reachableViaTestReflectiveLoader#0"#,
                r#"
import 'package:test_reflective_loader/test_reflective_loader.dart';

void main() {
  // Usually some reference via `defineReflectiveTests`.
  A;
}

@reflectiveTest
class A {
  A() {}
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_constructor_unnamed_reachableViaDefaultImplicitSuperCall#0"#,
                r#"
void main() {
  D();
}

class C {
  C();
}

class D extends C {
  // Just a default constructor.
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_constructor_unnamed_reachableViaDirectCall#0"#,
                r#"
void main() {
  C();
}

class C {
  C();
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_constructor_unnamed_reachableViaExplicitSuperCall#0"#,
                r#"
void main() {
  D();
}

class C {
  C();
}

class D extends C {
  D() : super();
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_constructor_unnamed_reachableViaImplicitSuperCall#0"#,
                r#"
void main() {
  D();
}

class C {
  C();
}

class D extends C {
  D();
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_constructor_unnamed_reachableViaImplicitSuperCall_indirectly#0"#,
                r#"
void main() {
  E();
}

class C {
  C();
}

class D extends C {
  // Just a default constructor.
}

class E extends D {
  E();
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_constructor_unnamed_reachableViaImplicitSuperCall_superParameters#0"#,
                r#"
void main() {
  D(1);
}

class C {
  C(int p);
}

class D extends C {
  D(super.p);
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_constructor_unnamed_reachableViaRedirectedConstructor#0"#,
                r#"
void main() {
  C.two();
}

class C {
  C();
  factory C.two() = C;
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_constructor_unnamed_reachableViaRedirection#0"#,
                r#"
void main() {
  C.two();
}

class C {
  C();

  C.two() : this();
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_constructor_unnamed_reachableViaTearoff#0"#,
                r#"
void main() {
  C.new;
}

class C {
  C();
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_constructor_unnamed_referencedInConstantPattern#0"#,
                r#"
class C {
  const C();
}

void main() {
  f();
}

void f([C? c]) {
  if (c case const C()) {}
}
"#,
                &[(2, 7, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_constructor_unnamed_referencedInConstantPattern_generic#0"#,
                r#"
class C<T> {
  const C();
}

void main() {
  f();
}

void f([C? c]) {
  if (c case const C<int>()) {}
}
"#,
                &[(2, 7, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_constructor_unnamed_unreachable#0"#,
                r#"
void main() {
  C;
}

class C {
  C();
}
"#,
                &[(7, 3, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_constructor_unnamed_unreachable_otherHasRedirection#0"#,
                r#"
void main() {
  C.two();
}

class C {
  C();
  C.one();
  C.two() : this.one();
}
"#,
                &[(7, 3, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_enum_reachableViaValue#0"#,
                r#"
void main() {
  E.one;
}

enum E { one, two }
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_enum_unreachable#0"#,
                r#"
void main() {}

enum E { one, two }
"#,
                &[(4, 6, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_extensionType_constructor_named_unreachable#0"#,
                r#"
void main() {
  E(7);
}

extension type E(int it) {
  E.named(this.it);
}
"#,
                &[(7, 5, 5)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_extensionType_instanceMethod_reachable#0"#,
                r#"
void main() {
  E(1).m();
}

extension type E(int i) {
  void m() {}
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_extensionType_instanceMethod_unreachable#0"#,
                r#"
void main() {
  E(1);
}

extension type E(int i) {
  void m() {}
}
"#,
                &[(7, 8, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_extensionType_member_redeclare_unreachable#0"#,
                r#"
void main() {
  var c = C();
  c.m();
  E(c);
}

class C {
  void m() {}
}

extension type E(C c) implements C {
  void m() {}
}
"#,
                &[(13, 8, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_extensionType_primaryConstructorBody_reachable#0"#,
                r#"
void main() {
  E(1);
  print(E(1).i);
}

extension type E(int i) {
  this {
    print(i);
  }
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_extensionType_reachable_referencedInTypeAnnotation_asExpression#0"#,
                r#"
void main() {
  1 as E;
}

extension type E(int it) {}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_extensionType_representation_private_unreachable#0"#,
                r#"
void main() {
  E(1);
}

extension type E(int _i) {}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_extensionType_representation_reachable#0"#,
                r#"
void main() {
  E(1).i;
}

extension type E(int i) {}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_extensionType_representation_unreachable#0"#,
                r#"
void main() {
  E(1);
}

extension type E(int i) {}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_extensionType_staticMethod_unreachable#0"#,
                r#"
void main() {
  E(1);
}

extension type E(int i) {
  static void f() {}
}
"#,
                &[(7, 15, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_extensionType_unreachable#0"#,
                r#"
void main() {}

extension type E(int i) {}
"#,
                &[(4, 16, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_extension_unreachable#0"#,
                r#"
void main() {}

extension IntExtension on int {}
"#,
                &[(4, 11, 12)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_instanceFieldOnExtension_unreachable#0"#,
                r#"
void main() {
  E.f;
}

extension E on int {
  static int f = 1;
  void m() {}
}
"#,
                &[(8, 8, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_instanceMethod_reachable_toJson#0"#,
                r#"
import 'dart:convert';

void main() async {
  jsonEncode([C()]);
}

class C {
  List<Object> toJson() => ['c'];
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_mixin_reachable_implemented#0"#,
                r#"
void main() {
  A();
}

mixin M {}

class A implements M {}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_mixin_reachable_mixed#0"#,
                r#"
void main() {
  A();
}

mixin M {}

class A with M {}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_mixin_unreachable#0"#,
                r#"
void main() {}

mixin M {}
"#,
                &[(4, 7, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_operator_index#0"#,
                r#"
void main() {
  print(V()[0]);
}

class V {
  int operator [](int index) => 7;
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_operator_indexAssignment#0"#,
                r#"
void main() {
  V()[0] = 2;
}

class V {
  void operator []=(int index, int value) {}
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_operator_lessThan#0"#,
                r#"
void main() {
  if (V() < V()) {
    print('ok');
  }
}

class V {
  bool operator <(V other) => false;
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_operator_postfixIncrement#0"#,
                r#"
void main() {
  var v = V();
  v++;
}

class V {
  V operator +(int other) => this;
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_operator_prefixIncrement#0"#,
                r#"
void main() {
  var v = V();
  ++v;
}

class V {
  V operator +(int other) => this;
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_operator_unary#0"#,
                r#"
void main() {
  print(~V());
}

class V {
  V operator ~() => this;
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_setUpClass_class_static_member_reachable#0"#,
                r#"
void main() {
  A();
}

final class A {
  static setUpClass() {}
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_setUpClass_top_level_unreachable#0"#,
                r#"
void main() {}

setUpClass() {}
"#,
                &[(4, 1, 10)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_staticFieldOnClass_reachable#0"#,
                r#"
void main() {
  C.f;
}

class C {
  static int f = 1;
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_staticFieldOnEnum_reachable#0"#,
                r#"
void main() {
  E.f;
}

enum E {
  one, two, three;
  static int f = 1;
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_staticFieldOnExtension_reachable#0"#,
                r#"
void main() {
  E.f;
}

extension E on int {
  static int f = 1;
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_staticFieldOnExtension_unreachable#0"#,
                r#"
void main() {
  E(1).m();
}

extension E on int {
  static int f = 1;
  void m() {}
}
"#,
                &[(7, 14, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_staticFieldOnMixin_reachable#0"#,
                r#"
void main() {
  M.f;
}

mixin M {
  static int f = 1;
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_staticField_unreachable#0"#,
                r#"
void main() {
  C;
}

class C {
  static int f = 1;
}
"#,
                &[(7, 14, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_staticGetter_reachable#0"#,
                r#"
void main() {
  C.g;
}

class C {
  static int get g => 7;
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_staticGetter_unreachable#0"#,
                r#"
void main() {
  C;
}

class C {
  static int get g => 7;
}
"#,
                &[(7, 18, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_staticMethodOnClass_reachable#0"#,
                r#"
void main() {
  C.f();
}

class C {
  static void f() {}
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_staticMethodOnEnum_reachable#0"#,
                r#"
void main() {
  E.f();
}

enum E {
  one, two, three;
  static void f() {}
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_staticMethodOnExtension_reachable#0"#,
                r#"
void main() {
  E.f();
}

extension E on int {
  static void f() {}
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_staticMethodOnMixin_reachable#0"#,
                r#"
void main() {
  M.f();
}

mixin M {
  static void f() {}
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_staticMethod_unreachable#0"#,
                r#"
void main() {
  C;
}

class C {
  static void f() {}
}
"#,
                &[(7, 15, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_staticSetter_reachable#0"#,
                r#"
void main() {
  C.s = 1;
}

class C {
  static set s(int value) {}
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_staticSetter_unreachable#0"#,
                r#"
void main() {
  C;
}

class C {
  static set s(int value) {}
}
"#,
                &[(7, 14, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_tearDownClass_class_static_member_reachable#0"#,
                r#"
void main() {
  A();
}

final class A {
  static tearDownClass() {}
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_tearDownClass_top_level_unreachable#0"#,
                r#"
void main() {}

tearDownClass() {}
"#,
                &[(4, 1, 13)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_topLevelFunction_reachable#0"#,
                r#"
void main() {
  f1();
}

void f1() {
  f2();
}

void f2() {}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_topLevelFunction_reachable_private#0"#,
                r#"
void main() {
  _f();
}

void _f() {}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_topLevelFunction_unreachable#0"#,
                r#"
void main() {}

void f() {}
"#,
                &[(4, 6, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_topLevelFunction_unreachable_unrelatedPragma#0"#,
                r#"
void main() {}

@pragma('other')
void f() {}
"#,
                &[(5, 6, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_topLevelFunction_unreachable_visibleForTesting#0"#,
                r#"
import 'package:meta/meta.dart';
void main() {}

@visibleForTesting
void f() {}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_topLevelFunction_vmEntryPoint#0"#,
                r#"
void main() {}

@pragma('vm:entry-point')
void f6() {}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_topLevelFunction_vmEntryPoint_const#0"#,
                r#"
void main() {}

const entryPoint = pragma('vm:entry-point');
@entryPoint
void f6() {}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_topLevelGetter_reachable#0"#,
                r#"
void main() {
  g;
}

int get g => 7;
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_topLevelGetter_unreachable#0"#,
                r#"
void main() {}

int get g => 7;
"#,
                &[(4, 9, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_topLevelSetter_reachable#0"#,
                r#"
void main() {
  s = 7;
}

set s(int value) {}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_topLevelSetter_unreachable#0"#,
                r#"
void main() {}

set s(int value) {}
"#,
                &[(4, 5, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_topLevelVariable_reachable#0"#,
                r#"
void main() {
  _f();
}

void _f() {
  x;
}

int x = 1;
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_topLevelVariable_unreachable#0"#,
                r#"
void main() {}

int x = 1;
"#,
                &[(4, 5, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_typedef_reachable_referencedAsInstanceCreation_named#0"#,
                r#"
void main() {
  T.named();
}

class C {
  C.named();
}

typedef T = C;
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_typedef_reachable_referencedAsInstanceCreation_unnamed#0"#,
                r#"
void main() {
  T();
}

class C {}

typedef T = C;
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_typedef_reachable_referencedInObjectPattern#0"#,
                r#"
typedef T = int;

void main() {
  f();
}

void f([Object? c]) {
  if (c case T()) {}
}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_typedef_reachable_referencedInTypeAnnotation_asExpression#0"#,
                r#"
void main() {
  1.5 as T;
}

typedef T = int;
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_typedef_reachable_referencedInTypeAnnotation_castPattern#0"#,
                r#"
void main() {
  var r = (1.5, );
  var (s as T, ) = r;
}

typedef T = int;
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_typedef_reachable_referencedInTypeAnnotation_genericFunctionType#0"#,
                r#"
void main() {
  t = () => 7;
}

T? Function()? t;

typedef T = int;
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_typedef_reachable_referencedInTypeAnnotation_isExpression#0"#,
                r#"
void main() {
  1.5 is T;
}

typedef T = int;
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_typedef_reachable_referencedInTypeAnnotation_parameter#0"#,
                r#"
void main() {
  f(() {});
}

void f([Cb? c]) {}

typedef Cb = void Function();
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_typedef_reachable_referencedInTypeAnnotation_recordType#0"#,
                r#"
void main() {
  t = (1, );
}

(T, )? t;

typedef T = int;
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_typedef_reachable_referencedInTypeAnnotation_topLevelVariable#0"#,
                r#"
void main() {
  c = () {};
}

Cb? c;

typedef Cb = void Function();
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_typedef_reachable_referencedInTypeAnnotation_variableDeclaration#0"#,
                r#"
void main() {
  Cb c = () {};
}

typedef Cb = void Function();
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_typedef_unreachable#0"#,
                r#"
void main() {}

typedef T = String;
"#,
                &[(4, 9, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_widgetPreview_classInstanceMethod#0"#,
                r#"
import 'package:flutter/widgets.dart';
import 'package:flutter/widget_previews.dart';

void main() {}

class B {
  // Widget previews can't be defined with instance methods.
  // ignore: invalid_widget_preview_application
  @Preview()
  Widget foo() => Text('');
}
"#,
                &[(7, 7, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_widgetPreview_classStaticMethod#0"#,
                r#"
import 'package:flutter/widgets.dart';
import 'package:flutter/widget_previews.dart';

void main() {}

class B {
  @Preview()
  static Widget foo() => Text('');
}
"#,
                &[(7, 7, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_widgetPreview_constructor#0"#,
                r#"
import 'package:flutter/widget_previews.dart';

void main() {}

class B {
  @Preview()
  const B();
}
"#,
                &[(6, 7, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_widgetPreview_factoryConstructor#0"#,
                r#"
import 'package:flutter/widget_previews.dart';

void main() {}

class B {
  // ignore: invalid_widget_preview_application
  @Preview()
  factory B.foo() => B();

  const B();
}
"#,
                &[(6, 7, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_widgetPreview_privatePreview#0"#,
                r#"
// Widget previews can't be defined with private functions.
import 'package:flutter/widget_previews.dart';
import 'package:flutter/widgets.dart';
void main() {}

class B {
  @Preview()
  factory B._foo() => B._();

  @Preview()
  B._();

  @Preview()
  static Widget _bar() => Text('');
}

@Preview()
void _f6() {}
"#,
                &[(7, 7, 1)],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_widgetPreview_topLevelFunction#0"#,
                r#"
import 'package:flutter/widget_previews.dart';

void main() {}

// ignore: invalid_widget_preview_application
@Preview()
void f6() {}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_widgetPreview_topLevelFunction_const#0"#,
                r#"
import 'package:flutter/widget_previews.dart';
void main() {}

const preview = Preview();
@preview
void f6() {}
"#,
                &[],
            ),
            (
                r#"lints/unreachable_from_main/UnreachableFromMainTest/test_widgetPreview_topLevelFunction_customPreviewClass#0"#,
                r#"
void main() {}

class Preview {
  const Preview();
}

// This isn't from package:flutter/widget_previews.dart and shouldn't be exempt.
@Preview()
void f6() {}
"#,
                &[(4, 7, 7), (10, 6, 2)],
            ),
            (
                r#"lints/unreachable_from_main/doc_bad_0"#,
                r#"main() {}
void f() {}
"#,
                &[(2, 6, 1)],
            ),
            (
                r#"lints/unreachable_from_main/doc_good_0"#,
                r#"main() {
  f();
}
void f() {}
"#,
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

        let flags: [(u32, bool); 0] = [];
        let face = crate::engine::ReachabilityFace {
            has_entry: true,
            flags: &flags,
        };
        let mut mismatches = Vec::new();
        for (case, source, oracle) in cases {
            let f = Fixture::parse(source);
            let mut lints = f.run_with(&one_rule("unreachable_from_main"), None, Some(&face), None);
            lints.sort_by_key(|lint| lint.range.start);
            let got: Vec<(u32, u32)> = lints
                .iter()
                .map(|lint| (lint.range.start, lint.range.end))
                .collect();
            let mut want: Vec<(u32, u32)> = oracle
                .iter()
                .map(|&(line, column, length)| {
                    let start = offset(source, line, column);
                    (start, start + length)
                })
                .collect();
            want.sort_unstable();
            if got != want {
                mismatches.push(format!("{case}: want {want:?} got {got:?}"));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }
}
