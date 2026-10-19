use super::{Check, Group, NodeKind, RuleSpec};

/// The `external` `ModSet` bit (bridge `mod_bits`, member family).
const EXTERNAL_BIT: u16 = 1 << 2;
/// The `augment` `ModSet` bit (bridge `mod_bits`).
const AUGMENT_BIT: u16 = 1 << 1;
/// The `var`/`final`/`const` `ModSet` bit (bridge `mod_bits`) — a
/// declaring primary-constructor parameter.
const VAR_FINAL_OR_CONST_BIT: u16 = 1 << 6;

/// Official: `pkg/linter/lib/src/rules/avoid_unused_constructor_parameters.dart`
/// — a constructor parameter that is no field/super formal (:50-51),
/// carries no `@deprecated` metadata (:53), is not named `_`… (:54),
/// and is never referenced in the body or the initializers (:100-107)
/// reports at the parameter. Augmented (:68), external (:70) and
/// redirecting-factory (`redirectedConstructor != null`) constructors
/// skip. The reference scan is the official
/// `visitSimpleIdentifier` element face over this constructor's
/// tokens: an identifier lexeme after the parameter list counts as a
/// use, so a shadowing local over-counts (the documented user-shadow
/// family). The primary-constructor face (:81-93) scans the primary
/// body (initializers and statements) and the class fields'
/// initializers only — method bodies never count, matching the
/// official. Message: `diagnostic.g.dart`
/// `avoidUnusedConstructorParameters`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "avoid_unused_constructor_parameters",
    group: Group::Standalone,
    problem: "The parameter '{0}' is not used in the constructor.",
    correction: "Try using the parameter or removing it.",
    kinds: &[NodeKind::Constructor, NodeKind::PrimaryConstructor],
    check,
};

fn check(c: &mut Check<'_>) {
    match c.kind(c.node()) {
        NodeKind::Constructor => check_constructor(c),
        NodeKind::PrimaryConstructor => check_primary(c),
        _ => {}
    }
}

fn check_constructor(c: &mut Check<'_>) {
    let node = c.node();
    let flags = dartq_ast::mod_flags(c.unit.ast, node);
    if flags & (EXTERNAL_BIT | AUGMENT_BIT) != 0 {
        return;
    }
    // A redirecting factory (`factory C(int p) = C.named;`) skips —
    // the official `node.redirectedConstructor != null` bail
    // (`test_redirectingConstructor1` reports only the target's
    // parameters, never the redirect's own).
    if c.collect_direct(node)
        .into_iter()
        .any(|kid| c.kind(kid) == NodeKind::RedirectingFactoryBody)
    {
        return;
    }
    let Some(params) = parameter_list(c, node) else {
        return;
    };
    // The reference surface: the constructor's tokens past the
    // parameter list — the initializers and the body (:100-107).
    let scan = crate::rules::tail_token(c, params) + 1..crate::rules::tail_token(c, node) + 1;
    report_unused(c, parameters(c, params), &[scan]);
}

fn check_primary(c: &mut Check<'_>) {
    let node = c.node();
    let Some(&class_row) = c.path.iter().rev().find(|&&a| {
        matches!(
            c.kind(a),
            NodeKind::ClassDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::ExtensionTypeDeclaration
        )
    }) else {
        return;
    };
    // The use surface: the primary body (:90) plus the class fields'
    // initializers (:109-112).
    let mut scans: Vec<core::ops::Range<u32>> = Vec::new();
    for member in super::body_members(c, class_row) {
        match c.kind(member) {
            NodeKind::PrimaryConstructorBody => {
                // The whole body row: `this`, the `:` and the
                // initializer tails are outside the parameter list, so
                // the parameter names only collide with real uses.
                let range = c.unit.ast.token_range(member);
                scans.push(range.start..range.end);
            }
            NodeKind::Fields => collect_field_scans(c, member, &mut scans),
            _ => {}
        }
    }
    let Some(params) = parameter_list(c, node) else {
        return;
    };
    report_unused(c, parameters(c, params), &scans);
}

/// The parameter list row of a constructor-like declaration.
fn parameter_list(c: &Check<'_>, node: u32) -> Option<u32> {
    c.collect_direct(node).into_iter().find(|&kid| {
        matches!(
            c.kind(kid),
            NodeKind::FormalParameters | NodeKind::OptionalFormalParameters
        )
    })
}

/// The `= …` initializer token ranges inside a `Fields` row (one per
/// declared variable).
fn collect_field_scans(c: &Check<'_>, fields: u32, out: &mut Vec<core::ops::Range<u32>>) {
    for decl in c.collect_direct(fields) {
        if c.kind(decl) != NodeKind::VariablesDeclaration {
            continue;
        }
        for var in c.collect_direct(decl) {
            if c.kind(var) != NodeKind::InitializedIdentifier {
                continue;
            }
            let Some(name) = crate::rules::initialized_identifier_name(c, var) else {
                continue;
            };
            let range = c.unit.ast.token_range(var);
            // The initializer opens at the `=` — the first token after
            // the name, when the variable carries one.
            if range.contains(&(name + 1)) {
                out.push(name + 1..range.end);
            }
        }
    }
}

/// The formal parameter rows under a parameter list (the optional
/// group nests one level).
fn parameters(c: &Check<'_>, params: u32) -> Vec<u32> {
    let mut rows = Vec::new();
    for kid in c.collect_direct(params) {
        match c.kind(kid) {
            NodeKind::FormalParameter | NodeKind::FunctionTypedFormalParameter => {
                rows.push(kid);
            }
            NodeKind::OptionalFormalParameters | NodeKind::FormalParameters => {
                rows.extend(parameters(c, kid));
            }
            _ => {}
        }
    }
    rows
}

/// The shared verdict: the exempt faces drop out, the survivors are
/// token-scanned over the use surfaces, and each unused name reports
/// at its parameter row (:115-117).
fn report_unused(c: &mut Check<'_>, params: Vec<u32>, scans: &[core::ops::Range<u32>]) {
    for param in params {
        // The parameter's own annotation tiles beside the row: a
        // `@Deprecated` star precedes its parameter under the formal
        // parameter list (:53 — `element.metadata.hasDeprecated` on the
        // parameter, never the constructor's).
        let Some(param_parent) = c.unit.ast.parent(param) else {
            continue;
        };
        let param_start = c.unit.ast.token_range(param).start;
        let deprecated = c.collect_direct(param_parent).into_iter().any(|sib| {
            c.kind(sib) == NodeKind::MetadataStar
                && c.unit.ast.token_range(sib).end == param_start
                && metadata_names_deprecated(c, sib)
        });
        if deprecated {
            continue;
        }
        // Field/super formals are the initializer faces (:50-51); a
        // declaring primary parameter (`var`/`final`) declares a field.
        let receiver = c.payload(param).first().copied();
        if receiver.is_some_and(|tok| tok != super::NONE_TOKEN)
            || dartq_ast::mod_flags(c.unit.ast, param) & VAR_FINAL_OR_CONST_BIT != 0
        {
            continue;
        }
        let Some(name_token) = c
            .payload(param)
            .get(1)
            .copied()
            .filter(|&tok| tok != super::NONE_TOKEN)
        else {
            continue;
        };
        let name = c.lexeme(name_token);
        // Wildcard names exempt unconditionally (:52-54).
        if crate::rules::is_just_underscores(name) {
            continue;
        }
        let used = scans
            .iter()
            .flat_map(std::clone::Clone::clone)
            .any(|token| c.lexeme(token) == name);
        if !used {
            c.report(c.utf16_span(param), &[name]);
        }
    }
}

/// Whether the metadata-star row carries a `Deprecated`/`deprecated`
/// annotation (the official `element.metadata.hasDeprecated`).
fn metadata_names_deprecated(c: &Check<'_>, star: u32) -> bool {
    c.collect_direct(star).into_iter().any(|meta| {
        c.kind(meta) == NodeKind::Metadata
            && c.collect_direct(meta).into_iter().any(|piece| {
                c.kind(piece) == NodeKind::Identifier
                    && matches!(c.lexeme(c.head_token(piece)), "Deprecated" | "deprecated")
            })
    })
}
