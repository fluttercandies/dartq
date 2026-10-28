use super::{Check, NodeKind, RuleSpec, query_site, slice, static_type_faces};
use crate::rules::{same_file_declaration, utf16_token_span};

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/avoid_slow_async_io.dart` — an invocation
/// resolving to a `dart:io` method whose synchronous twin exists
/// reports at the invocation (`reportAtNode(node)`): the
/// `FileSystemEntity` set `{exists, isDirectory, isFile, isLink, stat,
/// type}` and `File.lastModified` (the rule source :25-27/:41-58).
/// The upstream verdict is the resolved `MethodElement`'s enclosing
/// class + library; the tests cover instance receivers
/// (`dir.exists()`), the `FileSystemEntity` statics
/// (`FileSystemEntity.type(path)`), extension types implementing the
/// interface, and the sync twins' silence.
///
/// dartq faces over the production tiling, the dart:io member
/// identity face without SDK elements: the dispatch row is the
/// invocation send with a single bare identifier receiver. The static
/// face matches the receiver spelling `FileSystemEntity` unbound
/// same-file (a shadowing declaration keeps it silent — the element
/// library gate) with the slow static names `{stat, type,
/// isDirectory, isFile, isLink}` — `exists` is instance-only, so a
/// static spelling never resolves upstream. The instance face reads
/// the receiver's type — `File` carries `{lastModified, exists,
/// stat}`, `Directory`/`Link`/`FileSystemEntity` carry `{exists,
/// stat}` (the type-checking set is static-only upstream) — and stays
/// silent when the type names a same-file declaration (the user's own
/// `File` is not `dart:io`'s). Approximations (documented,
/// under-report): prefixed receivers (`io.FileSystemEntity.type`),
/// `this`/`super` receivers, extension-type receivers implementing the
/// interface, and cross-file shadowing classes resolve to nothing
/// here. Message: `diagnostic.g.dart` `avoidSlowAsyncIo`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_slow_async_io",
    &[NodeKind::Send],
    check,
);

/// The slow `FileSystemEntity` statics: `exists` is instance-only.
const STATIC_SLOW: [&str; 5] = ["stat", "type", "isDirectory", "isFile", "isLink"];

/// The instance-slow names per receiver class: `File.lastModified`
/// plus the `FileSystemEntity` instance members its subclasses carry.
fn instance_slow(class: &str) -> Option<&'static [&'static str]> {
    match class {
        "File" => Some(&["lastModified", "exists", "stat"]),
        "Directory" | "Link" | "FileSystemEntity" => Some(&["exists", "stat"]),
        _ => None,
    }
}

fn check(c: &mut Check<'_>) {
    let Some(analysis) = c.semantics() else {
        // The wave posture: no semantic face, no member identity.
        return;
    };
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
    let Some((head, arguments)) = slice::send_parts(c, &kids, pos) else {
        return;
    };
    // A tear-off is not a MethodInvocation upstream.
    let Some(arguments) = arguments else {
        return;
    };
    if slice::access_marker(c, &kids, pos, NodeKind::DotAccess).is_none() {
        return;
    }
    let selector_row = kids[head];
    let selector = c.head_token(selector_row);
    // The identity face reads single bare identifier receivers.
    let Some(receiver) = static_type_faces::single_receiver(c, &kids[..head], selector) else {
        return;
    };
    let name = c.lexeme(selector);
    // The static face: the receiver spells `FileSystemEntity` and
    // binds to nothing same-file (the element library gate).
    if c.lexeme(c.head_token(receiver)) == "FileSystemEntity"
        && crate::rules::expressions::same_file::binding(c, receiver, "FileSystemEntity")
            .is_none()
        && STATIC_SLOW.contains(&name)
    {
        report_invocation(c, receiver, arguments);
        return;
    }
    // The instance face: the receiver's declared class decides the
    // slow set; a same-file class of the same name is not `dart:io`'s.
    let Some(ty) = analysis.type_of(query_site(c, receiver)) else {
        return;
    };
    let dartq_semantic::model::Ty::Named { name: class, .. } = ty else {
        return;
    };
    let class = analysis.strings().resolve(class);
    if same_file_declaration(c, class).is_some() {
        return;
    }
    // The extension-type receiver face (`test_*_extensionType_implements`):
    // a same-file extension type is the user's own declaration — its
    // members answer first (a redeclared `exists` shadows the slow
    // interface member — `test_*_redeclares`), else an `implements`
    // interface's slow set applies.
    if let Some(ext_row) = extension_type_row(c, class) {
        if extension_type_redeclares(c, ext_row, name) {
            return;
        }
        let slow = crate::rules::supertypes(c, ext_row)
            .iter()
            .find_map(|&(sup, clause)| {
                (clause == crate::rules::SupertypeClause::Implements
                    && same_file_declaration(c, sup).is_none()
                    && extension_type_row(c, sup).is_none())
                .then_some(sup)
                .and_then(instance_slow)
            });
        if slow.is_some_and(|slow| slow.contains(&name)) {
            report_invocation(c, receiver, arguments);
        }
        return;
    }
    if instance_slow(class).is_some_and(|slow| slow.contains(&name)) {
        report_invocation(c, receiver, arguments);
    }
}

/// The same-file extension-type row named `name`: the header's payload
/// word 2 is the name token (`begin_extension_type_declaration`), so
/// the row never surfaces in `same_file_declaration`'s identifier
/// index.
fn extension_type_row(c: &Check<'_>, name: &str) -> Option<u32> {
    let root = c.unit.ast.root()?;
    c.collect_direct(root).into_iter().find_map(|top| {
        c.collect_direct(top).into_iter().find(|&kid| {
            c.kind(kid) == NodeKind::ExtensionTypeDeclaration
                && c.payload(kid).get(2).copied().is_some_and(|token| {
                    token != crate::rules::NONE_TOKEN && c.lexeme(token) == name
                })
        })
    })
}

/// Whether the extension type declares its own `name` member (a method
/// or the representation getter would shadow the interface's slow
/// member — the upstream member lookup answers the extension type
/// first).
fn extension_type_redeclares(c: &Check<'_>, ext_row: u32, name: &str) -> bool {
    crate::rules::body_members(c, ext_row)
        .iter()
        .any(|&member| {
            crate::rules::declarations::member_declaration_name(c, member)
                .is_some_and(|token| c.lexeme(token) == name)
        })
}

/// The upstream anchor is the whole invocation (`reportAtNode(node)`).
fn report_invocation(c: &mut Check<'_>, receiver: u32, arguments: u32) {
    let start = utf16_token_span(c, c.head_token(receiver)).start;
    let end = c.utf16_span(arguments).end;
    c.report(start..end, &[]);
}
