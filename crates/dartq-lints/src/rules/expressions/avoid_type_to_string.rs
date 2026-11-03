use dartq_semantic::model::Ty;

use super::{Check, NodeKind, RuleSpec, query_site, same_file, slice};
use crate::rules::{SupertypeClause, class_declaration_name, supertypes, utf16_token_span};

/// Official: `upstream Dart SDK source (3.13.7 pin),
/// pkg/linter/lib/src/rules/avoid_type_to_string.dart` — a `toString`
/// selector whose target's static type is a subtype of `Type` reports
/// at the name. The registered faces (:46-58): every method invocation
/// (:82-88) and, at argument positions only, the tear-off spellings
/// (:116-131 — `_validateArgument` runs over argument lists alone).
/// The verdict (:103-105): the name is `toString`, the resolved member
/// is the dart:core `Object` one — any indexed `toString` on the
/// receiver's chain is an override and stays silent (:99-101) — and
/// the target type is a subtype of `Type` (:105). The dartq subtype
/// face is name-level: a same-file container walks its written
/// supertype clauses (extends/with/implements), a cross-file container
/// resolves outside the file and stays silent (the documented
/// under-report). The thisType faces (:66-81) read the enclosing
/// declaration — a class-like's own name, an extension's written `on`
/// type; a mixin's self-type is no subtype of its `on` constraint (the
/// 3.13.7 oracle reads `M <: Type` false — the mixinOnType cases stay
/// silent). The exact core-`Type` receiver keeps its pinned fire (the
/// `Type t` receiver face, gap1 pin) — a face the census carries no
/// oracle-firing case for; the extension-on-`Type` thisType face, the
/// oracle-observed silence, keeps it. The upstream thisType leak
/// (never reset after a declaration, so a later dynamic receiver reads
/// the last visited class) and cascade-section receivers stay silent —
/// no census case covers either. Message: `diagnostic.g.dart`
/// `avoidTypeToString`.
pub(crate) static SPEC: RuleSpec = crate::spec!("avoid_type_to_string", &[NodeKind::Send], check,);

fn check(c: &mut Check<'_>) {
    // The wave posture: the rule reads the semantic face; face absent
    // is the plain-run posture and stays silent (the engine contract).
    let Some(analysis) = c.semantics() else {
        return;
    };
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
    let Some((head, arguments)) = slice::send_parts(c, &kids, pos) else {
        return;
    };
    let selector_row = kids[head];
    let selector = c.head_token(selector_row);
    if c.lexeme(selector) != "toString" {
        return;
    }
    // The registered faces (:46-58): an invocation fires anywhere; a
    // tear-off (no argument list) fires at argument positions only —
    // the upstream `_validateArgument` walk is an argument-list walk.
    if arguments.is_none() && c.kind(c.ancestor(1).unwrap_or(c.node())) != NodeKind::Arguments {
        return;
    }
    let Some((container, exact_core_silent)) = receiver_container(
        c,
        analysis,
        &kids[..head],
        selector,
        selector.checked_sub(1),
    ) else {
        return;
    };
    if !subtype_of_core_type(c, &container, exact_core_silent) {
        return;
    }
    // The Object-member gate (:99-101): the invoked `toString` must be
    // the dart:core declaration — an indexed declaration anywhere on
    // the receiver's chain is an override and stays silent.
    let member = analysis.strings().intern("toString");
    let container_id = analysis.strings().intern(&container);
    if analysis
        .inherited_member(container_id, member, false)
        .is_some()
    {
        return;
    }
    let range = utf16_token_span(c, selector);
    c.report(range, &[]);
}

/// The subtype face (:105): the receiver's container is a subtype of
/// the core `Type`. A same-file container walks its written supertype
/// clauses; a user declaration shadowing the `Type` spelling is never
/// the core face. An unshadowed written `Type` is the core face
/// itself: the receiver-expression faces keep its pinned fire, the
/// thisType faces keep the oracle-observed silence
/// (`exact_core_silent`).
fn subtype_of_core_type(c: &Check<'_>, container: &str, exact_core_silent: bool) -> bool {
    if same_file::declares_type(c, container) {
        return matches!(
            same_file::resolve(c, container, &["Type"]),
            same_file::Verdict::Core("Type")
        );
    }
    container == "Type" && !exact_core_silent
}

/// The target-type container of the `toString` selector at `selector`,
/// over the piece run before it: a non-nullable declared type name plus
/// whether the exact core-`Type` spelling stays silent (receiver faces
/// report it, thisType faces don't). `None` carries no verdict.
fn receiver_container<'a>(
    c: &'a Check<'a>,
    analysis: &dartq_semantic::Analysis<'a>,
    run: &[u32],
    selector: u32,
    prev: Option<u32>,
) -> Option<(&'a str, bool)> {
    match prev.map(|token| c.lexeme(token)) {
        // The cascade-section dot is its own spelling; the upstream
        // cascade face reads a leaked thisType dartq does not carry.
        Some("..") | Some("?..") => return None,
        // A member receiver: the piece run before the selector.
        Some(".") | Some("?.") => {}
        // No dot before the name: the implicit-this face (:129-131).
        _ => return this_container(c).map(|name| (name, true)),
    }
    let mut run = slice::access_run(c, run);
    // The receiver is the piece run before the dot: send-group strays
    // past the selector and the invocation's own argument list drop.
    while let Some(&last) = run.last() {
        if c.kind(last) == NodeKind::Arguments || c.unit.ast.token_range(last).start > selector {
            run.pop();
            continue;
        }
        break;
    }
    let Some(&last) = run.last() else {
        return None;
    };
    match c.kind(last) {
        // The `this`/`super` receiver face (:84): the enclosing
        // declaration's thisType. The row must sit directly before the
        // dot — earlier statements' stranded pieces stay out of the
        // verdict (the documented chain under-report).
        NodeKind::ThisExpression | NodeKind::SuperExpression
            if c.head_token(last) + 2 == selector =>
        {
            this_container(c).map(|name| (name, true))
        }
        NodeKind::NewExpression => creation_container(c, analysis, c.collect_direct(last)),
        NodeKind::Identifier => {
            let site = query_site(c, last);
            if c.head_token(last) + 2 == selector {
                // The plain-name face (:116-121): the flow read row's
                // declared type (a `Type2?` read is no `Type` subtype).
                match analysis.type_of(site) {
                    Some(Ty::Named {
                        name,
                        nullable: false,
                        ..
                    }) => Some((analysis.strings().resolve(name), false)),
                    _ => None,
                }
            } else {
                // The creation face: the send group's callee is the
                // type-reference position (`Type2().toString()`); a
                // non-type callee (`getC().toString()`) answers `None`.
                creation_container(c, analysis, vec![last])
            }
        }
        _ => None,
    }
}

/// The creation face: the first direct identifier of the creation row
/// is the type-reference position; the bridge answers the constructed
/// container (non-nullable by contract).
fn creation_container<'a>(
    c: &Check<'a>,
    analysis: &dartq_semantic::Analysis<'a>,
    kids: Vec<u32>,
) -> Option<(&'a str, bool)> {
    let head = kids
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::Identifier)?;
    match analysis.creation_type(query_site(c, head)) {
        Some(Ty::Named { name, .. }) => Some((analysis.strings().resolve(name), false)),
        _ => None,
    }
}

/// The enclosing declaration's thisType face (:66-81): a class-like's
/// written name, an extension's written `on` type. A mixin's self-type
/// satisfies no `on` constraint in the subtype model — no verdict.
fn this_container<'a>(c: &'a Check<'a>) -> Option<&'a str> {
    let ancestor = c.path.iter().rev().skip(1).copied().find(|&a| {
        matches!(
            c.kind(a),
            NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::ExtensionDeclaration
                | NodeKind::ExtensionTypeDeclaration
                | NodeKind::EnumDeclaration
        )
    })?;
    match c.kind(ancestor) {
        NodeKind::MixinDeclaration => None,
        NodeKind::ExtensionDeclaration => supertypes(c, ancestor)
            .into_iter()
            .find(|(_, clause)| *clause == SupertypeClause::On)
            .map(|(name, _)| name),
        _ => class_declaration_name(c, ancestor),
    }
}

#[cfg(test)]
mod tests {
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_resolution::{LibraryClassModifiers, LibraryGraph, LibraryGraphBuilder, Namespaces};
    use dartq_semantic::{ElementIndex, MicroKind, StrictModes, analyze_unit, collect};
    use dartq_source::{FileTable, StringTable};

    use crate::engine::Lint;
    use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

    const MESSAGE: &str = "Using 'toString' on a 'Type' is not safe in production code.";

    /// Runs the rule with the semantic face mounted (the gap-1 face
    /// construction): the source folded as the sole analyzed file of
    /// `package:t`. The context tables leak (`'static`); the tests are
    /// one-shot processes.
    fn run_semantic(source: &str) -> Vec<Lint> {
        let mut fixture = Fixture::parse(source);
        fixture.version = Some(LanguageVersion::new(3, 15));
        let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
        let mut files = FileTable::new();
        let file = files.intern(std::path::Path::new("lint_semantic.dart"));
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            LanguageVersion::new(3, 15),
        ));
        let scanned = dartq_scanner::scan(source, crate::testsupport::CONFIG, file);
        assert!(
            scanned.diagnostics.is_empty(),
            "scanner diagnostics: {source:?}"
        );
        let elements = collect(source, &scanned.tokens, features, strings);
        let mut builder = LibraryGraphBuilder::new(strings);
        let lib = builder.intern("package:t/lint_semantic.dart");
        for (name, declaration) in elements.declarations() {
            if declaration.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        let mut index = ElementIndex::default();
        index.insert(file, elements);
        let graph: &'static LibraryGraph = Box::leak(Box::new(builder.finish()));
        let namespaces: &'static Namespaces = Box::leak(Box::new(Namespaces::build(graph)));
        let index: &'static ElementIndex = Box::leak(Box::new(index));
        let analysis = analyze_unit(
            source,
            &scanned.tokens,
            features,
            strings,
            file,
            lib,
            graph,
            namespaces,
            index,
            StrictModes::default(),
            &LibraryClassModifiers::default(),
        );
        let unit = crate::engine::Unit {
            source: &fixture.source,
            tokens: &fixture.tokens,
            ast: &fixture.ast,
            lines: &fixture.lines,
            path: Some(&fixture.path),
            features: fixture.features,
            version: fixture.version,
            workspace: None,
            reachability: None,
            async_context_uses: None,
            library_type_uses: None,
        };
        unit.run_with_semantics(
            &one_rule("avoid_type_to_string"),
            &rustc_hash::FxHashSet::default(),
            Some(&analysis),
        )
    }

    /// The creation face (`Type2().toString()` — the upstream
    /// `test_creationReceiver`): the report lands at the `toString` name.
    #[test]
    fn creation_receiver_reports_at_the_name() {
        let source = "class C implements Type {}\nvoid f() {\n  C().toString();\n}\n";
        let lints = run_semantic(source);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, MESSAGE);
        let probe = Fixture::parse(source);
        let start = utf16_at(&probe, tok(source, &probe.tokens, "toString", 0));
        assert_eq!(lints[0].range, start..start + 8);
    }

    /// The plain-name face (`Type2 v = Type2(); v.toString()` — the
    /// upstream `test_plainNameReceiver`).
    #[test]
    fn plain_name_receiver_reports() {
        let source = "class C implements Type {}\nvoid f() {\n  C c = C();\n  c.toString();\n}\n";
        let lints = run_semantic(source);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, MESSAGE);
    }

    /// The `this`/`super` receiver faces (the upstream
    /// `test_thisReceiver`/`test_superReceiver` shapes over a same-file
    /// `implements Type` chain).
    #[test]
    fn this_and_super_receivers_report() {
        let source = "class C implements Type {\n  void f() {\n    this.toString();\n  }\n}\n";
        let lints = run_semantic(source);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, MESSAGE);

        let source = "class C implements Type {}\nclass D extends C {\n  void f() {\n    super.toString();\n  }\n}\n";
        let lints = run_semantic(source);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, MESSAGE);
    }

    /// The tear-off fires at argument positions only: `g(C().toString)`
    /// reports, a bare `C().toString;` statement never does (the upstream
    /// `_validateArgument` argument-list walk).
    #[test]
    fn tear_off_fires_at_argument_positions_only() {
        let source =
            "class C implements Type {}\nvoid g(Object? x) {}\nvoid f() {\n  g(C().toString);\n}\n";
        let lints = run_semantic(source);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, MESSAGE);

        let source = "class C implements Type {}\nvoid f() {\n  C().toString;\n}\n";
        assert!(run_semantic(source).is_empty());
    }

    /// The Object-member gate (:99-101): an indexed `toString` anywhere on
    /// the receiver's chain is an override and stays silent.
    #[test]
    fn an_override_on_the_chain_silences() {
        let source = "class C implements Type {\n  String toString() => 'C';\n}\nvoid f() {\n  C().toString();\n}\n";
        assert!(run_semantic(source).is_empty());
    }

    /// A nullable `Type?` receiver is no subtype of `Type` — silent (the
    /// upstream nullable-receiver faces).
    #[test]
    fn a_nullable_receiver_silences() {
        let source = "class C implements Type {}\nvoid f() {\n  C? c;\n  c?.toString();\n}\n";
        assert!(run_semantic(source).is_empty());
    }

    /// The thisType faces that stay silent: a mixin's self-type satisfies
    /// no `on` constraint (the 3.13.7 oracle reads `M <: Type` false) and
    /// the extension-on-`Type` keeps the oracle-observed silence.
    #[test]
    fn mixin_and_extension_this_type_silences() {
        let source = "mixin M on Type {\n  void f() {\n    toString();\n  }\n}\n";
        assert!(run_semantic(source).is_empty());

        let source = "extension E on Type {\n  void f() {\n    toString();\n  }\n}\n";
        assert!(run_semantic(source).is_empty());
    }

    /// The cascade-section receiver and a `dynamic` receiver stay silent —
    /// the upstream cascade face reads a leaked thisType dartq does not
    /// carry, and a dynamic receiver carries no container.
    #[test]
    fn cascade_and_dynamic_receivers_silence() {
        let source = "class C implements Type {}\nvoid f() {\n  C()..toString();\n}\n";
        assert!(run_semantic(source).is_empty());

        let source = "void f() {\n  dynamic d;\n  d.toString();\n}\n";
        assert!(run_semantic(source).is_empty());
    }
}
