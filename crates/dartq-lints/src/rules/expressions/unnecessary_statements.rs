use dartq_semantic::model::MemberKind;

use super::{PayloadField, Check, NodeKind, RuleSpec, query_site, same_file};
use crate::rules::{body_members, get_set_token, same_file_declaration, slice, type_face};

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/unnecessary_statements.dart`
/// — an expression statement, a C-style `for` initializer/updater, or a
/// cascade section that has no clear effect reports. The upstream walk
/// (`_ReportNoClearEffectVisitor`) never descends through `super` — every
/// unhandled node routes to `visitNode` and reports — so the verdict is
/// per expression form:
///
/// - clear-effect forms stay silent: invocations, assignments, pattern
///   assignments, awaits, throws, rethrows, casts (`as`, :57-59), postfix
///   operators, prefix `++`/`--`, cascades, instance creations, and
///   constructor invocations;
/// - `??`/`||`/`&&` walk the right operand only (:72-83); every other
///   binary reports at the whole expression;
/// - conditional expressions walk both arms (:91-94);
/// - a bare name or member chain keeps the getter face (:129-176): a
///   getter at its origin declaration stays silent (`GetterElement &&
///   isOriginDeclaration`), everything else reports at the whole chain;
/// - every other form reports at its node (literals, lambdas, type
///   tests, index accesses, parens, constructor tear-offs).
///
/// The dartq faces over the production tiling (the statement row owns
/// its expression's piece run; the outermost-completing row is the
/// expression root):
///
/// - the C-style `for` face (:209-217) reads the statement window's rows
///   byte-bounded by the [`NodeKind::ForParts`] separator payload tokens —
///   the initialization run and each comma-delimited updater walk
///   separately; the header condition's wrapper row (the parser parses the
///   condition through the statement face) stays silent here and in the
///   statement face — upstream visits the initialization and the updaters
///   only;
/// - the cascade face (:192-198) reads each [`NodeKind::CascadeAccess`]
///   section: a bare member-access section reports when the member's
///   static type is a function type (:194 — the `..method` tear-off);
///   invocation sections stay silent.
///
/// The getter faces resolve in two layers: the semantic model first —
/// a member chain reads `member_of` over the receiver's resolved type
/// (a `Getter` entry stays silent, a `Method`/`Field` tear-off or read
/// reports; cross-file classes included), and a bare name reads its
/// binding (an in-scope type parameter or import prefix reports, an
/// unresolved name reports — upstream's `visitNode` face — a cross-file
/// top-level declaration stays silent because the binding face cannot
/// split a getter from a value, the conservative family) — then the
/// same-file walk answers what the model leaves open (own getters,
/// fields, methods, and the implicit-`this` scope). The cascade
/// function-type face composes the same way: a semantic `Method` entry
/// is a tear-off; a getter returning a function type answers only over
/// the same-file written type (the model carries no function-type `Ty`
/// — that sliver under-reports until the function-type face lands).
/// The upstream `node.parent is FunctionBody` bail (:202-204) is
/// structurally dead over the statement tiling (an `ExpressionStatement`
/// row always sits in a block-family owner). Message: `diagnostic.g.dart`
/// `unnecessaryStatements`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_statements",
    &[
        NodeKind::ExpressionStatement,
        NodeKind::ForStatement,
        NodeKind::Cascade,
    ],
    check,
);

fn check(c: &mut Check<'_>) {
    // The wave posture: the rule reads the semantic face; face absent
    // is the plain-run posture and stays silent (the engine contract).
    let Some(analysis) = c.semantics() else {
        return;
    };
    match c.kind(c.node()) {
        NodeKind::ExpressionStatement => {
            // The classic-`for` header condition parses through this face
            // (its wrapper row shares the statement kind); the header reads
            // belong to the `for` face, the condition to the Boolean-
            // condition rules.
            if for_header_condition(c, c.node()) {
                return;
            }
            let run = statement_run(c, c.node());
            walk(c, analysis, &run);
        }
        NodeKind::ForStatement => check_for(c, analysis),
        NodeKind::Cascade => check_cascade(c, analysis),
        _ => {}
    }
}

/// The expression-form dispatch over a piece run (:114-116 `visitNode`
/// verdicts). The root is the row whose span covers every other row of
/// the run — the outermost completion of the expression.
fn walk(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>, run: &[u32]) {
    let Some(root) = root_of(c, run) else {
        return;
    };
    #[expect(
        clippy::match_same_arms,
        reason = "the explicit clear-effect set documents the upstream                       visitor's no-op visits; folding it into `_` erases the                       documented taxonomy"
    )]
    match c.kind(root) {
        // The clear-effect set: the upstream empty-bodied visits that do
        // not descend (:57-70, :86-88, :97-111, :119-121, :124-126,
        // :162-164, :178-186). `Send` is the bridge's bare-identifier
        // marker (`Identifier + NoTypeArguments + NoArguments + Send`)
        // as well as a real invocation's completion row — only the
        // `Arguments`-carrying send is a clear effect; the bare-name
        // face falls through to the getter walk.
        NodeKind::Send if run.iter().any(|&row| c.kind(row) == NodeKind::Arguments) => {}
        NodeKind::Send => {
            // The bare-name face: reduce the run through the send-group
            // strip and dispatch on what remains (an `Identifier` walks
            // the getter face).
            let stripped = slice::access_run(c, run);
            walk(c, analysis, &stripped);
        }
        NodeKind::Assignment
        | NodeKind::PatternAssignment
        | NodeKind::AwaitExpression
        | NodeKind::ThrowExpression
        | NodeKind::AsExpression
        | NodeKind::Cascade
        | NodeKind::NewExpression
        | NodeKind::ImplicitCreation
        | NodeKind::ConstExpression
        | NodeKind::UnaryPostfix
        | NodeKind::NonNullAssert => {}
        // `--`/`++` are clear (:141-147); every other prefix reports.
        // The bridge stamps the prefix as one childless token leaf over
        // the operator after the operand completes (no payload), so the
        // leaf's head token reads the operator and the report spans the
        // operator through the operand rows' content end (`!c` reports
        // len 2, `-(-a)` reports once at the whole expression — the
        // upstream visitNode face does not descend). The operand tiles
        // seal past their follower (the send-group markers stamp the
        // `;`), so the content end reads the non-marker rows only.
        NodeKind::UnaryPrefix => {
            if !matches!(c.lexeme(c.head_token(root)), "--" | "++") {
                let end = run
                    .iter()
                    .copied()
                    .filter(|&row| {
                        !matches!(
                            c.kind(row),
                            NodeKind::NoTypeArguments
                                | NodeKind::NoArguments
                                | NodeKind::Send
                                | NodeKind::DotAccess
                        )
                    })
                    .map(|row| c.utf16_span(row).end)
                    .max()
                    .unwrap_or_else(|| c.utf16_span(root).end);
                let range = c.utf16_span(root).start..end;
                c.report(range, &[]);
            }
        }
        // `??`/`||`/`&&` walk the right operand for control flow
        // (:72-83); every other binary reports at the whole expression.
        NodeKind::BinaryExpression => {
            let controls = c
                .field(root, PayloadField::Operator)
                .is_some_and(|op| matches!(c.lexeme(op), "??" | "||" | "&&"));
            if controls {
                let right = binary_right_run(c, run, root);
                walk(c, analysis, &right);
            } else {
                report(c, root, run);
            }
        }
        // The arms walk; the condition does not (:91-94).
        NodeKind::Conditional => {
            for arm in arm_runs(c, run, root) {
                walk(c, analysis, &arm);
            }
        }
        // The const literal is a `visitNode` face upstream — the whole
        // `const …` node reports with no descent (an empty inner list
        // included: `const [];`, `const [1]`, `const [1 + 2]` all report
        // at the keyword-to-close span). The row anchors at the literal
        // head — the `const` keyword is a width-0 begin anchor carried
        // by no row — so the keyword face reads the preceding token.
        NodeKind::ConstLiteral => {
            let head = c.head_token(root);
            let kw = if head > 0 && c.lexeme(head - 1) == "const" {
                head - 1
            } else {
                head
            };
            let start = crate::rules::utf16_token_span(c, kw).start;
            // The row's own span is unreliable at the empty shape (the
            // seal degenerates), so the statement dispatch bounds the
            // close: the trailing `;` token's start is the expression
            // content end (the tiling wobbles on whether the statement
            // row's token range carries the `;`).
            let end = match c.kind(c.node()) {
                NodeKind::ExpressionStatement => {
                    let range = c.unit.ast.token_range(c.node());
                    match range.end.checked_sub(1) {
                        Some(last) if c.lexeme(last) == ";" => {
                            crate::rules::utf16_token_span(c, last).start
                        }
                        _ => c.utf16_span(c.node()).end,
                    }
                }
                _ => c.utf16_span(root).end,
            };
            c.report(start..end, &[]);
        }
        // The report-at-node set: literals, lambdas, type tests, index
        // accesses, parens, constructor tear-offs (:114-116).
        NodeKind::LiteralBool
        | NodeKind::LiteralDouble
        | NodeKind::LiteralInt
        | NodeKind::LiteralNull
        | NodeKind::StringLiteral
        | NodeKind::ListLiteral
        | NodeKind::SetOrMapLiteral
        | NodeKind::RecordLiteral
        | NodeKind::SymbolLiteral
        | NodeKind::FunctionExpression
        | NodeKind::IsExpression
        | NodeKind::Indexed
        | NodeKind::ParenthesizedExpression
        | NodeKind::ConstructorReference => report(c, root, run),
        // The getter faces (:129-176): a bare name and a member chain.
        NodeKind::Identifier
        | NodeKind::ThisExpression
        | NodeKind::SuperExpression
        | NodeKind::DotAccess
        | NodeKind::Qualified => walk_access(c, analysis, run, root),
        // Unmodeled shapes stay silent (the conservative family).
        _ => {}
    }
}

/// The expression run an `ExpressionStatement` covers: the bridge
/// stamps the statement as a childless leaf (`leaf_span` — `expr;`),
/// so its pieces tile as siblings under the enclosing block. The run
/// is the parent's direct rows whose spans sit strictly inside the
/// statement's span (the `;` end is exclusive — no piece reaches it).
fn statement_run(c: &Check<'_>, stmt: u32) -> Vec<u32> {
    let Some(parent) = c.unit.ast.parent(stmt) else {
        return Vec::new();
    };
    let span = c.byte_span(stmt);
    c.collect_direct(parent)
        .into_iter()
        .filter(|&row| {
            if row == stmt {
                return false;
            }
            let r = c.byte_span(row);
            r.start >= span.start && r.end <= span.end
        })
        .collect()
}

/// The run's root row: the outermost row whose span contains every
/// other row's span (sibling pieces never nest; wrapper rows may tie —
/// the outermost completion is the last in run order), falling back to
/// the last row (postfix completion order puts the root last).
fn root_of(c: &Check<'_>, run: &[u32]) -> Option<u32> {
    let root = *run.last()?;
    run.iter()
        .copied()
        .rfind(|&row| {
            let span = c.byte_span(row);
            run.iter().all(|&other| {
                let other = c.byte_span(other);
                other.start >= span.start && other.end <= span.end
            })
        })
        .or(Some(root))
}

/// Reports at the whole expression `root`: the run's earliest head
/// token bounds the start — a container root's own covered span opens
/// at its delta-tile anchor (the operator or the trailing leaf), never
/// at the operand head.
fn report(c: &mut Check<'_>, root: u32, run: &[u32]) {
    let start = slice::run_utf16_start(c, run).unwrap_or(c.utf16_span(root).start);
    let tail = content_end_row(c, root, run);
    let range = start..c.utf16_span(tail).end;
    c.report(range, &[]);
}

/// The row whose end bounds the expression's true content: the
/// binary/assignment containers and the member-selector roots seal past
/// their last operand (the parser's expression return carries the
/// follower `;`/`,`/`)`/`:`), so the content ends at the children's
/// tiled tail; every other row seals at its own content exactly. The
/// `No*` sentinel leaves anchor at the follower token and the bare-send
/// `Send` leaf and the member-selector `DotAccess` handle tile through
/// it (the #585 send/selector span posture), so none of them bound the
/// content.
fn content_end_row(c: &Check<'_>, root: u32, run: &[u32]) -> u32 {
    match c.kind(root) {
        NodeKind::BinaryExpression | NodeKind::Assignment => {
            let window = c.children(root);
            (window.start..window.end)
                .filter(|&row| {
                    !matches!(
                        c.kind(row),
                        NodeKind::NoTypeArguments
                            | NodeKind::NoArguments
                            | NodeKind::Send
                            | NodeKind::DotAccess
                    )
                })
                .max_by_key(|&row| c.byte_span(row).end)
                .unwrap_or(root)
        }
        // The member-selector tiles are owner siblings (no child
        // window), so the tail reads off the run's non-marker rows.
        NodeKind::DotAccess | NodeKind::Qualified => run
            .iter()
            .copied()
            .filter(|&row| {
                !matches!(
                    c.kind(row),
                    NodeKind::NoTypeArguments
                        | NodeKind::NoArguments
                        | NodeKind::Send
                        | NodeKind::DotAccess
                )
            })
            .max_by_key(|&row| c.byte_span(row).end)
            .unwrap_or(root),
        _ => root,
    }
}

/// The `visitSimpleIdentifier`/`visitPrefixedIdentifier`/
/// `visitPropertyAccess` face (:129-176): a getter at its origin
/// declaration stays silent; everything else reports at the whole
/// chain. The bare-name face walks the same-file declarations first —
/// locals shadow, then top-level and enclosing-class getters — and the
/// semantic binding face answers the cross-file tail: an in-scope type
/// parameter or import prefix reports, an unresolved name reports (the
/// upstream `visitNode` verdict), and a cross-file top-level
/// declaration stays silent (the binding face cannot split a getter
/// from a value — the conservative family).
fn walk_access(
    c: &mut Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    run: &[u32],
    root: u32,
) {
    // A bare name: no access marker seals the run.
    let marker = run
        .iter()
        .copied()
        .rfind(|&row| c.kind(row) == NodeKind::DotAccess);
    let Some(marker) = marker else {
        let stripped = slice::access_run(c, run);
        let Some(&only) = stripped.first() else {
            return;
        };
        if stripped.len() != 1 || c.kind(only) != NodeKind::Identifier {
            return;
        }
        let name = c.lexeme(c.head_token(only));
        // A local/parameter/field/top-level variable binding reports
        // (`test_localVariable`, `test_topLevelVariable`).
        if same_file::binding(c, root, name).is_some() {
            report(c, root, run);
            return;
        }
        // An origin getter stays silent (:167-176 — the top-level and
        // implicit-`this` faces).
        if is_same_file_getter(c, root, name) {
            return;
        }
        // Every other same-file declaration (function tear-off, setter,
        // class) reports (:114-116).
        if same_file::declares_name(c, name) {
            report(c, root, run);
            return;
        }
        // The semantic cross-file tail: a resolved type-parameter or
        // prefix is not a getter (report); an unresolved name takes the
        // upstream `visitNode` report; a cross-file declaration stays
        // silent (getter-vs-value undecidable — the conservative
        // family).
        match analysis.binding_of(query_site(c, root)) {
            Some(binding)
                if matches!(
                    binding.kind,
                    dartq_semantic::BindingKind::TypeParam
                        | dartq_semantic::BindingKind::Prefix
                ) =>
            {
                report(c, root, run);
            }
            Some(_) => {}
            None => report(c, root, run),
        }
        return;
    };
    let Some(name_index) = slice::access_name_index(c, run, marker) else {
        return;
    };
    let name = c.lexeme(c.head_token(run[name_index]));
    // The `this.`/`super.` face: the enclosing class-like scope's
    // member (:150-159 — a same-file origin getter passes).
    if run.iter().any(|&row| {
        matches!(
            c.kind(row),
            NodeKind::ThisExpression | NodeKind::SuperExpression
        )
    }) {
        let getter = enclosing_class(c, root).is_some_and(|class_row| {
            matches!(class_member(c, class_row, name), Some(Member::Getter))
        });
        if !getter {
            report(c, root, run);
        }
        return;
    }
    // The bare-receiver face: the semantic member walk first — the
    // receiver's resolved type decides across files; the same-file
    // written-type channel answers what the model leaves open.
    let Some(receiver) = slice::bare_receiver(c, run, name_index) else {
        return;
    };
    let receiver_name = c.lexeme(c.head_token(receiver));
    if let Some(member) =
        analysis.member_of(query_site(c, receiver), analysis.strings().intern(name))
    {
        match member.entry.kind {
            // The origin-getter face (:129-138, :150-159).
            MemberKind::Getter => return,
            // A field read or a method tear-off reports (:117-121,
            // :139-147 — `test_instanceField`, `test_methodTearoff`).
            MemberKind::Method | MemberKind::Field => {
                report(c, root, run);
                return;
            }
            MemberKind::Setter => {}
        }
    }
    let Some(class_name) = same_file::written_type(c, receiver, receiver_name) else {
        return;
    };
    let Some(class_row) = same_file_declaration(c, class_name) else {
        return;
    };
    match class_member(c, class_row, name) {
        // The origin-getter face (:129-138, :150-159); a name not
        // declared on the same-file class — inherited or imported —
        // joins the conservative silent face.
        Some(Member::Getter) | None => {}
        // A field or a method tear-off reports (:117-121, :139-147 —
        // `test_instanceField`, `test_methodTearoff`).
        Some(_) => report(c, root, run),
    }
}

/// The same-file member verdicts the getter and cascade faces read.
enum Member {
    Getter,
    Field,
    Method,
    Setter,
}

/// The member `name` of the same-file class-like `class_row`, read off
/// the body rows (a `Method` getter/method/setter, a `Fields` field).
/// Static-ness does not change the verdict (an upstream static member
/// answers the same faces).
fn class_member(c: &Check<'_>, class_row: u32, name: &str) -> Option<Member> {
    for member in body_members(c, class_row) {
        match c.kind(member) {
            NodeKind::Method => {
                // The stamped-name read — a written return type tiles a
                // `TypeReference` `Identifier` ahead of the name.
                let Some(name_row) = crate::rules::member_name(c, member) else {
                    continue;
                };
                if c.lexeme(c.head_token(name_row)) != name {
                    continue;
                }
                return match get_set_token(c, member) {
                    Some("get") => Some(Member::Getter),
                    Some(_) => Some(Member::Setter),
                    None => Some(Member::Method),
                };
            }
            NodeKind::Fields => {
                for kid in c.collect_direct(member) {
                    if c.kind(kid) == NodeKind::Identifier
                        && c.lexeme(c.head_token(kid)) == name
                        && c.field(kid, PayloadField::Context)
                            == Some(dartq_parser::IdentifierContext::FieldDeclaration as u32)
                    {
                        return Some(Member::Field);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether `name` is a same-file origin getter at the use site: a
/// top-level getter, or an instance getter of the enclosing class-like
/// scope (the implicit-`this` face).
fn is_same_file_getter(c: &Check<'_>, use_node: u32, name: &str) -> bool {
    let root = c.path.first().copied();
    // The `get` TopLevelMethod's name is the
    // `TopLevelFunctionDeclaration`-context identifier — the generic
    // `member_name` head picks the return-type identifier for a typed
    // getter, so the context read is required here.
    let ctx = dartq_parser::IdentifierContext::TopLevelFunctionDeclaration as u32;
    if let Some(root) = root {
        for top in c.collect_direct(root) {
            for inner in c.collect_direct(top) {
                if c.kind(inner) != NodeKind::TopLevelMethod
                    || get_set_token(c, inner) != Some("get")
                {
                    continue;
                }
                let found = c.collect_direct(inner).into_iter().any(|kid| {
                    c.kind(kid) == NodeKind::Identifier
                        && c.field(kid, PayloadField::Context) == Some(ctx)
                        && c.lexeme(c.head_token(kid)) == name
                });
                if found {
                    return true;
                }
            }
        }
    }
    enclosing_class(c, use_node).is_some_and(|class_row| {
        matches!(class_member(c, class_row, name), Some(Member::Getter))
    })
}

/// The innermost enclosing class-like declaration row of the use site.
fn enclosing_class(c: &Check<'_>, use_node: u32) -> Option<u32> {
    let pos = c.path.iter().position(|&row| row == use_node)?;
    c.path[..pos].iter().rev().copied().find(|&row| {
        matches!(
            c.kind(row),
            NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::ExtensionDeclaration
                | NodeKind::ExtensionTypeDeclaration
        )
    })
}

/// The classic-`for` header face (:209-217): the initialization expression
/// and every updater walk separately; a for-in header is not
/// `ForPartsWithExpression` and passes. The parts leaf seals after the
/// whole header (childless — the fragments tile beside it in the statement
/// window), so the runs split off the statement window's rows by the
/// separator payload tokens; the updaters additionally bound at the parts
/// row's own end to keep the body's rows out.
fn check_for(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let fragments = c.collect_direct(c.node());
    let Some(parts) = fragments
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::ForParts)
    else {
        return;
    };
    let Some(left_sep) = c.field(parts, PayloadField::LeftSeparator) else {
        return;
    };
    let Some(right_sep) = c.field(parts, PayloadField::RightSeparator) else {
        return;
    };
    // The official `rightSeparator` is the token read after the condition
    // statement — the first updater's head when updaters exist, the `)`
    // otherwise — so the updater bound is that token's start, not its end.
    let (Some(left_start), Some(right_start)) = (
        c.unit
            .tokens
            .get(left_sep as usize)
            .map(|t| t.byte_offset as usize),
        c.unit
            .tokens
            .get(right_sep as usize)
            .map(|t| t.byte_offset as usize),
    ) else {
        return;
    };
    // The initialization run: the pieces before the first separator. The
    // content bound (not the row's own seal — a bare-send-tailed
    // initialization seals past the separator) keeps the row inside the
    // initialization window.
    let init: Vec<u32> = fragments
        .iter()
        .copied()
        .filter(|&row| c.byte_span(content_end_row(c, row, &[row])).end <= left_start)
        .collect();
    walk(c, analysis, &init);
    // Each comma-delimited updater walks separately (:212-215).
    let parts_end = c.byte_span(parts).end;
    let updater_rows: Vec<u32> = fragments
        .iter()
        .copied()
        .filter(|&row| {
            let span = c.byte_span(row);
            span.start >= right_start && span.end <= parts_end
        })
        .collect();
    for run in comma_groups(c, &updater_rows) {
        walk(c, analysis, &run);
    }
}

/// Whether `stmt` is the classic-`for` header condition's wrapper row: the
/// header parses the condition through `parseExpressionStatement`, so the
/// wrapper carries the statement kind and would dispatch like a real
/// statement. It shares the for statement's window (a non-block body
/// statement's wrapper does too) and closes at the parts' second separator;
/// everything past that separator is the body.
fn for_header_condition(c: &Check<'_>, stmt: u32) -> bool {
    let Some(parent) = c.unit.ast.parent(stmt) else {
        return false;
    };
    if c.kind(parent) != NodeKind::ForStatement {
        return false;
    }
    let Some(parts) = c
        .collect_direct(parent)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::ForParts)
    else {
        return false;
    };
    let Some(right_sep) = c.field(parts, PayloadField::RightSeparator) else {
        return false;
    };
    let Some(token) = c.unit.tokens.get(right_sep as usize) else {
        return false;
    };
    c.byte_span(stmt).end <= (token.byte_offset + token.byte_len) as usize
}

/// The maximal row groups split at the comma token indices (an updater
/// list tiling).
fn comma_groups(c: &Check<'_>, rows: &[u32]) -> Vec<Vec<u32>> {
    let Some((&first, rest)) = rows.split_first() else {
        return Vec::new();
    };
    let start = c.byte_span(first).start;
    let end = rest
        .last()
        .map_or_else(|| c.byte_span(first).end, |&row| c.byte_span(row).end);
    // The separator scan spans the whole window's token range: the
    // updater list tiles as sibling subtrees, so the first row's own
    // token range misses the commas between later segments.
    let first_range = c.unit.ast.token_range(first);
    let window = first_range.start
        ..rest
            .last()
            .map_or(first_range.end, |&row| c.unit.ast.token_range(row).end);
    let commas: Vec<u32> = window
        .filter(|&token| {
            let span = crate::rules::byte_span_of_token(c, token);
            c.lexeme(token) == "," && span.start >= start && span.end <= end
        })
        .collect();
    let mut groups: Vec<Vec<u32>> = Vec::new();
    for &row in rows {
        // The group key is the row's head token against the comma token
        // indices: operand tiles and container roots seal past their
        // follower (an assignment's span carries the trailing comma, a
        // send-group marker stamps over it) and completion order is not
        // monotonic in head tokens, so each row resolves its own segment
        // — the commas strictly before its head.
        let head = c.head_token(row);
        let index = commas.partition_point(|&token| token < head);
        while groups.len() <= index {
            groups.push(Vec::new());
        }
        groups[index].push(row);
    }
    groups.retain(|group| !group.is_empty());
    groups
}

/// The cascade section face (:192-198): a bare member-access section
/// whose member is a function type (a method tear-off — semantic or
/// same-file — or a getter/field declared with a written function type)
/// reports at the section; invocation and assignment sections stay
/// silent.
fn check_cascade(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let cascade = c.node();
    let kids = c.collect_direct(cascade);
    for &marker in &kids {
        if c.kind(marker) != NodeKind::CascadeAccess {
            continue;
        }
        let marker_span = c.byte_span(marker);
        let section: Vec<u32> = kids
            .iter()
            .copied()
            .filter(|&kid| {
                kid != marker && {
                    let span = c.byte_span(kid);
                    span.start >= marker_span.start && span.end <= marker_span.end
                }
            })
            .collect();
        // `..method()` and `..x = e` sections pass (:194 — only a
        // PropertyAccess section with a function type reports). The
        // `Send` marker sits in every section's run (the bare-name
        // send group tiles it beside the member identifier), so the
        // invocation read is the `Arguments` piece — a `NoArguments`
        // group is a bare access, not a call.
        if section
            .iter()
            .any(|&row| matches!(c.kind(row), NodeKind::Arguments | NodeKind::Assignment))
        {
            continue;
        }
        if cascade_section_function_typed(c, analysis, marker) {
            // The section marker's own span opens at the access's own
            // tile — pass the marker as its own run (a leaf's head is
            // its covered start).
            report(c, marker, &[marker]);
        }
    }
}

/// Whether the bare-access section's member is a function type
/// (:194 `staticType is FunctionType`): the semantic member walk first
/// (a `Method` entry is a tear-off — cross-file classes included), then
/// the same-file written-type channel (a method tear-off, or a
/// getter/field whose written type is a function type).
fn cascade_section_function_typed(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    marker: u32,
) -> bool {
    let name = c.lexeme(c.head_token(marker));
    // The target rows: the owner-slice pieces before the cascade row
    // (the statement tiling keeps the target beside the cascade).
    let Some(owner) = c.unit.ast.parent(c.node()) else {
        return false;
    };
    let owner_kids = c.collect_direct(owner);
    let Some(pos) = owner_kids.iter().position(|&kid| kid == c.node()) else {
        return false;
    };
    let target = slice::statement_run(c, &owner_kids[..pos]);
    let Some(&head) = target.first() else {
        return false;
    };
    // The cascade target is the send-group of the receiver itself —
    // `c..m` tiles `c`'s pieces as the `Identifier + NoTypeArguments +
    // NoArguments + Send` group; `access_run` strips the markers to
    // the receiver row. A `this` target reads the enclosing class-like
    // scope.
    let receiver_rows = slice::access_run(c, &target);
    let Some(&receiver) = receiver_rows.first() else {
        if c.kind(head) != NodeKind::ThisExpression {
            return false;
        }
        let Some(class_row) = enclosing_class(c, head) else {
            return false;
        };
        return member_function_typed(c, class_row, name);
    };
    // The semantic member face: a resolved `Method` entry is a
    // tear-off (cross-file classes included). A getter/field entry
    // falls through — the model carries no function-type `Ty`, so the
    // written-type face below answers what it can.
    if let Some(member) =
        analysis.member_of(query_site(c, receiver), analysis.strings().intern(name))
        && member.entry.kind == MemberKind::Method
    {
        return true;
    }
    let receiver_name = c.lexeme(c.head_token(receiver));
    let Some(class_name) = same_file::written_type(c, receiver, receiver_name) else {
        return false;
    };
    let Some(class_row) = same_file_declaration(c, class_name) else {
        return false;
    };
    member_function_typed(c, class_row, name)
}

/// Whether the same-file class `class_row` declares `name` as a method
/// or a function-typed getter/field (the `staticType is FunctionType`
/// face, :194).
fn member_function_typed(c: &Check<'_>, class_row: u32, name: &str) -> bool {
    for member in body_members(c, class_row) {
        match c.kind(member) {
            NodeKind::Method => {
                // The stamped-name read — a written return type tiles a
                // `TypeReference` `Identifier` ahead of the name.
                let Some(name_row) = crate::rules::member_name(c, member) else {
                    continue;
                };
                if c.lexeme(c.head_token(name_row)) != name {
                    continue;
                }
                match get_set_token(c, member) {
                    // A method tear-off is a function type (:194).
                    None => return true,
                    // A getter's written return type decides.
                    Some("get") => {
                        if let Some((type_row, _)) =
                            crate::rules::type_before(c, member, name_row)
                            && type_face(c, type_row).is_function_type
                        {
                            return true;
                        }
                        return false;
                    }
                    Some(_) => return false,
                }
            }
            NodeKind::Fields => {
                let mut written = None;
                for kid in c.collect_direct(member) {
                    match c.kind(kid) {
                        NodeKind::Type | NodeKind::FunctionType | NodeKind::VoidType => {
                            written = Some(kid);
                        }
                        NodeKind::Identifier
                            if c.field(kid, PayloadField::Context)
                                == Some(dartq_parser::IdentifierContext::FieldDeclaration as u32)
                                && c.lexeme(c.head_token(kid)) == name =>
                        {
                            return written
                                .is_some_and(|row| type_face(c, row).is_function_type);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    false
}

/// The right-operand run of the control-flow binary at `root`: the
/// container's window rows and owner-sibling pieces right of the
/// operator token (`slice::binary_operands`'s adopted shape, read from
/// the dispatch face).
fn binary_right_run(c: &Check<'_>, run: &[u32], root: u32) -> Vec<u32> {
    let Some(op_token) = c.field(root, PayloadField::Operator) else {
        return Vec::new();
    };
    let Some(token) = c.unit.tokens.get(op_token as usize) else {
        return Vec::new();
    };
    let op_start = token.byte_offset as usize;
    let mut right: Vec<u32> = run
        .iter()
        .copied()
        .filter(|&row| row != root && c.byte_span(row).start >= op_start && row > root)
        .collect();
    let window = c.children(root);
    right.extend(window.start..window.end);
    right.retain(|&row| c.byte_span(row).start >= op_start);
    right.sort_unstable();
    right
}

/// The then/else arm runs of the conditional at `root`: every row —
/// owner-sibling piece or child-window row — inside the arm's own byte
/// window. The arms tile as whole expression groups (a bare name is a
/// multi-row send group), so the windows split on the payload's `?`/`:`
/// tokens; the condition sits left of the `?` and enters neither window.
fn arm_runs(c: &Check<'_>, run: &[u32], root: u32) -> Vec<Vec<u32>> {
    let Some(question) = c.field(root, PayloadField::Question) else {
        return Vec::new();
    };
    let Some(colon) = c.field(root, PayloadField::Colon) else {
        return Vec::new();
    };
    let (Some(question_end), Some(colon_span)) = (
        c.unit
            .tokens
            .get(question as usize)
            .map(|t| (t.byte_offset + t.byte_len) as usize),
        c.unit
            .tokens
            .get(colon as usize)
            .map(|t| (t.byte_offset as usize, (t.byte_offset + t.byte_len) as usize)),
    ) else {
        return Vec::new();
    };
    // Membership is start-anchored: the chain tiles seal past their
    // follower (a member-selector row stamps over the colon), so an
    // end-bound would drop the row that owns the arm's verdict.
    let inside = |row: u32, lo: usize, hi: usize| {
        let span = c.byte_span(row);
        span.start >= lo && span.start < hi
    };
    let window = c.children(root);
    let root_end = c.byte_span(root).end;
    [(question_end, colon_span.0), (colon_span.1, root_end)]
        .into_iter()
        .map(|(lo, hi)| {
            let mut rows: Vec<u32> = run
                .iter()
                .copied()
                .filter(|&row| row != root && inside(row, lo, hi))
                .collect();
            rows.extend((window.start..window.end).filter(|&row| inside(row, lo, hi)));
            rows.sort_unstable();
            rows
        })
        .collect()
}
