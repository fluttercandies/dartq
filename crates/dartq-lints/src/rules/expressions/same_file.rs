use super::{Check, NodeKind, PayloadField, query_site, slice};

/// `ModSet` member family: static is bit 3 (`mod_bits` in
/// the bridge; [`dartq_ast::mod_flags`] reads the header bits back).
pub(crate) const STATIC_BIT: u16 = 1 << 3;

/// One same-file class-like declaration: the name token, the written
/// superclass-chain names (extends / mixin `on` / the `=` name of a
/// mixin application), and the with + implements names.
pub(crate) struct ClassDecl {
    name: u32,
    supers: Vec<u32>,
    clauses: Vec<u32>,
    /// Whether the declaration binds the TYPE namespace for the lint
    /// faces that consume this index — the upstream `_isTypeName`
    /// element whitelist (ClassElement/ExtensionTypeElement/
    /// TypeAliasElement/TypeParameterElement; `pkg/analyzer`
    /// element.dart :1073/:2460). A `mixin` or `enum` declaration
    /// still enters `classes` for the supertype-resolution walks but
    /// stays out of `has_class`/`has_type`.
    type_named: bool,
}

/// One same-file extension declaration: the member name tokens, the
/// `on` clause's written type name token, and whether that type is
/// written nullable.
pub(crate) struct ExtensionDecl {
    members: Vec<u32>,
    /// The `on` type's head name token (`None` when the clause never
    /// produced an honest name — the face can never apply).
    on_name: Option<u32>,
    extended_nullable: bool,
}

/// The unit's type declarations: class-like shapes plus typedef names
/// (a local typedef shadows any imported name in the combined
/// namespace), the same-file extensions, and the top-level value name
/// tokens `declares_name` answers from. Built once per run (the #703 F4
/// hoist — candidates no longer re-walk the top-level declarations);
/// the rows are token ids, and name queries compare lexemes over them
/// in declaration order.
pub(crate) struct Decls {
    classes: Vec<ClassDecl>,
    typedefs: Vec<u32>,
    extensions: Vec<ExtensionDecl>,
    /// Top-level function/getter/setter/variable name tokens.
    values: Vec<u32>,
}

/// The unit's same-file declaration index — built once per run on first
/// query and cached in the run's [`crate::engine::RunCaches`] (the
/// #703 F4 hoist). The returned borrow is a read-only view; re-entrant
/// calls share it. The build is the single whole-unit walk; queries
/// scan the cached token rows. Module-internal: the query faces below
/// borrow it for their read and hand back owned answers, so no rule
/// ever holds a cache guard.
fn scan<'a>(c: &'a Check<'a>) -> core::cell::Ref<'a, Decls> {
    let cached = c.caches.decls.borrow();
    if cached.is_some() {
        return core::cell::Ref::map(cached, |slot| slot.as_ref().expect("present above"));
    }
    drop(cached);
    *c.caches.decls.borrow_mut() = Some(build(c));
    let cache = c.caches.decls.borrow();
    core::cell::Ref::map(cache, |slot| slot.as_ref().expect("just built"))
}

/// The one whole-unit walk behind [`scan`]: the previous per-candidate
/// `scan` body plus the second walk `declares_name` used to do per
/// query, folded into the same pass over the top-level wrappers.
fn build(c: &Check<'_>) -> Decls {
    let mut decls = Decls {
        classes: Vec::new(),
        typedefs: Vec::new(),
        extensions: Vec::new(),
        values: Vec::new(),
    };
    let Some(root) = c.path.first().copied() else {
        return decls;
    };
    for top in c.collect_direct(root) {
        // The wrapper's direct children: class-like families name
        // through a preceding Identifier sibling, so match every child
        // by kind instead of peeling a single "the declaration" child.
        for inner in c.collect_direct(top) {
            match c.kind(inner) {
                NodeKind::Typedef => {
                    if let Some(name) = c
                        .collect_direct(inner)
                        .into_iter()
                        .find(|&kid| c.kind(kid) == NodeKind::Identifier)
                    {
                        decls.typedefs.push(c.head_token(name));
                    }
                }
                NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::NamedMixinApplication
                | NodeKind::ExtensionTypeDeclaration
                | NodeKind::EnumDeclaration => {
                    if let Some(decl) = class_decl(c, top, inner) {
                        decls.classes.push(decl);
                    }
                }
                NodeKind::ExtensionDeclaration => {
                    decls.extensions.push(extension_decl(c, inner));
                }
                // Top-level function/getter/setter names (a written
                // return type is a `TypeReference` identifier, filtered
                // out here).
                NodeKind::TopLevelMethod | NodeKind::TopLevelFields => {
                    let ctx = if c.kind(inner) == NodeKind::TopLevelMethod {
                        dartq_parser::IdentifierContext::TopLevelFunctionDeclaration
                    } else {
                        dartq_parser::IdentifierContext::TopLevelVariableDeclaration
                    } as u32;
                    for kid in c.collect_direct(inner) {
                        if c.kind(kid) == NodeKind::Identifier
                            && c.field(kid, PayloadField::Context) == Some(ctx)
                        {
                            decls.values.push(c.head_token(kid));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    decls
}

impl Decls {
    /// Whether a class-like declaration carries `name` (the upstream
    /// `_isTypeName` whitelist — a `mixin`/`enum` declaration is
    /// scanned into `classes` for the supertype walks but does not
    /// bind the type namespace).
    #[must_use]
    fn has_class(&self, c: &Check<'_>, name: &str) -> bool {
        self.classes
            .iter()
            .any(|d| d.type_named && c.lexeme(d.name) == name)
    }

    /// Whether any type declaration (class-like or typedef) carries
    /// `name`.
    #[must_use]
    fn has_type(&self, c: &Check<'_>, name: &str) -> bool {
        self.has_class(c, name) || self.typedefs.iter().any(|&tok| c.lexeme(tok) == name)
    }

    /// Whether the unit declares `name` in any namespace — types plus
    /// top-level values (functions, getters, variables).
    #[must_use]
    fn declares_name(&self, c: &Check<'_>, name: &str) -> bool {
        self.has_type(c, name) || self.values.iter().any(|&tok| c.lexeme(tok) == name)
    }
}

/// Whether the unit declares `name` in any namespace — types plus
/// top-level values (functions, getters, variables). A local declaration
/// shadows the imported face in every namespace, which is what the
/// annotation rules (`library_annotations`, `provide_deprecation_message`)
/// and `unnecessary_this` bail on.
#[must_use]
pub fn declares_name(c: &Check<'_>, name: &str) -> bool {
    scan(c).declares_name(c, name)
}

/// How a written type name resolves over the same-file declarations.
pub(crate) enum Verdict {
    /// The name (or a same-file supertype chain over it) lands on the
    /// core type carried in `Core` — an unshadowed written name is the
    /// library type.
    Core(&'static str),
    /// Provably not the core family: a same-file declaration on the
    /// chain shadows the core name, or the chain provably dead-ends.
    NotCore,
    /// A supertype (or the name itself) resolves outside the file — the
    /// caller's documented fallback family decides.
    Unknown,
}

/// Resolves `name` as an instance receiver: all written supertypes
/// (extends / on / with / implements) walk — the official
/// `element.allSupertypes` face (null_closures.dart:245-250). `core` is
/// the set of written names the caller's table keys on.
pub(crate) fn resolve(c: &Check<'_>, name: &str, core: &[&'static str]) -> Verdict {
    let decls = scan(c);
    walk(c, &decls, name, false, core, &mut Vec::new())
}

/// Resolves `name` as a creation receiver: the superclass chain only —
/// the official `extendsClass` walk (extensions.dart:305-353).
pub(crate) fn resolve_super(c: &Check<'_>, name: &str, core: &[&'static str]) -> Verdict {
    let decls = scan(c);
    walk(c, &decls, name, true, core, &mut Vec::new())
}

/// The transitive written supertype name set of `name` (extends/with/
/// implements — the [`walk`] sequence) — the user-class-vs-user-class
/// comparability face the `unrelated_type_equality_checks` same-file
/// pair needs (`typesAreUnrelated` shares any supertype link).
/// `None` when the chain is incomplete: a supertype resolves outside
/// the file, and the unindexed link could reach the other name.
pub(crate) fn supertype_closure(c: &Check<'_>, name: &str) -> Option<Vec<String>> {
    let decls = scan(c);
    let mut names = Vec::new();
    let mut unknown = false;
    collect_chain(c, &decls, name, &mut names, &mut unknown, &mut Vec::new());
    (!unknown).then_some(names)
}

/// The [`supertype_closure`] collector: every distinct supertype name
/// on the transitive chain; a not-same-file link marks the closure
/// incomplete.
fn collect_chain<'a>(
    c: &Check<'a>,
    decls: &Decls,
    name: &'a str,
    out: &mut Vec<String>,
    unknown: &mut bool,
    seen: &mut Vec<&'a str>,
) {
    if seen.contains(&name) {
        return; // cyclic same-file chains are erroneous code
    }
    seen.push(name);
    let Some(decl) = decls.classes.iter().find(|d| c.lexeme(d.name) == name) else {
        *unknown = true;
        return;
    };
    for token in decl.supers.iter().chain(decl.clauses.iter()) {
        let spelled = c.lexeme(*token);
        if !out.iter().any(|n| n == spelled) {
            out.push(spelled.to_owned());
        }
        collect_chain(c, decls, spelled, out, unknown, seen);
    }
}

/// The exact-name verdict: the static-call face matches
/// `element.name == function.type` with no supertype walk
/// (null_closures.dart:224-233), so a same-file class is never the core
/// element even when it lexically shares the name.
#[must_use]
pub fn direct(c: &Check<'_>, name: &str, core: &[&'static str]) -> Verdict {
    if scan(c).has_class(c, name) {
        return Verdict::NotCore;
    }
    verdict_of(core, name)
}

fn verdict_of(core: &[&'static str], name: &str) -> Verdict {
    core.iter()
        .find(|&&k| k == name)
        .map_or(Verdict::Unknown, |&k| Verdict::Core(k))
}

fn walk<'a>(
    c: &Check<'a>,
    decls: &Decls,
    name: &'a str,
    supers_only: bool,
    core: &[&'static str],
    seen: &mut Vec<&'a str>,
) -> Verdict {
    if seen.contains(&name) {
        // Cyclic same-file chains are erroneous code; provably no core
        // behind them.
        return Verdict::NotCore;
    }
    seen.push(name);
    let Some(decl) = decls.classes.iter().find(|d| c.lexeme(d.name) == name) else {
        // Not same-file: an unshadowed written core name is the library
        // type; anything else resolves outside the file.
        return verdict_of(core, name);
    };
    // A same-file declaration is never the core element — only its
    // written supertypes can reach the core family.
    let chain: Vec<u32> = if supers_only {
        decl.supers.clone()
    } else {
        decl.supers
            .iter()
            .chain(decl.clauses.iter())
            .copied()
            .collect()
    };
    let mut unknown = false;
    for token in chain {
        match walk(c, decls, c.lexeme(token), supers_only, core, seen) {
            Verdict::Core(core_name) => return Verdict::Core(core_name),
            Verdict::Unknown => unknown = true,
            Verdict::NotCore => {}
        }
    }
    if unknown {
        Verdict::Unknown
    } else {
        Verdict::NotCore
    }
}

/// The scope family of the innermost same-file value binding. The
/// promotable split is upstream's: only locals and parameters are
/// `PromotableElementImpl` (upstream Dart SDK source,
/// `pkg/analyzer/lib/src/dart/element/element.dart` :3981/:8421/:9947);
/// every field or top-level variable read resolves to a `GetterElement`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BindingScope {
    /// A local declared in a block before the use.
    Local,
    /// A formal parameter of the enclosing function.
    Parameter,
    /// An instance field of the enclosing class-like scope.
    Field,
    /// A static field of the enclosing class-like scope.
    StaticField,
    /// A top-level variable.
    TopLevel,
}

/// The innermost value binding of `name` at use site `use_node`:
/// `(scope, written type row)` — the row `None` for an untyped binding,
/// the lookup `None` when no same-file binding exists. The order is
/// identifier resolution: locals declared before the use, parameters,
/// fields (instance and static), top-level variables.
#[must_use]
pub(crate) fn binding(
    c: &Check<'_>,
    use_node: u32,
    name: &str,
) -> Option<(BindingScope, Option<u32>)> {
    let use_start = c.byte_span(use_node).start;
    let len = c.path.len();
    for i in (0..len.saturating_sub(1)).rev() {
        let ancestor = c.path[i];
        match c.kind(ancestor) {
            // The production tiling keeps a block's statements flat in
            // the owner slice — locals precede the use as pieces.
            NodeKind::Block | NodeKind::BlockBody => {
                if let Some(row) = block_binding(c, ancestor, use_start, name) {
                    return Some((BindingScope::Local, row));
                }
            }
            NodeKind::FormalParameters => {
                if let Some(row) = parameter_binding(c, ancestor, name) {
                    return Some((BindingScope::Parameter, row));
                }
            }
            // Class-member fields and the member's own parameters: the
            // production tiling keeps both as owner-slice pieces of the
            // member. Order-independent — members scope whole.
            NodeKind::Method
            | NodeKind::Constructor
            | NodeKind::Factory
            | NodeKind::PrimaryConstructorBody
            | NodeKind::TopLevelMethod
            | NodeKind::FunctionExpression
            | NodeKind::NamedFunctionExpression
            | NodeKind::LocalFunctionDeclaration
                if i > 0 =>
            {
                if let Some(params) = c
                    .collect_direct(ancestor)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
                    && let Some(row) = parameter_binding(c, params, name)
                {
                    return Some((BindingScope::Parameter, row));
                }
                if let Some((is_static, row)) = field_binding(c, c.path[i - 1], name) {
                    let scope = if is_static {
                        BindingScope::StaticField
                    } else {
                        BindingScope::Field
                    };
                    return Some((scope, row));
                }
            }
            NodeKind::CompilationUnit => {
                if let Some(row) = top_level_field_binding(c, ancestor, name) {
                    return Some((BindingScope::TopLevel, row));
                }
            }
            _ => {}
        }
    }
    None
}

/// [`binding`] for a rule dispatched AT a block row
/// (`cascade_invocations` keys on `Block`/`BlockBody`): the dispatched
/// block is itself a binding scope — the standard walk excludes
/// `path.last()`, which is exactly the row the statement-level rules
/// dispatch on. Falls through to [`binding`] for the outer scopes.
#[must_use]
pub(crate) fn binding_in_block(
    c: &Check<'_>,
    use_node: u32,
    name: &str,
) -> Option<(BindingScope, Option<u32>)> {
    let node = c.node();
    if matches!(
        c.kind(node),
        NodeKind::Block | NodeKind::BlockBody | NodeKind::SwitchCase
    ) && let Some(row) = block_binding(c, node, c.byte_span(use_node).start, name)
    {
        return Some((BindingScope::Local, row));
    }
    binding(c, use_node, name)
}

/// The initializer range of the innermost `const` LOCAL binding of
/// `name` at use site `use_node`, `None` when the name binds to no
/// const local before the use (or carries no initializer). The
/// scope chain is the block walk of [`binding`]: innermost block
/// first, shadowing wins. (`binding` answers the same lookup for the
/// written type; this face keeps the initializer for the constant-
/// value fold the type-literal test needs.)
#[must_use]
pub(crate) fn local_const_initializer(
    c: &Check<'_>,
    use_node: u32,
    name: &str,
) -> Option<core::ops::Range<u32>> {
    let use_start = c.byte_span(use_node).start;
    let len = c.path.len();
    let ctx = dartq_parser::IdentifierContext::LocalVariableDeclaration as u32;
    for i in (0..len.saturating_sub(1)).rev() {
        let ancestor = c.path[i];
        if !matches!(c.kind(ancestor), NodeKind::Block | NodeKind::BlockBody) {
            continue;
        }
        // The innermost block declaring the name decides — a non-const
        // binding there shadows any outer const.
        for kid in c.collect_direct(ancestor) {
            if c.kind(kid) != NodeKind::VariablesDeclaration {
                continue;
            }
            for var in c
                .collect_direct(kid)
                .into_iter()
                .filter(|&var| c.kind(var) == NodeKind::InitializedIdentifier)
            {
                // The name: nested inside the row (hand-built trees), or
                // the production tiling — the name `Identifier` is a
                // direct child of the declaration row and anchors the
                // `InitializedIdentifier` row at the same token, so the
                // row's span head is the name.
                let binds = c.collect_direct(var).into_iter().any(|piece| {
                    c.kind(piece) == NodeKind::Identifier
                        && c.field(piece, PayloadField::Context) == Some(ctx)
                        && c.lexeme(c.head_token(piece)) == name
                        && c.byte_span(piece).start < use_start
                }) || (crate::rules::initialized_identifier_name(c, var)
                    .is_some_and(|tok| c.lexeme(tok) == name)
                    && c.byte_span(var).start < use_start);
                if binds
                    && let Some(name_row) = crate::rules::initialized_identifier_name(c, var)
                    && c.lexeme(name_row) == name
                {
                    let const_kw = crate::rules::variables_declaration_keyword(c, kid)
                        .is_some_and(|kw| c.lexeme(kw) == "const");
                    return const_kw.then(|| {
                        crate::rules::flow::exhaustive_cases::initializer_range(c, var)
                    })?;
                }
            }
        }
    }
    None
}

/// The written type of the identifier `use` bound to `name`: the
/// innermost local/parameter/field/top-level declaration carrying an
/// explicit type. Untyped bindings (`var`, patterns, for-in) return
/// `None` — the caller falls back.
#[must_use]
pub fn written_type<'a>(c: &'a Check<'a>, use_node: u32, name: &str) -> Option<&'a str> {
    let row = binding(c, use_node, name)?.1?;
    // A named type only: a function-type head spells the return type,
    // not the variable's type name.
    if c.kind(row) != NodeKind::Type {
        return None;
    }
    Some(c.lexeme(c.head_token(row)))
}

/// Whether any same-file type declaration (class-like or typedef)
/// carries `name` — the type-namespace half of [`declares_name`].
#[must_use]
pub(crate) fn declares_type(c: &Check<'_>, name: &str) -> bool {
    scan(c).has_type(c, name)
}

/// Whether `name` binds to a same-file VALUE at `use_node` — a local,
/// parameter, field, or top-level variable ([`binding`]), or a
/// top-level function/getter/setter (the values rows). The
/// type-literal faces use it to tell a type literal
/// (`(List<int>)`) from a same-file generic tear-off (`(fn<int>)`).
#[must_use]
pub(crate) fn declares_value(c: &Check<'_>, use_node: u32, name: &str) -> bool {
    binding(c, use_node, name).is_some() || scan(c).values.iter().any(|&tok| c.lexeme(tok) == name)
}

/// The resolve-then-judge verdict for the paren's trailing accessor
/// (the unnecessary_parenthesis null-aware-shorting faces, upstream
/// Dart SDK source, pkg/linter/lib/src/rules/unnecessary_parenthesis.dart
/// :117-124, :132-139): `true` keeps the parentheses, `false` lets the
/// one-token report stand. The accessor is judged per RESOLVED member
/// over the receiver's written type — never per name — so two
/// same-named members whose `on` types differ in nullability judge
/// independently:
///
/// 1. a cross-file interface member (the semantic member walk) is not
///    an extension application — upstream reports;
/// 2. a same-file class chain declaring the name wins over every
///    extension application (the oracle resolves the interface face
///    first) — upstream reports;
/// 3. the applicable same-file extension declarers split by `on`-type
///    nullability: a non-nullable `on` type applies only where the
///    nullable one does, so it is always the more specific resolution
///    and upstream reports; a nullable-only split is the bail face —
///    the parentheses are what stops the null-aware shorting;
/// 4. an accessor no same-file face declares may resolve through an
///    import (another library's extension pool, which a file-local
///    scan cannot see): upstream resolves through imports and stays
///    silent over a nullable on type, so the same keep covers it (the
///    genuinely undefined accessor under-reports by the same keep).
#[must_use]
pub(crate) fn nullable_extension_member(c: &Check<'_>, paren: u32, name: &str) -> bool {
    // The bare-name receiver tiles in the four-piece send-group form
    // (`parse_send` emits the full group for every bare name, inside
    // the parentheses too) — strip the wrapper pieces before the
    // single-row face matches.
    let run = slice::access_run(c, &c.collect_direct(paren));
    let Some(&head) = run.first() else {
        return true;
    };
    // The cross-file class face (the semantic member walk first — the
    // receiver's resolved type decides across files): a resolved
    // interface member is never an extension application. The read row
    // exists for a bare identifier receiver only.
    if run.len() == 1
        && c.kind(head) == NodeKind::Identifier
        && let Some(analysis) = c.semantics()
        && analysis
            .member_of(query_site(c, head), analysis.strings().intern(name))
            .is_some()
    {
        return false;
    }
    let Some((ty_name, ty_nullable)) = receiver_written_type(c, &run) else {
        // The receiver's type is not decidable from this file (an
        // imported head, a deeper chain, an untyped binding): the
        // accessor may resolve through imports — keep the parens.
        return true;
    };
    // Extensions never apply to `dynamic` (the oracle's element is
    // null) — the one-token face reports.
    if ty_name == "dynamic" {
        return false;
    }
    // The interface member wins over every extension application.
    if class_chain_declares(c, ty_name, name, &mut Vec::new()) {
        return false;
    }
    // The same-file extension pool, judged per resolved member.
    let scan = scan(c);
    let mut nullable = 0usize;
    let mut non_nullable = 0usize;
    for decl in &scan.extensions {
        let Some(on_name) = decl.on_name else {
            continue;
        };
        // Applicability at name level: the written on type names the
        // receiver's type, and a non-nullable on type never applies to
        // a nullable receiver (`String? <: String` is false).
        if c.lexeme(on_name) != ty_name
            || (ty_nullable && !decl.extended_nullable)
            || !decl.members.iter().any(|&tok| c.lexeme(tok) == name)
        {
            continue;
        }
        if decl.extended_nullable {
            nullable += 1;
        } else {
            non_nullable += 1;
        }
    }
    if non_nullable > 0 {
        // The most specific resolution's on type is non-nullable —
        // upstream falls through and reports.
        return false;
    }
    if nullable > 0 {
        // The resolved member's on type is nullable — the parentheses
        // are required (:117-124, :132-139).
        return true;
    }
    // No same-file face declares the accessor: an import may carry it.
    // Upstream resolves through imports and keeps the parens over a
    // nullable on type; the same keep covers the genuinely undefined
    // accessor (the documented under-report).
    true
}

/// The written type face of the parenthesized receiver run — the
/// `(name, nullable)` the accessor resolves against. Only a bare
/// identifier receiver is decidable here: it reads its innermost
/// binding. Every other shape — access chains, invocations, `this` —
/// stays `None` (the caller's conservative keep): the run is built by
/// [`slice::access_run`], which strips the access markers, so no access
/// member's type can be read off this run.
fn receiver_written_type<'a>(c: &'a Check<'a>, run: &[u32]) -> Option<(&'a str, bool)> {
    let Some(&head) = run.first() else {
        return None;
    };
    if c.kind(head) != NodeKind::Identifier {
        return None;
    }
    let face = crate::rules::written_type_face(c, head, c.lexeme(c.head_token(head)))?;
    Some((face.name?, face.nullable))
}

/// Whether the same-file class-like chain rooted at `name` declares
/// `member` — the interface face the oracle resolves before any
/// extension application. A cyclic chain is erroneous code; provably
/// no member behind it (the bounded `seen` guard).
fn class_chain_declares<'a>(
    c: &'a Check<'a>,
    name: &'a str,
    member: &str,
    seen: &mut Vec<&'a str>,
) -> bool {
    if seen.contains(&name) {
        return false;
    }
    seen.push(name);
    let decls = scan(c);
    let Some(decl) = decls.classes.iter().find(|d| c.lexeme(d.name) == name) else {
        return false;
    };
    if crate::rules::same_file_declaration(c, name)
        .is_some_and(|row| body_declares_member(c, row, member))
    {
        return true;
    }
    decl.supers
        .iter()
        .any(|&sup| class_chain_declares(c, c.lexeme(sup), member, seen))
}

/// Whether the class-like `row` declares `member` in its own body
/// (methods and fields alike — a resolved static face reports the
/// same way upstream).
fn body_declares_member(c: &Check<'_>, row: u32, member: &str) -> bool {
    crate::rules::body_members(c, row)
        .into_iter()
        .any(|kid| match c.kind(kid) {
            NodeKind::Method => crate::rules::member_name(c, kid)
                .is_some_and(|name_row| c.lexeme(c.head_token(name_row)) == member),
            NodeKind::Fields => c.collect_direct(kid).into_iter().any(|decl| {
                c.kind(decl) == NodeKind::VariablesDeclaration
                    && c.collect_direct(decl).into_iter().any(|var| {
                        c.kind(var) == NodeKind::InitializedIdentifier
                            && crate::rules::initialized_identifier_name(c, var)
                                .is_some_and(|name_row| c.lexeme(name_row) == member)
                    })
            }),
            _ => false,
        })
}

/// The explicit type of a local declared in `block` before `use_start`.
/// The production shape: the name identifier carries the local-variable
/// context word inside its `VariablesDeclaration` slice, typed by the
/// nearest preceding `Type` piece.
#[expect(
    clippy::option_option,
    reason = "three states by design: name absent (outer None) vs \
                  present-untyped vs present-typed; callers branch on the \
                  outer layer first"
)]
fn block_binding(c: &Check<'_>, block: u32, use_start: usize, name: &str) -> Option<Option<u32>> {
    let ctx = dartq_parser::IdentifierContext::LocalVariableDeclaration as u32;
    let mut written: Option<u32> = None;
    for kid in c.collect_direct(block) {
        match c.kind(kid) {
            NodeKind::Type | NodeKind::FunctionType | NodeKind::VoidType => written = Some(kid),
            NodeKind::VariablesDeclaration => {
                // The name identifier: a direct child (hand-built trees)
                // or nested inside an `InitializedIdentifier` row (the
                // production declaration tiling).
                let binds = c.collect_direct(kid).into_iter().any(|var| {
                    (c.kind(var) == NodeKind::Identifier
                        && c.field(var, PayloadField::Context) == Some(ctx)
                        && c.lexeme(c.head_token(var)) == name
                        && c.byte_span(var).start < use_start)
                        || (c.kind(var) == NodeKind::InitializedIdentifier
                            && c.collect_direct(var).into_iter().any(|piece| {
                                c.kind(piece) == NodeKind::Identifier
                                    && c.field(piece, PayloadField::Context) == Some(ctx)
                                    && c.lexeme(c.head_token(piece)) == name
                                    && c.byte_span(piece).start < use_start
                            }))
                });
                if binds {
                    return Some(written);
                }
            }
            _ => {}
        }
    }
    None
}

/// The explicit type of a formal parameter.
#[expect(
    clippy::option_option,
    reason = "three states by design: name absent (outer None) vs \
                  present-untyped vs present-typed; callers branch on the \
                  outer layer first"
)]
fn parameter_binding(c: &Check<'_>, params: u32, name: &str) -> Option<Option<u32>> {
    for kid in c.collect_direct(params) {
        if c.kind(kid) != NodeKind::FormalParameter {
            continue;
        }
        let named = c
            .field(kid, PayloadField::Name)
            .filter(|&tok| tok != crate::rules::NONE_TOKEN)
            .is_some_and(|tok| c.lexeme(tok) == name);
        if !named {
            continue;
        }
        return Some(
            c.collect_direct(kid)
                .into_iter()
                .find(|&t| c.kind(t) == NodeKind::Type),
        );
    }
    None
}

/// The explicit type of a field declared in the owner body, paired with
/// whether the field is `static`. Statics bind the same namespace — a
/// `static` field read resolves to the field's getter element upstream,
/// and an innermost static shadows any outer binding.
fn field_binding(c: &Check<'_>, owner: u32, name: &str) -> Option<(bool, Option<u32>)> {
    for member in c.collect_direct(owner) {
        if c.kind(member) != NodeKind::Fields {
            continue;
        }
        if let Some(row) = fields_binding(c, member, name) {
            return Some((
                dartq_ast::mod_flags(c.unit.ast, member) & STATIC_BIT != 0,
                row,
            ));
        }
    }
    None
}

/// The explicit type of a top-level variable.
#[expect(
    clippy::option_option,
    reason = "three states by design: name absent (outer None) vs \
                  present-untyped vs present-typed; callers branch on the \
                  outer layer first"
)]
fn top_level_field_binding(c: &Check<'_>, unit: u32, name: &str) -> Option<Option<u32>> {
    for top in c.collect_direct(unit) {
        for inner in c.collect_direct(top) {
            if c.kind(inner) == NodeKind::TopLevelFields
                && let Some(row) = fields_binding(c, inner, name)
            {
                return Some(row);
            }
        }
    }
    None
}

/// Shared `Fields` scan: the name identifiers carry the field context
/// (a written type is a `TypeReference` identifier), and the type head
/// precedes each name among the children.
#[expect(
    clippy::option_option,
    reason = "three states by design: name absent (outer None) vs \
                  present-untyped vs present-typed; callers branch on the \
                  outer layer first"
)]
fn fields_binding(c: &Check<'_>, fields: u32, name: &str) -> Option<Option<u32>> {
    let ctx = if c.kind(fields) == NodeKind::TopLevelFields {
        dartq_parser::IdentifierContext::TopLevelVariableDeclaration as u32
    } else {
        dartq_parser::IdentifierContext::FieldDeclaration as u32
    };
    let mut written: Option<u32> = None;
    for kid in c.collect_direct(fields) {
        match c.kind(kid) {
            NodeKind::Type | NodeKind::FunctionType | NodeKind::VoidType => written = Some(kid),
            NodeKind::Identifier
                if c.field(kid, PayloadField::Context) == Some(ctx)
                    && c.lexeme(c.head_token(kid)) == name =>
            {
                return Some(written);
            }
            _ => {}
        }
    }
    None
}

/// One class-like declaration's shape: the name token plus the clause
/// name tokens (read off the header payload words as token bounds).
fn class_decl(c: &Check<'_>, top: u32, inner: u32) -> Option<ClassDecl> {
    let none = crate::rules::NONE_TOKEN;
    // Extension types name through the row's `Name` word; the other
    // families through the wrapper's first Identifier.
    let name = if c.kind(inner) == NodeKind::ExtensionTypeDeclaration {
        let tok = c.field(inner, PayloadField::Name).unwrap_or(none);
        (tok != none).then_some(tok)?
    } else {
        let id = c
            .collect_direct(top)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::Identifier)?;
        c.head_token(id)
    };
    let type_names = |lo: u32, hi: u32| -> Vec<u32> {
        c.collect_direct(inner)
            .into_iter()
            .filter(|&kid| c.kind(kid) == NodeKind::Identifier)
            .map(|kid| c.head_token(kid))
            .filter(|&tok| tok > lo && tok < hi)
            .collect()
    };
    let mut decl = ClassDecl {
        name,
        supers: Vec::new(),
        clauses: Vec::new(),
        // The upstream `_isTypeName` element whitelist binds only
        // ClassElement/ExtensionTypeElement/TypeAliasElement/
        // TypeParameterElement; a `mixin` declaration stays out of
        // `has_class` (but still feeds the supertype walks through
        // `classes`).
        type_named: !matches!(
            c.kind(inner),
            NodeKind::MixinDeclaration | NodeKind::EnumDeclaration
        ),
    };
    if c.kind(inner) == NodeKind::ExtensionTypeDeclaration {
        // The implements names are the direct type-name identifiers
        // (the representation type lives inside the primary
        // constructor's parameters).
        decl.clauses = type_names(0, u32::MAX);
        return Some(decl);
    }
    let (supers_kw, clauses) = match c.kind(inner) {
        // `class P = Q with R implements S` shares the class shape: Q
        // is the superclass (the `=` bounds its run).
        NodeKind::ClassDeclaration => (
            c.field(inner, PayloadField::ExtendsKeyword).unwrap_or(none),
            (
                c.field(inner, PayloadField::WithKeyword).unwrap_or(none),
                c.field(inner, PayloadField::ImplementsKeyword)
                    .unwrap_or(none),
            ),
        ),
        NodeKind::NamedMixinApplication => (
            c.field(inner, PayloadField::Equals).unwrap_or(none),
            (
                c.field(inner, PayloadField::WithKeyword).unwrap_or(none),
                c.field(inner, PayloadField::ImplementsKeyword)
                    .unwrap_or(none),
            ),
        ),
        // `mixin M on A with B`: the on-clause is a superclass
        // constraint; the with-clause an interface face.
        NodeKind::MixinDeclaration => (
            c.field(inner, PayloadField::OnKeyword).unwrap_or(none),
            (
                c.field(inner, PayloadField::WithKeyword).unwrap_or(none),
                none,
            ),
        ),
        _ => return None,
    };
    if supers_kw != none {
        decl.supers = type_names(supers_kw, clauses.0.min(clauses.1).max(supers_kw + 1));
    }
    if clauses.0 != none {
        decl.clauses
            .extend(type_names(clauses.0, clauses.1.max(clauses.0 + 1)));
    }
    if clauses.1 != none {
        decl.clauses.extend(type_names(clauses.1, u32::MAX));
    }
    Some(decl)
}

/// One extension declaration's shape ([`ExtensionDecl`]): the `on`
/// type's nullability read lexically off the header tokens (the
/// built-in `on` keyword's next token names the extended type; a `?`
/// directly after it, or after a generic's closing `>`, spells the
/// nullable face), and the member names off the body rows — methods
/// through the method-declaration context identifier, fields through
/// the field-declaration context children (the validated
/// [`fields_binding`] tiling).
fn extension_decl(c: &Check<'_>, inner: u32) -> ExtensionDecl {
    let end = c.unit.ast.token_range(inner).end;
    let mut on_name = None;
    let mut extended_nullable = false;
    let mut token = c.head_token(inner);
    for _ in 0..64 {
        if token >= end {
            break;
        }
        match c.lexeme(token) {
            "{" | ";" => break,
            "on" => {
                on_name = (token + 1 < end).then_some(token + 1);
                extended_nullable = written_nullable(c, token + 1, end);
                break;
            }
            _ => token += 1,
        }
    }
    let mut members = Vec::new();
    if let Some(body) = c
        .collect_direct(inner)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::DeclarationBody)
    {
        let method_ctx = dartq_parser::IdentifierContext::MethodDeclaration as u32;
        let field_ctx = dartq_parser::IdentifierContext::FieldDeclaration as u32;
        for row in c.children(body) {
            match c.kind(row) {
                NodeKind::Method => {
                    if let Some(name) = c.collect_direct(row).into_iter().find(|&kid| {
                        c.kind(kid) == NodeKind::Identifier
                            && c.field(kid, PayloadField::Context) == Some(method_ctx)
                    }) {
                        members.push(c.head_token(name));
                    }
                }
                NodeKind::Fields => {
                    for kid in c.collect_direct(row) {
                        if c.kind(kid) == NodeKind::Identifier
                            && c.field(kid, PayloadField::Context) == Some(field_ctx)
                        {
                            members.push(c.head_token(kid));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    ExtensionDecl {
        members,
        on_name,
        extended_nullable,
    }
}

/// Whether the extended-type spelling starting at `name` carries a
/// written `?` — directly (`String?`), past a generic's closing `>`
/// (`List<String>?`), or past a prefixed name's dots (`p.T?`).
/// Bounded by the header's token extent.
fn written_nullable(c: &Check<'_>, name: u32, end: u32) -> bool {
    if name >= end {
        return false;
    }
    match c.lexeme(name) {
        "<" => {
            let mut depth = 1usize;
            let mut token = name + 1;
            for _ in 0..64 {
                if token >= end {
                    return false;
                }
                match c.lexeme(token) {
                    "<" => depth += 1,
                    ">" => {
                        depth -= 1;
                        if depth == 0 {
                            return token + 1 < end && c.lexeme(token + 1) == "?";
                        }
                    }
                    _ => {}
                }
                token += 1;
            }
            false
        }
        "." => false, // degenerate (the walk anchored mid-prefix)
        _ => {
            // A prefixed name walks to its last identifier first.
            let mut token = name;
            while token + 2 < end && c.lexeme(token + 1) == "." {
                token += 2;
            }
            token + 1 < end && c.lexeme(token + 1) == "?"
        }
    }
}
