use super::{Check, NodeKind, PayloadField, RuleSpec};
use crate::rules::expressions::{query_site, static_type_faces as faces};
use crate::rules::first_child;
use dartq_parser::MemberKind;
use dartq_semantic::model::Ty;

/// Official: `pkg/linter/lib/src/rules/unnecessary_nullable_for_final_variable_declarations.dart:35-117`
/// — a `final`/`const` declaration with a nullable written type whose
/// initializer's static type is provably non-nullable reports at the
/// variable name (`isNullable(declared) && isNonNullable(initializer)`,
/// :106-107).
///
/// dartq faces: the initializer static type answers over the semantic
/// bridge — a bare name's flow read (`type_of`), an invocation's callee
/// return type (`callee_type`), a single-name member access
/// (`member_type`), and an instance creation's container
/// (`creation_type`) — and reports only on a provably non-nullable
/// answer; `null`, unknown and unresolved initializers stay silent
/// (the conservative direction). Literals keep the syntactic face (the
/// bridge's read rows ride identifier reads only). Binary and
/// conditional initializers, and multi-hop chains, carry no bridge face
/// and under-report (the `no_duplicate_case_values` constant-evaluation
/// precedent). Fields keep the official private-or-static gate
/// (:83-88). The `PatternVarDeclStatement` face
/// (`visitPatternVariableDeclaration` :63-79) walks a `final`
/// `ListPattern`/`RecordPattern`'s `DeclaredVariablePattern` leaves —
/// the same verdict over the written type and the literal-initializer
/// matched-type faces ([`check_pattern_decl`]). Message:
/// `diagnostic.g.dart`
/// `unnecessaryNullableForFinalVariableDeclarations`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_nullable_for_final_variable_declarations",
    &[
        NodeKind::VariablesDeclaration,
        NodeKind::PatternVarDeclStatement
    ],
    check,
);

fn check(c: &mut Check<'_>) {
    if c.kind(c.node()) == NodeKind::PatternVarDeclStatement {
        check_pattern_decl(c);
        return;
    }
    let node = c.node();
    // `isFinal || isConst` (:97): the storage keyword — production rows
    // anchor a local declaration at the name, a field at the keyword;
    // `variables_declaration_keyword` recovers `final`/`const` across
    // both (`late final` resolves to `final`).
    let Some(keyword) = crate::rules::variables_declaration_keyword(c, node) else {
        return;
    };
    if !matches!(c.lexeme(keyword), "final" | "const") {
        return;
    }
    // `declaredElement.type is DynamicType` bails (:101-103) — the
    // written type answers from three shapes: a `Type`-kind child
    // (hand-built tables), the token run after the storage keyword
    // ([`local_written_type`] — production rows anchored at the name),
    // and the keyword-anchored field tiling (the type runs from the row
    // head, after `late`/`final`/`const`, up to the first variable's
    // name). The written `?` is the span's tail token (#174).
    let kids = c.collect_direct(node);
    let first_var_token = kids
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::InitializedIdentifier)
        .map(|kid| c.head_token(kid));
    let written_type = match kids
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::Type)
    {
        Some(type_node) => {
            let range = c.unit.ast.token_range(type_node);
            Some((range.start, range.end))
        }
        None => super::local_written_type(c, node).or_else(|| {
            let name = first_var_token?;
            let mut start = c.head_token(node);
            for _ in 0..2 {
                match c.lexeme(start) {
                    "late" | "var" | "final" | "const" => start += 1,
                    _ => break,
                }
            }
            (start < name).then_some((start, name))
        }),
    };
    let nullable_written = match written_type {
        Some((start, end)) => start < end && c.lexeme(end - 1) == "?",
        None => false,
    };
    if !nullable_written {
        return;
    }
    // Fields: private or static only (:83-88).
    let is_field = c
        .ancestor(1)
        .is_some_and(|parent| c.kind(parent) == NodeKind::Fields);
    if is_field {
        let parent = c.ancestor(1).expect("checked above");
        let is_static =
            c.field(parent, PayloadField::MemberKind) == Some(MemberKind::StaticField as u32);
        let is_private = c
            .collect_direct(node)
            .iter()
            .any(|&var| c.lexeme(c.head_token(var)).starts_with('_'));
        if !is_static && !is_private {
            return;
        }
    }
    for var in c.collect_direct(node) {
        if c.kind(var) != NodeKind::InitializedIdentifier {
            continue;
        }
        // `variable.initializer?.staticType` (:98) — an uninitialized
        // variable bails per-variable. The initializer's piece run is
        // the VariableInitializer's direct-child tiling; the literal
        // face keeps its head-row answer (a prefixed literal tiles its
        // operator leaf after the head literal).
        let Some(init) = c
            .collect_direct(var)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer)
        else {
            continue;
        };
        let head = first_child(c, init);
        let run = c.collect_direct(init);
        if !initializer_non_nullable(c, head, &run) {
            continue;
        }
        // `rule.reportAtToken(variable.name)` (:110).
        let Some(name_token) = crate::rules::initialized_identifier_name(c, var) else {
            continue;
        };
        let Some(start) = c.utf16_of_token(name_token) else {
            continue;
        };
        c.report(start..start, &[]);
    }
}

/// `initializerType != null && isNonNullable(initializerType)`
/// (:98-107): the initializer's static type reports only on a provably
/// non-nullable answer. The literal face answers syntactically on the
/// head row; every other shape answers over the semantic bridge —
/// `null`, unknown and unresolved faces answer `false`, the
/// under-report direction.
fn initializer_non_nullable(c: &Check<'_>, head: Option<u32>, run: &[u32]) -> bool {
    if head.is_some_and(|head| is_non_null_literal(c.kind(head))) {
        return true;
    }
    let Some(analysis) = c.semantics() else {
        return false;
    };
    let Some(root) = faces::run_root(c, run) else {
        return false;
    };
    if c.kind(root) == NodeKind::LiteralNull {
        return false;
    }
    let ty = match c.kind(root) {
        // A bare name answers at its flow read row.
        NodeKind::Identifier => analysis.type_of(query_site(c, root)),
        // An instance creation answers through the container
        // resolution (the constructed type is non-nullable by shape).
        NodeKind::ImplicitCreation | NodeKind::NewExpression => {
            super::constructed_type(c, analysis, root)
        }
        // A trailing `DotAccess` marker roots a bare member access
        // (`c.v`) — the shared `expr_type` face reads its selector off
        // the marker's span tail, but the production marker's window
        // swallows the `;`, so the name answers off the row's `Operator`
        // dot token instead (`operator + 1` is the member name). The
        // receiver run ends at the dot — the selector's own tear-off
        // group tiles before the marker and must not join it.
        NodeKind::DotAccess => (|| {
            let dot = c.field(root, PayloadField::Operator)?;
            let selector = dot + 1;
            let receiver_run: Vec<u32> = run[..run.len() - 1]
                .iter()
                .copied()
                .filter(|&piece| c.unit.ast.token_range(piece).end <= dot)
                .collect();
            let receiver = faces::single_receiver(c, &receiver_run, selector)?;
            let name = analysis.strings().intern(c.lexeme(selector));
            analysis.member_type(query_site(c, receiver), name)
        })(),
        // The remaining bridge faces: a bare send (the callee return
        // type) and a single-name member access — chains and unmodeled
        // shapes answer `None`.
        _ => faces::expr_type(c, analysis, run).or_else(|| {
            // A bare `D()` send is an implicit class creation — the
            // callee face answers `None` on a type target (no
            // `type_uses` row on an expression-context selector); the
            // binding resolves the class name.
            if c.kind(root) != NodeKind::Send {
                return None;
            }
            let selector = run
                .iter()
                .copied()
                .find(|&piece| c.kind(piece) == NodeKind::Identifier)?;
            analysis
                .binding_of(query_site(c, selector))
                .filter(|binding| {
                    binding.kind == dartq_semantic::BindingKind::Decl
                        && crate::rules::is_upper_camel_case(
                            analysis.strings().resolve(binding.target_name),
                        )
                })
                .map(|binding| Ty::named(binding.target_name))
        }),
    };
    ty.is_some_and(Ty::is_strictly_non_nullable)
}

/// The non-null literal face: any literal form but `null`.
fn is_non_null_literal(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::LiteralInt
            | NodeKind::LiteralDouble
            | NodeKind::LiteralBool
            | NodeKind::StringLiteral
            | NodeKind::ListLiteral
            | NodeKind::SetOrMapLiteral
            | NodeKind::RecordLiteral
            | NodeKind::ConstExpression
            | NodeKind::ConstLiteral
    )
}

/// `visitPatternVariableDeclaration`
/// (unnecessary_nullable_for_final_variable_declarations.dart:63-79) — a
/// `final` destructuring declaration walks its `ListPattern` elements /
/// `RecordPattern` fields; each `DeclaredVariablePattern` reports at its
/// name when the written type is nullable and the matched value type is
/// provably non-nullable (`isNullable(declared) &&
/// isNonNullable(matchedValueType)`, `check` :42-56).
///
/// The statement seals as a leaf row — the pattern pieces and the
/// initializer tile flat in the owner window before it: the
/// `ListPattern`/`RecordPattern` marker splits the run into pattern
/// pieces (before it) and initializer pieces (after it). A
/// `DeclaredVariablePattern` leaf's `Type` sibling is the piece
/// immediately before it; the `?` tail marks the nullable declared type,
/// and `dynamic` never reports (`type is DynamicType` bail, :48-50).
///
/// The matched value type answers over the initializer LITERAL shapes —
/// the semantic face carries no per-`DeclaredVariablePattern`
/// `matchedValueType` row (the flow's read rows ride identifier reads
/// only, and `ArgListId` does not decompose through `Analysis`). A list
/// pattern matched against a `ListLiteral` whose every top-level element
/// is a non-null literal infers a non-nullable element type; a record
/// pattern's positional field matched against the `RecordLiteral`'s
/// positional literal of the same index answers that literal's
/// nullability. Every other initializer shape — a bare name, an
/// invocation, a non-literal element — stays silent (the under-report
/// direction); nested patterns and named record fields bail whole.
fn check_pattern_decl(c: &mut Check<'_>) {
    let node = c.node();
    let Some(parent) = c.ancestor(1) else { return };
    let kids = c.collect_direct(parent);
    let Some(pos) = kids.iter().position(|&kid| kid == node) else {
        return;
    };
    // The statement's own flat pieces: owner children before the
    // statement row whose heads sit inside the statement's token span
    // (the declaration keyword heads the range — earlier statements'
    // rows all head before it).
    let stmt_head = c.head_token(node);
    let stmt_end = c.unit.ast.token_range(node).end;
    let pieces: Vec<u32> = kids[..pos]
        .iter()
        .copied()
        .filter(|&row| c.head_token(row) >= stmt_head && c.head_token(row) < stmt_end)
        .collect();
    // `node.keyword.keyword != Keyword.FINAL` bails (:64-65) — the
    // `final`/`var` keyword is the statement's first real token (a
    // metadata rider can head the range before it).
    let keyword = (stmt_head..stmt_end).find(|&t| matches!(c.lexeme(t), "final" | "var"));
    if !matches!(keyword.map(|t| c.lexeme(t)), Some("final")) {
        return;
    }
    let Some(marker_pos) = pieces
        .iter()
        .position(|&row| matches!(c.kind(row), NodeKind::ListPattern | NodeKind::RecordPattern))
    else {
        return;
    };
    let marker = pieces[marker_pos];
    let pattern_pieces = &pieces[..marker_pos];
    // A nested pattern container inside the pattern window changes the
    // matched-type level — bail rather than mix elements across levels
    // (upstream recurses; the census face stays at the flat layer).
    if pattern_pieces.iter().any(|&row| {
        matches!(
            c.kind(row),
            NodeKind::ListPattern
                | NodeKind::RecordPattern
                | NodeKind::MapPattern
                | NodeKind::ObjectPattern
                | NodeKind::CastPattern
                | NodeKind::RelationalPattern
                | NodeKind::ConstantPattern
                | NodeKind::NullAssertPattern
                | NodeKind::NullCheckPattern
        )
    }) {
        return;
    }
    let elements: Vec<u32> = pattern_pieces
        .iter()
        .copied()
        .filter(|&row| c.kind(row) == NodeKind::DeclaredVariablePattern)
        .collect();
    if elements.is_empty() {
        return;
    }
    let initializer = &pieces[marker_pos + 1..];
    match c.kind(marker) {
        NodeKind::ListPattern => {
            // `matchedValueType` for every element is the shared iterable
            // element type — a `ListLiteral` of all-non-null literals
            // infers a non-nullable element.
            if !list_initializer_non_null_elements(c, initializer) {
                return;
            }
            for &element in &elements {
                if let Some(start) = nullable_pattern_name(c, pattern_pieces, element) {
                    c.report(start..start, &[]);
                }
            }
        }
        NodeKind::RecordPattern => {
            // A named `PatternField` (`name:` carries the `Colon` payload)
            // takes the record's named-field type — unanswerable here;
            // positional fields map to the initializer's positional
            // literals by index. A named field or a named literal element
            // skips its own slot without disturbing the positional count.
            let Some(positional) = record_positional_literals(c, initializer) else {
                return;
            };
            let mut position = 0usize;
            for &element in &elements {
                // The `PatternField` marker trailing the variable leaf
                // carries the named field's `:` — its matched type is the
                // record's named field, unanswerable here (skip without
                // consuming a positional slot).
                let piece_index = pattern_pieces
                    .iter()
                    .position(|&row| row == element)
                    .unwrap_or(0);
                let named = pattern_pieces.get(piece_index + 1).is_some_and(|&row| {
                    c.kind(row) == NodeKind::PatternField
                        && c.field(row, PayloadField::Colon)
                            .is_some_and(|colon| colon != dartq_ast::TOKEN_NONE)
                });
                if named {
                    continue;
                }
                let non_null = positional
                    .get(position)
                    .is_some_and(|&lit| is_non_null_literal(c.kind(lit)));
                position += 1;
                if !non_null {
                    continue;
                }
                if let Some(start) = nullable_pattern_name(c, pattern_pieces, element) {
                    c.report(start..start, &[]);
                }
            }
        }
        _ => {}
    }
}

/// The `ListLiteral` element-nullability face: the initializer's last
/// piece is the literal container; its top-level element rows are the
/// owner-window pieces inside its token span that no other element's
/// span strictly contains (a nested `[[0]]`'s inner rows drop under the
/// inner literal). `Some(true)` only when at least one element stands
/// and every element is a non-null literal — the LUB-of-literals answer
/// for the iterable element type.
fn list_initializer_non_null_elements(c: &Check<'_>, initializer: &[u32]) -> bool {
    let Some(&literal) = initializer.last() else {
        return false;
    };
    if c.kind(literal) != NodeKind::ListLiteral {
        return false;
    }
    let span = c.unit.ast.token_range(literal);
    let candidates: Vec<u32> = initializer
        .iter()
        .copied()
        .filter(|&row| row < literal && c.head_token(row) >= span.start)
        .collect();
    let elements: Vec<u32> = candidates
        .iter()
        .copied()
        .filter(|&row| {
            !candidates.iter().any(|&host| {
                host != row && {
                    let host_span = c.unit.ast.token_range(host);
                    let row_span = c.unit.ast.token_range(row);
                    row_span.start > host_span.start && row_span.end <= host_span.end
                }
            })
        })
        .filter(|&row| {
            !matches!(
                c.kind(row),
                NodeKind::NoTypeArguments
                    | NodeKind::TypeArguments
                    | NodeKind::Arguments
                    | NodeKind::NoArguments
                    | NodeKind::Send
                    | NodeKind::Spread
                    | NodeKind::IfControlFlow
                    | NodeKind::ForControlFlow
            )
        })
        .collect();
    // A control-flow/spread piece anywhere in the literal suspends the
    // literal-LUB answer — bail whole rather than miscount.
    if candidates.iter().any(|&row| {
        matches!(
            c.kind(row),
            NodeKind::Spread | NodeKind::IfControlFlow | NodeKind::ForControlFlow
        )
    }) {
        return false;
    }
    !elements.is_empty() && elements.iter().all(|&row| is_non_null_literal(c.kind(row)))
}

/// The `RecordLiteral`'s positional element rows in order —
/// `collect_direct` reaches its adopted members (`([], 1)` tiles its
/// literals inside the record's own window). `None` on any named or
/// control-flow element: the positional mapping is undecidable (named
/// record fields join the pattern's named-field check instead).
fn record_positional_literals(c: &Check<'_>, initializer: &[u32]) -> Option<Vec<u32>> {
    let &literal = initializer.last()?;
    if c.kind(literal) != NodeKind::RecordLiteral {
        return None;
    }
    let kids = c.collect_direct(literal);
    if kids.iter().any(|&row| {
        matches!(
            c.kind(row),
            NodeKind::NamedRecordField
                | NodeKind::Spread
                | NodeKind::IfControlFlow
                | NodeKind::ForControlFlow
        )
    }) {
        return None;
    }
    Some(
        kids.into_iter()
            .filter(|&row| {
                !matches!(
                    c.kind(row),
                    NodeKind::NoTypeArguments
                        | NodeKind::TypeArguments
                        | NodeKind::Arguments
                        | NodeKind::NoArguments
                        | NodeKind::Send
                )
            })
            .collect(),
    )
}

/// The report site of a nullable `DeclaredVariablePattern`: its `Type`
/// sibling is the pattern piece immediately before it (the production
/// tiling — `num? c` seals `Type` then the variable leaf), the `?` tail
/// marks `isNullable`, and `dynamic` keeps the upstream `DynamicType`
/// bail. `Some(offset)` of the variable name when the written type is
/// nullable.
fn nullable_pattern_name(c: &Check<'_>, pattern_pieces: &[u32], element: u32) -> Option<u32> {
    let index = pattern_pieces.iter().position(|&row| row == element)?;
    let ty = *pattern_pieces.get(index.wrapping_sub(1))?;
    if c.kind(ty) != NodeKind::Type {
        return None;
    }
    let range = c.unit.ast.token_range(ty);
    if range.start >= range.end || c.lexeme(range.end - 1) != "?" {
        return None;
    }
    if c.lexeme(range.start) == "dynamic" {
        return None;
    }
    // `rule.reportAtToken(node.name)` (:54) — the variable leaf's own
    // token is the name.
    c.utf16_of_token(c.head_token(element))
}
