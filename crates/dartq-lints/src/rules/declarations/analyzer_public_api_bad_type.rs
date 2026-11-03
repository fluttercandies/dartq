use dartq_ast::{NodeKind, PayloadField};

use crate::engine::Check;
use crate::registry::RuleSpec;
use crate::rules::{body_members, initialized_identifier_name, same_file_declaration, supertypes};

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/analyzer_public_api.dart` — one code of the
/// internal `analyzer_public_api` multi-rule (:32-38). A top-level
/// fragment in the analyzer public API reports the names of the types it
/// exposes when those types are not themselves public API
/// (`_checkTopLevelFragment`/`_checkMember`/`_checkType` :138-262): the
/// supertype clauses, member signature types, type-parameter bounds and
/// aliased types walk `_problemsForAnalyzerPublicApi` (:357-405), whose
/// problematic element is one neither visible through a public import
/// nor `isOkForAnalyzerPublicApi`. The report anchors at the fragment's
/// name (`_offsetAndLengthForFragment` :325-356).
/// Message: `diagnostic.g.dart:229-237`.
///
/// Approximation register: `isInAnalyzerPublicApi` (:442-463) reduces to
/// the `@AnalyzerPublicApi()` annotation arm — the `package:analyzer/*`
/// library-URI leg has no mounted face (the lint unit carries no package
/// URI), so public-named unannotated declarations stay silent (the
/// corpus' only firing surface is the annotation opt-in). A type name is
/// problematic only when positively proven user code: a same-file
/// class-like/typedef declaration of the name lacking the annotation.
/// Cross-file declarations (the resolution face carries no foreign
/// annotation rows), prefixed spellings, unresolved names (the official
/// `InvalidType` arm) and SDK bindings all under-report — never a wrong
/// row. Enum constructors stay skipped upstream (:142-147) and so do
/// private-named members (:149-152).
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "analyzer_public_api_bad_type",
    &[
        NodeKind::ClassDeclaration,
        NodeKind::MixinDeclaration,
        NodeKind::EnumDeclaration,
        NodeKind::ExtensionDeclaration,
        NodeKind::ExtensionTypeDeclaration,
        NodeKind::NamedMixinApplication,
        NodeKind::Typedef,
        NodeKind::TopLevelFields,
        NodeKind::TopLevelMethod,
    ],
    check,
);

fn check(c: &mut Check<'_>) {
    let decl = c.node();
    if !annotated_public_api(c, decl) {
        return;
    }
    match c.kind(decl) {
        NodeKind::TopLevelFields => {
            let vars: Vec<u32> = c
                .collect_direct(decl)
                .into_iter()
                .flat_map(|row| {
                    if c.kind(row) == NodeKind::InitializedIdentifier {
                        vec![row]
                    } else {
                        c.collect_direct(row)
                            .into_iter()
                            .filter(|&var| c.kind(var) == NodeKind::InitializedIdentifier)
                            .collect()
                    }
                })
                .collect();
            for var in vars {
                if let Some(name) = initialized_identifier_name(c, var) {
                    report_problems(c, written_types(c, decl), name);
                }
            }
        }
        NodeKind::Typedef | NodeKind::TopLevelMethod => {
            if let Some(name) = decl_name(c, decl) {
                report_problems(c, written_types(c, decl), name);
            }
        }
        _ => {
            let Some(name) = decl_name(c, decl) else {
                return;
            };
            // The instance-fragment face: supertype clauses and the
            // declaration's own type-parameter bounds plus the public
            // members' signature types (upstream's `supertype`/
            // `interfaces`/`mixins`/`extendedType`/`superclassConstraints`
            // + `_checkTypeParameter` + `_checkMember` walks).
            let mut types: Vec<String> = supertypes(c, decl)
                .into_iter()
                .map(|(name, _)| name.to_owned())
                .collect();
            for vars in c
                .collect_direct(decl)
                .into_iter()
                .filter(|&kid| c.kind(kid) == NodeKind::TypeVariables)
            {
                types.extend(written_types(c, vars));
            }
            report_problems(c, types, name);
            check_members(c, decl);
        }
    }
}

/// Whether the top-level declaration carries the `@AnalyzerPublicApi`
/// opt-in (the `metadata.annotations.any(_isPublicApiAnnotation)` arm —
/// read by spelling like the upstream test fixtures' same-name class).
fn annotated_public_api(c: &Check<'_>, decl: u32) -> bool {
    super::top_level_annotations(c, decl)
        .into_iter()
        .any(|meta| super::annotation_names(c, meta, "AnalyzerPublicApi"))
}

/// The declaration's name token: the `Name` payload word (the
/// extension/extension-type face patches the name onto the row), else
/// the `Identifier` carrying the matching declaration context — direct
/// child of the row or its left sibling under the `TopLevelDeclaration`
/// wrapper (the production tiling keeps the name beside the class-like
/// row).
fn decl_name(c: &Check<'_>, decl: u32) -> Option<u32> {
    if matches!(
        c.kind(decl),
        NodeKind::ExtensionDeclaration | NodeKind::ExtensionTypeDeclaration
    ) && let Some(name) = c.field_token(decl, PayloadField::Name)
    {
        return Some(name);
    }
    const CONTEXTS: [u32; 4] = [
        dartq_parser::IdentifierContext::ClassOrMixinOrExtensionDeclaration as u32,
        dartq_parser::IdentifierContext::EnumDeclaration as u32,
        dartq_parser::IdentifierContext::TypedefDeclaration as u32,
        dartq_parser::IdentifierContext::TopLevelFunctionDeclaration as u32,
    ];
    let named = |kid: u32| {
        c.kind(kid) == NodeKind::Identifier
            && c.field(kid, PayloadField::Context)
                .is_some_and(|ctx| CONTEXTS.contains(&ctx))
    };
    if let Some(name) = c.collect_direct(decl).into_iter().find(|&kid| named(kid)) {
        return Some(c.head_token(name));
    }
    let wrapper = c.unit.ast.parent(decl)?;
    let siblings = c.collect_direct(wrapper);
    let pos = siblings.iter().position(|&kid| kid == decl)?;
    siblings[..pos]
        .iter()
        .rev()
        .copied()
        .find(|&kid| named(kid))
        .map(|name| c.head_token(name))
}

/// The public members' type surfaces of an annotated class-like row —
/// each checked independently at the member's own name (upstream's
/// `_checkMember` reports at the member fragment). Private names,
/// operators and enum constructors take the official skips.
fn check_members(c: &mut Check<'_>, decl: u32) {
    if c.kind(decl) == NodeKind::ExtensionTypeDeclaration {
        extension_type_faces(c, decl);
        return;
    }
    for member in body_members(c, decl) {
        if c.kind(member) == NodeKind::EnumElement {
            continue; // enum constructors/constants aren't public API
        }
        for name in member_name_rows(c, member) {
            if c.lexeme(name).starts_with('_') {
                continue;
            }
            report_problems(c, written_types(c, member), name);
        }
    }
}

/// The extension-type face: the primary constructor's signature types
/// report at the declaration name (`typeNameOffset`), the representation
/// field's type again at the parameter's own name — the implicit getter's
/// `element.type` face (`C(B b)` reports at `C` and at `b`).
fn extension_type_faces(c: &mut Check<'_>, decl: u32) {
    let Some(ctor) = c
        .collect_direct(decl)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::PrimaryConstructor)
    else {
        return;
    };
    if let Some(name) = decl_name(c, decl) {
        report_problems(c, written_types(c, ctor), name);
    }
    for param in parameter_rows_of(c, ctor) {
        if let Some(name) = param_name_token(c, param) {
            report_problems(c, written_types(c, param), name);
        }
    }
}

/// The `FormalParameter` rows inside a member/primary-constructor row.
fn parameter_rows_of(c: &Check<'_>, member: u32) -> Vec<u32> {
    let window = c.children(member);
    (window.start..window.end)
        .filter(|&row| c.kind(row) == NodeKind::FormalParameter)
        .collect()
}

/// A formal parameter's written name token — the `Name` payload word
/// (`fields.rs` `FormalParameter`), falling back to its last `Identifier`
/// head for the row's hand-built face.
fn param_name_token(c: &Check<'_>, param: u32) -> Option<u32> {
    c.field_token(param, PayloadField::Name).or_else(|| {
        let window = c.children(param);
        (window.start..window.end)
            .rev()
            .find(|&row| c.kind(row) == NodeKind::Identifier)
            .map(|row| c.head_token(row))
    })
}

/// The member's name-bearing rows: the `MethodDeclaration`-context
/// `Identifier` (methods), the `InitializedIdentifier` name tokens
/// (fields — one level down the `VariablesDeclaration`), the
/// `OperatorName` row (operators). Read off the direct rows only — a
/// method body's own locals tile in the subtree and are not members.
fn member_name_rows(c: &Check<'_>, member: u32) -> Vec<u32> {
    c.collect_direct(member)
        .into_iter()
        .flat_map(|row| match c.kind(row) {
            NodeKind::Identifier
                if matches!(
                    c.field(row, PayloadField::Context),
                    Some(ctx)
                        if ctx == dartq_parser::IdentifierContext::MethodDeclaration as u32
                            || ctx == dartq_parser::IdentifierContext::MethodDeclarationContinuation
                                as u32
                ) =>
            {
                vec![c.head_token(row)]
            }
            NodeKind::VariablesDeclaration => c
                .collect_direct(row)
                .into_iter()
                .filter(|&var| c.kind(var) == NodeKind::InitializedIdentifier)
                .filter_map(|var| initialized_identifier_name(c, var))
                .collect(),
            NodeKind::OperatorName => c.field_token(row, PayloadField::Name).into_iter().collect(),
            _ => Vec::new(),
        })
        .collect()
}

/// The written type names inside a row's subtree, minus its body,
/// initializer and metadata regions (upstream walks the fragment's
/// `element.type` — signature surfaces only, expression bodies never).
/// Qualified `p.N` spellings carry no bare name (a `.` in the leaf's run
/// marks the prefixed face — under-report, like every non-same-file
/// name).
fn written_types(c: &Check<'_>, row: u32) -> Vec<String> {
    const SKIP: [NodeKind; 8] = [
        NodeKind::BlockBody,
        NodeKind::ExpressionBody,
        NodeKind::EmptyBody,
        NodeKind::NoBody,
        NodeKind::SkippedBody,
        NodeKind::Initializers,
        NodeKind::FieldInitializer,
        NodeKind::MetadataStar,
    ];
    let window = c.children(row);
    // The body/initializer/metadata subtrees tile inside the window
    // (post-order rows precede their container) — collect the regions
    // first, then keep only the `Type` rows outside them.
    let skipped: Vec<core::ops::Range<u32>> = (window.start..window.end)
        .filter(|&piece| SKIP.contains(&c.kind(piece)))
        .map(|piece| {
            let sub = c.children(piece);
            sub.start..piece
        })
        .collect();
    let mut names = Vec::new();
    for piece in window.start..window.end {
        if skipped.iter().any(|region| region.contains(&piece)) {
            continue;
        }
        if c.kind(piece) != NodeKind::Type {
            continue;
        }
        if let Some(name) = type_name(c, piece) {
            names.push(name);
        }
    }
    names
}

/// The bare name of a `Type` leaf — `None` for a qualified `p.N`
/// spelling (the `.` rides the same leaf run) or a keyword head
/// (`Function`, `void`-family spellings that emit `Type` rows).
fn type_name(c: &Check<'_>, row: u32) -> Option<String> {
    let range = c.unit.ast.token_range(row);
    let mut name = None;
    for token in range.start..range.end {
        let entry = c.unit.tokens.get(token as usize)?;
        match entry.kind {
            dartq_scanner::TokenKind::Identifier => name = Some(token),
            dartq_scanner::TokenKind::Period => return None,
            _ => {}
        }
    }
    name.map(|token| c.lexeme(token).to_owned())
}

/// Whether `name` is user code outside the analyzer public API — the
/// proven-positive face: a same-file class-like/typedef declaration of
/// the spelling that carries no `@AnalyzerPublicApi` itself (upstream's
/// `!_isPubliclyImported && !isOkForAnalyzerPublicApi` reduced to the
/// same-file leg — every other resolution class under-reports).
fn problematic(c: &Check<'_>, name: &str) -> bool {
    let Some(decl) = same_file_declaration(c, name) else {
        return false;
    };
    !annotated_public_api(c, decl)
}

/// Reports `names`'s deduplicated problematic set at `at` (upstream
/// joins the `_problemsForAnalyzerPublicApi` set with `, `).
fn report_problems(c: &mut Check<'_>, names: Vec<String>, at: u32) {
    let mut problems: Vec<String> = Vec::new();
    for name in names {
        if !problems.contains(&name) && problematic(c, &name) {
            problems.push(name);
        }
    }
    if problems.is_empty() {
        return;
    }
    let joined = problems.join(", ");
    let Some(end) = c.utf16_end_of_token(at) else {
        return;
    };
    let Some(start) = c.utf16_of_token(at) else {
        return;
    };
    c.report(start..end, &[&joined]);
}

#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

    const API: &str = "class AnalyzerPublicApi {\n  const AnalyzerPublicApi();\n}\n\nclass B {}\n";

    /// The `*_notIgnoredIfAnnotatedPublic` face over one class-like
    /// declaration kind: the annotated public fragment reports its
    /// exposed non-public types at the fragment's own name
    /// (`_offsetAndLengthForFragment` — `nameOffset`/`name.length`).
    #[test]
    fn annotated_class_like_reports_at_its_name() {
        for (src, bad) in [
            ("@AnalyzerPublicApi()\nclass C extends B {}\n", "B"),
            (
                "@AnalyzerPublicApi()\nclass C = Object with M;\nmixin M {}\n",
                "M",
            ),
            ("@AnalyzerPublicApi()\nmixin C implements B {}\n", "B"),
            (
                "@AnalyzerPublicApi()\nenum E implements B {\n  e1, e2\n}\n",
                "B",
            ),
            ("@AnalyzerPublicApi()\nextension E on B {}\n", "B"),
        ] {
            let f = Fixture::parse(&format!("{API}{src}"));
            let lints = f.run(&one_rule("analyzer_public_api_bad_type"));
            assert_eq!(lints.len(), 1, "{src} -> {lints:?}");
            assert_eq!(
                lints[0].message,
                format!(
                    "Element makes use of type(s) which is not part of the analyzer public API: {bad}."
                ),
                "{src}"
            );
        }
    }

    /// The `test_badType_extensionType_notIgnoredIfAnnotatedPublic`
    /// shape — `extension type C(B b)` reports twice: the primary
    /// constructor's signature face at `C`, the representation field's
    /// getter face at `b`.
    #[test]
    fn extension_type_reports_at_name_and_representation_field() {
        let f = Fixture::parse(&format!(
            "{API}@AnalyzerPublicApi()\nextension type C(B b) {{}}\n"
        ));
        let lints = f.run(&one_rule("analyzer_public_api_bad_type"));
        assert_eq!(lints.len(), 2, "{lints:?}");
        let c_name = tok(&f.source, &f.tokens, "C", 0);
        let b_name = tok(&f.source, &f.tokens, "b", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, c_name));
        assert_eq!(lints[1].range.start, utf16_at(&f, b_name));
    }

    /// The typedef and top-level faces (`test_badType_typedef*` /
    /// `test_badType_topLevelVariableDeclaration` /
    /// `test_badType_functionDeclaration`): the report anchors at the
    /// fragment name.
    #[test]
    fn typedef_variable_and_function_faces() {
        for src in [
            "@AnalyzerPublicApi()\ntypedef B F();\n",
            "@AnalyzerPublicApi()\ntypedef F = B Function();\n",
            "@AnalyzerPublicApi()\nB? v;\n",
            "@AnalyzerPublicApi()\nB F() => B();\n",
        ] {
            let f = Fixture::parse(&format!("{API}{src}"));
            let lints = f.run(&one_rule("analyzer_public_api_bad_type"));
            assert_eq!(lints.len(), 1, "{src} -> {lints:?}");
            assert_eq!(
                lints[0].message,
                "Element makes use of type(s) which is not part of the analyzer public API: B.",
                "{src}"
            );
        }
    }

    /// The silent faces: unannotated declarations never enter the public
    /// API (`isInAnalyzerPublicApi`'s only mounted arm is the
    /// annotation), and an annotated name's own use stays
    /// `isOkForAnalyzerPublicApi`.
    #[test]
    fn unannotated_and_self_annotated_stay_silent() {
        let f = Fixture::parse(&format!("{API}class C extends B {{}}\n"));
        assert!(f.run(&one_rule("analyzer_public_api_bad_type")).is_empty());

        let f = Fixture::parse(
            "class AnalyzerPublicApi {\n  const AnalyzerPublicApi();\n}\n\n\
             @AnalyzerPublicApi()\nclass B {}\n@AnalyzerPublicApi()\nclass C extends B {}\n",
        );
        assert!(f.run(&one_rule("analyzer_public_api_bad_type")).is_empty());
    }
}
