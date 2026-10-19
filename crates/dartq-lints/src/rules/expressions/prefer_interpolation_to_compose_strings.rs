use dartq_scanner::TokenKind;

use super::{Check, Group, NodeKind, RuleSpec, same_file, slice};

/// Official: `pkg/linter/lib/src/rules/prefer_interpolation_to_compose_strings.dart:32-88`
/// — an adjacent `(literal, value)` pair in a `+` chain reports over the
/// pair span.
///
/// The left-operand String type check
/// (`leftOperand.staticType.isDartCoreString`, :85) reads through the
/// shared literal face ([`slice::literal_faces`]) — a string literal
/// operand decides, a provably non-string literal (a record) passes,
/// and an opaque operand answers at its same-file written type (a
/// non-`String` binding is the user type — #792's
/// `test_variableNotString`), an untyped local's string-literal
/// initializer answering `String` first (the #1135 declaration-side
/// gate), the remaining untyped face presuming the user type when the
/// same file declares any `operator +` and the string presumption
/// otherwise. Chains report once at the chain root; the
/// official's per-node re-visit would re-report inner pairs. The operand
/// units are the chain's piece runs closed at expression tails; a
/// multi-piece operand without an invocation tail (`x.y + 'a'`) splits
/// piece-wise — a documented divergence affecting the report span only
/// (the gate reads the operand faces, never the span shape, so the
/// span stays un-normalized by design). Raw strings and both-literal
/// pairs pass verbatim (:70-75); a `toString` invocation with arguments
/// passes (:78-82). Message: `diagnostic.g.dart`
/// `preferInterpolationToComposeStrings`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "prefer_interpolation_to_compose_strings",
    group: Group::Recommended,
    problem: "Use interpolation to compose strings and values.",
    correction: "Try using string interpolation to build the composite string.",
    kinds: &[NodeKind::BinaryExpression],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // Only the chain root flattens (the official `chainedAdditions` walk
    // re-reports from every inner node; once per chain is the anchor).
    if c.payload_word(node, 0).map(|op| c.lexeme(op)) != Some("+") {
        return;
    }
    let is_inner_plus = c.ancestor(1).is_some_and(|parent| {
        c.kind(parent) == NodeKind::BinaryExpression
            && c.payload_word(parent, 0)
                .is_none_or(|op| c.lexeme(op) == "+")
    });
    if is_inner_plus {
        return;
    }
    // The chain root's operand runs read through the adopted window
    // ([`slice::binary_operands`]); nested `+` binaries descend into
    // their own child roots.
    let Some((left, right)) = slice::binary_operands(c) else {
        return;
    };
    let mut pieces = Vec::with_capacity(left.len() + right.len());
    for &kid in &left {
        if c.kind(kid) == NodeKind::BinaryExpression
            && c.payload_word(kid, 0).map(|op| c.lexeme(op)) == Some("+")
        {
            chain_pieces(c, kid, &mut pieces);
        } else {
            pieces.push(kid);
        }
    }
    pieces.extend_from_slice(&right);
    let units = operand_units(c, &pieces);
    let mut skip_next = false;
    for pair in units.windows(2) {
        let (left, right) = (&pieces[pair[0].clone()], &pieces[pair[1].clone()]);
        if skip_next {
            skip_next = false;
            continue;
        }
        let left_literal = is_string_literal(c, left);
        let right_literal = is_string_literal(c, right);
        // OK(#735): neither side is a literal.
        if !left_literal && !right_literal {
            continue;
        }
        // OK(#2490): a raw string on either side.
        if (left_literal && is_raw(c, left[0])) || (right_literal && is_raw(c, right[0])) {
            continue;
        }
        // OK: both literals (prefer_adjacent_string_concatenation's face).
        if left_literal && right_literal {
            continue;
        }
        // OK(sdk#52610): a `toString` invocation with arguments.
        if is_to_string_with_arguments(c, left) || is_to_string_with_arguments(c, right) {
            continue;
        }
        // `leftOperand.staticType?.isDartCoreString` (:85): a string
        // literal decides, a provably non-string literal passes, the
        // opaque face answers at the same-file written type.
        let string_left = match slice::literal_faces(c, left[0]) {
            Some(faces) => faces & slice::STRING != 0,
            None => opaque_left_is_string(c, left),
        };
        if !string_left {
            continue;
        }
        // The chain root's head anchors the report (the official
        // re-reports inner pairs at the same span).
        let start = c.utf16_span(pieces[0]).start;
        let end = c.utf16_span(*right.last().unwrap_or(&right[0])).end;
        c.report(start..end, &[]);
        // The official skips the pair consuming the reported right
        // operand (:86-88).
        skip_next = true;
    }
}

/// The flattened `+` chain (`chainedAdditions`, :90-98): the recursion
/// descends into left-nested `+` binaries; their operand pieces collect
/// in order.
fn chain_pieces(c: &Check<'_>, node: u32, out: &mut Vec<u32>) {
    for kid in c.collect_direct(node) {
        if c.kind(kid) == NodeKind::BinaryExpression
            && c.payload_word(kid, 0).map(|op| c.lexeme(op)) == Some("+")
        {
            chain_pieces(c, kid, out);
        } else {
            out.push(kid);
        }
    }
}

/// The operand units over the chain's flat pieces: a unit is the maximal
/// piece run closed by an expression tail — a `Send`, an access/postfix
/// marker, or a single-piece expression. The glue pieces (identifiers,
/// type-argument wrappers, argument groups, prefixes) extend the open
/// run; an access marker after a closed run merges into it (the marker
/// tails the invocation it closes, `a.toString(x)`), or opens its own.
fn operand_units(c: &Check<'_>, pieces: &[u32]) -> Vec<core::ops::Range<usize>> {
    let mut units = Vec::new();
    let mut start: Option<usize> = None;
    for (i, &piece) in pieces.iter().enumerate() {
        match c.kind(piece) {
            NodeKind::Identifier
            | NodeKind::NoTypeArguments
            | NodeKind::TypeArguments
            | NodeKind::Arguments
            | NodeKind::NoArguments
            | NodeKind::UnaryPrefix => {
                start.get_or_insert(i);
            }
            NodeKind::DotAccess | NodeKind::CascadeAccess => match start.take() {
                Some(from) => units.push(from..i + 1),
                None => match units.last_mut() {
                    Some(last) => last.end = i + 1,
                    None => units.push(i..i + 1),
                },
            },
            _ => {
                let from = start.take().unwrap_or(i);
                units.push(from..i + 1);
            }
        }
    }
    units
}

/// `StringLiteral` operand: a single-piece unit (adjacent strings tile
/// as siblings, one unit each).
fn is_string_literal(c: &Check<'_>, unit: &[u32]) -> bool {
    unit.len() == 1 && c.kind(unit[0]) == NodeKind::StringLiteral
}

/// `SimpleStringLiteral.isRaw`: the literal opens with `r`.
fn is_raw(c: &Check<'_>, literal: u32) -> bool {
    c.lexeme(c.head_token(literal)).starts_with('r')
}

/// `isToStringInvocationWithArguments`: a unit carrying a `toString`
/// call with a non-empty argument list.
fn is_to_string_with_arguments(c: &Check<'_>, unit: &[u32]) -> bool {
    let Some(send) = unit.iter().position(|&p| c.kind(p) == NodeKind::Send) else {
        return false;
    };
    let Some((head, arguments)) = slice::send_parts(c, unit, send) else {
        return false;
    };
    if c.kind(unit[head]) != NodeKind::Identifier
        || c.lexeme(c.head_token(unit[head])) != "toString"
    {
        return false;
    }
    arguments.is_some_and(|args| !c.collect_direct(args).is_empty())
}

/// The opaque left operand's `isDartCoreString` face (:74): the
/// official reads the operand's static type. The same-file written
/// face: a binding with a written non-`String` type is the user type
/// (silent — #792), a written `String` reports, an untyped local whose
/// initializer is a plain string-literal run is dart:core `String`
/// (adjudication #1135's declaration-side gate — the file's
/// `operator +` never fires on a `String` receiver), and the remaining
/// untyped or unbound face presumes the user type when the same file
/// declares any `operator +` (a user-type `+` makes the operand's
/// static type that user type, never dart:core `String`), the string
/// presumption otherwise.
fn opaque_left_is_string(c: &Check<'_>, unit: &[u32]) -> bool {
    if let Some(root) = unit
        .iter()
        .copied()
        .find(|&p| c.kind(p) == NodeKind::Identifier)
    {
        let name = c.lexeme(c.head_token(root));
        match same_file::written_type(c, root, name) {
            Some("String") => return true,
            Some(_) => return false,
            None => {
                if inferred_string_local(c, root) {
                    return true;
                }
            }
        }
    }
    !same_file_declares_plus(c)
}

/// Whether `row`'s name binds to an untyped local declared before the
/// use whose initializer is a plain string-literal run (a sole literal
/// or an adjacent chain — static type non-nullable `String`). The same
/// innermost-block walk the strings module's inferred-local face
/// runs; a written type never reaches here.
fn inferred_string_local(c: &Check<'_>, row: u32) -> bool {
    let name = c.lexeme(c.head_token(row));
    let use_start = c.byte_span(row).start;
    for i in (0..c.path.len().saturating_sub(1)).rev() {
        let ancestor = c.path[i];
        if !matches!(c.kind(ancestor), NodeKind::Block | NodeKind::BlockBody) {
            continue;
        }
        for kid in c.collect_direct(ancestor) {
            if c.kind(kid) != NodeKind::VariablesDeclaration {
                continue;
            }
            for var in c
                .collect_direct(kid)
                .into_iter()
                .filter(|&var| c.kind(var) == NodeKind::InitializedIdentifier)
            {
                let binds = crate::rules::initialized_identifier_name(c, var)
                    .is_some_and(|tok| c.lexeme(tok) == name)
                    && c.byte_span(var).start < use_start;
                if binds {
                    let Some(range) =
                        crate::rules::flow::exhaustive_cases::initializer_range(c, var)
                    else {
                        return false;
                    };
                    return range.start < range.end
                        && (range.start..range.end).all(|token| {
                            c.unit
                                .tokens
                                .get(token as usize)
                                .is_some_and(|t| t.kind == TokenKind::String)
                        });
                }
            }
        }
    }
    false
}

/// Whether the same file declares an `operator +` on a user type — a
/// class, mixin, enum, or extension type whose `+` is a member
/// declaration. Extensions are exempt: an extension `+` never changes
/// the receiver's static type (`'a' + x` stays `String` and reports).
fn same_file_declares_plus(c: &Check<'_>) -> bool {
    let Some(&root) = c.path.first() else {
        return false;
    };
    for top in c.collect_direct(root) {
        for inner in c.collect_direct(top) {
            if matches!(
                c.kind(inner),
                NodeKind::ClassDeclaration
                    | NodeKind::MixinDeclaration
                    | NodeKind::EnumDeclaration
                    | NodeKind::ExtensionTypeDeclaration
            ) && class_declares_plus(c, inner)
            {
                return true;
            }
        }
    }
    false
}

/// Whether the declaration's members carry an `operator +`: the
/// [`NodeKind::OperatorName`] marker tiles beside its `Method` in the
/// declaration body's completion window (the production bridge keeps
/// it out of the method's own children), so the window scan reads the
/// marker's operator token (payload word 0) directly.
fn class_declares_plus(c: &Check<'_>, decl: u32) -> bool {
    for member in c.children(decl) {
        if c.kind(member) == NodeKind::OperatorName
            && c.lexeme(
                c.payload(member)
                    .first()
                    .copied()
                    .unwrap_or(crate::rules::NONE_TOKEN),
            ) == "+"
        {
            return true;
        }
    }
    false
}
