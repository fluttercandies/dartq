use std::path::Component;

use super::{PayloadField, Check, NodeKind, RuleSpec, get_set_token, narrow_u32, type_before};

/// Official: `pkg/linter/lib/src/rules/always_declare_return_types.dart:38-85`
/// — a function, method, getter, or old-form `typedef` written without a
/// return type reports at its name. Setters never report (:47/:69-70),
/// `operator []=` never reports (:70) while every other operator
/// reports like a method (the operator's lexeme is the name),
/// augmentations never report (:71), and inside a test directory
/// methods named `test_*` / `solo_test_*` are exempt (:76-81).
/// Messages: `diagnostic.g.dart`
/// `alwaysDeclareReturnTypesOfFunctions` /
/// `alwaysDeclareReturnTypesOfMethods` (the two-variant report face).
///
/// Faces: [`NodeKind::TopLevelMethod`] (top-level functions and
/// accessors), [`NodeKind::Method`] (class members),
/// [`NodeKind::LocalFunctionDeclaration`] (local functions — the
/// official `visitFunctionDeclaration` arm), and old-form
/// [`NodeKind::Typedef`] (the official `visitFunctionTypeAlias` arm —
/// the `=` alias form carries a function type, not an omitted return
/// type). The written return type is the row before the name
/// ([`type_before`]; `NoType` = absent).
///
/// The test-directory exemption reads [`crate::WorkspaceFacts::root`]
/// (the official `context.isInTestDirectory`,
/// `workspace.dart:127-131` — a `test` / `integration_test` /
/// `test_driver` prefix of the package-relative path); without package
/// facts mounted the face stays out, the official no-package posture.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "always_declare_return_types",
    &[
        NodeKind::TopLevelMethod,
        NodeKind::Method,
        NodeKind::LocalFunctionDeclaration,
        NodeKind::Typedef,
    ],
    check,
);

const METHODS_PROBLEM: &str = "The method '{0}' should have a return type but doesn't.";
const METHODS_CORRECTION: &str = "Try adding a return type to the method.";

/// The `augment` `ModSet` bit (bridge `mod_bits`; bit 1 of the class and
/// member families).
const AUGMENT_BIT: u16 = 1 << 1;

fn check(c: &mut Check<'_>) {
    let node = c.node();
    match c.kind(node) {
        NodeKind::TopLevelMethod => {
            // Setters skip (:69-70).
            if get_set_token(c, node) == Some("set") {
                return;
            }
            if is_augmented(c, node) {
                return;
            }
            report_untyped(c, node, false);
        }
        NodeKind::Method => {
            if get_set_token(c, node) == Some("set") {
                return;
            }
            if is_augmented(c, node) {
                return;
            }
            // The operator name rides the row's left sibling (probe).
            // Upstream skips only `operator []=` (`TokenType.INDEX_EQ`,
            // :70) — every other untyped operator reports with the
            // method face (`reportAtToken(node.name)`, the operator's
            // own lexeme, :79-83).
            if let Some(op_row) = super::adjacent_operator_name(c, node) {
                report_untyped_operator(c, node, op_row);
                return;
            }
            let name_lexeme = report_name(c, node).map(|row| c.lexeme(c.head_token(row)));
            // `test_*` / `solo_test_*` methods inside a test directory
            // skip (:76-81).
            if in_test_directory(c)
                && name_lexeme.is_some_and(|name| {
                    name.starts_with("test_") || name.starts_with("solo_test_")
                })
            {
                return;
            }
            report_untyped(c, node, true);
        }
        NodeKind::LocalFunctionDeclaration => {
            let Some(name) = c
                .collect_direct(node)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::FunctionName)
            else {
                return;
            };
            report_untyped_named(c, node, name, false);
        }
        NodeKind::Typedef => {
            // The generic alias form (`typedef F = ...`) is a different
            // official node (:57-63 fires only on `FunctionTypeAlias`).
            if c.field(node, PayloadField::Equals) != Some(crate::rules::NONE_TOKEN) {
                return;
            }
            // The name carries the `TypedefDeclaration` context; the
            // old form's written return type rides as an earlier
            // `Identifier` row (`typedef int _Old();`), so the
            // first-identifier read alone answers the type.
            let kids = c.collect_direct(node);
            let name = kids
                .iter()
                .copied()
                .find(|&kid| {
                    c.kind(kid) == NodeKind::Identifier
                        && c.field(kid, PayloadField::Context)
                            == Some(dartq_parser::IdentifierContext::TypedefDeclaration as u32)
                })
                .or_else(|| {
                    kids.iter()
                        .copied()
                        .find(|&kid| c.kind(kid) == NodeKind::Identifier)
                });
            let Some(name) = name else {
                return;
            };
            report_untyped_named(c, node, name, false);
        }
        _ => {}
    }
}

/// The untyped-operator verdict: upstream skips only `operator []=`
/// (the official `TokenType.INDEX_EQ` guard, :70); every other
/// operator reports like a method — at the operator token, with the
/// operator's lexeme as the name (`reportAtToken(node.name)`,
/// `node.name.lexeme`, :79-83 — e.g. `The method '+' should have a
/// return type but doesn't.`). The written return type is the row
/// before the `OperatorName` row among the class-body siblings
/// ([`type_before`] on the enclosing window — the operator name is a
/// sibling of the `Method` row, not a child).
fn report_untyped_operator(c: &mut Check<'_>, node: u32, op_row: u32) {
    let Some(op_token) = c.field_token(op_row, PayloadField::Name) else {
        return;
    };
    // `operator []=` skips (:70; `TokenType.INDEX_EQ`).
    if c.lexeme(op_token) == "[]=" {
        return;
    }
    // The written return type tiles inside the `Method` row (the
    // `OperatorName` sibling precedes it in the parent's window), so
    // the read is the method's own leading type row — `NoType`/absent
    // is the untyped face.
    let typed = c
        .collect_direct(node)
        .into_iter()
        .take_while(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::Identifier | NodeKind::Type | NodeKind::NoType
            )
        })
        .any(|kid| {
            matches!(
                c.kind(kid),
                NodeKind::Type
                    | NodeKind::VoidType
                    | NodeKind::FunctionType
                    | NodeKind::RecordType
            )
        });
    if typed {
        return;
    }
    let name = c.lexeme(op_token);
    let Some(start) = c.utf16_of_token(op_token) else {
        return;
    };
    let len = narrow_u32(name.encode_utf16().count());
    c.report_texts(
        start..start + len,
        METHODS_PROBLEM,
        METHODS_CORRECTION,
        &[name],
    );
}

/// The declaration's name row: the direct `Identifier` child carrying an
/// official name context (`MethodDeclaration` /
/// `TopLevelFunctionDeclaration` — bridge `handle_identifier` stamps the
/// context). An arrow body's expression identifiers leak into the member
/// window as later `Expression`-context rows and a written return type
/// rides as an earlier `TypeReference` row, so the context is the only
/// exact disambiguator (`int get count => _count;` names `count`, never
/// `_count`). Falls back to the LAST `Identifier` child (the pre-context
/// read) when no context-stamped name row exists.
fn report_name(c: &Check<'_>, node: u32) -> Option<u32> {
    let kids = c.collect_direct(node);
    if let Some(name) = kids.iter().copied().find(|&kid| {
        c.kind(kid) == NodeKind::Identifier
            && matches!(
                c.field(kid, PayloadField::Context),
                Some(ctx)
                    if ctx == dartq_parser::IdentifierContext::MethodDeclaration as u32
                        || ctx
                            == dartq_parser::IdentifierContext::TopLevelFunctionDeclaration
                                as u32
            )
    }) {
        return Some(name);
    }
    kids.into_iter()
        .rfind(|&kid| c.kind(kid) == NodeKind::Identifier)
}

/// The shared verdict: the row before the name row decides (`NoType` =
/// the official `returnType == null`), then the two-variant report at
/// the name token.
fn report_untyped(c: &mut Check<'_>, node: u32, is_method: bool) {
    let Some(name) = report_name(c, node) else {
        return;
    };
    report_untyped_named(c, node, name, is_method);
}

fn report_untyped_named(c: &mut Check<'_>, node: u32, name_row: u32, is_method: bool) {
    let typed = match type_before(c, node, name_row) {
        None => false,
        Some((_, kind)) => matches!(
            kind,
            NodeKind::Type | NodeKind::VoidType | NodeKind::FunctionType | NodeKind::RecordType
        ),
    };
    if typed {
        return;
    }
    let name_token = c.head_token(name_row);
    let name = c.lexeme(name_token);
    let Some(start) = c.utf16_of_token(name_token) else {
        return;
    };
    let len = narrow_u32(name.encode_utf16().count());
    if is_method {
        c.report_texts(
            start..start + len,
            super::always_declare_return_types::METHODS_PROBLEM,
            super::always_declare_return_types::METHODS_CORRECTION,
            &[name],
        );
    } else {
        c.report(start..start + len, &[name]);
    }
}

/// Whether `node` or a declaration-shaped ancestor carries the
/// `augment` modifier (the official `isAugmentation` guard).
fn is_augmented(c: &Check<'_>, node: u32) -> bool {
    if dartq_ast::mod_flags(c.unit.ast, node) & AUGMENT_BIT != 0 {
        return true;
    }
    c.path.iter().rev().skip(1).any(|&ancestor| {
        matches!(
            c.kind(ancestor),
            NodeKind::Method
                | NodeKind::TopLevelMethod
                | NodeKind::Fields
                | NodeKind::TopLevelFields
                | NodeKind::Constructor
                | NodeKind::Factory
                | NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::ExtensionDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::Typedef
        ) && dartq_ast::mod_flags(c.unit.ast, ancestor) & AUGMENT_BIT != 0
    })
}

/// The official `context.isInTestDirectory`
/// (`workspace.dart:72-77,122-131`): the file sits under the package
/// root's `test` / `integration_test` / `test_driver` directory.
fn in_test_directory(c: &Check<'_>) -> bool {
    let (Some(path), Some(workspace)) = (c.unit.path, c.unit.workspace) else {
        return false;
    };
    path.strip_prefix(&workspace.root)
        .ok()
        .and_then(|rel| rel.components().next())
        .is_some_and(|comp| {
            matches!(
                comp,
                Component::Normal(name)
                    if name == "test" || name == "integration_test" || name == "test_driver"
            )
        })
}
