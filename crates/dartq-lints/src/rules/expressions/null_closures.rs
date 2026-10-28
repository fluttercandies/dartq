use super::{Check, NodeKind, RuleSpec, query_site, same_file, slice};

/// Official: `pkg/linter/lib/src/rules/null_closures.dart` — a `null`
/// passed in a closure position of a known core API reports at the
/// argument. Three faces (:212-260): instance creations
/// (`Future(null)`, `Timer.periodic(d, null)`, ...), static calls
/// (`Future.wait(..., cleanUp: null)`, ...), and instance calls
/// (`list.map(null)`, ...). Positions come from the fixed tables
/// (:26-180); the report lands on the `null` literal, or on the whole
/// named argument for the named face (:268-291).
///
/// Type pinning (the :212-250 element faces over same-file facts):
/// - creations (`extendsClass` walk, extensions.dart:305-353) resolve
///   the written type through the same-file superclass chain, so a user
///   subclass (`class MyFuture extends Future`, `MyFuture(null)`)
///   reports like the official and a same-file shadow (`class Future`,
///   then `Future(null)`) passes;
/// - static calls match `element.name == function.type` exactly
///   (:224-233) — no supertype walk, the same-file class shadows;
/// - instance calls resolve the receiver's written type (local,
///   parameter, field, top-level variable) through the same-file
///   all-supertypes chain (:245-250), and the method table filters on
///   the owning core class (`String.map` stays silent like the official
///   `_getInstanceMethod` miss).
///
/// Fallback (documented over-report residue, the `prefer_contains`
/// precedent): a receiver or type that resolves outside the file
/// (imported class, `dynamic`, an unknown supertype) matches by written
/// name — a user class reusing a core method name over-reports.
/// `scheduleMicrotask` stays out: its table entry can never fire in the
/// official either (`element.name == null`, :229-231). Message:
/// `diagnostic.g.dart` `nullClosures`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "null_closures",
    &[
        NodeKind::Send,
        NodeKind::ImplicitCreation,
        NodeKind::NewExpression,
    ],
    check,
);

/// The closure slots of one table entry: raw argument indexes (named
/// arguments come last in legal Dart, so the raw index of a non-named
/// argument is its positional index) and argument labels.
type Slots = (&'static [usize], &'static [&'static str]);

/// Creation face (`_constructorsWithNonNullableArguments`, :26-33);
/// the unnamed-constructor entries key on the type name alone.
const CONSTRUCTORS: &[((&str, Option<&str>), Slots)] = &[
    (("Future", None), (&[0], &[])),
    (("Future", Some("microtask")), (&[0], &[])),
    (("Future", Some("sync")), (&[0], &[])),
    (("Timer", None), (&[1], &[])),
    (("Timer", Some("periodic")), (&[1], &[])),
    (("List", Some("generate")), (&[1], &[])),
];

/// Static face (`_staticFunctionsWithNonNullableArguments`, :172-178).
const STATICS: &[(&str, &str, Slots)] = &[
    ("Future", "doWhile", (&[0], &[])),
    ("Future", "forEach", (&[1], &[])),
    ("Future", "wait", (&[], &["cleanUp"])),
    ("Timer", "run", (&[0], &[])),
];

/// Instance face (`_instanceMethodsWithNonNullableArguments`, :35-170),
/// keyed by method name with the core classes whose element carries it
/// (the entry `type` values) plus the core subtypes that inherit it —
/// the official `_getInstanceMethod` walks `allSupertypes` of the
/// receiver's element (:245-250), so a `List` receiver reaches the
/// `Iterable.map` entry.
const INSTANCE_METHODS: &[(&str, &[&str], Slots)] = &[
    ("any", &["Iterable", "List", "Set", "Queue"], (&[0], &[])),
    ("complete", &["Future"], (&[0], &[])),
    ("every", &["Iterable", "List", "Set", "Queue"], (&[0], &[])),
    ("expand", &["Iterable", "List", "Set", "Queue"], (&[0], &[])),
    (
        "firstWhere",
        &["Iterable", "List", "Set", "Queue"],
        (&[0], &["orElse"]),
    ),
    (
        "forEach",
        &["Iterable", "List", "Set", "Queue", "Map"],
        (&[0], &[]),
    ),
    ("fold", &["Iterable", "List", "Set", "Queue"], (&[1], &[])),
    (
        "lastWhere",
        &["Iterable", "List", "Set", "Queue"],
        (&[0], &["orElse"]),
    ),
    ("map", &["Iterable", "List", "Set", "Queue"], (&[0], &[])),
    ("putIfAbsent", &["Map"], (&[1], &[])),
    ("reduce", &["Iterable", "List", "Set", "Queue"], (&[0], &[])),
    ("removeWhere", &["Queue", "List", "Set"], (&[0], &[])),
    ("replaceAllMapped", &["String"], (&[1], &[])),
    ("replaceFirstMapped", &["String"], (&[1], &[])),
    ("retainWhere", &["Queue", "List", "Set"], (&[0], &[])),
    (
        "singleWhere",
        &["Iterable", "List", "Set", "Queue"],
        (&[0], &["orElse"]),
    ),
    (
        "skipWhile",
        &["Iterable", "List", "Set", "Queue"],
        (&[0], &[]),
    ),
    (
        "splitMapJoin",
        &["String"],
        (&[], &["onMatch", "onNonMatch"]),
    ),
    (
        "takeWhile",
        &["Iterable", "List", "Set", "Queue"],
        (&[0], &[]),
    ),
    ("then", &["Future"], (&[0], &["onError"])),
    ("where", &["Iterable", "List", "Set", "Queue"], (&[0], &[])),
];

/// The written names the tables key on (dart:async / dart:core /
/// dart:collection members; unprefixed and same-file-unshadowed names
/// are the library types).
const CORE_TYPES: &[&str] = &[
    "Future", "Timer", "List", "Iterable", "Map", "Set", "Queue", "String",
];

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // Which table matches, gated on the same-file type verdict:
    // `NotCore` bails (provably not the core API), `Unknown` falls
    // back to the written-name match (instance faces: silent).
    let face = match c.kind(node) {
        // The explicit-new shape owns its callee and argument list.
        NodeKind::NewExpression | NodeKind::ImplicitCreation => {
            let kids = c.collect_direct(node);
            let args = kids
                .iter()
                .copied()
                .find(|&kid| c.kind(kid) == NodeKind::Arguments);
            args.zip(creation_slots(c, &kids))
        }
        // A plain send owns nothing — rebuild from the owner slice.
        NodeKind::Send => slice::owner_slice(c).and_then(|(kids, pos)| {
            let (head, args) = slice::send_parts(c, &kids, pos)?;
            // A `None` argument list is a function tear-off.
            send_slots(c, &kids, head, pos).and_then(|s| Some((args?, s)))
        }),
        _ => return,
    };
    let Some((args, (positions, named))) = face else {
        return;
    };
    report_nulls(c, args, positions, named);
}

/// The unnamed-creation face over an owned child list (`Future(null)`,
/// `new Future(null)`): the callee is the identifier before the
/// argument list — the explicit-new shape carries a
/// `ConstructorReference` marker between them.
fn creation_slots(c: &Check<'_>, kids: &[u32]) -> Option<Slots> {
    let args_pos = kids
        .iter()
        .position(|&kid| c.kind(kid) == NodeKind::Arguments)?;
    let callee = kids[..args_pos]
        .iter()
        .rev()
        .find_map(|&kid| match c.kind(kid) {
            NodeKind::Identifier => Some(kid),
            // The explicit-new shape: the marker owns the callee name.
            NodeKind::ConstructorReference => c
                .collect_direct(kid)
                .into_iter()
                .find(|&k| c.kind(k) == NodeKind::Identifier),
            _ => None,
        })?;
    unnamed_slots(c, query_site(c, callee), c.lexeme(c.head_token(callee)))
}

/// The unnamed-creation table hit for one written type name, gated on
/// the same-file superclass chain (`extendsClass`, extensions.dart
/// :305-353): a same-file class shadows its core namesake, a same-file
/// subclass reports like the official, and a cross-file resolution is
/// a user class — the official supertype walk cannot reach
/// dart:async through it (the supertypes live outside this file), so
/// the written-name fallback stays silent for it.
fn unnamed_slots(c: &Check<'_>, site: dartq_source::SourceRange, name: &str) -> Option<Slots> {
    match same_file::resolve_super(c, name, CORE_TYPES) {
        same_file::Verdict::NotCore => None,
        same_file::Verdict::Core(core) => CONSTRUCTORS
            .iter()
            .find(|entry| entry.0.0 == core && entry.0.1.is_none())
            .map(|entry| entry.1),
        same_file::Verdict::Unknown => {
            if resolves_declared_container(c, site) {
                return None;
            }
            CONSTRUCTORS
                .iter()
                .find(|entry| entry.0.0 == name && entry.0.1.is_none())
                .map(|entry| entry.1)
        }
    }
}

/// Whether the written type name at `site` resolves as a declared
/// container through the bridge (a user class, same-file resolution
/// aside — the core library types are not in the graph and stay
/// unresolved). No mounted face answers `false` (the fallback keeps
/// the plain-run posture).
fn resolves_declared_container(c: &Check<'_>, site: dartq_source::SourceRange) -> bool {
    c.semantics()
        .is_some_and(|analysis| analysis.resolve_constructor(site, None).is_some())
}

/// The send face over the owner slice with the invocation send at
/// `pos` and its callee head at `head`: a trailing same-span
/// `DotAccess` marker switches to the member faces.
fn send_slots(c: &Check<'_>, kids: &[u32], head: usize, pos: usize) -> Option<Slots> {
    if c.kind(kids[head]) != NodeKind::Identifier {
        return None;
    }
    let member_face = slice::access_marker(c, kids, pos, NodeKind::DotAccess);
    match member_face {
        None => unnamed_slots(
            c,
            query_site(c, kids[head]),
            c.lexeme(c.head_token(kids[head])),
        ),
        Some(access) => member_slots(c, kids, head, access),
    }
}

/// The member-access faces over the owner slice with the selector at
/// `sel` and the `DotAccess` marker after the send. The bare-receiver
/// faces stand (the static and named-creation tables key on the
/// written receiver name — `Timer.periodic`; the instance face
/// resolves the receiver's declared type through the same-file
/// all-supertypes chain). A chained or `this.` receiver reads its
/// bridge static type when mounted (`a.b.map(null)`,
/// `this.list.map(null)`) — the method table filters on the named
/// core class, and an unresolvable receiver stays silent like the
/// official `_getInstanceMethod` miss.
fn member_slots(c: &Check<'_>, kids: &[u32], sel: usize, _access: u32) -> Option<Slots> {
    let member = c.lexeme(c.head_token(kids[sel]));
    let group_start = sel.checked_sub(4)?;
    if slice::is_tear_off_group(c, kids, group_start) {
        let written = c.lexeme(c.head_token(kids[group_start]));
        if let Some(slots) = named_slots(c, query_site(c, kids[group_start]), written, member) {
            return Some(slots);
        }
        let verdict = same_file::written_type(c, kids[group_start], written)
            .map_or(same_file::Verdict::NotCore, |ty| {
                same_file::resolve(c, ty, CORE_TYPES)
            });
        return match verdict {
            same_file::Verdict::Core(core) => INSTANCE_METHODS
                .iter()
                .find(|entry| entry.0 == member && entry.1.contains(&core))
                .map(|entry| entry.2),
            same_file::Verdict::NotCore | same_file::Verdict::Unknown => None,
        };
    }
    let analysis = c.semantics()?;
    let core = super::static_type_faces::receiver_type_name(c, analysis, &kids[..sel])?;
    INSTANCE_METHODS
        .iter()
        .find(|entry| entry.0 == member && entry.1.contains(&core.as_str()))
        .map(|entry| entry.2)
}

/// `_checkNullArgForClosure` (:268-293): a `null` in a closure slot
/// reports at the null literal, or over the whole named argument.
/// The argument list owns its pieces: leading callee type-argument
/// wrappers, then the arguments in order — a bare-identifier argument
/// wraps in the four-piece tear-off group, and a named argument
/// flattens as `name value… NamedArgument` (the marker anchors at the
/// value head, so the name is the identifier two tokens back).
fn report_nulls(c: &mut Check<'_>, arguments: u32, positions: &[usize], named: &[&str]) {
    let pieces = c.collect_direct(arguments);
    let mut consumed = vec![false; pieces.len()];
    for (j, &marker) in pieces.iter().enumerate() {
        if c.kind(marker) != NodeKind::NamedArgument {
            continue;
        }
        // The marker anchors at the value head: `name : value` are
        // three consecutive tokens, so the name is the identifier two
        // tokens back (`slice::named_arg_name`).
        let Some(name_idx) = slice::named_arg_name(c, &pieces, j) else {
            continue;
        };
        consumed[name_idx] = true;
        consumed[name_idx + 1..=j].fill(true);
        let value = &pieces[name_idx + 1..j];
        if value.len() == 1
            && c.kind(value[0]) == NodeKind::LiteralNull
            && named.contains(&c.lexeme(c.head_token(pieces[name_idx])))
        {
            // The marker's own span is empty (it anchors at the value
            // head) — the whole-argument range ends at the value.
            let start = c.utf16_span(pieces[name_idx]).start;
            let end = c.utf16_span(*value.last().unwrap_or(&marker)).end;
            c.report(start..end, &[]);
        }
    }
    let mut index = 0usize;
    let mut i = 0usize;
    while i < pieces.len() {
        if consumed[i] {
            i += 1;
            continue;
        }
        let tear_off = matches!(
            pieces.get(i..i + 4),
            Some([name, tyargs, no_args, send])
                if c.kind(*name) == NodeKind::Identifier
                    && matches!(
                        c.kind(*tyargs),
                        NodeKind::NoTypeArguments | NodeKind::TypeArguments
                    )
                    && c.kind(*no_args) == NodeKind::NoArguments
                    && c.kind(*send) == NodeKind::Send
        );
        if c.kind(pieces[i]) == NodeKind::LiteralNull && positions.contains(&index) {
            let range = c.utf16_span(pieces[i]);
            c.report(range, &[]);
        }
        index += 1;
        i += if tear_off { 4 } else { 1 };
    }
}

/// The named-creation / static-call face keyed on the written name
/// before the dot: the named-creation table first, then the static
/// table (the official splits them on the target's element — a class
/// element vs a top-level function; one written shape serves both). A
/// cross-file container resolution is a user class — the official
/// element identity (`element.name == function.type`) cannot hold for
/// it, so the written-name fallback stays silent.
fn named_slots(
    c: &Check<'_>,
    site: dartq_source::SourceRange,
    class_name: &str,
    member_name: &str,
) -> Option<Slots> {
    let constructor = match same_file::resolve_super(c, class_name, CORE_TYPES) {
        same_file::Verdict::NotCore => None,
        same_file::Verdict::Core(core) => CONSTRUCTORS
            .iter()
            .find(|entry| entry.0.0 == core && entry.0.1 == Some(member_name))
            .map(|entry| entry.1),
        same_file::Verdict::Unknown => {
            if resolves_declared_container(c, site) {
                None
            } else {
                CONSTRUCTORS
                    .iter()
                    .find(|entry| entry.0.0 == class_name && entry.0.1 == Some(member_name))
                    .map(|entry| entry.1)
            }
        }
    };
    constructor.or_else(|| match same_file::direct(c, class_name, CORE_TYPES) {
        same_file::Verdict::NotCore => None,
        same_file::Verdict::Core(core) => STATICS
            .iter()
            .find(|entry| entry.0 == core && entry.1 == member_name)
            .map(|entry| entry.2),
        same_file::Verdict::Unknown => {
            if resolves_declared_container(c, site) {
                None
            } else {
                STATICS
                    .iter()
                    .find(|entry| entry.0 == class_name && entry.1 == member_name)
                    .map(|entry| entry.2)
            }
        }
    })
}
