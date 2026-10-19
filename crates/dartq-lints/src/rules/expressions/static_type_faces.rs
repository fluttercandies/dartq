use dartq_semantic::model::Ty;

use super::{Check, NodeKind, query_site, slice};

/// Reads through transparent `(...)` wrappers to the expression root
/// row (the content is the wrapper window's last row).
pub(crate) fn unparenthesized(c: &Check<'_>, mut root: u32) -> u32 {
    while c.kind(root) == NodeKind::ParenthesizedExpression {
        let window = c.children(root);
        if window.start >= window.end {
            break;
        }
        root = window.end - 1;
    }
    root
}

/// The expression root of a piece run: containers complete after their
/// subtree, so the last row roots the run; wrappers read through.
pub(crate) fn run_root(c: &Check<'_>, run: &[u32]) -> Option<u32> {
    Some(unparenthesized(c, *run.last()?))
}

/// Whether the run's unparenthesized root is an explicit `as` cast —
/// the upstream `_isExplicitCast` bail (`node?.unParenthesized is
/// AsExpression`) every dynamic-consumer arm reads.
pub(crate) fn is_explicit_cast(c: &Check<'_>, run: &[u32]) -> bool {
    run_root(c, run).is_some_and(|root| c.kind(root) == NodeKind::AsExpression)
}

/// The expression's static type over its piece run — the M1 faces:
/// an identifier read, a bare send (an invocation or a tear-off
/// reference, both answered through the callee face), and a
/// single-name member access. Everything else — chains beyond the
/// first hop, creations, literals — carries no face and answers
/// `None` (the under-report direction every caller keeps silent on).
pub(crate) fn expr_type(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    run: &[u32],
) -> Option<Ty> {
    let root = run_root(c, run)?;
    if c.kind(root) == NodeKind::Send {
        return send_type(c, analysis, run);
    }
    // The byte-split operand run can be the bare identifier alone
    // (the group wrappers anchor forward and split right) — the
    // plain flow-row read.
    if c.kind(root) == NodeKind::Identifier {
        return analysis.type_of(query_site(c, root));
    }
    // A bare-name group whose Send was split off (the operand run's
    // byte split leaves the operator-straddling Send on the right) —
    // the plain flow-row read at the run's last identifier.
    if slice::is_send_group_piece(c, root) {
        let stripped = slice::access_run(c, run);
        let last = stripped.last()?;
        return (c.kind(*last) == NodeKind::Identifier)
            .then_some(analysis.type_of(query_site(c, *last)))?;
    }
    // A trailing whole-access marker without an invocation is the
    // bare property-access tile.
    access_type(c, analysis, run)
}

/// The send face: the selector identifier sits before the argument
/// list; a trailing whole-access marker proves a member receiver. A
/// bare name's tear-off group splits across an owner boundary (its
/// `NoArguments` piece can tile outside the run), so `send_parts`
/// fails on it — the single-`Identifier` prefix is a plain read and
/// answers at the flow row (`type_of`), not the callee face.
fn send_type(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    run: &[u32],
) -> Option<Ty> {
    let pos = run.iter().rposition(|&row| c.kind(row) == NodeKind::Send)?;
    let plain_read = |c: &Check<'_>| {
        // The plain read answers at the flow row (`type_of`) of the
        // run's last identifier — the bare name the group carries
        // (earlier statements' pieces may precede it in an owner
        // window).
        let prefix = slice::access_run(c, &run[..pos]);
        let Some(&last) = prefix.last() else {
            return None;
        };
        if c.kind(last) != NodeKind::Identifier {
            return None;
        }
        analysis.type_of(query_site(c, last))
    };
    let (head, arguments) = match slice::send_parts(c, run, pos) {
        Some((head, arguments)) => (head, arguments),
        None => return plain_read(c),
    };
    // A `NoArguments` send without a trailing access marker is the
    // bare-name tear-off — the flow-row plain read, not a callee
    // face. With the marker it is the property-access tile and the
    // member face below answers (the getter's type).
    let marker = slice::access_marker(c, run, pos, NodeKind::DotAccess);
    if arguments.is_none() && marker.is_none() {
        return plain_read(c);
    }
    let selector_row = run[head];
    let selector = c.head_token(selector_row);
    if marker.is_some() {
        let receiver = single_receiver(c, &run[..head], selector)?;
        let name = analysis.strings().intern(c.lexeme(selector));
        return analysis.member_type(query_site(c, receiver), name);
    }
    analysis.callee_type(query_site(c, selector_row))
}

/// The bare property-access face (`a.b`): the selector rides on the
/// whole-access marker's last token; the receiver must be one exact
/// name directly before it.
fn access_type(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    run: &[u32],
) -> Option<Ty> {
    let pos = run
        .iter()
        .rposition(|&row| c.kind(row) == NodeKind::DotAccess)?;
    // The whole-access marker's payload word 0 is the dot token; the
    // member name is the token right after it (the trailing piece rides
    // `name.`'s close, so `token_range.next_back` lands on the dot, not
    // the name).
    let selector = c.payload(run[pos]).first().copied()? + 1;
    let receiver = single_receiver(c, &run[..pos], selector)?;
    let name = analysis.strings().intern(c.lexeme(selector));
    analysis.member_type(query_site(c, receiver), name)
}

/// The exact single-name receiver of a selector: one identifier piece
/// whose own token sits directly before the selector's dot (`a.m` —
/// a chain's head strands extra tokens in between and answers `None`,
/// the documented chain under-report). The selector's own send group
/// (`a.b`'s `b` rides a `[Identifier, NoTypeArguments, NoArguments,
/// Send]` tail) leads the slice — drop a trailing group whose head is
/// the selector token before reading the receiver.
pub(crate) fn single_receiver(c: &Check<'_>, run: &[u32], selector: u32) -> Option<u32> {
    let mut run = slice::access_run(c, run);
    // The selector's own send tail reduces to its `Identifier` after
    // the access-piece strip: a property access rides
    // `[Identifier, NoTypeArguments, NoArguments, Send]`, an
    // invocation `[Identifier, NoTypeArguments, Arguments, Send]` —
    // the Arguments row survives the strip and drops here beside the
    // selector (every row from the selector token onward is the
    // selector's own tail; earlier calls' rows anchor below it).
    while let Some(&last) = run.last() {
        if c.kind(last) == NodeKind::Identifier && c.head_token(last) == selector {
            run.pop();
            continue;
        }
        if c.unit.ast.token_range(last).start > selector {
            run.pop();
            continue;
        }
        break;
    }
    // The receiver is the one exact name directly before the
    // selector's dot: the last surviving piece, an `Identifier` whose
    // token sits at `selector - 2`. Everything before it (a method
    // header's return-type strays, a chain's stranded call) stays out
    // of the verdict — the documented chain under-report.
    let Some(&last) = run.last() else {
        return None;
    };
    if c.kind(last) != NodeKind::Identifier {
        return None;
    }
    let receiver = c.head_token(last);
    (receiver + 2 == selector).then_some(last)
}

/// The upstream `_reportIfDynamicOrFunction` verdict: the type is
/// `dynamic` or the bare `Function` type.
pub(crate) fn is_dynamic_or_function(analysis: &dartq_semantic::Analysis<'_>, ty: Ty) -> bool {
    match ty {
        Ty::Dynamic => true,
        Ty::Named { name, .. } => analysis.strings().resolve(name) == "Function",
        _ => false,
    }
}

/// Whether `ty` classifies nullable (`Ty::Null` or a `?` projection) —
/// `dynamic` stays out (the official `isNullable` callers here exclude
/// `DynamicType` explicitly before the nullable read). The `Record`/
/// `Function` arms are nominal: the flow `Base` has neither record nor
/// function base, so the read-type channel never produces those
/// variants today — the disjunct keeps the predicate total for the
/// faces that can carry one.
pub(crate) fn is_nullable(ty: Ty) -> bool {
    matches!(
        ty,
        Ty::Null
            | Ty::Named { nullable: true, .. }
            | Ty::Record { nullable: true, .. }
            | Ty::Function { nullable: true, .. }
    )
}

/// The written-target gate the implicit-cast contexts share: the type
/// must decide, and it is neither `dynamic`/`void` nor `Object?` (the
/// official `_check` exemptions).
pub(crate) fn typed_non_dynamic_target(
    analysis: &dartq_semantic::Analysis<'_>,
    ty: Ty,
) -> bool {
    match ty {
        Ty::Named { name, nullable, .. } => {
            let name = analysis.strings().resolve(name);
            name != "dynamic" && name != "void" && !(name == "Object" && nullable)
        }
        _ => false,
    }
}

// —— the typed-receiver family face ————————————————————————————————

/// The dart:core family bits of a written type name — the same table
/// the shared literal face keys (`slice::written_family_bits`):
/// `Iterable`/`List`/`Set`/dart:collection's `Queue` carry
/// [`slice::ITERABLE`], `String` carries [`slice::STRING`], and the map
/// family carries [`slice::MAP`] (never requested by an instance
/// receiver gate).
fn core_family_bits(name: &str) -> u8 {
    match name {
        "Iterable" | "List" | "Set" | "Queue" => slice::ITERABLE,
        "String" => slice::STRING,
        "Map" | "LinkedHashMap" | "LinkedHashSet" => slice::MAP,
        _ => 0,
    }
}

/// The family verdict of a bridge type against `required`: a
/// same-file declaration of the name walks its written supertype
/// clauses (the user-subclass faces), an unshadowed name is the core
/// type, and everything else (a cross-file class, `dynamic`, an
/// unknown) carries no verdict — the caller keeps its bail face.
pub(crate) fn ty_family(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    ty: Ty,
    required: u8,
) -> Option<bool> {
    let Ty::Named { name, .. } = ty else {
        return None;
    };
    let spelled = analysis.strings().resolve(name);
    if crate::rules::expressions::same_file::declares_type(c, spelled) {
        return match crate::rules::expressions::same_file::resolve(
            c,
            spelled,
            &[
                "Iterable",
                "List",
                "Set",
                "Queue",
                "String",
                "Map",
                "LinkedHashMap",
                "LinkedHashSet",
            ],
        ) {
            crate::rules::expressions::same_file::Verdict::Core(core) => {
                Some(core_family_bits(core) & required != 0)
            }
            crate::rules::expressions::same_file::Verdict::NotCore => Some(false),
            crate::rules::expressions::same_file::Verdict::Unknown => None,
        };
    }
    let bits = core_family_bits(spelled);
    (bits != 0).then(|| bits & required != 0)
}

/// The member-receiver family face (`this.x.m(…)`, `a.b.m(…)`): the
/// receiver's static type through the bridge — `this.x` reads the
/// same-class field's written type (no read row exists for `this`),
/// a single-name access resolves the member's declared type. `None`
/// when the receiver shape or the type carries no verdict — the
/// caller keeps its documented bail face.
pub(crate) fn member_receiver_family(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    receiver_run: &[u32],
    required: u8,
) -> Option<bool> {
    // The `this.x` face: the stripped run is the lone `this` and the
    // trailing whole-access marker names the field.
    let stripped = slice::access_run(c, receiver_run);
    if let [single] = stripped.as_slice()
        && c.kind(*single) == NodeKind::ThisExpression
    {
        let marker = receiver_run
            .iter()
            .rev()
            .find(|&&row| c.kind(row) == NodeKind::DotAccess)?;
        let selector = c.unit.ast.token_range(*marker).next_back()?;
        let face = crate::rules::type_face_of_field(c, c.lexeme(selector))?;
        let name = analysis.strings().intern(face.name?);
        return ty_family(
            c,
            analysis,
            dartq_semantic::model::Ty::named(name),
            required,
        );
    }
    // A single-name member access: the access face over the run.
    let ty = access_type(c, analysis, receiver_run)?;
    ty_family(c, analysis, ty, required)
}

/// The receiver family verdict over the operand pieces before the
/// selector piece at `name` — the shared typed-receiver gate
/// (`prefer_contains` :143-149, `prefer_is_empty` :229-234,
/// `avoid_function_literals_in_foreach_calls` :40-45): literal faces
/// decide closed; a bare-identifier receiver — including one revealed
/// by expanding transparent `(...)`/`as` wrappers (the official
/// `_drillDownTo` walk) — resolves its written type through the
/// same-file channel; a member receiver reads the bridge static type
/// when mounted. `None` carries no verdict; the caller applies its own
/// documented bail/presumption.
pub(crate) fn receiver_family(
    c: &Check<'_>,
    analysis: Option<&dartq_semantic::Analysis<'_>>,
    pieces: &[u32],
    name: usize,
    required: u8,
) -> Option<bool> {
    if let Some(faces) = slice::receiver_faces(c, pieces, name) {
        return Some(faces & required != 0);
    }
    if let Some(decides) = slice::resolved_receiver_family(c, pieces, name, required) {
        return Some(decides);
    }
    let run = expand_wrappers(c, &pieces[..name]);
    // A wrapper-revealed bare name reads its written type.
    if let [only] = run.as_slice()
        && c.kind(*only) == NodeKind::Identifier
    {
        let receiver_name = c.lexeme(c.head_token(*only));
        let written =
            crate::rules::expressions::same_file::written_type(c, *only, receiver_name)?;
        if crate::rules::expressions::same_file::declares_type(c, written) {
            return Some(false);
        }
        let bits = core_family_bits(written);
        return (bits != 0).then(|| bits & required != 0);
    }
    member_receiver_family(c, analysis?, &run, required)
}

/// Expands the transparent `(...)` / `as` wrapper rows of an operand
/// run in place (the official `_drillDownTo(ignoreParens, ignoreAs)`):
/// each wrapper's child window replaces the wrapper row.
fn expand_wrappers(c: &Check<'_>, pieces: &[u32]) -> Vec<u32> {
    let mut run: Vec<u32> = pieces.to_vec();
    loop {
        let expand = match run.last() {
            Some(&last) => matches!(
                c.kind(last),
                NodeKind::ParenthesizedExpression | NodeKind::AsExpression
            ),
            None => false,
        };
        if !expand {
            return run;
        }
        let &last = run.last().expect("checked above");
        let inner = c.collect_direct(last);
        if inner.is_empty() {
            return run;
        }
        run.truncate(run.len() - 1);
        run.extend(inner);
    }
}

/// The receiver's static type NAME (the `null_closures` instance face):
/// `this.x` reads the same-class field's written type head; any other
/// shape reads the bridge expression face — a single-name access
/// resolves the member's declared type, a bare send its callee return
/// (`getC().map(null)`). The spelled name only — the method tables
/// match by written core name.
pub(crate) fn receiver_type_name(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    receiver_run: &[u32],
) -> Option<String> {
    let stripped = slice::access_run(c, receiver_run);
    // `this.<field>` tiles two ways: the selector rides the access
    // marker (no row — the stripped run is the lone `this`), or it
    // carries its own identifier row beside the `this`. Both read the
    // same-class field's written type (a `this.` target cannot be
    // shadowed).
    match stripped.as_slice() {
        [single] if c.kind(*single) == NodeKind::ThisExpression => {
            let marker = receiver_run
                .iter()
                .rev()
                .find(|&&row| c.kind(row) == NodeKind::DotAccess)?;
            let selector = c.unit.ast.token_range(*marker).next_back()?;
            let face = crate::rules::type_face_of_field(c, c.lexeme(selector))?;
            return Some(face.name?.to_owned());
        }
        [target, name]
            if c.kind(*target) == NodeKind::ThisExpression
                && c.kind(*name) == NodeKind::Identifier =>
        {
            let face = crate::rules::type_face_of_field(c, c.lexeme(c.head_token(*name)))?;
            return Some(face.name?.to_owned());
        }
        _ => {}
    }
    match expr_type(c, analysis, receiver_run)
        .or_else(|| access_type(c, analysis, receiver_run))
    {
        Some(Ty::Named { name, .. }) => Some(analysis.strings().resolve(name).to_owned()),
        _ => None,
    }
}
