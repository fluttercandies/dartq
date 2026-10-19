use super::{Check, Group, NONE_TOKEN, NodeKind, RuleSpec};

/// Official: `pkg/linter/lib/src/rules/type_literal_in_constant_pattern.dart:40-63`
/// — a constant pattern whose expression's static type is the `Type`
/// type reports at the pattern. The official bails for a
/// `const`-marked pattern (:45-47) and for a `Type`-typed matched
/// value (:50-59, the raw-type exemption).
///
/// The type-literal test (`expression.staticType.isDartCoreType`,
/// :57-63) resolves over the same-file channel: a bare name answers
/// through the innermost value binding first (a value is never a type
/// literal — the `const String = 'x'` shadow over-report is dead), a
/// top-level `const` folds its initializer through the alias channel
/// (`const t = int; case t:` — a Type-valued constant — reports like
/// the official static type), a typed binding reports when written
/// `Type`, and a name with no value binding is a type literal: a
/// same-file type declaration (`case MyType:` — the user-type
/// under-report is dead) or an unshadowed built-in type name. A
/// two-segment name folds a same-file class-static alias (`A.t`);
/// anything else there (an enum member, a prefixed spelling — the
/// `Type` row's head is the prefix) stays silent, the prefixed type
/// literal being the documented under-report. The type-literal test is
/// the official `isDartCoreType` element read — nullability-blind, so a
/// `Type`-written binding reports through `Type?` too. The raw-type
/// exemption reads the matched value through the scrutinee face and —
/// for a constant nested in an object pattern (`A(field: int)`) — the
/// field's same-file written type, both the nullability-sensitive
/// `isSubtypeOf` face (`Type?` exempts nothing). The class-scope
/// static face (`static const int = 3;` shadowing the spelling inside
/// its container, the enum's own constants included) reads the
/// enclosing containers directly; the object-pattern field face reads
/// instance fields
/// only (a getter, a prefixed container, a name-less field, and
/// list/map element positions keep the name reading). Message:
/// `diagnostic.g.dart` `typeLiteralInConstantPattern`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "type_literal_in_constant_pattern",
    group: Group::Core,
    problem: "Use 'TypeName _' instead of a type literal.",
    correction: "Replace with 'TypeName _'.",
    kinds: &[NodeKind::ConstantPattern],
    check,
};

/// The built-in type names a bare constant-pattern identifier can spell
/// (`dynamic` included; `void` is a keyword and never reaches an
/// identifier) — the unshadowed fallback the same-file channel cannot
/// resolve better (dart:core is auto-imported, so an unshadowed
/// spelling of these IS the library type). The official test corpus
/// exercises the dart:core names.
pub(crate) fn is_builtin_type_name(lexeme: &str) -> bool {
    matches!(
        lexeme,
        "int"
            | "double"
            | "num"
            | "bool"
            | "String"
            | "Object"
            | "dynamic"
            | "Never"
            | "Null"
            | "Function"
            | "Iterable"
            | "List"
            | "Map"
            | "Set"
            | "Symbol"
            | "Type"
            | "Future"
            | "Stream"
            | "Enum"
            | "Record"
            | "Uri"
            | "Pattern"
            | "RegExp"
            | "DateTime"
            | "Duration"
    )
}

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `node.constKeyword != null` bails (:45-47): a const-marked pattern
    // owns the `const` keyword as its span head.
    if c.lexeme(c.head_token(node)) == "const" {
        return;
    }
    let Some(expression) = super::first_child(c, node) else {
        return;
    };
    // The bare constant parses as a tear-off send group — the pattern
    // adopts the childless `Send` leaf (span = the identifier; the
    // `Identifier` / `NoTypeArguments` leaves precede it as owner
    // siblings), so the constant name reads off the send's span head.
    if !matches!(
        c.kind(expression),
        NodeKind::Identifier | NodeKind::Send | NodeKind::DotAccess
    ) {
        return;
    }
    if !constant_is_type_literal(c, expression) {
        return;
    }
    // OK to use raw types if the matched value is of type `Type`
    // (:50-59) — the scrutinee face or the object-pattern field face.
    if matched_value_is_type(c) || pattern_field_is_type(c) {
        return;
    }
    let range = c.utf16_span(node);
    c.report(range, &[]);
}

/// The `Type`-matched-value exemption (:50-59): the enclosing match's
/// scrutinee proves a `Type` matched value. The syntactic face: a
/// `.runtimeType` tail is always `Type`, and a bare-identifier scrutinee
/// resolves its written type through the same-file face — the
/// `Type`-typed parameter / local / field / top-level shapes
/// (`type_literal_in_constant_pattern_test.dart`
/// `test_constType_matchType`).
fn matched_value_is_type(c: &Check<'_>) -> bool {
    for (depth, &ancestor) in c.path.iter().enumerate().rev() {
        // The scrutinee piece run: if-case (the condition container
        // holds the scrutinee itself), a switch statement case, a
        // switch expression case.
        let scrutinee = match c.kind(ancestor) {
            NodeKind::ParenthesizedCondition => {
                // The condition's pieces leak past the container to the
                // enclosing statement: the scrutinee is the container's
                // parent's direct rows before the container row, ending
                // before the pattern starts. (The path step after the
                // container is the container's child on the path — the
                // pattern itself — so the owner sits one step before
                // it.) The boundary reads the pattern's own head row
                // (the container's adopted child when present) — the
                // container's span can anchor wider than the constant.
                let Some(&outer) = depth.checked_sub(1).and_then(|i| c.path.get(i)) else {
                    return false;
                };
                let pattern_window = c.children(c.node());
                let pattern_start = if pattern_window.start < pattern_window.end {
                    c.byte_span(pattern_window.start).start
                } else {
                    c.byte_span(c.node()).start
                };
                c.collect_direct(outer)
                    .into_iter()
                    .take_while(|&kid| kid != ancestor)
                    .filter(|&kid| c.byte_span(kid).end <= pattern_start)
                    .collect()
            }
            NodeKind::SwitchCase | NodeKind::SwitchExpressionCase => {
                let boundary = if c.kind(ancestor) == NodeKind::SwitchCase {
                    NodeKind::SwitchBlock
                } else {
                    NodeKind::SwitchExpressionBlock
                };
                let Some(root_index) = depth.checked_sub(2) else {
                    return false;
                };
                let root = c.path[root_index];
                let kids = c.collect_direct(root);
                let Some(boundary_pos) = kids.iter().position(|&kid| c.kind(kid) == boundary)
                else {
                    return false;
                };
                kids[..boundary_pos].to_vec()
            }
            _ => continue,
        };
        // A `.runtimeType` tail is always `Type`.
        if let Some(&tail) = scrutinee.last()
            && c.kind(tail) == NodeKind::DotAccess
            && c.lexeme(c.head_token(tail)) == "runtimeType"
        {
            return true;
        }
        // A bare-identifier scrutinee resolves through the same-file
        // written-type face — exactly `Type`, not the nullable `Type?`
        // (the official `isSubtypeOf(matchedValueType, typeType)`).
        if let Some(&head) = scrutinee.first()
            && c.kind(head) == NodeKind::Identifier
        {
            let face = crate::rules::written_type_face(c, head, c.lexeme(c.head_token(head)));
            if face.is_some_and(|face| face.name == Some("Type") && !face.nullable) {
                return true;
            }
        }
        return false;
    }
    false
}

/// The static-type question over the expression's name segments (the
/// module doc carries the decision table).
fn constant_is_type_literal(c: &Check<'_>, expression: u32) -> bool {
    let range = c.unit.ast.token_range(expression);
    let Some((leading_dot, names)) =
        super::exhaustive_cases::segments(c, super::exhaustive_cases::strip_const(c, range))
    else {
        return false;
    };
    if leading_dot {
        return false;
    }
    match names.as_slice() {
        [name] => bare_name_is_type_literal(c, expression, name),
        [.., class, field] => super::exhaustive_cases::class_const_init(c, class, field)
            .is_some_and(|init| folds_to_type(c, init, &mut Vec::new(), 0)),
        _ => false,
    }
}

/// The bare-name face: the innermost value binding first, then the
/// type reading.
fn bare_name_is_type_literal(c: &Check<'_>, use_row: u32, name: &str) -> bool {
    use crate::rules::expressions::same_file::BindingScope;
    match crate::rules::expressions::same_file::binding(c, use_row, name) {
        Some((scope, typed)) => match scope {
            // A `const` binding folds its initializer through the alias
            // channel (a Type-valued constant); any other value ends
            // the reading off the type family.
            BindingScope::TopLevel => super::exhaustive_cases::top_level_const_init(c, name)
                .map_or(typed_face_is_type(c, typed), |init| {
                    folds_to_type(c, init, &mut Vec::new(), 0)
                }),
            BindingScope::Local => {
                crate::rules::expressions::same_file::local_const_initializer(c, use_row, name)
                    .is_some_and(|init| folds_to_type(c, init, &mut Vec::new(), 0))
            }
            BindingScope::Parameter | BindingScope::Field => typed_face_is_type(c, typed),
            // A `static` member read is a value pattern, never a type
            // literal (the `enclosing_static_declares` posture below —
            // the binding walk now reports static fields itself).
            BindingScope::StaticField => false,
        },
        // No same-file value binding: a type literal — a same-file type
        // declaration or an unshadowed built-in type name. A class-scope
        // static getter/method or enum constant of the enclosing
        // container shadows the spelling first (the binding walk sees
        // static fields; the remaining static member kinds stay here).
        None => {
            !enclosing_static_declares(c, name)
                && (crate::rules::expressions::same_file::declares_type(c, name)
                    || is_builtin_type_name(name))
        }
    }
}

/// Whether a class-like container enclosing the use site declares a
/// STATIC value member spelled `name` — a `static` field, a static
/// getter/method, or (inside an enum) one of the enum's own constants.
/// The same-file binding walk reads instance members only, so a user
/// constant named like a built-in type (`static const int = 3;`) would
/// otherwise keep the type reading and over-report; the bare name
/// resolves to that member, which is a value pattern, never a type
/// literal. The walk stops at the innermost declaring container (an
/// outer one is shadowed either way).
fn enclosing_static_declares(c: &Check<'_>, name: &str) -> bool {
    let field_ctx = dartq_parser::IdentifierContext::FieldDeclaration as u32;
    for &ancestor in c.path.iter().rev() {
        match c.kind(ancestor) {
            NodeKind::ClassDeclaration
            | NodeKind::MixinDeclaration
            | NodeKind::ExtensionDeclaration
            | NodeKind::ExtensionTypeDeclaration
            | NodeKind::EnumDeclaration => {
                let bodies = core::iter::once(ancestor).chain(
                    c.collect_direct(ancestor)
                        .into_iter()
                        .filter(|&kid| c.kind(kid) == NodeKind::DeclarationBody),
                );
                for container in bodies {
                    for member in c.collect_direct(container) {
                        let static_member = dartq_ast::mod_flags(c.unit.ast, member)
                            & crate::rules::expressions::same_file::STATIC_BIT
                            != 0;
                        match c.kind(member) {
                            NodeKind::Fields if static_member => {
                                if c.collect_direct(member).into_iter().any(|kid| {
                                    c.kind(kid) == NodeKind::Identifier
                                        && c.payload(kid).first() == Some(&field_ctx)
                                        && c.lexeme(c.head_token(kid)) == name
                                }) {
                                    return true;
                                }
                            }
                            NodeKind::Method if static_member => {
                                // The name row's head token — `member_name`
                                // answers a ROW id, never a token index.
                                if crate::rules::declarations::member_declaration_name(
                                    c, member,
                                )
                                .is_some_and(|row| c.lexeme(c.head_token(row)) == name)
                                {
                                    return true;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            NodeKind::EnumBody => {
                // The enclosing enum's own constants: plain values in
                // scope inside the enum's members (a childless handle
                // anchored at the name — the span head answers).
                if c.collect_direct(ancestor).into_iter().any(|element| {
                    c.kind(element) == NodeKind::EnumElement
                        && c.lexeme(c.head_token(element)) == name
                }) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// The object-pattern field face of the matched-value exemption: a
/// constant pattern nested in an object pattern (`A(field: int)`)
/// matches the field's own value, whose static type is the field's
/// written type — a `Type`-written field exempts the nested constant
/// (`type_literal_in_constant_pattern_test.dart`
/// `test_constType_matchType_nested`). The bridge leaks the
/// sub-pattern rows past the field windows (the constant's tree parent
/// is the statement container; the `PatternField` / `ObjectPattern`
/// rows are span-only overlays completing right after the leak), so
/// the wrapping pair resolves by span containment over the container's
/// arena window — the innermost containing rows. The field reads
/// through the same-file written-type face of the object pattern's
/// class; a prefixed spelling (`prefix.A(...)`, payload word 1), a
/// name-less field (`A(: sub)`, payload word 0 is the colon), and an
/// unresolvable class keep the name reading.
fn pattern_field_is_type(c: &Check<'_>) -> bool {
    let node = c.node();
    let span = c.byte_span(node);
    let contains = |row: u32| {
        let outer = c.byte_span(row);
        outer.start <= span.start && span.end <= outer.end
    };
    let Some(container) = c.path.len().checked_sub(2).map(|i| c.path[i]) else {
        return false;
    };
    let window = c.children(container).start..container;
    // Post-order rows: the innermost containing overlay completes
    // first.
    let Some(object) = window
        .clone()
        .filter(|&row| c.kind(row) == NodeKind::ObjectPattern && contains(row))
        .min()
    else {
        return false;
    };
    let Some(field) = (c.children(container).start..object)
        .filter(|&row| c.kind(row) == NodeKind::PatternField && contains(row))
        .min()
    else {
        return false;
    };
    // A prefixed container (`prefix.A(...)`) resolves outside the file.
    let words = c.payload(object);
    let Some(&class_tok) = words.first() else {
        return false;
    };
    if class_tok == NONE_TOKEN || words.get(1).copied() != Some(NONE_TOKEN) {
        return false;
    }
    // The name-less field face carries the colon token, not a name.
    let Some(&name_tok) = c.payload(field).first() else {
        return false;
    };
    if name_tok == NONE_TOKEN || c.lexeme(name_tok) == ":" {
        return false;
    }
    let Some(class_row) = same_file_class_named(c, c.lexeme(class_tok)) else {
        return false;
    };
    instance_field_is_type(c, class_row, c.lexeme(name_tok))
}

/// The same-file top-level class declaration named `name` — the local
/// class-only face of the `exhaustive_cases::class_like_named` walk
/// (module-private there): the object-pattern head resolves against a
/// class's instance members.
fn same_file_class_named(c: &Check<'_>, name: &str) -> Option<u32> {
    let root = c.unit.ast.root()?;
    for top in c.collect_direct(root) {
        if c.kind(top) != NodeKind::TopLevelDeclaration {
            continue;
        }
        for kid in c.collect_direct(top) {
            if c.kind(kid) == NodeKind::ClassDeclaration
                && crate::rules::class_declaration_name(c, kid) == Some(name)
            {
                return Some(kid);
            }
        }
    }
    None
}

/// Whether the instance field `field` of the same-file `class_row` is
/// written exactly `Type` (the non-static `Fields` walk of
/// `type_face_of_field`'s index builder, class-row parameterized — the
/// member's single written type covers all its declarators). Untyped
/// fields, every other written type, and the nullable `Type?` (not a
/// subtype of `Type`) keep the name reading; a getter or an
/// inferred-from-initializer type is invisible (the documented channel
/// face).
fn instance_field_is_type(c: &Check<'_>, class_row: u32, field: &str) -> bool {
    let body = c
        .collect_direct(class_row)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::DeclarationBody)
        .unwrap_or(class_row);
    for member in crate::rules::rows_with_parent(c, body) {
        if c.kind(member) != NodeKind::Fields
            || c.payload(member).first()
                != Some(&(dartq_parser::MemberKind::NonStaticField as u32))
        {
            continue;
        }
        let written = c
            .collect_direct(member)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::Type);
        let Some(written) = written else {
            continue;
        };
        let face = crate::rules::type_face(c, written);
        if face.name != Some("Type") || face.nullable {
            continue;
        }
        for decl in c
            .collect_direct(member)
            .into_iter()
            .filter(|&kid| c.kind(kid) == NodeKind::VariablesDeclaration)
        {
            for var in c
                .collect_direct(decl)
                .into_iter()
                .filter(|&var| c.kind(var) == NodeKind::InitializedIdentifier)
            {
                if crate::rules::initialized_identifier_name(c, var)
                    .is_some_and(|name| c.lexeme(name) == field)
                {
                    return true;
                }
            }
        }
    }
    false
}

/// A typed value binding is a type literal whenever written `Type` —
/// nullability-blind, the official `isDartCoreType` reading the type's
/// element (`Type?` still names dart:core `Type`, :792-794) — while
/// the matched-value exemption is the nullability-sensitive
/// `isSubtypeOf` face.
fn typed_face_is_type(c: &Check<'_>, typed: Option<u32>) -> bool {
    typed.is_some_and(|row| {
        c.kind(row) == dartq_ast::NodeKind::Type
            && crate::rules::type_face(c, row).name == Some("Type")
    })
}

/// The alias fold over a top-level/class-static const initializer
/// chain: the chain landing on a type name (a same-file type or an
/// unshadowed built-in) makes the constant Type-valued. `seen` breaks
/// alias cycles, `depth` bounds them.
fn folds_to_type(
    c: &Check<'_>,
    init: core::ops::Range<u32>,
    seen: &mut Vec<u32>,
    depth: usize,
) -> bool {
    if depth > super::exhaustive_cases::MAX_ALIAS_DEPTH || seen.contains(&init.start) {
        return false;
    }
    let init = super::exhaustive_cases::strip_const(c, init);
    let Some((false, names)) = super::exhaustive_cases::segments(c, init.clone()) else {
        return false;
    };
    match names.as_slice() {
        // Within a top-level initializer only top-level names resolve:
        // a const alias recurses, a non-const value ends the chain off
        // the type family.
        [name] => match super::exhaustive_cases::top_level_const_init(c, name) {
            Some(next) => {
                seen.push(init.start);
                folds_to_type(c, next, seen, depth + 1)
            }
            None => {
                crate::rules::expressions::same_file::declares_type(c, name)
                    || is_builtin_type_name(name)
            }
        },
        [.., class, field] => super::exhaustive_cases::class_const_init(c, class, field)
            .is_some_and(|next| folds_to_type(c, next, seen, depth + 1)),
        _ => false,
    }
}
