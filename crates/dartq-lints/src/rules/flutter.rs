//! The Flutter widget family: the widget-instance-creation shapes
//! (`avoid_unnecessary_containers`, `use_colored_box`, `use_decorated_box`,
//! `sized_box_for_whitespace`, `sized_box_shrink_expand`,
//! `sort_child_properties_last`,
//! `use_full_hex_values_for_flutter_colors`) and the widget-declaration
//! shapes (`use_key_in_widget_constructors`, `no_logic_in_create_state`,
//! `diagnostic_describe_all_properties`).
//!
//! Domain-wide approximation register (risk class R4): no flutter
//! framework library-identity face exists (dependency packages are
//! unindexed — GAP-4/5), so widget identity (`Container`, `SizedBox`,
//! `Color`, `StatefulWidget`, `Diagnosticable`, ...) is by
//! ctor/identifier SPELLING with a NEGATIVE-IDENTITY refutation: a
//! same-file declaration of the name is user code (the spelling bail),
//! and a creation head or supertype spelling that RESOLVES to an indexed
//! container is user code too (`Check::semantics` answers; the official
//! element-identity posture for that case). An unindexed spelling keeps
//! the framework verdict — `semantics: None` (the plain run) and
//! unresolved heads included — so a user class inside an unindexed
//! dependency still matches (what full dependency indexing would close).
//! Member-kind facts (a `key` parameter, a `createState` override, the
//! debug-method references) read their syntactic shapes; each rule's
//! module doc registers its direction.

use dartq_ast::{NodeKind, PayloadField};
use dartq_parser::IdentifierContext;

use crate::engine::Check;
use crate::registry::RuleSpec;
use crate::rules::slice;
use crate::rules::{SupertypeClause, same_file_declaration, supertypes};

/// The domain's incoming registrations: a new standalone rule appends its
/// `SPEC` reference here (name order) and joins the registry through the
/// aggregated tail — adding a rule touches only this file. Batch close
/// promotes the block into the registry's frozen skeleton together with
/// the pin-test wave list (`tests/registry_pin.rs`).
pub(crate) static INCOMING_RULES: &[&RuleSpec] = &[];

// —— shared faces ——————————————————————————————————————————————————————————

/// The row's own token span as the semantic query face's `SourceRange`
/// (UTF-16 — the fold rows' coordinate convention; the queries match on
/// the start offset, the length rides along for construction).
fn query_span(c: &Check<'_>, row: u32) -> dartq_source::SourceRange {
    let span = c.utf16_span(row);
    dartq_source::SourceRange {
        offset: span.start,
        length: span.end - span.start,
    }
}

/// Whether the written position of `row` resolves to an indexed container
/// — proof the spelling is user code, not the framework family's (the
/// negative-identity refutation; the official element-identity posture).
/// `false` when the semantic face is unmounted (the plain run), the
/// position carries no semantic row, or the spelling is unindexed: the
/// framework verdict stands there (a user class inside an unindexed
/// dependency keeps it too — what dependency indexing would close).
fn resolves_user_container(c: &Check<'_>, row: u32) -> bool {
    let Some(analysis) = c.semantics() else {
        return false;
    };
    let span = query_span(c, row);
    analysis.resolve_constructor(span, None).is_some()
        || matches!(
            analysis.binding_of(span).map(|b| b.kind),
            Some(dartq_semantic::BindingKind::Decl)
        )
}

/// The framework-identity spelling gate over an instance-creation head:
/// `name` is undeclared in the file AND its callee position does not
/// resolve to an indexed container. A same-file class/typedef of a
/// framework name is user code (the official element posture for that
/// case); a cross-file user class carrying the spelling resolves through
/// the workspace namespaces and is refuted the same way (negative
/// identity). An undeclared, unindexed spelling carries the framework
/// verdict — `semantics: None` (the plain run) and unresolved heads
/// included, so the documented under-report fallback stays put.
///
/// The verdict is a pure function of the callee row (the head lexeme and
/// the mounted semantic face are row-derived), and the seven creation
/// rules refute the identical rows, so verdicts are shared through
/// [`REFUTATION_MEMO`]: the first rule computes, the rest hit.
fn spells_framework(c: &Check<'_>, callee: u32, name: &str) -> bool {
    // The run pin: the unit's source identity plus the semantic face's
    // address. Verdicts answer for exactly one pinned (source, face) pair;
    // any pin change — the next run, a plain/semantic switch, a different
    // file — drops the map wholesale, so entries cannot answer for a
    // different unit's rows. The map dies with the run's dispatch; row ids
    // are unit-local, and the callee's lexeme (every caller derives `name`
    // from `callee` via [`creation_parts`]) rides the same row.
    let pin = (
        core::ptr::from_ref(c.unit) as usize,
        c.unit.source.as_ptr() as usize,
        c.unit.source.len(),
        c.semantics().map_or(0, |a| core::ptr::from_ref(a) as usize),
    );
    REFUTATION_MEMO.with(|memo| {
        let mut memo = memo.borrow_mut();
        if memo.pin != pin {
            memo.pin = pin;
            memo.verdicts.clear();
        }
        *memo.verdicts.entry(callee).or_insert_with(|| {
            !crate::rules::expressions::same_file::declares_name(c, name)
                && !resolves_user_container(c, callee)
        })
    })
}

/// The shared refutation verdicts of [`spells_framework`] — see its doc.
/// One slot per thread (a run dispatches on one thread), pinned per
/// (unit, semantic face); the pin check clears on every run boundary the
/// engine can present.
struct RefutationMemo {
    pin: (usize, usize, usize, usize),
    verdicts: rustc_hash::FxHashMap<u32, bool>,
}

// The memo is per-run: the engine clears it on entry to every run walk
// (`reset_refutation_memo`); the address pin below is belt-and-suspenders
// only, never the primary defense.
thread_local! {
    static REFUTATION_MEMO: core::cell::RefCell<RefutationMemo> =
        core::cell::RefCell::new(RefutationMemo {
            pin: (0, 0, 0, 0),
            verdicts: rustc_hash::FxHashMap::default(),
        });
}

/// The run-boundary clear: called on entry to every run walk so verdicts
/// computed in one run can never answer for another — same-size allocator
/// reuse can otherwise satisfy every pin leg across different files
/// (equal-length sources, job-local stack addresses, a plain run's
/// constant `0` analysis leg).
pub(crate) fn reset_refutation_memo() {
    REFUTATION_MEMO.with(|memo| {
        let mut memo = memo.borrow_mut();
        memo.pin = (0, 0, 0, 0);
        memo.verdicts.clear();
    });
}

/// The dispatch node's instance creation over both production tilings:
/// `(callee row, written name, arguments node)`. A member-chain callee
/// (`SizedBox.shrink()`, `p.Container()`) carries a trailing same-span
/// access marker and never matches — the bare-spelling face (a prefixed
/// framework call under-reports, the documented R4 boundary).
fn creation_parts<'a>(c: &'a Check<'_>) -> Option<(u32, &'a str, u32)> {
    match c.kind(c.node()) {
        NodeKind::NewExpression | NodeKind::ImplicitCreation => {
            let node = c.node();
            let kids = c.collect_direct(node);
            let args = kids
                .iter()
                .copied()
                .find(|&kid| c.kind(kid) == NodeKind::Arguments)?;
            let args_pos = kids.iter().position(|&kid| kid == args)?;
            let row = kids[..args_pos]
                .iter()
                .rev()
                .find_map(|&kid| match c.kind(kid) {
                    NodeKind::Identifier => Some(kid),
                    NodeKind::ConstructorReference => c
                        .collect_direct(kid)
                        .into_iter()
                        .find(|&n| c.kind(n) == NodeKind::Identifier),
                    _ => None,
                })?;
            let name = c.lexeme(c.head_token(row));
            Some((row, name, args))
        }
        NodeKind::ConstExpression | NodeKind::ConstLiteral => {
            // The const-creation tiling: the callee rides a
            // `ConstructorReference` row beside the argument list (no
            // send-family rows tile inside a const window). A qualified
            // callee keeps its bare head here — the name check below
            // matches on the head spelling, so `const p.C()` reads `p` and
            // stays silent (the bare-spelling face's prefixed boundary).
            let kids = c.collect_direct(c.node());
            let args = kids
                .iter()
                .copied()
                .find(|&kid| c.kind(kid) == NodeKind::Arguments)?;
            let ctor = kids
                .iter()
                .copied()
                .find(|&kid| c.kind(kid) == NodeKind::ConstructorReference)?;
            let row = c
                .collect_direct(ctor)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Identifier)?;
            let name = c.lexeme(c.head_token(row));
            Some((row, name, args))
        }
        NodeKind::Send => {
            let (kids, pos) = slice::owner_slice(c)?;
            if slice::access_marker(c, &kids, pos, NodeKind::DotAccess).is_some() {
                return None;
            }
            let (head, args) = slice::send_parts(c, &kids, pos)?;
            let args = args?;
            let row = kids[head];
            Some((row, c.lexeme(c.head_token(row)), args))
        }
        _ => None,
    }
}

/// The named-argument groups of an argument list over its flattened
/// pieces: `(name row, value piece range)` in source order; `None` when any
/// argument is positional (the upstream argument walks bail on positional
/// arguments).
fn argument_groups(c: &Check<'_>, pieces: &[u32]) -> Option<Vec<(u32, core::ops::Range<usize>)>> {
    let mut out = Vec::new();
    for group in slice::arguments(c, pieces) {
        let marker = group.end - 1;
        if c.kind(pieces[marker]) != NodeKind::NamedArgument {
            return None;
        }
        out.push((pieces[group.start], group.start + 1..marker));
    }
    Some(out)
}

/// [`argument_groups`] skipping positional arguments instead of bailing —
/// the `sort_child` face (`isChildArg` filters named arguments, it never
/// bails on positional ones).
fn named_argument_groups(c: &Check<'_>, pieces: &[u32]) -> Vec<(u32, core::ops::Range<usize>)> {
    slice::arguments(c, pieces)
        .into_iter()
        .filter(|group| c.kind(pieces[group.end - 1]) == NodeKind::NamedArgument)
        .map(|group| (pieces[group.start], group.start + 1..group.end - 1))
        .collect()
}

/// The value root over a named argument's value piece range: the OUTERMOST
/// run piece whose span contains the head piece's span (a reverse walk —
/// an inner match would be an interior wrapper), with its piece index.
fn value_root(
    c: &Check<'_>,
    pieces: &[u32],
    range: core::ops::Range<usize>,
) -> Option<(u32, usize)> {
    let head = *pieces.get(range.start)?;
    let head_span = c.byte_span(head);
    let run = pieces.get(range.start..range.end)?;
    run.iter()
        .copied()
        .enumerate()
        .rev()
        .find(|&(_, piece)| {
            let span = c.byte_span(piece);
            span.start <= head_span.start && head_span.end <= span.end
        })
        .map(|(i, piece)| (piece, range.start + i))
}

/// Whether the named-argument value is an instance creation: the value
/// root is an invocation send (not the bare-name tear-off group), a
/// `new`/implicit creation, or a const wrapper carrying one (the official
/// `argumentExpression is InstanceCreationExpression` face over the
/// spelling channel).
fn value_is_creation(c: &Check<'_>, pieces: &[u32], range: core::ops::Range<usize>) -> bool {
    let Some((root, index)) = value_root(c, pieces, range) else {
        return false;
    };
    match c.kind(root) {
        NodeKind::NewExpression | NodeKind::ImplicitCreation => true,
        NodeKind::ConstExpression | NodeKind::ConstLiteral => {
            let window = c.children(root);
            // Production const tiling keeps no send-family rows inside the
            // window: a creation is a `ConstructorReference` beside an
            // argument-list row (hand-built tables may carry the send
            // faces).
            (window.start..window.end).any(|row| {
                matches!(
                    c.kind(row),
                    NodeKind::Send
                        | NodeKind::NewExpression
                        | NodeKind::ImplicitCreation
                        | NodeKind::Arguments
                )
            })
        }
        NodeKind::Send => !index
            .checked_sub(3)
            .is_some_and(|start| slice::is_tear_off_group(c, pieces, start)),
        _ => false,
    }
}

/// The bounded written-supertype walk (same-file links): whether a
/// supertype passing `keep` spells one of `roots`. A same-file declaration
/// of the name shadows the framework spelling (the walk descends into the
/// user class instead); an undeclared root spelling matches unless its
/// written type-use position resolves to an indexed container — the
/// negative-identity refutation (a cross-file user class carrying the
/// root spelling is user code; an unindexed spelling keeps the verdict,
/// the plain-run posture included; the terminal decides, not each link —
/// a same-file link is still descended). One resolvable supertype is
/// followed per step, the walk stops past 8 links (the bounded-chain
/// idiom).
fn chain_spells_root(
    c: &Check<'_>,
    class: u32,
    roots: &[&str],
    keep: fn(SupertypeClause) -> bool,
) -> bool {
    let mut current = Some(class);
    for _ in 0..8 {
        let Some(row) = current else {
            return false;
        };
        let mut next = None;
        for (name, clause) in supertypes(c, row) {
            if !keep(clause) {
                continue;
            }
            if let Some(declared) = same_file_declaration(c, name) {
                next = next.or(Some(declared));
            } else if roots.contains(&name)
                && !supertype_head(c, row, name)
                    .is_some_and(|head| resolves_user_container(c, head))
            {
                return true;
            }
        }
        current = next;
    }
    false
}

/// The head row of the supertype clause spelling `name` on `class` — the
/// `Type` row whose head token carries the written type use (the
/// [`supertypes`] walk keeps only lexemes; the refutation needs the
/// position). A prefixed supertype's head is the prefix spelling, which
/// never names a root, so the bare-name lookup suffices.
fn supertype_head(c: &Check<'_>, class: u32, name: &str) -> Option<u32> {
    c.collect_direct(class)
        .into_iter()
        .take_while(|&kid| !matches!(c.kind(kid), NodeKind::DeclarationBody | NodeKind::EnumBody))
        .filter(|&kid| c.kind(kid) == NodeKind::Type)
        .find(|&kid| c.lexeme(c.head_token(kid)) == name)
}

fn extends_only(clause: SupertypeClause) -> bool {
    matches!(clause, SupertypeClause::Extends)
}

fn any_clause(_: SupertypeClause) -> bool {
    true
}

/// Whether another same-file class-like declaration names `class`'s name in
/// a written supertype clause (the file's subclass face — the superclass
/// constructor's verdict is then carried by the subclass's class face,
/// which reads the same unnamed constructor through
/// [`super_constructor_carries_key`]-shaped gates; keeping both faces
/// would double-report one key posture).
fn subclassed_same_file(c: &Check<'_>, class: u32, class_name: &str) -> bool {
    let Some(root) = c.unit.ast.root() else {
        return false;
    };
    for top in c.collect_direct(root) {
        if c.kind(top) != NodeKind::TopLevelDeclaration {
            continue;
        }
        for kid in c.collect_direct(top) {
            if !matches!(
                c.kind(kid),
                NodeKind::ClassDeclaration | NodeKind::MixinDeclaration
            ) || kid == class
            {
                continue;
            }
            if supertypes(c, kid)
                .iter()
                .any(|(name, _)| *name == class_name)
            {
                return true;
            }
        }
    }
    false
}

/// Whether the row carries an annotation named `name`: the `MetadataStar`
/// siblings preceding it, read by spelling. The production tiling anchors
/// member metadata at the member's level (a `MetadataStar` left sibling
/// under the body) and a top-level class's metadata beside the
/// `TopLevelDeclaration` wrapper one level up, so the walk checks both
/// anchors.
fn carries_annotation(c: &Check<'_>, row: u32, name: &str) -> bool {
    let names_annotation = |star: u32| {
        c.collect_direct(star).into_iter().any(|meta| {
            c.kind(meta) == NodeKind::Metadata
                && c.collect_direct(meta).into_iter().any(|piece| {
                    c.kind(piece) == NodeKind::Identifier && c.lexeme(c.head_token(piece)) == name
                })
        })
    };
    let mut anchor = row;
    for _ in 0..2 {
        let Some(parent) = c.unit.ast.parent(anchor) else {
            return false;
        };
        let kids = c.collect_direct(parent);
        let Some(pos) = kids.iter().position(|&kid| kid == anchor) else {
            return false;
        };
        let mut left = pos;
        while left > 0 && c.kind(kids[left - 1]) == NodeKind::MetadataStar {
            left -= 1;
            if names_annotation(kids[left]) {
                return true;
            }
        }
        if left != pos {
            return false; // metadata siblings present, none named `name`
        }
        anchor = parent; // the wrapper face: retry beside the parent wrapper
    }
    false
}

/// The class-like declaration's name row: the production wrapper keeps the
/// name `Identifier` as the row's left sibling; hand-built tables nest it
/// inside the row (the `class_declaration_name` walk keeping the row).
fn class_name_row(c: &Check<'_>, class: u32) -> Option<u32> {
    let is_class_name = |kid: u32| {
        c.kind(kid) == NodeKind::Identifier
            && c.field(kid, PayloadField::Context)
                == Some(IdentifierContext::ClassOrMixinOrExtensionDeclaration as u32)
    };
    if let Some(name) = c
        .collect_direct(class)
        .into_iter()
        .find(|&kid| is_class_name(kid))
    {
        return Some(name);
    }
    let parent = c.unit.ast.parent(class)?;
    let siblings = c.collect_direct(parent);
    let pos = siblings.iter().position(|&kid| kid == class)?;
    siblings[..pos]
        .iter()
        .rev()
        .copied()
        .find(|&kid| is_class_name(kid))
}

/// The member declaration's name row over the production tiling: the
/// return-type / receiver identifiers tile beside the name, so the name is
/// the direct `Identifier` carrying the `MethodDeclaration` context, and a
/// named constructor's part after the dot carries
/// `MethodDeclarationContinuation` (the official `node.name` face —
/// `Base._()` names `_`, not `Base`).
fn member_name_row(c: &Check<'_>, member: u32) -> Option<u32> {
    let named = |ctx: IdentifierContext| {
        c.collect_direct(member).into_iter().find(|&kid| {
            c.kind(kid) == NodeKind::Identifier
                && c.field(kid, PayloadField::Context) == Some(ctx as u32)
        })
    };
    named(IdentifierContext::MethodDeclarationContinuation)
        .or_else(|| named(IdentifierContext::MethodDeclaration))
}

/// Whether the member row is named `name` (its name-bearing `Identifier`).
fn member_named(c: &Check<'_>, member: u32, name: &str) -> bool {
    member_name_row(c, member)
        .is_some_and(|n| c.kind(n) == NodeKind::Identifier && c.lexeme(c.head_token(n)) == name)
}

/// The constructor-like declaration's formal parameter list row.
fn parameter_list(c: &Check<'_>, node: u32) -> Option<u32> {
    c.collect_direct(node).into_iter().find(|&kid| {
        matches!(
            c.kind(kid),
            NodeKind::FormalParameters | NodeKind::OptionalFormalParameters
        )
    })
}

/// The formal parameter rows under a parameter list (the optional group
/// nests one level; the `avoid_unused_constructor_parameters` idiom).
fn formal_rows(c: &Check<'_>, params: u32) -> Vec<u32> {
    let mut rows = Vec::new();
    for kid in c.collect_direct(params) {
        match c.kind(kid) {
            NodeKind::FormalParameter | NodeKind::FunctionTypedFormalParameter => {
                rows.push(kid);
            }
            NodeKind::OptionalFormalParameters | NodeKind::FormalParameters => {
                rows.extend(formal_rows(c, kid));
            }
            _ => {}
        }
    }
    rows
}

/// A simple formal parameter's name (the row's `Name` word).
fn param_name<'a>(c: &'a Check<'_>, param: u32) -> Option<&'a str> {
    let name = c.field(param, PayloadField::Name)?;
    (name != crate::rules::NONE_TOKEN).then(|| c.lexeme(name))
}

/// The unnamed constructor row of a class-like declaration: the
/// `Constructor` member whose name spells the class name.
fn unnamed_ctor(c: &Check<'_>, class: u32, class_name: &str) -> Option<u32> {
    crate::rules::body_members(c, class)
        .into_iter()
        .find(|&m| c.kind(m) == NodeKind::Constructor && member_named(c, m, class_name))
}

/// The named constructor row of a class-like declaration.
fn named_ctor(c: &Check<'_>, class: u32, name: &str) -> Option<u32> {
    crate::rules::body_members(c, class)
        .into_iter()
        .find(|&m| c.kind(m) == NodeKind::Constructor && member_named(c, m, name))
}

/// `definesKeyParameter` over the spelling face: any formal parameter named
/// `key` (the Key-type check is approximated away — a `key` parameter of an
/// unrelated type counts, so the silent face under-reports there).
fn ctor_defines_key(c: &Check<'_>, ctor: u32) -> bool {
    parameter_list(c, ctor).is_some_and(|params| {
        formal_rows(c, params)
            .iter()
            .any(|&p| param_name(c, p) == Some("key"))
    })
}

pub(crate) mod avoid_unnecessary_containers {
    use super::{Check, NodeKind, RuleSpec, creation_parts, spells_framework, value_is_creation};
    use crate::rules::slice;

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/avoid_unnecessary_containers.dart` — an
    /// instance creation whose parent is a `child:` named argument, that
    /// argument list carrying exactly one argument, and whose parent
    /// creation is a `Container` reports at the Container's constructor
    /// name. The visitor walks the inner creation up to the outer one; the
    /// face here reads the outer creation directly (the same verdict: a
    /// `Container` whose sole argument is a named `child` whose value is
    /// itself an instance creation). Message: `diagnostic.g.dart`
    /// `avoidUnnecessaryContainers`.
    ///
    /// Approximation register (R4): widget identity by spelling with the
    /// negative-identity refutation — the `Container` callee spelling,
    /// with same-file declarations and resolution-resolved workspace
    /// containers refuting (the official element posture); a user class
    /// inside an unindexed dependency still matches (what dependency
    /// indexing would close). The inner value's `isWidgetType` gate reads
    /// the creation shape (any instance creation counts — a non-widget
    /// creation over-reports); a bare-identifier value is not a creation
    /// (the tear-off-group face). A prefixed `p.Container(...)`
    /// under-reports, dispatch or value; the const faces dispatch on the
    /// `ConstExpression`/`ConstLiteral` wrapper rows and match the bare
    /// callee spelling inside.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
        "avoid_unnecessary_containers",
        &[
            NodeKind::Send,
            NodeKind::NewExpression,
            NodeKind::ImplicitCreation,
            NodeKind::ConstExpression,
            NodeKind::ConstLiteral,
        ],
        check,
    );

    fn check(c: &mut Check<'_>) {
        let Some((callee, name, args)) = creation_parts(c) else {
            return;
        };
        if name != "Container" || !spells_framework(c, callee, name) {
            return;
        }
        let pieces = c.collect_direct(args);
        let groups = slice::arguments(c, &pieces);
        let [group] = groups.as_slice() else {
            return;
        };
        let marker = group.end - 1;
        if c.kind(pieces[marker]) != NodeKind::NamedArgument
            || c.lexeme(c.head_token(pieces[group.start])) != "child"
            || !value_is_creation(c, &pieces, group.start + 1..marker)
        {
            return;
        }
        c.report(c.utf16_span(callee), &[]);
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

        /// `test/rules/avoid_unnecessary_containers_test.dart` — a
        /// Container whose sole argument is a `child:` creation reports at
        /// the Container.
        #[test]
        fn reports_a_child_only_container() {
            let f = Fixture::parse("void f() { var c = Container(child: Text('x')); }");
            let lints = f.run(&one_rule("avoid_unnecessary_containers"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(lints[0].message, "Unnecessary instance of 'Container'.");
            assert_eq!(
                lints[0].correction.as_deref(),
                Some("Try removing the 'Container' (but not its children) from the widget tree.")
            );
            let container = tok(&f.source, &f.tokens, "Container", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, container));
        }

        /// A const-wrapped child creation is still an instance creation.
        #[test]
        fn reports_a_const_child_creation() {
            let f = Fixture::parse("void f() { var c = Container(child: const Text('x')); }");
            assert_eq!(f.run(&one_rule("avoid_unnecessary_containers")).len(), 1);
        }

        /// A dispatch-level const wrap is still an instance creation.
        #[test]
        fn reports_a_const_container() {
            let f = Fixture::parse("void f() { var c = const Container(child: Text('x')); }");
            assert_eq!(f.run(&one_rule("avoid_unnecessary_containers")).len(), 1);
        }

        /// Extra arguments and a bare-identifier child value pass (the
        /// official `arguments.length == 1` and
        /// `is InstanceCreationExpression` faces).
        #[test]
        fn passes_with_extra_arguments_or_a_bare_value() {
            let f = Fixture::parse("void f(c, t) { Container(child: t, color: c); }");
            assert!(f.run(&one_rule("avoid_unnecessary_containers")).is_empty());

            let f = Fixture::parse("void f(t) { Container(child: t); }");
            assert!(f.run(&one_rule("avoid_unnecessary_containers")).is_empty());
        }

        /// The R4 shadow face: a same-file `class Container` is user code,
        /// not the framework widget.
        #[test]
        fn same_file_container_is_user_code() {
            let f = Fixture::parse("class Container {}\nvoid f() { Container(child: Text('x')); }");
            assert!(f.run(&one_rule("avoid_unnecessary_containers")).is_empty());
        }
    }
}

pub(crate) mod sort_child_properties_last {
    use super::{
        Check, NodeKind, RuleSpec, creation_parts, named_argument_groups, spells_framework,
        value_root,
    };

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/sort_child_properties_last.dart` — an
    /// instance creation with at least two arguments and exactly one
    /// `child`/`children` named argument reports at that argument when it
    /// is not the last argument and some non-closure named argument
    /// follows it (:41-61; trailing closures like `onTap` are exempt).
    /// Message: `diagnostic.g.dart` `sortChildPropertiesLast`.
    ///
    /// Approximation register (R4): widget identity by spelling with the
    /// negative-identity refutation — the `isWidgetType` creation gate
    /// refutes same-file declarations and resolution-resolved workspace
    /// classes (an unindexed dependency class carrying the name still
    /// participates — what dependency indexing would close). The child
    /// argument's `isWidgetProperty` static-type gate reads no element
    /// model — a non-widget child value keeps the counting posture (a
    /// creation or primitive value carries no identifier read row; what
    /// the full static-type face would close). A prefixed callee
    /// under-reports (no library-URI face, GAP-4/5).
    pub(crate) static SPEC: RuleSpec = crate::spec!(
        "sort_child_properties_last",
        &[
            NodeKind::Send,
            NodeKind::NewExpression,
            NodeKind::ImplicitCreation,
        ],
        check,
    );

    fn check(c: &mut Check<'_>) {
        let Some((callee, name, args)) = creation_parts(c) else {
            return;
        };
        // The free syntactic gate first (:41-49): two-plus named groups
        // carrying exactly one `child`/`children` — the same gate-first
        // order the name-gated creation rules use, so only rows that can
        // report pay the refutation. Both predicates are pure; the verdict
        // (their conjunction with the refutation) is order-independent.
        let pieces = c.collect_direct(args);
        let groups = named_argument_groups(c, &pieces);
        if groups.len() < 2 {
            return;
        }
        // Exactly one `child`/`children` argument (:45-49).
        let mut child_index = None;
        let mut child_count = 0usize;
        for (i, (row, _)) in groups.iter().enumerate() {
            let arg = c.lexeme(c.head_token(*row));
            if arg == "child" || arg == "children" {
                child_count += 1;
                child_index = Some(i);
            }
        }
        if child_count != 1 {
            return;
        }
        if !spells_framework(c, callee, name) {
            return;
        }
        let index = child_index.expect("counted above");
        if index + 1 == groups.len() {
            return; // the child argument is last
        }
        // Every argument after the child must be a named closure
        // (`onlyClosuresAfterChild`, :51-58); the first non-closure
        // reports the child argument.
        for (_, range) in &groups[index + 1..] {
            let closure = value_root(c, &pieces, range.clone())
                .is_some_and(|(root, _)| c.kind(root) == NodeKind::FunctionExpression);
            if closure {
                continue;
            }
            let (child_row, child_range) = &groups[index];
            let Some((root, _)) = value_root(c, &pieces, child_range.clone()) else {
                return;
            };
            let range = c.utf16_span(*child_row).start..c.utf16_span(root).end;
            c.report(range, &[c.lexeme(c.head_token(*child_row))]);
            return;
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

        /// `test/rules/sort_child_properties_last_test.dart`
        /// `test_childLast` inverse — a named argument after `child`
        /// reports at the child argument with its name filled in.
        #[test]
        fn reports_an_unsorted_child_argument() {
            let f =
                Fixture::parse("void f() { var c = Container(child: Text('x'), height: 100); }");
            let lints = f.run(&one_rule("sort_child_properties_last"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "The 'child' argument should be last in widget constructor invocations."
            );
            assert_eq!(
                lints[0].correction.as_deref(),
                Some("Try moving the argument to the end of the argument list.")
            );
            let child = tok(&f.source, &f.tokens, "child", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, child));
        }

        /// The `children:` spelling participates the same way.
        #[test]
        fn reports_an_unsorted_children_argument() {
            let f =
                Fixture::parse("void f() { var s = Stack(children: [Text('a')], alignment: 0); }");
            let lints = f.run(&one_rule("sort_child_properties_last"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "The 'children' argument should be last in widget constructor invocations."
            );
        }

        /// A last or sole child argument passes (:43-44).
        #[test]
        fn passes_when_the_child_argument_is_last_or_sole() {
            let f =
                Fixture::parse("void f() { var c = Container(height: 100, child: Text('x')); }");
            assert!(f.run(&one_rule("sort_child_properties_last")).is_empty());

            let f = Fixture::parse("void f() { var c = Container(child: Text('x')); }");
            assert!(f.run(&one_rule("sort_child_properties_last")).is_empty());
        }

        /// Trailing closures are exempt (`onlyClosuresAfterChild`, :51-58);
        /// a non-closure after them still reports.
        #[test]
        fn closure_arguments_after_child_pass() {
            let f = Fixture::parse("void f() { var w = Foo(child: Text('x'), onTap: () {}); }");
            assert!(f.run(&one_rule("sort_child_properties_last")).is_empty());

            let f = Fixture::parse(
                "void f() { var w = Foo(child: Text('x'), onTap: () {}, width: 1); }",
            );
            assert_eq!(f.run(&one_rule("sort_child_properties_last")).len(), 1);
        }

        /// Two child spellings never fire (exactly-one gate, :46-49).
        #[test]
        fn two_child_arguments_pass() {
            let f = Fixture::parse("void f(a, b) { var w = Foo(child: a, children: b, x: 1); }");
            assert!(f.run(&one_rule("sort_child_properties_last")).is_empty());
        }

        /// The R4 shadow face: a same-file class owning the creation
        /// spelling is user code (the official `isWidgetType` gate
        /// resolves it silent).
        #[test]
        fn same_file_class_with_child_parameter_passes() {
            let f = Fixture::parse(
                "class Foo { Foo({child, x}); }\nvoid f() { var w = Foo(child: 1, x: 2); }",
            );
            assert!(f.run(&one_rule("sort_child_properties_last")).is_empty());
        }
    }
}

pub(crate) mod use_colored_box {
    use super::{
        Check, NodeKind, RuleSpec, argument_groups, creation_parts, query_span, spells_framework,
        value_root,
    };

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/use_colored_box.dart` — a `Container`
    /// creation whose arguments are only `child` and `color` (plus `key`)
    /// reports at the constructor name (:29-39, the argument walk bails on
    /// positional and foreign names). Message: `diagnostic.g.dart`
    /// `useColoredBox`.
    ///
    /// Approximation register (R4): widget identity by spelling with the
    /// negative-identity refutation — the `Container` callee spelling,
    /// with same-file declarations and resolution-resolved workspace
    /// containers refuting; a user class inside an unindexed dependency
    /// still matches (what dependency indexing would close). The `color`
    /// argument's non-nullable static-type gate reads the flow type at
    /// the value's identifier read (the official `isPotentiallyNull`
    /// inverse): a nullable color value passes when its read row answers,
    /// and a shape without a read row (a literal, a member chain, a
    /// creation) keeps the counting posture. A prefixed `p.Container(...)`
    /// under-reports.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
        "use_colored_box",
        &[
            NodeKind::Send,
            NodeKind::NewExpression,
            NodeKind::ImplicitCreation,
        ],
        check,
    );

    fn check(c: &mut Check<'_>) {
        let Some((callee, name, args)) = creation_parts(c) else {
            return;
        };
        if name != "Container" || !spells_framework(c, callee, name) {
            return;
        }
        let pieces = c.collect_direct(args);
        let Some(groups) = argument_groups(c, &pieces) else {
            return;
        };
        let mut child = false;
        let mut color = false;
        for (row, range) in &groups {
            match c.lexeme(c.head_token(*row)) {
                "child" => child = true,
                "color" => {
                    // The nullability gate (:61): a color value whose
                    // static type is potentially null never counts.
                    if color_argument_potentially_null(c, &pieces, range.clone()) {
                        continue;
                    }
                    color = true;
                }
                // Ignore `key` as both ColoredBox and Container have it.
                "key" => {}
                // Other named arguments are not supported.
                _ => return,
            }
        }
        if child && color {
            c.report(c.utf16_span(callee), &[]);
        }
    }

    /// The upstream color gate (`use_colored_box.dart` :61): a color
    /// value whose static type is potentially null
    /// (`nullabilitySuffix != question` inverse) never counts. Reads the
    /// flow type at the value's bare-read face — the value root is an
    /// identifier, or the production tiling's argument-position `Send`
    /// wrapper over one (no `Arguments` row; the read row shares the
    /// wrapper's start offset). Any other shape (a literal, a member
    /// chain, an invocation, a creation) answers unknown and keeps the
    /// counting posture (the documented residual — what the full
    /// static-type face would close).
    fn color_argument_potentially_null(
        c: &Check<'_>,
        pieces: &[u32],
        range: core::ops::Range<usize>,
    ) -> bool {
        let Some(analysis) = c.semantics() else {
            return false;
        };
        let Some((root, _)) = value_root(c, pieces, range) else {
            return false;
        };
        let bare_read = match c.kind(root) {
            NodeKind::Identifier => true,
            NodeKind::Send => !c
                .collect_direct(root)
                .iter()
                .any(|&kid| c.kind(kid) == NodeKind::Arguments),
            _ => false,
        };
        bare_read && analysis.is_potentially_null(query_span(c, root))
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

        /// `test/rules/use_colored_box_test.dart` `test_color_child` — the
        /// child+color Container reports at the constructor name.
        #[test]
        fn reports_a_color_child_container() {
            let f = Fixture::parse("void f(c) { var w = Container(color: c, child: Text('x')); }");
            let lints = f.run(&one_rule("use_colored_box"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Use a 'ColoredBox' rather than a 'Container' with only a 'Color'."
            );
            assert_eq!(
                lints[0].correction.as_deref(),
                Some("Try replacing the 'Container' with a 'ColoredBox'.")
            );
            let container = tok(&f.source, &f.tokens, "Container", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, container));
        }

        /// A missing face or an unsupported argument passes (:51-63).
        #[test]
        fn passes_without_a_face_or_with_foreign_arguments() {
            let f = Fixture::parse("void f(t) { var w = Container(child: t); }");
            assert!(f.run(&one_rule("use_colored_box")).is_empty());

            let f = Fixture::parse("void f(c) { var w = Container(color: c); }");
            assert!(f.run(&one_rule("use_colored_box")).is_empty());

            let f = Fixture::parse(
                "void f(c, p, t) { var w = Container(color: c, child: t, padding: p); }",
            );
            assert!(f.run(&one_rule("use_colored_box")).is_empty());

            let f = Fixture::parse("void f(c, t) { var w = Container(t, color: c); }");
            assert!(f.run(&one_rule("use_colored_box")).is_empty());
        }

        /// `key` is ignored as both classes carry it (:55-56).
        #[test]
        fn key_argument_passes() {
            let f = Fixture::parse(
                "void f(k, c) { var w = Container(key: k, color: c, child: Text('x')); }",
            );
            assert_eq!(f.run(&one_rule("use_colored_box")).len(), 1);
        }

        /// The R4 shadow face: a same-file `class Container` is user code.
        #[test]
        fn same_file_container_is_user_code() {
            let f = Fixture::parse(
                "class Container {}\nvoid f(c) { Container(color: c, child: Text('x')); }",
            );
            assert!(f.run(&one_rule("use_colored_box")).is_empty());
        }
    }
}

pub(crate) mod use_decorated_box {
    use super::{Check, NodeKind, RuleSpec, argument_groups, creation_parts, spells_framework};

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/use_decorated_box.dart` — a `Container`
    /// creation whose arguments are only `child` and `decoration` (plus
    /// `key`) reports at the constructor name (:29-39, the argument walk
    /// bails on positional and foreign names). Message: `diagnostic.g.dart`
    /// `useDecoratedBox`.
    ///
    /// Approximation register (R4): widget identity by spelling with the
    /// negative-identity refutation — the `Container` callee spelling,
    /// with same-file declarations and resolution-resolved workspace
    /// containers refuting; a user class inside an unindexed dependency
    /// still matches (what dependency indexing would close). A prefixed
    /// `p.Container(...)` under-reports.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
        "use_decorated_box",
        &[
            NodeKind::Send,
            NodeKind::NewExpression,
            NodeKind::ImplicitCreation,
        ],
        check,
    );

    fn check(c: &mut Check<'_>) {
        let Some((callee, name, args)) = creation_parts(c) else {
            return;
        };
        if name != "Container" || !spells_framework(c, callee, name) {
            return;
        }
        let pieces = c.collect_direct(args);
        let Some(groups) = argument_groups(c, &pieces) else {
            return;
        };
        let mut child = false;
        let mut decoration = false;
        for (row, _) in &groups {
            match c.lexeme(c.head_token(*row)) {
                "child" => child = true,
                "decoration" => decoration = true,
                // Ignore `key` as both DecoratedBox and Container have it.
                "key" => {}
                // Other named arguments are not supported.
                _ => return,
            }
        }
        if child && decoration {
            c.report(c.utf16_span(callee), &[]);
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule};

        /// `test/rules/use_decorated_box_test.dart` `test_decoration_child`.
        #[test]
        fn reports_a_decoration_child_container() {
            let f =
                Fixture::parse("void f(d) { var w = Container(decoration: d, child: Text('x')); }");
            let lints = f.run(&one_rule("use_decorated_box"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Use 'DecoratedBox' rather than a 'Container' with only a 'Decoration'."
            );
            assert_eq!(
                lints[0].correction.as_deref(),
                Some("Try replacing the 'Container' with a 'DecoratedBox'.")
            );
        }

        /// A missing face or an unsupported argument passes (:51-62).
        #[test]
        fn passes_without_a_face_or_with_foreign_arguments() {
            let f = Fixture::parse("void f(d) { var w = Container(decoration: d); }");
            assert!(f.run(&one_rule("use_decorated_box")).is_empty());

            let f = Fixture::parse(
                "void f(c, d, t) { var w = Container(color: c, decoration: d, child: t); }",
            );
            assert!(f.run(&one_rule("use_decorated_box")).is_empty());

            let f = Fixture::parse("void f(t, d) { var w = Container(t, decoration: d); }");
            assert!(f.run(&one_rule("use_decorated_box")).is_empty());
        }

        /// The R4 shadow face: a same-file `class Container` is user code.
        #[test]
        fn same_file_container_is_user_code() {
            let f = Fixture::parse(
                "class Container {}\nvoid f(d) { Container(decoration: d, child: Text('x')); }",
            );
            assert!(f.run(&one_rule("use_decorated_box")).is_empty());
        }
    }
}

pub(crate) mod sized_box_for_whitespace {
    use super::{Check, NodeKind, RuleSpec, argument_groups, creation_parts, spells_framework};

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/sized_box_for_whitespace.dart` — a
    /// `Container` creation whose arguments are only `child`/`width`/
    /// `height` (plus `key`) and that carries a child with a size, or both
    /// sizes, reports at the constructor name (:29-39; the verdict is
    /// `hasChild && (hasWidth || hasHeight) || hasWidth && hasHeight`).
    /// Message: `diagnostic.g.dart` `sizedBoxForWhitespace`.
    ///
    /// Approximation register (R4): widget identity by spelling with the
    /// negative-identity refutation — the `Container` callee spelling,
    /// with same-file declarations and resolution-resolved workspace
    /// containers refuting; a user class inside an unindexed dependency
    /// still matches (what dependency indexing would close). A prefixed
    /// `p.Container(...)` under-reports.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
        "sized_box_for_whitespace",
        &[
            NodeKind::Send,
            NodeKind::NewExpression,
            NodeKind::ImplicitCreation,
        ],
        check,
    );

    fn check(c: &mut Check<'_>) {
        let Some((callee, name, args)) = creation_parts(c) else {
            return;
        };
        if name != "Container" || !spells_framework(c, callee, name) {
            return;
        }
        let pieces = c.collect_direct(args);
        let Some(groups) = argument_groups(c, &pieces) else {
            return;
        };
        let mut child = false;
        let mut width = false;
        let mut height = false;
        for (row, _) in &groups {
            match c.lexeme(c.head_token(*row)) {
                "child" => child = true,
                "width" => width = true,
                "height" => height = true,
                // Ignore `key` as both SizedBox and Container have it.
                "key" => {}
                // Other named arguments are not supported.
                _ => return,
            }
        }
        if (child && (width || height)) || (width && height) {
            c.report(c.utf16_span(callee), &[]);
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule};

        /// `test/rules/sized_box_for_whitespace_test.dart`
        /// `test SizedBox width height` — a sized whitespace Container
        /// reports at the constructor name.
        #[test]
        fn reports_a_sized_whitespace_container() {
            let f = Fixture::parse("void f(t) { var w = Container(height: 5, child: t); }");
            let lints = f.run(&one_rule("sized_box_for_whitespace"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Use a 'SizedBox' to add whitespace to a layout."
            );
            assert_eq!(
                lints[0].correction.as_deref(),
                Some("Try using a 'SizedBox' rather than a 'Container'.")
            );
        }

        /// Both sizes without a child report too (:61 the
        /// `hasWidth && hasHeight` arm).
        #[test]
        fn reports_both_sizes_without_a_child() {
            let f = Fixture::parse("void f() { var w = Container(width: 5, height: 6); }");
            assert_eq!(f.run(&one_rule("sized_box_for_whitespace")).len(), 1);
        }

        /// An unsupported argument, a lone size, and a positional argument
        /// pass (:48-59).
        #[test]
        fn passes_on_unsupported_or_partial_faces() {
            let f = Fixture::parse("void f(p) { var w = Container(padding: p); }");
            assert!(f.run(&one_rule("sized_box_for_whitespace")).is_empty());

            let f = Fixture::parse("void f() { var w = Container(width: 5); }");
            assert!(f.run(&one_rule("sized_box_for_whitespace")).is_empty());

            let f = Fixture::parse("void f(t) { var w = Container(t, width: 5); }");
            assert!(f.run(&one_rule("sized_box_for_whitespace")).is_empty());
        }

        /// `key` is ignored as both classes carry it.
        #[test]
        fn key_argument_passes() {
            let f =
                Fixture::parse("void f(k, t) { var w = Container(key: k, child: t, width: 1); }");
            assert_eq!(f.run(&one_rule("sized_box_for_whitespace")).len(), 1);
        }

        /// The R4 shadow face: a same-file `class Container` is user code.
        #[test]
        fn same_file_container_is_user_code() {
            let f =
                Fixture::parse("class Container {}\nvoid f(t) { Container(height: 5, child: t); }");
            assert!(f.run(&one_rule("sized_box_for_whitespace")).is_empty());
        }
    }
}

pub(crate) mod sized_box_shrink_expand {
    use super::{
        Check, NodeKind, PayloadField, RuleSpec, argument_groups, creation_parts, spells_framework,
        value_root,
    };

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/sized_box_shrink_expand.dart` — a default
    /// `SizedBox` creation whose `width` and `height` are both the literal
    /// zero reports `SizedBox.shrink`, both `double.infinity` reports
    /// `SizedBox.expand` (:31-46); unknown argument values never fire and
    /// other named arguments pass silently (no default arm).
    /// Message: `diagnostic.g.dart` `sizedBoxShrinkExpand`.
    ///
    /// Approximation register (R4): widget identity by spelling with the
    /// negative-identity refutation — the `SizedBox` callee spelling,
    /// with same-file declarations and resolution-resolved workspace
    /// containers refuting; a user class inside an unindexed dependency
    /// still matches (what dependency indexing would close). The
    /// default-constructor gate reads the bare-spelling face
    /// (`SizedBox.shrink()` never reaches the face — a member-chain
    /// callee carries the trailing access marker); a prefixed
    /// `p.SizedBox(...)` under-reports.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
        "sized_box_shrink_expand",
        &[
            NodeKind::Send,
            NodeKind::NewExpression,
            NodeKind::ImplicitCreation,
        ],
        check,
    );

    fn check(c: &mut Check<'_>) {
        let Some((callee, name, args)) = creation_parts(c) else {
            return;
        };
        if name != "SizedBox" || !spells_framework(c, callee, name) {
            return;
        }
        let pieces = c.collect_direct(args);
        let Some(groups) = argument_groups(c, &pieces) else {
            return;
        };
        let mut width = None;
        let mut height = None;
        for (row, range) in &groups {
            let argument = c.lexeme(c.head_token(*row));
            if argument != "width" && argument != "height" {
                continue;
            }
            let Some(value) = literal_argument_value(c, &pieces, range.clone()) else {
                continue;
            };
            if argument == "width" {
                width = Some(value);
            } else {
                height = Some(value);
            }
        }
        let kind = if width == Some(0.0) && height == Some(0.0) {
            "shrink"
        } else if width == Some(f64::INFINITY) && height == Some(f64::INFINITY) {
            "expand"
        } else {
            return;
        };
        c.report(c.utf16_span(callee), &[kind]);
    }

    /// The `argumentValue` face (:79-90): an integer or double literal, or
    /// the `double.infinity` prefixed-identifier spelling; anything else is
    /// unknown (`None` — the comparison never fires on it).
    fn literal_argument_value(
        c: &Check<'_>,
        pieces: &[u32],
        range: core::ops::Range<usize>,
    ) -> Option<f64> {
        // The send-chain `a.b` face first: a trailing `DotAccess` whose
        // receiver is the run head is the prefixed-identifier tiling
        // (`double.infinity` tiles as receiver `Send` + selector
        // `DotAccess` siblings — no production row spans the pair).
        if let Some(value) = infinity_access(c, pieces, range.clone()) {
            return Some(value);
        }
        let (root, _) = value_root(c, pieces, range)?;
        match c.kind(root) {
            NodeKind::LiteralInt => {
                let mut lexeme = c.lexeme(c.head_token(root)).to_owned();
                lexeme.retain(|ch| ch != '_');
                let value = match lexeme
                    .strip_prefix("0x")
                    .or_else(|| lexeme.strip_prefix("0X"))
                {
                    Some(hex) => i64::from_str_radix(hex, 16).ok()?,
                    None => lexeme.parse::<i64>().ok()?,
                };
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "a Dart int literal in double context takes the \
                              nearest f64 value (official double-literal \
                              semantics); the loss is inherent"
                )]
                Some(value as f64)
            }
            NodeKind::LiteralDouble => {
                let mut lexeme = c.lexeme(c.head_token(root)).to_owned();
                lexeme.retain(|ch| ch != '_');
                lexeme.parse::<f64>().ok()
            }
            NodeKind::DotAccess => c
                .field(root, PayloadField::Operator)
                .and_then(|dot| infinity_at(c, dot)),
            _ => None,
        }
    }

    /// The `double.infinity` prefixed-identifier spelling: the DotAccess row
    /// seals the selector extent and keeps the dot token in payload word 0;
    /// the receiver identifier sits one token before the dot.
    fn infinity_at(c: &Check<'_>, dot: u32) -> Option<f64> {
        if dot < 1 || dot + 1 >= c.unit.tokens.as_slice().len() as u32 {
            return None;
        }
        (c.lexeme(dot - 1) == "double" && c.lexeme(dot) == "." && c.lexeme(dot + 1) == "infinity")
            .then_some(f64::INFINITY)
    }

    /// The value's `double.infinity` face over the run: the send-chain pair —
    /// receiver `Send` at the run head plus a trailing `DotAccess` (no
    /// production row spans the whole `double.infinity`; the receiver `Send`
    /// and the selector `DotAccess` tile as siblings). A selector invocation
    /// (`double.infinity()`) is not the spelling — the selector `Send` then
    /// carries a real `Arguments` row.
    fn infinity_access(
        c: &Check<'_>,
        pieces: &[u32],
        range: core::ops::Range<usize>,
    ) -> Option<f64> {
        let run = pieces.get(range.clone())?;
        let dot = match c.kind(*run.last()?) {
            NodeKind::DotAccess => c.field(*run.last()?, PayloadField::Operator)?,
            _ => return None,
        };
        let invoked = run
            .iter()
            .any(|&piece| c.kind(piece) == NodeKind::Arguments);
        if invoked || c.head_token(*run.first()?) != dot - 1 {
            return None;
        }
        infinity_at(c, dot)
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule};

        /// `test/rules/sized_box_shrink_expand_test.dart` `test_shrink` —
        /// zero width and height report the shrink named constructor.
        #[test]
        fn reports_shrink_for_double_zero() {
            let f = Fixture::parse("void f() { var s = SizedBox(width: 0, height: 0); }");
            let lints = f.run(&one_rule("sized_box_shrink_expand"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Use 'SizedBox.shrink' to avoid needing to specify the 'height' and 'width'."
            );
            assert_eq!(
                lints[0].correction.as_deref(),
                Some(
                    "Try using 'SizedBox.shrink' and removing the 'height' and 'width' arguments."
                )
            );
        }

        /// A fractional zero is still zero (:79-90 the double face).
        #[test]
        fn reports_shrink_for_a_fractional_zero() {
            let f = Fixture::parse("void f() { var s = SizedBox(width: 0.0, height: 0); }");
            assert_eq!(f.run(&one_rule("sized_box_shrink_expand")).len(), 1);
        }

        /// `test_shrink`/`test_expand` — `double.infinity` on both axes
        /// reports the expand named constructor.
        #[test]
        fn reports_expand_for_double_infinity() {
            let f = Fixture::parse(
                "void f() { var s = SizedBox(width: double.infinity, height: double.infinity); }",
            );
            let lints = f.run(&one_rule("sized_box_shrink_expand"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Use 'SizedBox.expand' to avoid needing to specify the 'height' and 'width'."
            );
        }

        /// Unknown values, a partial pair, and a nonzero pair pass
        /// (:46-53 the `argumentValue` null posture).
        #[test]
        fn passes_on_unknown_or_partial_values() {
            let f = Fixture::parse("void f(w, h) { var s = SizedBox(width: w, height: h); }");
            assert!(f.run(&one_rule("sized_box_shrink_expand")).is_empty());

            let f = Fixture::parse("void f() { var s = SizedBox(width: 0); }");
            assert!(f.run(&one_rule("sized_box_shrink_expand")).is_empty());

            let f = Fixture::parse("void f() { var s = SizedBox(width: 0, height: 1); }");
            assert!(f.run(&one_rule("sized_box_shrink_expand")).is_empty());
        }

        /// The named constructors never re-fire (the
        /// `constructorName.name == null` gate over the access-marker
        /// face), and `key` rides along silently (:50-55 no default arm).
        #[test]
        fn named_constructors_and_other_arguments_pass() {
            let f = Fixture::parse("void f() { SizedBox.shrink(); }");
            assert!(f.run(&one_rule("sized_box_shrink_expand")).is_empty());

            let f = Fixture::parse("void f(k) { var s = SizedBox(key: k, width: 0, height: 0); }");
            assert_eq!(f.run(&one_rule("sized_box_shrink_expand")).len(), 1);
        }

        /// The R4 shadow face: a same-file `class SizedBox` is user code.
        #[test]
        fn same_file_sized_box_is_user_code() {
            let f =
                Fixture::parse("class SizedBox {}\nvoid f() { SizedBox(width: 0, height: 0); }");
            assert!(f.run(&one_rule("sized_box_shrink_expand")).is_empty());
        }
    }
}

pub(crate) mod use_full_hex_values_for_flutter_colors {
    use super::{Check, NodeKind, RuleSpec, creation_parts, spells_framework};
    use crate::rules::slice;

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/use_full_hex_values_for_flutter_colors.dart`
    /// — a `Color` creation whose first argument is an integer literal
    /// that is not an 8-digit hexadecimal integer reports at the argument
    /// (:40-53: the lexeme lowercased and stripped of underscores must
    /// start `0x` and carry ten characters). Message: `diagnostic.g.dart`
    /// `useFullHexValuesForFlutterColors`.
    ///
    /// Approximation register (R4): `Color` identity by spelling with the
    /// negative-identity refutation — the unnamed-ctor callee spelling,
    /// with same-file declarations and resolution-resolved workspace
    /// containers refuting (the official `isSameAs dart.ui/Color`
    /// posture's reachable half); a user class inside an unindexed
    /// dependency still matches (what dependency indexing would close).
    /// A prefixed `p.Color(...)` under-reports.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
        "use_full_hex_values_for_flutter_colors",
        &[
            NodeKind::Send,
            NodeKind::NewExpression,
            NodeKind::ImplicitCreation,
        ],
        check,
    );

    fn check(c: &mut Check<'_>) {
        let Some((callee, name, args)) = creation_parts(c) else {
            return;
        };
        if name != "Color" || !spells_framework(c, callee, name) {
            return;
        }
        let pieces = c.collect_direct(args);
        let groups = slice::arguments(c, &pieces);
        let Some(group) = groups.first() else {
            return;
        };
        // `arguments.first is IntegerLiteral` (:49): the bare first group
        // is one literal piece — a named argument never matches.
        if group.len() != 1 || c.kind(pieces[group.start]) != NodeKind::LiteralInt {
            return;
        }
        let literal = pieces[group.start];
        let mut value = c.lexeme(c.head_token(literal)).to_lowercase();
        value.retain(|ch| ch != '_');
        if !value.starts_with("0x") || value.len() != 10 {
            c.report(c.utf16_span(literal), &[]);
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

        /// `test/rules/use_full_hex_values_for_flutter_colors_test.dart`
        /// `test_partialHex`/`test_decimal` — partial hex and decimal
        /// integers report at the argument.
        #[test]
        fn reports_partial_or_decimal_integers() {
            let f = Fixture::parse("void f() { var c = Color(0xFF); }");
            let lints = f.run(&one_rule("use_full_hex_values_for_flutter_colors"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Instances of 'Color' should be created using an 8-digit hexadecimal integer (such as '0xFFFFFFFF')."
            );
            assert_eq!(lints[0].correction, None);

            let f = Fixture::parse("void f() { var c = Color(12); }");
            assert_eq!(
                f.run(&one_rule("use_full_hex_values_for_flutter_colors"))
                    .len(),
                1
            );
        }

        /// Full hex (separators included) passes, a named first argument
        /// and a member callee never fire (:44-53).
        #[test]
        fn passes_on_full_hex_and_other_faces() {
            let f = Fixture::parse("void f() { var c = Color(0xFFFFFFFF); }");
            assert!(
                f.run(&one_rule("use_full_hex_values_for_flutter_colors"))
                    .is_empty()
            );

            let f = Fixture::parse("void f() { var c = Color(0xFF_FF_FF_FF); }");
            assert!(
                f.run(&one_rule("use_full_hex_values_for_flutter_colors"))
                    .is_empty()
            );

            let f = Fixture::parse("void f() { var c = Color(color: 0xFF); }");
            assert!(
                f.run(&one_rule("use_full_hex_values_for_flutter_colors"))
                    .is_empty()
            );

            let f = Fixture::parse("void f() { var c = Color.fromARGB(1, 2, 3, 4); }");
            assert!(
                f.run(&one_rule("use_full_hex_values_for_flutter_colors"))
                    .is_empty()
            );
        }

        /// The report pins the integer literal (:53).
        #[test]
        fn report_pins_the_integer_literal() {
            let f = Fixture::parse("void f() { var c = Color(0xFF); }");
            let lints = f.run(&one_rule("use_full_hex_values_for_flutter_colors"));
            let literal = tok(&f.source, &f.tokens, "0xFF", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, literal));
        }

        /// The R4 shadow face: a same-file `class Color` is user code.
        #[test]
        fn same_file_color_is_user_code() {
            let f = Fixture::parse("class Color {}\nvoid f() { var c = Color(1); }");
            assert!(
                f.run(&one_rule("use_full_hex_values_for_flutter_colors"))
                    .is_empty()
            );
        }
    }
}

pub(crate) mod use_key_in_widget_constructors {
    use dartq_ast::mod_flags;

    use super::{
        Check, NodeKind, RuleSpec, SupertypeClause, carries_annotation, chain_spells_root,
        class_name_row, ctor_defines_key, extends_only, formal_rows, member_name_row, named_ctor,
        param_name, parameter_list, subclassed_same_file, unnamed_ctor,
    };
    use crate::rules::{
        body_members, class_declaration_name, same_file_declaration, supertypes, tail_token,
        utf16_token_span,
    };

    /// The `augment` `ModSet` bit (bridge `mod_bits`).
    const AUGMENT_BIT: u16 = 1 << 1;

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/use_key_in_widget_constructors.dart` — a
    /// public widget class without constructors reports at the class name
    /// when the superclass's unnamed constructor can carry a key
    /// (:45-57); a public widget constructor without a `super.key`
    /// parameter and without a key-forwarding explicit super/redirecting
    /// initializer reports at the constructor name (:59-124).
    /// Message: `diagnostic.g.dart` `useKeyInWidgetConstructors`.
    ///
    /// Approximation register (R4): widget identity by spelling with the
    /// negative-identity refutation — the `extendsWidget` chain reads
    /// written `extends` names against the widget roots with same-file
    /// links followed, and a root spelling that resolves to a workspace
    /// container refutes; a user class inside an unindexed dependency
    /// carrying a widget root name still matches (what dependency
    /// indexing would close). Member kinds read their syntactic shapes: a
    /// `key` formal parameter stands for the Key type (`definesKeyParameter`
    /// — a `key` parameter of an unrelated type keeps the silent face,
    /// what the Key element identity would close), and a constructor of a
    /// same-file-extended class stays silent — the subclass's class face
    /// reads the same unnamed constructor and answers once, where the
    /// oracle reports both faces (the ruled divergence row). Initializer
    /// targets resolve through the same-file channel (a foreign `super`
    /// target presumes the framework key posture — direction:
    /// over-reports). A superclass with zero declared constructors
    /// resolves its unnamed face to the element model's synthetic default
    /// — keyless, the silent `test_missingConstructor_superWithoutKey`
    /// posture. `@visibleForTesting` reads by annotation spelling. The
    /// primary-constructor face (:70-78) is not mounted —
    /// primary-constructor shapes under-report.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
        "use_key_in_widget_constructors",
        &[NodeKind::ClassDeclaration, NodeKind::Constructor],
        check,
    );

    /// The framework widget roots the `extendsWidget` chain matches by
    /// spelling (`flutter_utils` `_nameWidget`/`_nameStatefulWidget` plus
    /// the `StatelessWidget` shorthand the corpus writes).
    const WIDGET_ROOTS: &[&str] = &["Widget", "StatelessWidget", "StatefulWidget"];

    fn check(c: &mut Check<'_>) {
        match c.kind(c.node()) {
            NodeKind::ClassDeclaration => check_class(c),
            NodeKind::Constructor => check_ctor(c),
            _ => {}
        }
    }

    fn check_class(c: &mut Check<'_>) {
        let class = c.node();
        let Some(name_row) = class_name_row(c, class) else {
            return;
        };
        let name = c.lexeme(c.head_token(name_row));
        // Public, not the Widget class itself, not test-only (:46-49).
        if name.starts_with('_')
            || name == "Widget"
            || carries_annotation(c, class, "visibleForTesting")
        {
            return;
        }
        if !chain_spells_root(c, class, WIDGET_ROOTS, extends_only) {
            return;
        }
        // No origin constructors (:50-51).
        let members = body_members(c, class);
        if members.iter().any(|&m| {
            matches!(
                c.kind(m),
                NodeKind::Constructor
                    | NodeKind::Factory
                    | NodeKind::PrimaryConstructor
                    | NodeKind::PrimaryConstructorBody
            )
        }) {
            return;
        }
        // The superclass's unnamed constructor must be able to carry the
        // key (:52-55).
        if !super_constructor_carries_key(c, class) {
            return;
        }
        c.report(utf16_token_span(c, c.head_token(name_row)), &[]);
    }

    fn check_ctor(c: &mut Check<'_>) {
        let ctor = c.node();
        // Augmentations skip (:61); factories are Factory nodes and never
        // dispatch here.
        if mod_flags(c.unit.ast, ctor) & AUGMENT_BIT != 0 {
            return;
        }
        let Some(class) = c
            .path
            .iter()
            .rev()
            .copied()
            .find(|&a| c.kind(a) == NodeKind::ClassDeclaration)
        else {
            return;
        };
        let Some(class_name_row) = class_name_row(c, class) else {
            return;
        };
        let class_name = c.lexeme(c.head_token(class_name_row));
        // Private constructor/class, test-only, or the Widget class itself
        // (:75-83). The named-constructor part after the dot names the
        // official `node.name` face (`Base._()` is private through `_`).
        let Some(ctor_name_row) = member_name_row(c, ctor) else {
            return;
        };
        let ctor_name = c.lexeme(c.head_token(ctor_name_row));
        if ctor_name.starts_with('_')
            || class_name.starts_with('_')
            || class_name == "Widget"
            || carries_annotation(c, ctor, "visibleForTesting")
            || carries_annotation(c, class, "visibleForTesting")
        {
            return;
        }
        if !chain_spells_root(c, class, WIDGET_ROOTS, extends_only) {
            return;
        }
        // The subclass face owns a same-file superclass's verdict: the
        // subclass's class face reads this constructor through the same
        // unnamed-constructor gate, so the constructor face stays silent
        // for a class the file extends (the key posture reports once).
        if subclassed_same_file(c, class, class_name) {
            return;
        }
        // A `key` parameter is the fix (:126-134) — the `super.key` face
        // rides the same spelling (`definesKeyParameter` over the name).
        let Some(params) = parameter_list(c, ctor) else {
            return;
        };
        if formal_rows(c, params)
            .iter()
            .any(|&p| param_name(c, p) == Some("key"))
        {
            return;
        }
        // Every explicit super/redirecting initializer must be key-clean
        // (`isMissingKey`, :136-143); with none present the implicit
        // super gate decides (:100-110).
        let mut explicit = false;
        for invocation in explicit_invocation_initializers(c, ctor) {
            if initializer_missing_key(c, class, ctor, invocation) {
                return;
            }
            explicit = true;
        }
        if !explicit && !super_constructor_carries_key(c, class) {
            return;
        }
        c.report(c.utf16_span(ctor_name_row), &[]);
    }

    /// The direct `extends` superclass of a class: a foreign spelling, or
    /// the same-file declared row.
    enum Superclass {
        Foreign,
        SameFile(u32),
    }

    fn direct_superclass(c: &Check<'_>, class: u32) -> Option<Superclass> {
        let name = supertypes(c, class)
            .into_iter()
            .find(|(_, clause)| *clause == SupertypeClause::Extends)
            .map(|(name, _)| name)?;
        Some(same_file_declaration(c, name).map_or(Superclass::Foreign, Superclass::SameFile))
    }

    /// Whether the class-like declares any constructor: a `Constructor`,
    /// `Factory`, or primary-constructor member (the origin-ctor set of
    /// :50-51). A class with none carries the element model's synthetic
    /// default constructor (element.dart `unnamedConstructor`).
    fn declares_ctor(c: &Check<'_>, class: u32) -> bool {
        body_members(c, class).iter().any(|&m| {
            matches!(
                c.kind(m),
                NodeKind::Constructor
                    | NodeKind::Factory
                    | NodeKind::PrimaryConstructor
                    | NodeKind::PrimaryConstructorBody
            )
        })
    }

    /// The `superUnnamedConstructor` gate (:52-55, :126-128): the direct
    /// superclass's unnamed constructor must define (or, for a foreign
    /// spelling, presumably define) a key parameter — otherwise there is
    /// nothing for this class to declare. A superclass with zero declared
    /// constructors carries the synthetic default, which defines no `key`
    /// and answers silent (`test_missingConstructor_superWithoutKey`);
    /// constructors present but none unnamed answers report (the official
    /// `superConstructor == null` fall-through).
    fn super_constructor_carries_key(c: &Check<'_>, class: u32) -> bool {
        match direct_superclass(c, class) {
            None | Some(Superclass::Foreign) => true,
            Some(Superclass::SameFile(row)) => {
                let Some(name) = class_declaration_name(c, row) else {
                    return true;
                };
                match unnamed_ctor(c, row, name) {
                    Some(ctor) => ctor_defines_key(c, ctor),
                    None if !declares_ctor(c, row) => false,
                    None => true,
                }
            }
        }
    }

    /// One explicit super/redirecting initializer over the token face.
    #[derive(Clone, Copy)]
    enum InitializerFace {
        Unnamed,
        Named,
    }

    /// One explicit super/redirecting initializer over the token face.
    #[derive(Clone, Copy)]
    struct InitializerInvocation<'a> {
        head: u32,
        member: Option<&'a str>,
        face: InitializerFace,
    }

    /// The explicit super/redirecting initializer faces over the
    /// constructor's token surface between the parameter list and the body
    /// (the `avoid_unused_constructor_parameters` scan idiom):
    /// `super(`, `super.name(`, `this(`, `this.name(`. A `this.name = …`
    /// field initializer and asserts never match.
    fn explicit_invocation_initializers<'a>(
        c: &'a Check<'_>,
        ctor: u32,
    ) -> Vec<InitializerInvocation<'a>> {
        let Some(params) = parameter_list(c, ctor) else {
            return Vec::new();
        };
        let start = tail_token(c, params) + 1;
        let mut end = tail_token(c, ctor) + 1;
        // The initializers sit before the body's opening brace.
        for token in start..end {
            if c.lexeme(token) == "{" {
                end = token;
                break;
            }
        }
        let mut out = Vec::new();
        let mut t = start;
        while t < end {
            let face = match c.lexeme(t) {
                "super" if t + 1 < end => Some(match c.lexeme(t + 1) {
                    "." => (Some(c.lexeme(t + 2)), InitializerFace::Named),
                    _ => (None, InitializerFace::Unnamed),
                }),
                "this" if t + 1 < end => match c.lexeme(t + 1) {
                    "(" => Some((None, InitializerFace::Unnamed)),
                    "." if t + 3 < end && c.lexeme(t + 3) == "(" => {
                        Some((Some(c.lexeme(t + 2)), InitializerFace::Named))
                    }
                    _ => None,
                },
                _ => None,
            };
            if let Some((member, face)) = face {
                out.push(InitializerInvocation {
                    head: t,
                    member,
                    face,
                });
            }
            t += 1;
        }
        out
    }

    /// The resolved target constructor of an invocation initializer.
    enum InitializerTarget {
        /// A foreign spelling: the framework constructor posture —
        /// resolved and key-defining.
        Foreign,
        Row(u32),
        /// The synthetic default constructor the element model
        /// materializes for a class with zero declared constructors —
        /// resolved, and defines no `key`.
        Synthetic,
        /// A same-file class without the named/unnamed constructor —
        /// never missing (the official `element == null` arm).
        Unresolved,
    }

    fn initializer_target<'a>(
        c: &'a Check<'_>,
        class: u32,
        invocation: InitializerInvocation<'a>,
    ) -> InitializerTarget {
        let InitializerInvocation { head, member, face } = invocation;
        match c.lexeme(head) {
            "super" => match direct_superclass(c, class) {
                None => InitializerTarget::Unresolved,
                Some(Superclass::Foreign) => InitializerTarget::Foreign,
                Some(Superclass::SameFile(row)) => {
                    let class_name = class_declaration_name(c, row);
                    match (face, member) {
                        (InitializerFace::Unnamed, _) => {
                            match class_name.and_then(|name| unnamed_ctor(c, row, name)) {
                                Some(ctor) => InitializerTarget::Row(ctor),
                                // Zero declared constructors: `super()`
                                // resolves to the synthetic default, which
                                // defines no `key` (the official
                                // `isMissingKey` true arm).
                                None if !declares_ctor(c, row) => InitializerTarget::Synthetic,
                                None => InitializerTarget::Unresolved,
                            }
                        }
                        (InitializerFace::Named, Some(member)) => named_ctor(c, row, member)
                            .map_or(InitializerTarget::Unresolved, InitializerTarget::Row),
                        (InitializerFace::Named, None) => InitializerTarget::Unresolved,
                    }
                }
            },
            "this" => {
                let class_name = class_declaration_name(c, class);
                let target = match (face, member) {
                    (InitializerFace::Unnamed, _) => {
                        class_name.and_then(|name| unnamed_ctor(c, class, name))
                    }
                    (InitializerFace::Named, Some(member)) => named_ctor(c, class, member),
                    (InitializerFace::Named, None) => None,
                };
                target.map_or(InitializerTarget::Unresolved, InitializerTarget::Row)
            }
            _ => InitializerTarget::Unresolved,
        }
    }

    /// `argumentList.containsKeyArgument` over the spelling face: a
    /// top-level `key:` named argument inside the invocation's parentheses.
    fn passes_key_argument(c: &Check<'_>, invocation: InitializerInvocation<'_>, end: u32) -> bool {
        let InitializerInvocation { head, face, .. } = invocation;
        let open = match face {
            InitializerFace::Unnamed => head + 1,
            InitializerFace::Named => head + 3,
        };
        if open >= end || c.lexeme(open) != "(" {
            return false;
        }
        let mut depth = 0u32;
        let mut t = open;
        while t < end {
            match c.lexeme(t) {
                "(" => depth += 1,
                ")" => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            t += 1;
        }
        if t >= end {
            return false; // unbalanced recovery: no verdict
        }
        (open + 1..t).any(|i| {
            c.lexeme(i) == "key" && c.lexeme(i + 1) == ":" && matches!(c.lexeme(i - 1), "(" | ",")
        })
    }

    /// The `isMissingKey` verdict (:136-143): a super/redirecting
    /// initializer is key-missing when its target has no `key` parameter
    /// or the invocation passes one.
    fn initializer_missing_key(
        c: &Check<'_>,
        class: u32,
        ctor: u32,
        invocation: InitializerInvocation<'_>,
    ) -> bool {
        let end = tail_token(c, ctor) + 1;
        match initializer_target(c, class, invocation) {
            InitializerTarget::Unresolved => false,
            InitializerTarget::Foreign => passes_key_argument(c, invocation, end),
            // The synthetic default defines no `key`: the official
            // `!definesKeyParameter || containsKeyArgument` verdict is
            // unconditionally true.
            InitializerTarget::Synthetic => true,
            InitializerTarget::Row(row) => {
                !ctor_defines_key(c, row) || passes_key_argument(c, invocation, end)
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

        /// `test/rules/use_key_in_widget_constructors_test.dart`
        /// `test_noKey` — a public widget class without constructors
        /// reports at the class name.
        #[test]
        fn class_without_constructors_reports() {
            let f = Fixture::parse("class MyWidget extends StatelessWidget {}\n");
            let lints = f.run(&one_rule("use_key_in_widget_constructors"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Constructors for public widgets should have a named 'key' parameter."
            );
            assert_eq!(
                lints[0].correction.as_deref(),
                Some("Try adding a named parameter to the constructor.")
            );
            let name = tok(&f.source, &f.tokens, "MyWidget", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, name));
        }

        /// Private classes, non-widgets, and test-only annotations pass
        /// (:46-49).
        #[test]
        fn private_and_non_widget_classes_pass() {
            let f = Fixture::parse("class _W extends StatelessWidget {}\n");
            assert!(
                f.run(&one_rule("use_key_in_widget_constructors"))
                    .is_empty()
            );

            let f = Fixture::parse("class C {}\n");
            assert!(
                f.run(&one_rule("use_key_in_widget_constructors"))
                    .is_empty()
            );

            let f = Fixture::parse("@visibleForTesting\nclass W extends StatelessWidget {}\n");
            assert!(
                f.run(&one_rule("use_key_in_widget_constructors"))
                    .is_empty()
            );
        }

        /// `test_constructorNoKey` — a widget constructor without a key
        /// parameter reports at the constructor name; `super.key` passes.
        #[test]
        fn constructor_without_key_reports_and_super_key_passes() {
            let f = Fixture::parse(
                "class MyWidget extends StatelessWidget {\n  MyWidget(this.x);\n  final int x;\n}\n",
            );
            let lints = f.run(&one_rule("use_key_in_widget_constructors"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            let name = tok(&f.source, &f.tokens, "MyWidget", 1);
            assert_eq!(lints[0].range.start, utf16_at(&f, name));

            let f = Fixture::parse(
                "class MyWidget extends StatelessWidget {\n  MyWidget({super.key});\n}\n",
            );
            assert!(
                f.run(&one_rule("use_key_in_widget_constructors"))
                    .is_empty()
            );
        }

        /// The explicit-initializer faces: a key-forwarding `super(key: …)`
        /// passes, a bare `super()` reports (:136-143).
        #[test]
        fn explicit_super_faces() {
            let f = Fixture::parse(
                "class MyWidget extends StatelessWidget {\n  MyWidget(Key k) : super(key: k);\n}\n",
            );
            assert!(
                f.run(&one_rule("use_key_in_widget_constructors"))
                    .is_empty()
            );

            let f = Fixture::parse(
                "class MyWidget extends StatelessWidget {\n  MyWidget(this.x) : super();\n  final int x;\n}\n",
            );
            assert_eq!(f.run(&one_rule("use_key_in_widget_constructors")).len(), 1);
        }

        /// The same-file superclass gate (:52-55): an unnamed constructor
        /// defining `key` lets the subclass report, one without `key`
        /// keeps it silent.
        #[test]
        fn same_file_superclass_gate() {
            let f = Fixture::parse(
                "class Base extends StatelessWidget { Base({Key? key}); }\nclass MyWidget extends Base {}\n",
            );
            assert_eq!(f.run(&one_rule("use_key_in_widget_constructors")).len(), 1);

            let f = Fixture::parse(
                "class Base extends StatelessWidget { Base(); }\nclass MyWidget extends Base {}\n",
            );
            assert!(
                f.run(&one_rule("use_key_in_widget_constructors"))
                    .is_empty()
            );
        }

        /// The R4 spelling faces: an undeclared widget-root supertype
        /// spelling decides (the positive face); a same-file declaration
        /// of the name is user code and the chain descends into it
        /// instead.
        #[test]
        fn same_file_supertype_shadow_passes() {
            let f = Fixture::parse(
                "class StatelessWidget {}\nclass MyWidget extends StatelessWidget {}\n",
            );
            assert!(
                f.run(&one_rule("use_key_in_widget_constructors"))
                    .is_empty()
            );
        }

        /// `test_missingConstructor_superWithoutKey` — a superclass with
        /// zero declared constructors carries the element model's synthetic
        /// default constructor (no `key`): the superclass itself reports,
        /// the subclass stays silent.
        #[test]
        fn constructorless_superclass_reports_itself_not_subclass() {
            let f = Fixture::parse(
                "class Base extends StatelessWidget {}\nclass MyWidget extends Base {}\n",
            );
            let lints = f.run(&one_rule("use_key_in_widget_constructors"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            let base = tok(&f.source, &f.tokens, "Base", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, base));
        }

        /// Constructors present but none unnamed: `unnamedConstructor` is
        /// null and the official gate still reports (the over-correction
        /// guard for the synthetic face). The private `._()` keeps the
        /// superclass's own constructor face out of the count.
        #[test]
        fn named_only_superclass_still_reports() {
            let f = Fixture::parse(
                "class Base extends StatelessWidget { Base._(); }\nclass MyWidget extends Base {}\n",
            );
            let lints = f.run(&one_rule("use_key_in_widget_constructors"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            let my = tok(&f.source, &f.tokens, "MyWidget", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, my));
        }

        /// `: super()` on a zero-declared-constructor superclass resolves
        /// to the synthetic default (no `key`) — the official
        /// `isMissingKey` arm answers true and the constructor stays
        /// silent; the ctor-less superclass itself still reports.
        #[test]
        fn super_initializer_on_constructorless_superclass_passes() {
            let f = Fixture::parse(
                "class Base extends StatelessWidget {}\nclass MyWidget extends Base {\n  MyWidget() : super();\n}\n",
            );
            let lints = f.run(&one_rule("use_key_in_widget_constructors"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            let base = tok(&f.source, &f.tokens, "Base", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, base));
        }
    }
}

pub(crate) mod no_logic_in_create_state {
    use super::{Check, NodeKind, RuleSpec, chain_spells_root, extends_only, member_name_row};
    use crate::rules::{is_statement_row, slice};

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/no_logic_in_create_state.dart` — a
    /// `createState` method of a `StatefulWidget` class reports when its
    /// body is not exactly one return of an argument-less instance
    /// creation (:33-58): a block body with other statements (or a bare
    /// `return;`) reports at the body, an expression or returned value
    /// that is not an argument-less creation reports at that expression,
    /// an empty body stays silent.
    /// Message: `diagnostic.g.dart` `noLogicInCreateState`.
    ///
    /// Approximation register (R4): widget identity by spelling with the
    /// negative-identity refutation — the `StatefulWidget` gate reads the
    /// written `extends` chain against the spelling (same-file
    /// declarations descend; resolution-resolved workspace classes
    /// refute; a user class inside an unindexed dependency with the name
    /// still matches — what dependency indexing would close). The
    /// instance-creation face reads the creation shapes (a dot-shorthand
    /// constructor invocation under-reports its silent face).
    pub(crate) static SPEC: RuleSpec =
        crate::spec!("no_logic_in_create_state", &[NodeKind::Method], check,);

    fn check(c: &mut Check<'_>) {
        let method = c.node();
        let Some(name_row) = member_name_row(c, method) else {
            return;
        };
        if c.kind(name_row) != NodeKind::Identifier
            || c.lexeme(c.head_token(name_row)) != "createState"
        {
            return;
        }
        // The enclosing class must extend the StatefulWidget spelling.
        let Some(class) = c
            .path
            .iter()
            .rev()
            .copied()
            .find(|&a| c.kind(a) == NodeKind::ClassDeclaration)
        else {
            return;
        };
        if !chain_spells_root(c, class, &["StatefulWidget"], extends_only) {
            return;
        }
        let kids = c.collect_direct(method);
        let Some(body) = kids.into_iter().find(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::BlockBody
                    | NodeKind::ExpressionBody
                    | NodeKind::EmptyBody
                    | NodeKind::NoBody
                    | NodeKind::SkippedBody
            )
        }) else {
            return;
        };
        match c.kind(body) {
            NodeKind::ExpressionBody => {
                // The production body row is a childless marker; its
                // expression tiles beside it under the method, after the
                // `=>` token (hand-built trees nest it inside the body).
                let window = c.children(body);
                let rows: Vec<u32> = if window.is_empty() {
                    let arrow = c.head_token(body);
                    let method_window = c.children(method);
                    (method_window.start..method_window.end)
                        .filter(|&row| {
                            row as u32 != body && c.unit.ast.token_range(row as u32).start >= arrow
                        })
                        .collect()
                } else {
                    (window.start..window.end).collect()
                };
                let Some(&root) = rows.last() else {
                    return;
                };
                if is_bare_creation(c, &rows, rows.len() - 1) {
                    return;
                }
                c.report(c.utf16_span(root), &[]);
            }
            NodeKind::BlockBody => {
                // Hand-built trees nest a `Block` row inside the body; the
                // production tiling keeps the statements as direct body
                // children.
                let block = c
                    .collect_direct(body)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::Block);
                let rows: Vec<u32> = match block {
                    Some(block) => c.collect_direct(block),
                    None => c.collect_direct(body),
                };
                let statement_count = rows.iter().filter(|&&row| is_statement_row(c, row)).count();
                let return_pos = rows
                    .iter()
                    .position(|&row| c.kind(row) == NodeKind::ReturnStatement);
                match (statement_count, return_pos) {
                    (1, Some(pos)) => {
                        let ret = rows[pos];
                        // The returned expression tiles inside the return
                        // statement's subtree, after the `return` keyword.
                        let arrow = c.head_token(ret);
                        let ret_window = c.children(ret);
                        let returned: Vec<u32> = (ret_window.start..ret_window.end)
                            .filter(|&row| c.unit.ast.token_range(row as u32).start > arrow)
                            .collect();
                        match returned.last() {
                            Some(&root) if !is_bare_creation(c, &returned, returned.len() - 1) => {
                                c.report(c.utf16_span(root), &[]);
                            }
                            Some(_) => {}
                            None => c.report(c.utf16_span(body), &[]), // bare `return;`
                        }
                    }
                    _ => c.report(c.utf16_span(body), &[]),
                }
            }
            _ => {} // an empty body stays silent (:49-51)
        }
    }

    /// The silent face: the returned expression is an argument-less
    /// instance creation (`expressionToTest is InstanceCreationExpression
    /// && arguments.isEmpty`, :70-78) — an invocation send (not the
    /// bare-name tear-off group), a `new`/implicit creation, or a const
    /// wrapper carrying one.
    fn is_bare_creation(c: &Check<'_>, run: &[u32], index: usize) -> bool {
        let root = run[index];
        match c.kind(root) {
            NodeKind::Send => {
                if index
                    .checked_sub(3)
                    .is_some_and(|start| slice::is_tear_off_group(c, run, start))
                {
                    return false; // a bare identifier, not a creation
                }
                let mut i = index;
                while i > 0
                    && matches!(
                        c.kind(run[i - 1]),
                        NodeKind::TypeArguments | NodeKind::NoTypeArguments
                    )
                {
                    i -= 1;
                }
                i > 0
                    && c.kind(run[i - 1]) == NodeKind::Arguments
                    && slice::arguments(c, &c.collect_direct(run[i - 1])).is_empty()
            }
            NodeKind::NewExpression | NodeKind::ImplicitCreation => c
                .collect_direct(root)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Arguments)
                .is_some_and(|args| slice::arguments(c, &c.collect_direct(args)).is_empty()),
            NodeKind::ConstExpression | NodeKind::ConstLiteral => {
                let window = c.children(root);
                let mut creation = None;
                for row in window.start..window.end {
                    if matches!(
                        c.kind(row),
                        NodeKind::Send | NodeKind::NewExpression | NodeKind::ImplicitCreation
                    ) {
                        creation = Some(row);
                    }
                }
                match creation {
                    None => false, // `const []` and friends are not creations
                    Some(row) if c.kind(row) == NodeKind::Send => {
                        // A const creation is bare unless the const window
                        // carries an argument list.
                        !(window.start..window.end).any(|row| c.kind(row) == NodeKind::Arguments)
                    }
                    Some(row) => is_bare_creation(c, &[row], 0),
                }
            }
            _ => false,
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule};

        /// `test/rules/no_logic_in_create_state_test.dart`
        /// `test_logicInCreateState` — a creation with arguments reports
        /// at the expression.
        #[test]
        fn reports_a_stateful_creation_with_arguments() {
            let f = Fixture::parse(
                "class W extends StatefulWidget {\n  @override\n  S createState() { return S(x); }\n}\n",
            );
            let lints = f.run(&one_rule("no_logic_in_create_state"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(lints[0].message, "Don't put any logic in 'createState'.");
            assert_eq!(
                lints[0].correction.as_deref(),
                Some("Try moving the logic out of 'createState'.")
            );
        }

        /// `test_noLogic` — the bare creation faces stay silent.
        #[test]
        fn bare_creations_pass() {
            let f = Fixture::parse(
                "class W extends StatefulWidget {\n  S createState() { return S(); }\n}\n",
            );
            assert!(f.run(&one_rule("no_logic_in_create_state")).is_empty());

            let f =
                Fixture::parse("class W extends StatefulWidget {\n  S createState() => S();\n}\n");
            assert!(f.run(&one_rule("no_logic_in_create_state")).is_empty());
        }

        /// Non-creation returns report at the expression; a multi-statement
        /// body reports at the body (:48-64).
        #[test]
        fn non_creation_returns_and_other_bodies_report() {
            let f =
                Fixture::parse("class W extends StatefulWidget {\n  S createState() => s;\n}\n");
            assert_eq!(f.run(&one_rule("no_logic_in_create_state")).len(), 1);

            let f = Fixture::parse(
                "class W extends StatefulWidget {\n  S createState() { var s = S(); return s; }\n}\n",
            );
            assert_eq!(f.run(&one_rule("no_logic_in_create_state")).len(), 1);

            let f = Fixture::parse(
                "class W extends StatefulWidget {\n  S createState() { return; }\n}\n",
            );
            assert_eq!(f.run(&one_rule("no_logic_in_create_state")).len(), 1);
        }

        /// A class outside the `StatefulWidget` chain and an empty body
        /// never fire (:34-37, :49-51).
        #[test]
        fn foreign_classes_and_empty_bodies_pass() {
            let f = Fixture::parse("class W {\n  S createState() => S(x);\n}\n");
            assert!(f.run(&one_rule("no_logic_in_create_state")).is_empty());

            let f = Fixture::parse("class W extends StatefulWidget {\n  S createState();\n}\n");
            assert!(f.run(&one_rule("no_logic_in_create_state")).is_empty());
        }

        /// The R4 shadow face: a same-file `class StatefulWidget` is user
        /// code, so the chain descends into it and stays silent.
        #[test]
        fn same_file_stateful_widget_shadow_passes() {
            let f = Fixture::parse(
                "class StatefulWidget {}\nclass W extends StatefulWidget {\n  S createState() => S(x);\n}\n",
            );
            assert!(f.run(&one_rule("no_logic_in_create_state")).is_empty());
        }
    }
}

pub(crate) mod diagnostic_describe_all_properties {
    use dartq_ast::mod_flags;
    use dartq_parser::MemberKind;

    use super::{
        Check, NodeKind, PayloadField, RuleSpec, any_clause, chain_spells_root, member_name_row,
        member_named,
    };
    use crate::rules::{
        body_members, get_set_token, initialized_identifier_name, utf16_token_span,
    };

    /// The `static` `ModSet` bit (bridge `mod_bits`, member family).
    const STATIC_BIT: u16 = 1 << 3;

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/diagnostic_describe_all_properties.dart`
    /// — a Diagnosticable class's public instance fields and getters that
    /// are referenced by neither `debugFillProperties` nor
    /// `debugDescribeChildren` report at each property name (:105-124).
    /// Message: `diagnostic.g.dart` `diagnosticDescribeAllProperties`.
    ///
    /// Approximation register (R4): Diagnosticable identity by spelling
    /// with the negative-identity refutation — written supertypes of any
    /// clause, same-file links followed and resolution-resolved workspace
    /// containers refuting (a user class inside an unindexed dependency
    /// with the name still matches — what dependency indexing would
    /// close). The `isWidgetProperty` and `overriddenMember` exemptions
    /// read no element model — widget-typed properties and overrides
    /// over-report. Primary-constructor declaring parameters (:89-103)
    /// under-report. The debug-method reference scan reads the body's
    /// identifier spellings (a shadowing declaration over-credits, the
    /// documented user-shadow family).
    pub(crate) static SPEC: RuleSpec = crate::spec!(
        "diagnostic_describe_all_properties",
        &[NodeKind::ClassDeclaration],
        check,
    );

    fn check(c: &mut Check<'_>) {
        let class = c.node();
        if !chain_spells_root(c, class, &["Diagnosticable"], any_clause) {
            return;
        }
        let members = body_members(c, class);
        let mut properties: Vec<u32> = Vec::new();
        for member in &members {
            match c.kind(*member) {
                NodeKind::Method => {
                    if get_set_token(c, *member) != Some("get")
                        || mod_flags(c.unit.ast, *member) & STATIC_BIT != 0
                    {
                        continue;
                    }
                    let Some(name_row) = member_name_row(c, *member) else {
                        continue;
                    };
                    if c.kind(name_row) != NodeKind::Identifier {
                        continue;
                    }
                    let token = c.head_token(name_row);
                    if c.lexeme(token).starts_with('_') {
                        continue;
                    }
                    properties.push(token);
                }
                NodeKind::Fields => {
                    if c.field(*member, PayloadField::MemberKind)
                        != Some(MemberKind::NonStaticField as u32)
                    {
                        continue;
                    }
                    for decl in c.collect_direct(*member) {
                        if c.kind(decl) != NodeKind::VariablesDeclaration {
                            continue;
                        }
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
                            properties.push(name);
                        }
                    }
                }
                _ => {}
            }
        }
        if properties.is_empty() {
            return;
        }
        // The two debug methods remove their referenced properties
        // (:116-122).
        for target in ["debugFillProperties", "debugDescribeChildren"] {
            if let Some(method) = members
                .iter()
                .copied()
                .find(|&m| c.kind(m) == NodeKind::Method && member_named(c, m, target))
            {
                remove_referenced(c, method, &mut properties);
            }
        }
        for token in properties {
            c.report(utf16_token_span(c, token), &[]);
        }
    }

    /// The debug↔plain duality (`_IdentifierVisitor`): `x` ↔ `debugX`.
    fn dual_name(name: &str) -> Option<String> {
        if let Some(rest) = name.strip_prefix("debug") {
            if rest.is_empty() {
                return None;
            }
            let mut chars = rest.chars();
            let head = chars.next()?.to_lowercase().to_string();
            Some(format!("{head}{}", chars.as_str()))
        } else {
            let mut chars = name.chars();
            let head = chars.next()?.to_uppercase().to_string();
            Some(format!("debug{head}{}", chars.as_str()))
        }
    }

    /// Removes the properties the method body references: any identifier
    /// spelling in the body's extent matching the property or its dual.
    /// The production `ExpressionBody` row is a childless marker — its
    /// expression tiles beside it under the method — so a childless body
    /// reads the method's subtree; a `BlockBody` keeps the extent itself.
    fn remove_referenced(c: &Check<'_>, method: u32, properties: &mut Vec<u32>) {
        let Some(body) = c.collect_direct(method).into_iter().find(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::BlockBody | NodeKind::ExpressionBody | NodeKind::Block
            )
        }) else {
            return;
        };
        let window = if c.children(body).is_empty() {
            c.children(method)
        } else {
            c.children(body)
        };
        let referenced: Vec<&str> = (window.start..window.end)
            .filter(|&row| c.kind(row as u32) == NodeKind::Identifier)
            .map(|row| c.lexeme(c.head_token(row as u32)))
            .collect();
        properties.retain(|&property| {
            let name = c.lexeme(property);
            !(referenced.contains(&name)
                || dual_name(name).is_some_and(|dual| referenced.contains(&dual.as_str())))
        });
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

        /// `test/rules/diagnostic_describe_all_properties_test.dart`
        /// `test_un.described` — the property no debug method references
        /// reports at its name; the described ones stay silent.
        #[test]
        fn reports_the_undescribed_property() {
            let f = Fixture::parse(
                "class C implements Diagnosticable {\n  final int x;\n  int get y => 0;\n  int get z => 1;\n  void debugFillProperties(DiagnosticPropertiesBuilder p) {\n    p.add(IntProperty('x', x));\n    p.add(IntProperty('y', y));\n  }\n}\n",
            );
            let lints = f.run(&one_rule("diagnostic_describe_all_properties"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "The public property isn't described by either 'debugFillProperties' or 'debugDescribeChildren'."
            );
            assert_eq!(
                lints[0].correction.as_deref(),
                Some("Try describing the property.")
            );
            let z = tok(&f.source, &f.tokens, "z", 0);
            assert_eq!(lints[0].range.start, utf16_at(&f, z));
        }

        /// Private and static members are not properties; every public
        /// property referenced by a debug method passes (:110-115).
        #[test]
        fn described_private_and_static_members_pass() {
            let f = Fixture::parse(
                "class C implements Diagnosticable {\n  final int _p;\n  static int s = 0;\n  final int d;\n  void debugFillProperties(DiagnosticPropertiesBuilder p) {\n    p.add(IntProperty('d', d));\n  }\n}\n",
            );
            assert!(
                f.run(&one_rule("diagnostic_describe_all_properties"))
                    .is_empty()
            );
        }

        /// `debugDescribeChildren` removes too, and the dual spelling
        /// (`theta` ↔ `debugTheta`) counts as a reference (:20-36).
        #[test]
        fn debug_describe_children_and_dual_spellings_remove() {
            let f = Fixture::parse(
                "class C implements Diagnosticable {\n  int get theta => 0;\n  List<DiagnosticsNode> get debugDescribeChildren => [DiagnosticsNode('theta', theta)];\n}\n",
            );
            assert!(
                f.run(&one_rule("diagnostic_describe_all_properties"))
                    .is_empty()
            );

            let f = Fixture::parse(
                "class C implements Diagnosticable {\n  int get theta => 0;\n  void debugFillProperties(DiagnosticPropertiesBuilder p) {\n    p.add(DoubleProperty('debugTheta', debugTheta));\n  }\n}\n",
            );
            assert!(
                f.run(&one_rule("diagnostic_describe_all_properties"))
                    .is_empty()
            );
        }

        /// A class outside the Diagnosticable family never fires.
        #[test]
        fn classes_outside_the_family_pass() {
            let f = Fixture::parse("class C {\n  int get z => 1;\n}\n");
            assert!(
                f.run(&one_rule("diagnostic_describe_all_properties"))
                    .is_empty()
            );
        }

        /// The R4 shadow face: a same-file `class Diagnosticable` is user
        /// code, so the chain descends into it and stays silent.
        #[test]
        fn same_file_diagnosticable_shadow_passes() {
            let f = Fixture::parse(
                "class Diagnosticable {}\nclass C implements Diagnosticable {\n  int get z => 1;\n}\n",
            );
            assert!(
                f.run(&one_rule("diagnostic_describe_all_properties"))
                    .is_empty()
            );
        }
    }
}

/// The negative-identity pins: the resolution refutation under the
/// mounted semantic face, with the plain-run fallback beside it. One
/// module holds the family — the helpers are shared across the ten rules
/// and the faces differ only in the rule name.
#[cfg(test)]
mod semantic_identity {
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_resolution::{
        Combinators, ImportEdge, LibraryClassModifiers, LibraryGraph, LibraryGraphBuilder,
        Namespaces,
    };
    use dartq_semantic::{ElementIndex, MicroKind, StrictModes, analyze_unit, collect};
    use dartq_source::{FileTable, StringTable};

    use crate::engine::{Lint, Unit};
    use crate::registry::EnableSet;
    use crate::testsupport::{Fixture, one_rule};

    /// Runs `rule` over `source` with the semantic face mounted: the
    /// source folded as the analyzed file of `package:t`, with `dep`
    /// analyzed as a second file `kit.dart` and imported unprefixed when
    /// given — the cross-file container faces. The context tables leak
    /// (`'static`); the tests are one-shot processes.
    fn run_semantic(source: &str, rule: &str, dep: Option<&str>) -> Vec<Lint> {
        let (rules, unknown) = EnableSet::resolve([rule]);
        assert!(unknown.is_empty(), "unknown rule {rule}");
        run_semantic_set(source, &rules, dep)
    }

    /// [`run_semantic`] over an arbitrary enable set — the shared-refutation
    /// pins' hot face (several creation rules refute the identical rows in
    /// one run, the first verdict computed, the rest memo hits).
    fn run_semantic_set(source: &str, rules: &EnableSet, dep: Option<&str>) -> Vec<Lint> {
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
        if let Some(dep) = dep {
            let dep_file = files.intern(std::path::Path::new("kit.dart"));
            let scanned_dep = dartq_scanner::scan(dep, crate::testsupport::CONFIG, dep_file);
            assert!(
                scanned_dep.diagnostics.is_empty(),
                "scanner diagnostics: {dep:?}"
            );
            let dep_elements = collect(dep, &scanned_dep.tokens, features, strings);
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
        let unit = Unit {
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
        unit.run_with_semantics(rules, &rustc_hash::FxHashSet::default(), Some(&analysis))
    }

    /// The creation-head refutation: a cross-file user class carrying the
    /// `Container` spelling refutes under the semantic run (the official
    /// element posture — user code, not the framework widget); the plain
    /// run keeps the spelling verdict (the documented fallback).
    #[test]
    fn cross_file_container_creation_refutes() {
        let src = "void f() { var c = Container(child: Text('x')); }";
        let plain = Fixture::parse(src).run(&one_rule("avoid_unnecessary_containers"));
        assert_eq!(plain.len(), 1, "{plain:?}");
        let resolved = run_semantic(
            src,
            "avoid_unnecessary_containers",
            Some("class Container {}"),
        );
        assert!(resolved.is_empty(), "{resolved:?}");
    }

    /// An unindexed head keeps the spelling verdict even under the
    /// semantic run — the unindexed-dependency posture the refutation
    /// cannot reach.
    #[test]
    fn unindexed_container_head_keeps_the_spelling_verdict() {
        let src = "void f() { var c = Container(child: Text('x')); }";
        let resolved = run_semantic(src, "avoid_unnecessary_containers", None);
        assert_eq!(resolved.len(), 1, "{resolved:?}");
    }

    /// The color gate under the semantic run: a nullable `color` value's
    /// read row answers potentially null and never counts (the official
    /// `nullabilitySuffix` face); the plain run keeps the counting
    /// posture (no read face).
    #[test]
    fn nullable_color_argument_passes_resolved() {
        let src = "void f(Color? c, t) { var w = Container(color: c, child: Text('x')); }";
        let plain = Fixture::parse(src).run(&one_rule("use_colored_box"));
        assert_eq!(plain.len(), 1, "{plain:?}");
        let resolved = run_semantic(src, "use_colored_box", None);
        assert!(resolved.is_empty(), "{resolved:?}");
    }

    /// The chain-terminal refutation: a cross-file user class carrying
    /// the `StatefulWidget` root spelling refutes the extends gate under
    /// the semantic run; the plain run keeps the spelling verdict.
    #[test]
    fn cross_file_widget_root_chain_refutes() {
        let src = "class MyWidget extends StatefulWidget {}\n";
        let plain = Fixture::parse(src).run(&one_rule("use_key_in_widget_constructors"));
        assert_eq!(plain.len(), 1, "{plain:?}");
        let resolved = run_semantic(
            src,
            "use_key_in_widget_constructors",
            Some("class StatefulWidget {}"),
        );
        assert!(resolved.is_empty(), "{resolved:?}");
    }

    /// The any-clause chain face: a cross-file user class carrying the
    /// `Diagnosticable` spelling on an `implements` clause refutes the
    /// same way.
    #[test]
    fn cross_file_diagnosticable_chain_refutes() {
        let src = "class C implements Diagnosticable {\n  int get z => 1;\n}\n";
        let plain = Fixture::parse(src).run(&one_rule("diagnostic_describe_all_properties"));
        assert_eq!(plain.len(), 1, "{plain:?}");
        let resolved = run_semantic(
            src,
            "diagnostic_describe_all_properties",
            Some("class Diagnosticable {}"),
        );
        assert!(resolved.is_empty(), "{resolved:?}");
    }

    /// The seven creation rules share `spells_framework` verdicts through
    /// its refutation memo; the shared answers must equal each rule's lone
    /// run — reported and refuted rows alike (one `color:`+`child:`+`key:`
    /// Container is the colored-box and the sort-child verdict at once:
    /// one callee row, one memo entry, two hits).
    #[test]
    fn memo_hits_match_the_memo_cold_verdict() {
        const RULES: [&str; 7] = [
            "avoid_unnecessary_containers",
            "sized_box_for_whitespace",
            "sort_child_properties_last",
            "use_colored_box",
            "use_decorated_box",
            "sized_box_shrink_expand",
            "use_full_hex_values_for_flutter_colors",
        ];
        let (multi, unknown) = EnableSet::resolve(RULES);
        assert!(unknown.is_empty(), "{unknown:?}");
        let sorted = |mut lints: Vec<Lint>| {
            lints.sort_by(|a, b| (a.range.start, &a.message).cmp(&(b.range.start, &b.message)));
            lints
        };

        let f = Fixture::parse(
            "void f(c, k) { var w = Container(color: c, child: Text('x'), key: k); }",
        );
        let cold: Vec<Lint> = RULES
            .iter()
            .flat_map(|rule| f.run(&one_rule(rule)))
            .collect();
        assert_eq!(cold.len(), 2, "{cold:?}");
        assert_eq!(sorted(f.run(&multi)), sorted(cold));

        // The refuted row: a same-file `class Container` is user code —
        // the memo holds `false` and every rule stays silent, hot or cold.
        let f = Fixture::parse(
            "class Container {}\nvoid f(c) { var w = Container(color: c, child: Text('x')); }",
        );
        assert!(f.run(&multi).is_empty());
        assert!(RULES.iter().all(|rule| f.run(&one_rule(rule)).is_empty()));
    }

    /// The memo under the mounted semantic face: the cross-file refutation
    /// answers through resolution, and the memo-hot multi-rule run equals
    /// the memo-cold single-rule runs — the refuted `Container` row stays
    /// silent, the unrefuted one keeps its two verdicts.
    #[test]
    fn memo_holds_under_the_semantic_face() {
        const RULES: [&str; 7] = [
            "avoid_unnecessary_containers",
            "sized_box_for_whitespace",
            "sort_child_properties_last",
            "use_colored_box",
            "use_decorated_box",
            "sized_box_shrink_expand",
            "use_full_hex_values_for_flutter_colors",
        ];
        let (multi, unknown) = EnableSet::resolve(RULES);
        assert!(unknown.is_empty(), "{unknown:?}");
        let sorted = |mut lints: Vec<Lint>| {
            lints.sort_by(|a, b| (a.range.start, &a.message).cmp(&(b.range.start, &b.message)));
            lints
        };
        let src = "void f(c, k) { var w = Container(color: c, child: Text('x'), key: k); }";

        let refuted = run_semantic_set(src, &multi, Some("class Container {}"));
        assert!(refuted.is_empty(), "{refuted:?}");
        for rule in RULES {
            assert!(run_semantic(src, rule, Some("class Container {}")).is_empty());
        }

        let cold: Vec<Lint> = RULES
            .iter()
            .map(|rule| run_semantic(src, rule, None))
            .flatten()
            .collect();
        assert_eq!(cold.len(), 2, "{cold:?}");
        assert_eq!(sorted(run_semantic_set(src, &multi, None)), sorted(cold));
    }

    /// The one-run-per-memo contract: a run whose rows refute (the
    /// same-file `class Container`) must not lend its verdicts to the next
    /// run on the same thread. The two sources are byte-equal in length —
    /// the pin's source-len leg cannot tell them apart, and the unit/
    /// source addresses are allocator-dependent (same-size reuse), so only
    /// the clear-at-entry guarantee stands between the runs. The
    /// allocator-dependent pin may mask the leak without the fix; this pin
    /// holds the contract either way — the structural proof is the
    /// engine's `reset_refutation_memo` at both run entries.
    #[test]
    fn memo_does_not_leak_across_runs() {
        const RULES: [&str; 7] = [
            "avoid_unnecessary_containers",
            "sized_box_for_whitespace",
            "sort_child_properties_last",
            "use_colored_box",
            "use_decorated_box",
            "sized_box_shrink_expand",
            "use_full_hex_values_for_flutter_colors",
        ];
        let (multi, unknown) = EnableSet::resolve(RULES);
        assert!(unknown.is_empty(), "{unknown:?}");
        let sorted = |mut lints: Vec<Lint>| {
            lints.sort_by(|a, b| (a.range.start, &a.message).cmp(&(b.range.start, &b.message)));
            lints
        };
        // Equal-length prefixes (`class Container {}` vs the comment) put
        // the shared use line at identical offsets — same callee row ids.
        let refuted_src =
            "class Container {}\nvoid f(c) { var w = Container(color: c, child: Text('x')); }";
        let fresh_src =
            "// run pad filler.\nvoid f(c) { var w = Container(color: c, child: Text('x')); }";
        assert_eq!(refuted_src.len(), fresh_src.len());

        let cold = sorted(run_semantic_set(fresh_src, &multi, None));
        assert!(!cold.is_empty(), "{cold:?}");
        let refuted = run_semantic_set(refuted_src, &multi, None);
        assert!(refuted.is_empty(), "{refuted:?}");
        let after = sorted(run_semantic_set(fresh_src, &multi, None));
        assert_eq!(
            after, cold,
            "verdicts leaked across runs: {after:?} vs {cold:?}"
        );
    }
}
