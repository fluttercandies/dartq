use dartq_semantic::model::{ArgListId, Ty};

use super::{Check, NodeKind, PayloadField, RuleSpec, query_site, slice};

const DART_IS_JS_PROBLEM: &str = "Runtime check between '{0}' and '{1}' checks whether a Dart value is a JS interop type, which might not be platform-consistent.";
const DART_IS_JS_CORRECTION: &str =
    "Try using 'isA' from 'dart:js_interop' to check if the Dart value is a JS interop type.";
const DART_AS_JS_PROBLEM: &str = "Cast from '{0}' to '{1}' casts a Dart value to a JS interop type, which might not be platform-consistent.";
const DART_AS_JS_CORRECTION: &str = "Try using conversion methods from 'dart:js_interop' to convert between Dart types and JS interop types.";

/// Official:
/// `pkg/linter/lib/src/rules/invalid_runtime_check_with_js_interop_types.dart`
/// — a runtime type test involving JS interop types that is not
/// statically guaranteed reports (:360-398, eight diagnostic
/// variants; the visit faces :400-437).
///
/// The interop classification (`_isJsInteropType`, :43-72 — extension
/// types, `@staticInterop`, SDK web libraries) reduces on this
/// substrate to classes annotated `@JS` in a file importing
/// `dart:js_interop`: same-file through the class body, cross-file
/// through [`Analysis::container_has_annotation`] — the resolved
/// container's own annotation rows. An unresolvable container (`None`)
/// and a resolvable Dart type (`Some(false)`) stay silent. The ported
/// face is the dart-is/as-js slice with the
/// two left-expression faces: `this is JSClass` / `this as JSClass`
/// (the left type is the enclosing Dart class) and `x is JSClass` /
/// `x as JSClass` over a bare operand whose bridge read row names a
/// type (the display argument is exact — an argument-carrying type
/// stays silent), reported at the expression with `(dart type, js
/// type)` (:400-437). The catch-clause face and the js-side variants
/// stay silent. Message: `diagnostic.g.dart`
/// `invalidRuntimeCheckWithJsInteropTypesDart{Is,As}Js`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "invalid_runtime_check_with_js_interop_types",
    &[NodeKind::IsExpression, NodeKind::AsExpression],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let is_check = c.kind(node) == NodeKind::IsExpression;
    // The left operand rows precede the operator container; the tested
    // type tiles inside it.
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    // Left faces: a lone `this` (the enclosing Dart class) immediately
    // before the operator container — an arrow body's signature rows
    // tile before it in the member window, so only the trailing `this`
    // is the tested expression and everything earlier must be
    // signature — or a bare operand whose bridge read row names a
    // type.
    // A condition container's own window holds only the operator
    // container — the condition's operand rows tile flat in the
    // statement owner's window (the same probe posture
    // `avoid_double_and_int_checks` reads) — so a condition parent
    // takes the left run from the grandparent statement: the rows
    // inside the condition's token range that close before the
    // operator. The arrow-body return-type window (`preceding`)
    // exists only in the member-window shape.
    let (left, preceding): (Vec<u32>, Vec<u32>) = if c.kind(parent)
        == NodeKind::ParenthesizedCondition
    {
        let Some(stmt) = c.ancestor(2) else {
            return;
        };
        let cond = c.unit.ast.token_range(parent);
        let node_start = c.unit.ast.token_range(node).start;
        let run: Vec<u32> = c
            .collect_direct(stmt)
            .into_iter()
            .filter(|&row| {
                let span = c.unit.ast.token_range(row);
                span.start >= cond.start
                    && span.end <= node_start
                    && !matches!(c.kind(row), NodeKind::DotAccess | NodeKind::Qualified)
            })
            .collect();
        (run, Vec::new())
    } else {
        let kids = c.collect_direct(parent);
        let Some(pos) = kids.iter().position(|&kid| kid == node) else {
            return;
        };
        (
            kids[..pos]
                .iter()
                .copied()
                .filter(|&row| !matches!(c.kind(row), NodeKind::DotAccess | NodeKind::Qualified))
                .collect(),
            kids[..pos].to_vec(),
        )
    };
    let Some(&root) = left.last() else {
        return;
    };
    // The operand row decides both the display name and the report
    // start — `IsExpression`/`AsExpression` containers anchor at the
    // `is`/`as` operator, so `utf16_span(node)` would open at the
    // operator; the official `reportAtNode` opens at the operand.
    let (dart_type, operand_row): (Option<String>, u32) = if c.kind(root)
        == NodeKind::ThisExpression
        && left[..left.len() - 1]
            .iter()
            .all(|&row| is_signature_row(c, row))
    {
        (enclosing_class_name(c).map(str::to_owned), root)
    } else {
        let stripped = slice::access_run(c, &left);
        match stripped.as_slice() {
            [only] if c.kind(*only) == NodeKind::Identifier => {
                (typed_identifier_display(c, *only), *only)
            }
            _ => (None, root),
        }
    };
    let Some(dart_type) = dart_type else {
        return;
    };
    // The tested type: the `Type` row naming a same-file `@JS` class,
    // in a file importing `dart:js_interop`. The type tiles inside the
    // operator container (the bridge begins it at the operator, before
    // the type parses); the sibling face — the rows completing before
    // the container seals — is accepted too. The head token after the
    // operator keyword tells the tested type from the member's return
    // type (token ids are source-ordered).
    let op_head = c.head_token(node);
    let mut candidates = c.collect_direct(node);
    candidates.extend_from_slice(&preceding);
    let Some(type_row) = candidates
        .into_iter()
        .rev()
        .find(|&row| c.kind(row) == NodeKind::Type && c.head_token(row) > op_head)
    else {
        return;
    };
    let js_type = c.lexeme(c.head_token(type_row));
    if !imports_js_interop(c) {
        return;
    }
    if !same_file_js_class(c, js_type) {
        // The cross-file arm: the resolved container's own `@JS` rows.
        // `None` — an unresolvable container could carry the
        // annotation — and `Some(false)` — a resolvable Dart type —
        // both stay silent; only a proven `@JS` proceeds.
        let Some(analysis) = c.semantics() else {
            return;
        };
        let strings = analysis.strings();
        if analysis.container_has_annotation(strings.intern(js_type), strings.intern("JS"))
            != Some(true)
        {
            return;
        }
    }
    // The container anchors at the `is`/`as` operator (a delta-tile
    // row — `token_range().start` is the operator token); the official
    // `reportAtNode` opens at the operand. `head_token` on the operand
    // row answers that start.
    let range =
        crate::rules::utf16_token_span(c, c.head_token(operand_row)).start..c.utf16_span(node).end;
    if is_check {
        c.report_texts(
            range,
            DART_IS_JS_PROBLEM,
            DART_IS_JS_CORRECTION,
            &[dart_type.as_str(), js_type],
        );
    } else {
        c.report_texts(
            range,
            DART_AS_JS_PROBLEM,
            DART_AS_JS_CORRECTION,
            &[dart_type.as_str(), js_type],
        );
    }
}

/// The static type display of a bare operand read: the bridge read
/// row's named type (the display argument must be exact — an
/// argument-carrying type under-reports; `dynamic` and untyped reads
/// carry no Dart-type face).
fn typed_identifier_display(c: &Check<'_>, row: u32) -> Option<String> {
    let analysis = c.semantics()?;
    let Ty::Named {
        name,
        nullable,
        args,
    } = analysis.type_of(query_site(c, row))?
    else {
        return None;
    };
    if args != ArgListId::EMPTY {
        return None;
    }
    let mut display = analysis.strings().resolve(name).to_owned();
    if nullable {
        display.push('?');
    }
    Some(display)
}

/// The enclosing class-like declaration's name (the wrapper-aware
/// read — the production name is the row's left sibling).
fn enclosing_class_name<'a>(c: &'a Check<'_>) -> Option<&'a str> {
    let class_row = c.path.iter().rev().find(|&&a| {
        matches!(
            c.kind(a),
            NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::ExtensionDeclaration
        )
    })?;
    crate::rules::class_declaration_name(c, *class_row)
}

/// The member-signature rows that tile before an arrow body's
/// expression (the written return type, the name, the parameter list,
/// their wrappers — and the childless `ExpressionBody` handle the
/// expression rows trail).
fn is_signature_row(c: &Check<'_>, row: u32) -> bool {
    matches!(
        c.kind(row),
        NodeKind::Type
            | NodeKind::VoidType
            | NodeKind::FunctionType
            | NodeKind::Identifier
            | NodeKind::FormalParameters
            | NodeKind::OptionalFormalParameters
            | NodeKind::TypeVariables
            | NodeKind::OperatorName
            | NodeKind::NoTypeArguments
            | NodeKind::TypeArguments
            | NodeKind::ExpressionBody
    )
}

/// Whether the unit imports `dart:js_interop`.
fn imports_js_interop(c: &Check<'_>) -> bool {
    let Some(root) = c.unit.ast.root() else {
        return false;
    };
    // The import directive tiles inside its `TopLevelDeclaration`
    // wrapper; a root-direct Import is accepted too should a shape ever
    // leave it bare.
    let has_uri = |c: &Check<'_>, row: u32| {
        c.kind(row) == NodeKind::Import
            && c.collect_direct(row).into_iter().any(|kid| {
                c.kind(kid) == NodeKind::StringLiteral && {
                    let span = c.byte_span(kid);
                    let text = &c.unit.source[span.start..span.end];
                    text.trim_matches(|q| q == '\'' || q == '"') == "dart:js_interop"
                }
            })
    };
    c.collect_direct(root).into_iter().any(|row| {
        has_uri(c, row)
            || (c.kind(row) == NodeKind::TopLevelDeclaration
                && c.collect_direct(row).into_iter().any(|kid| has_uri(c, kid)))
    })
}

/// Whether `name` is a same-file class carrying a `@JS` annotation
/// (the annotation star precedes the top-level wrapper at the root).
fn same_file_js_class(c: &Check<'_>, name: &str) -> bool {
    let Some(root) = c.unit.ast.root() else {
        return false;
    };
    let tops = c.collect_direct(root);
    for (i, &top) in tops.iter().enumerate() {
        if c.kind(top) != NodeKind::TopLevelDeclaration {
            continue;
        }
        let named = c.collect_direct(top).into_iter().any(|kid| {
            c.kind(kid) == NodeKind::Identifier
                && c.field(kid, PayloadField::Context)
                    == Some(
                        dartq_parser::IdentifierContext::ClassOrMixinOrExtensionDeclaration as u32,
                    )
                && c.lexeme(c.head_token(kid)) == name
        });
        if !named {
            continue;
        }
        // The class's annotation star tiles as the wrapper's left
        // sibling at the root (the annotation seals before the wrapper
        // opens), or inside the wrapper when the wrapper opened first —
        // read both faces.
        let mut stars: Vec<u32> = Vec::new();
        if i > 0 && c.kind(tops[i - 1]) == NodeKind::MetadataStar {
            stars.push(tops[i - 1]);
        }
        for kid in c.collect_direct(top) {
            if c.kind(kid) == NodeKind::MetadataStar {
                stars.push(kid);
            }
        }
        let annotated = stars.into_iter().any(|star| {
            c.collect_direct(star).into_iter().any(|meta| {
                c.kind(meta) == NodeKind::Metadata
                    && c.collect_direct(meta).into_iter().any(|piece| {
                        c.kind(piece) == NodeKind::Identifier
                            && c.lexeme(c.head_token(piece)) == "JS"
                    })
            })
        });
        if annotated {
            return true;
        }
    }
    false
}
