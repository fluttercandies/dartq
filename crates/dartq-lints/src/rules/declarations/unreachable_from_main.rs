use rustc_hash::FxHashSet;

use super::{Check, RuleSpec, container_name_token};
use crate::engine::ReachabilityFace;
use crate::rules::{initialized_identifier_name, member_name, rows_with_parent, utf16_token_span};
use dartq_ast::NodeKind;

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
/// Approximations (documented): the official override exemptions
/// (`isOverride` on field getters and methods) are not modeled — the
/// face's reference rows carry no override relation and the shell never
/// guesses, so an overridden-only base member reports where the
/// official stays silent (the engine face's documented divergence
/// family; the `toJson` auto-reachability and `@reflectiveTest`
/// constructor edges under-reach in the same direction). Three
/// substrate deltas lean the same under-report direction and stay
/// engine-owned (the shell never compensates): the propagation graph is
/// workspace-global where the official walk is per-library, the entry
/// seeds admit every declaration kind where the official seeds only
/// functions, and type-annotation references carry no `NamedType`
/// filter. Annotation reads (`@visibleForTesting`, `@Preview`) are by
/// spelling — a same-file annotation class of either name
/// over-suppresses (the flutter-family approximation register). An
/// unnamed extension has no name token to key a face flag with: it
/// never reports, and its members judge on their own flags.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unreachable_from_main",
    &[NodeKind::CompilationUnit],
    check,
);

/// The candidate class the widget-preview suppression reads (upstream
/// `hasWidgetPreview`: constructors, top-level functions, and static
/// executables are the suppressible shapes).
#[derive(Clone, Copy)]
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
    // `if (entryPoints.isEmpty) return` — a library without an entry
    // point reports nothing at all.
    if !face.has_entry {
        return;
    }
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
                        top,
                        row,
                        Shape::TopLevelFunction,
                        name,
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
                                top,
                                row,
                                Shape::Other,
                                name,
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
                    judge_container(c, &face, top, row, &mut dead);
                }
                NodeKind::Typedef => {
                    let Some(name) = name_token_of(c, top, row) else {
                        continue;
                    };
                    judge(
                        c,
                        &face,
                        top,
                        row,
                        Shape::Other,
                        name,
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
    let is_dead = name_offset.is_some_and(|offset| face.is_reachable(offset) == Some(false));
    if let (Some(offset), true) = (name_offset, is_dead) {
        let public = name.is_some_and(|token| !c.lexeme(token).starts_with('_'));
        if public {
            dead.insert(offset);
        }
    }
    if let Some(name) = name {
        judge(
            c,
            face,
            wrapper,
            row,
            Shape::Other,
            name,
            c.lexeme(name),
            c.lexeme(name),
        );
    }
    // Members gather only under a public container, and a dead one
    // suppresses them all (the gate above).
    let public = name.is_some_and(|token| !c.lexeme(token).starts_with('_'));
    if !public || is_dead {
        return;
    }
    let container = c.lexeme(name.expect("public implies a name")).to_owned();
    let is_enum = c.kind(row) == NodeKind::EnumDeclaration;
    for member in member_rows(c, row) {
        match c.kind(member) {
            NodeKind::Constructor | NodeKind::Factory => {
                // An enum's constructors never gather (upstream
                // `member.parent?.parent is! EnumDeclaration`).
                if is_enum {
                    continue;
                }
                judge_constructor(c, face, member, &container);
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
                        judge(
                            c,
                            face,
                            member,
                            member,
                            Shape::Other,
                            name,
                            c.lexeme(name),
                            c.lexeme(name),
                        );
                    }
                }
            }
            NodeKind::Method => {
                let Some(name) = member_name(c, member) else {
                    continue;
                };
                // An operator names through its `OperatorName` payload
                // token (the head would hit the `operator` keyword).
                let name = if c.kind(name) == NodeKind::OperatorName {
                    c.payload(name)
                        .first()
                        .copied()
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
                judge(
                    c,
                    face,
                    member,
                    member,
                    Shape::Method(is_static),
                    name,
                    lexeme,
                    lexeme,
                );
            }
            _ => {}
        }
    }
}

/// Judges one constructor: the display name is `Type.written`, the
/// unnamed face `Type.new` (upstream `nameForError`'s
/// `self.name?.lexeme ?? 'new'`), anchored at the name token.
fn judge_constructor(
    c: &mut Check<'_>,
    face: &ReachabilityFace<'_>,
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
                (c.head_token(row), "new")
            } else {
                (c.head_token(node), lexeme)
            }
        }
        None => (c.head_token(row), "new"),
    };
    let display = format!("{container}.{member}");
    judge(
        c,
        face,
        row,
        row,
        Shape::Constructor,
        name_token,
        member,
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
    wrapper: u32,
    annotation_row: u32,
    shape: Shape,
    name_token: u32,
    public: &str,
    display: &str,
) {
    // 1. Unreachable (the face's flag; `None` — outside the face's
    // universe — never reports).
    let offset = utf16_token_span(c, name_token).start;
    if face.is_reachable(offset) != Some(false) {
        return;
    }
    // 2. Public (`element.isPublic` — the written declaration name; a
    // constructor's display carries the `Type.` prefix, so the public
    // test reads the member name itself).
    if public.starts_with('_') {
        return;
    }
    // 5. Outermost container wins — a member of a dead container never
    // reaches this far: the member loop is skipped entirely for a dead
    // container (the caller's gate).
    // 3. Not `@visibleForTesting` (`element.hasVisibleForTesting`).
    if carries_annotation(c, wrapper, annotation_row, "visibleForTesting") {
        return;
    }
    // 4. Not a public widget-preview member (`hasWidgetPreview`:
    // constructors, top-level functions, and static executables).
    let previewable = match shape {
        Shape::TopLevelFunction | Shape::Constructor => true,
        Shape::Method(is_static) => is_static,
        Shape::Other => false,
    };
    if previewable && carries_annotation(c, wrapper, annotation_row, "Preview") {
        return;
    }
    c.report(utf16_token_span(c, name_token), &[display]);
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
/// payload token, the head would hit the `operator` keyword), else the
/// production wrapper's name row (the class-like left-sibling
/// placement), else [`container_name_token`].
fn name_token_of(c: &Check<'_>, wrapper: u32, row: u32) -> Option<u32> {
    if let Some(name) = c
        .collect_direct(row)
        .into_iter()
        .find(|&kid| matches!(c.kind(kid), NodeKind::Identifier | NodeKind::OperatorName))
    {
        return Some(if c.kind(name) == NodeKind::OperatorName {
            c.payload(name)
                .first()
                .copied()
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
/// reading — an unnamed constructor's class-name echo carries the same
/// context, the caller compares against the container name).
fn constructor_name_node(c: &Check<'_>, row: u32) -> Option<u32> {
    let ctx = dartq_parser::IdentifierContext::MethodDeclarationContinuation as u32;
    c.collect_direct(row).into_iter().find(|&kid| {
        c.kind(kid) == NodeKind::Identifier && c.payload(kid).first() == Some(&ctx)
    })
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
                    c.kind(piece) == NodeKind::Identifier
                        && c.lexeme(c.head_token(piece)) == name
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
