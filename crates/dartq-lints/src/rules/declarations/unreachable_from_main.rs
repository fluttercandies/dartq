use rustc_hash::FxHashSet;

use super::{Check, PayloadField, RuleSpec, container_name_token};
use crate::engine::ReachabilityFace;
use crate::rules::{
    MemberQuery, initialized_identifier_name, member_matches, member_name, rows_with_parent,
    same_file_ancestors, utf16_token_span,
};
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
/// Shell-side suppression faces (same-file approximations of the
/// official's element queries): a member whose name resolves against a
/// same-file ancestor's member table never gathers (the `isOverride`
/// arm — `getOverridden` across the interface), a non-static method
/// named `toJson` never gathers (the `dart:convert` dynamic-invocation
/// auto-reach arm :293-303), and a declaration whose doc-comment
/// `[Name]` references match its own name reads as referenced (the
/// comment-reference edge the engine face does not carry).
///
/// Approximations (documented): the override read is same-file only —
/// an override relation through an imported or SDK interface stays
/// invisible, matching the face family's documented cross-file
/// under-reach. Three substrate deltas lean the same under-report
/// direction and stay engine-owned (the shell never compensates): the
/// propagation graph is workspace-global where the official walk is
/// per-library, the entry seeds admit every declaration kind where the
/// official seeds only functions, and type-annotation references carry
/// no `NamedType` filter. Annotation reads (`@visibleForTesting`,
/// `@Preview`) are by spelling — a same-file annotation class of either
/// name over-suppresses (the flutter-family approximation register). An
/// unnamed extension has no name token to key a face flag with: it
/// never reports, and its members judge on their own flags.
pub(crate) static SPEC: RuleSpec =
    crate::spec!("unreachable_from_main", &[NodeKind::CompilationUnit], check,);

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
    // The lexical-use supplement: the name → counting-position index
    // the doc-reference and anti-reach arms consult. Built once per
    // unit — `Identifier` rows in a reference context (expression,
    // expression-continuation, or constructor-reference position) count
    // the candidate's written name; declaration-context names and
    // `TypeReference`/`PrefixedTypeReference` rows do not (the
    // official's `SimpleIdentifier`-vs-`NamedType` split :349-386,
    // :307-346 — a plain type annotation never counts). Doc-comment
    // `[Name]` references count through the same index (the official
    // resolves comment references against the element model).
    let uses = counting_uses(c);
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
                        &uses,
                        top,
                        row,
                        Shape::TopLevelFunction,
                        name,
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
                                &uses,
                                top,
                                row,
                                Shape::Other,
                                name,
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
                    judge_container(c, &face, &uses, top, row, &mut dead);
                }
                NodeKind::Typedef => {
                    let Some(name) = name_token_of(c, top, row) else {
                        continue;
                    };
                    judge(
                        c,
                        &face,
                        &uses,
                        top,
                        row,
                        Shape::Other,
                        name,
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
    uses: &rustc_hash::FxHashMap<String, u32>,
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
            uses,
            wrapper,
            row,
            Shape::Other,
            name,
            c.lexeme(name),
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
    // The ancestor chain resolves once per container — every member's
    // `isOverride` read consults the same table (the official gathers
    // `members` per container element; the same-file approximation).
    let ancestors = same_file_ancestors(c, row, false);
    for member in member_rows(c, row) {
        match c.kind(member) {
            NodeKind::Constructor | NodeKind::Factory => {
                // An enum's constructors never gather (upstream
                // `member.parent?.parent is! EnumDeclaration`).
                if is_enum {
                    continue;
                }
                judge_constructor(c, face, uses, member, &container);
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
                        // member never gathers. The getter/setter face
                        // reads the field's own name — upstream keys on
                        // `element.getter`, the accessor the variable
                        // carries.
                        if is_override_member(c, &ancestors, c.lexeme(name), MemberQuery::Getter) {
                            continue;
                        }
                        judge(
                            c,
                            face,
                            uses,
                            member,
                            member,
                            Shape::Other,
                            name,
                            c.lexeme(name),
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
                // The `isOverride` arm (:147): a method resolving
                // against an ancestor's member table never gathers.
                if is_override_member(c, &ancestors, lexeme, MemberQuery::Method) {
                    continue;
                }
                judge(
                    c,
                    face,
                    uses,
                    member,
                    member,
                    Shape::Method(is_static),
                    name,
                    lexeme,
                    lexeme,
                    lexeme,
                );
            }
            _ => {}
        }
    }
}

/// Whether `name` resolves against any ancestor's member table — the
/// `isOverride` approximation (`getOverridden` across the same-file
/// interface chain). `concrete=false` joins `implements` links, the
/// official `InterfaceElement.getOverridden` semantics.
fn is_override_member(c: &Check<'_>, ancestors: &[u32], name: &str, query: MemberQuery) -> bool {
    ancestors
        .iter()
        .any(|&ancestor| !member_matches(c, ancestor, query, name).is_empty())
}

/// Judges one constructor: the display name is `Type.written`, the
/// unnamed face `Type.new` (upstream `nameForError`'s
/// `self.name?.lexeme ?? 'new'`), anchored at the name token.
fn judge_constructor(
    c: &mut Check<'_>,
    face: &ReachabilityFace<'_>,
    uses: &rustc_hash::FxHashMap<String, u32>,
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
        uses,
        row,
        row,
        Shape::Constructor,
        name_token,
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
    uses: &rustc_hash::FxHashMap<String, u32>,
    wrapper: u32,
    annotation_row: u32,
    shape: Shape,
    name_token: u32,
    public: &str,
    counted: &str,
    display: &str,
) {
    // 1. Unreachable (the face's flag; `None` — outside the face's
    // universe — never reports).
    let offset = utf16_token_span(c, name_token).start;
    if face.is_reachable(offset) != Some(false) {
        return;
    }
    // 1.5. Counted use (the anti-reach arm): a candidate the face's
    // under-reach flagged unreachable but whose name has a same-file
    // counting position reads as reachable — a plain type reference or
    // a pattern name never counts (upstream's visitor split), an
    // expression or constructor use does. Constructors count under the
    // qualified `C.name` key AND the bare member (a `: this.one()`
    // redirect writes `one`, never `C.one`).
    let counted_count = uses.get(counted).copied().unwrap_or(0)
        + if counted == public {
            0
        } else {
            uses.get(public).copied().unwrap_or(0)
        };
    if counted_count > 0 {
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
/// reading — an unnamed constructor's class-name echo carries the same
/// context, the caller compares against the container name).
fn constructor_name_node(c: &Check<'_>, row: u32) -> Option<u32> {
    let ctx = dartq_parser::IdentifierContext::MethodDeclarationContinuation as u32;
    c.collect_direct(row).into_iter().find(|&kid| {
        c.kind(kid) == NodeKind::Identifier && c.field(kid, PayloadField::Context) == Some(ctx)
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

/// The name → counting-position index of the unit: every `Identifier`
/// row whose [`dartq_parser::IdentifierContext`] marks a reference
/// position (`Expression`, `ExpressionContinuation`,
/// `ConstructorReference`, `ConstructorReferenceContinuation`,
/// `ConstructorReferenceContinuationAfterTypeArguments`, or
/// `PatternFieldName`) contributes its lexeme, plus every doc-comment
/// `[Name]` reference (the official resolves comment references
/// against the element model — a `/// See [C]` read marks `C` used).
/// Declaration contexts, directive names, label positions, and plain
/// `TypeReference`/`PrefixedTypeReference` rows do not count — the
/// official's `SimpleIdentifier`-vs-`NamedType` split
/// (`unreachable_from_main.dart` `visitNamedType` :307-346 keeps a
/// plain type annotation out of the use set; `visitSimpleIdentifier`
/// :386-394 keeps every expression reference in it).
fn counting_uses(c: &Check<'_>) -> rustc_hash::FxHashMap<String, u32> {
    use dartq_parser::IdentifierContext as Ctx;
    const EXPRESSION: u32 = Ctx::Expression as u32;
    const EXPRESSION_CONT: u32 = Ctx::ExpressionContinuation as u32;
    const CTOR: u32 = Ctx::ConstructorReference as u32;
    const PATTERN_FIELD: u32 = Ctx::PatternFieldName as u32;
    let mut uses: rustc_hash::FxHashMap<String, u32> = rustc_hash::FxHashMap::default();
    let count = |uses: &mut rustc_hash::FxHashMap<String, u32>, key: String| {
        *uses.entry(key).or_default() += 1;
    };
    let Some(root) = c.unit.ast.root() else {
        return uses;
    };
    for row in c.children(root) {
        if c.kind(row) != NodeKind::Identifier {
            continue;
        }
        let Some(ctx) = c.field(row, PayloadField::Context) else {
            continue;
        };
        // Upstream's `_patternLevel` (:170, :237-243): a reference
        // inside a `ConstantPattern` subtree never counts — a constant
        // pattern's `C()` or `name` is matched against, never invoked.
        // The pattern-field name is exempt (visitPatternField carries
        // no level gate).
        if ctx != PATTERN_FIELD && inside_constant_pattern(c, row) {
            continue;
        }
        if ctx == EXPRESSION || ctx == EXPRESSION_CONT || ctx == PATTERN_FIELD {
            count(&mut uses, c.lexeme(c.head_token(row)).to_owned());
            continue;
        }
        // A `ConstructorReferenceContinuation` counts its written
        // member name (the `: this.one()` redirect names `one` — the
        // qualified `C.one` key is the head arm's job; the bare lexeme
        // covers the redirect and the `super.name()` faces the head
        // arm cannot key).
        if ctx == Ctx::ConstructorReferenceContinuation as u32
            || ctx == Ctx::ConstructorReferenceContinuationAfterTypeArguments as u32
        {
            count(&mut uses, c.lexeme(c.head_token(row)).to_owned());
            continue;
        }
        if ctx != CTOR {
            continue;
        }
        // A constructor head counts the class and its member: `C()`
        // marks `C` and `C.new`, `C.two()` marks `C` and `C.two`. The
        // tokens after the head's span decide the arm — `.name` reads
        // the written continuation, `(`/`else` reads the unnamed form
        // (upstream's `ConstructorName` element split: the
        // constructor the invocation resolves is the use).
        let lexeme = c.lexeme(c.head_token(row));
        count(&mut uses, lexeme.to_owned());
        // The member token sits after the head's own type arguments:
        // `C<T>.named` reads the `.named`, `C()` and `C<T>()` read
        // `new`. The scan walks the balanced `<…>` run off the token
        // stream (the row's span ends at `C`; a `>` deeper than it
        // opened closes the args).
        let mut dot = c.unit.ast.token_range(row).end;
        if c.lexeme(dot) == "<" {
            let last = c.unit.tokens.len().saturating_sub(1) as u32;
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
            dot = (dot + 1).min(last);
        }
        let member = if dot + 1 < c.unit.tokens.len() as u32 && c.lexeme(dot) == "." {
            c.lexeme(dot + 1).to_owned()
        } else {
            "new".to_owned()
        };
        count(&mut uses, format!("{lexeme}.{member}"));
    }
    // The doc-comment `[Name]` face: a bracketed name in any comment
    // reads as a reference. `Comment.range` is byte offsets into the
    // source — the scan walks the comment text once per unit.
    for comment in c.comments.all() {
        let text = &c.unit.source[comment.range.start as usize..comment.range.end as usize];
        let bytes = text.as_bytes();
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
                    *uses.entry(text[start..end].to_owned()).or_default() += 1;
                    i = end;
                    continue;
                }
            }
            i += 1;
        }
    }
    uses
}

/// Whether `row` sits inside a `ConstantPattern` subtree — the
/// `_patternLevel` arm the use index consults (parent walk; the
/// subtree boundary is the pattern row's own completion window).
fn inside_constant_pattern(c: &Check<'_>, row: u32) -> bool {
    let mut current = row;
    while let Some(parent) = c.unit.ast.parent(current) {
        if c.kind(parent) == NodeKind::ConstantPattern {
            return true;
        }
        current = parent;
    }
    false
}
