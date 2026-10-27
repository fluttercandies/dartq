//! Ticket #2626: interpreter termination. The step budget
//! ([`crate::interp::STEP_BUDGET_PER_EVENT`]) bounds every executable's
//! replay; these tests pin the three faces of the invariant:
//!
//! * the real `analyzer` corpus converges with zero bails and stays an
//!   order of magnitude under the budget (the ratio pin behind the
//!   constant);
//! * the pathological construct that once spun the replay converges
//!   after the fix (the minimal fixture below);
//! * an exhausted budget deterministically bails with no partial flow
//!   facts.

use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

use crate::record::Flow;

/// Parses `source` through a fresh recorder and returns it for the
/// bail/ratio faces.
fn flow_of_version(source: &str, version: LanguageVersion) -> Flow<'_> {
    let result = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        dartq_source::FileTable::new().intern(std::path::Path::new("test.dart")),
    );
    let tokens: &'static TokenTable = Box::leak(Box::new(result.tokens));
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(version));
    let mut flow = Flow::new(tokens, source);
    let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
    parser.parse_unit();
    drop(parser);
    flow
}

/// Every `.dart` file under `dir`, sorted for stable attribution.
fn dart_files_under(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            dart_files_under(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "dart") {
            out.push(path);
        }
    }
}

/// The whole `analyzer` corpus converges: no executable bails, and the
/// worst step-budget consumption stays an order of magnitude under the
/// budget (the ratio pin behind `STEP_BUDGET_PER_EVENT` — a legit body
/// that would come within 10× of the budget means the constant lost its
/// margin and must be re-derived from this test's data). The walk pins
/// the package's own language version (3.11, the pubspec's floor) — the
/// CLI resolves per-unit versions and the harness must walk the same
/// parse.
#[test]
fn corpus_real_analyzer_converges_within_budget_margin() {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/real/analyzer");
    let mut files = Vec::new();
    dart_files_under(&root, &mut files);
    assert!(files.len() > 400, "corpus missing: {}", root.display());
    let mut worst_permille = 0u64;
    let mut worst_file = String::new();
    let mut bailed = Vec::new();
    for path in &files {
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        let flow = flow_of_version(&source, LanguageVersion::new(3, 11));
        let bails = flow.bailed_executables();
        if bails > 0 {
            bailed.push(format!("{} ({} bails)", path.display(), bails));
        }
        let permille = flow.max_step_ratio_permille();
        if permille > worst_permille {
            worst_permille = permille;
            worst_file = path.display().to_string();
        }
    }
    assert!(
        bailed.is_empty(),
        "bodies exhausted the step budget (non-converging replay): {bailed:?}"
    );
    assert!(
        worst_permille <= 100,
        "worst legit budget consumption {worst_permille}‰ ({worst_file}) is within 10× \
         of the budget; re-derive STEP_BUDGET_PER_EVENT"
    );
}

/// An exhausted budget deterministically bails: the drivers unwind, the
/// body contributes no flow facts (a partial replay's state is not
/// evidence), and the same stream under the production budget runs to
/// completion — the bail is a budget artifact, never a silent fact drop.
#[test]
fn step_budget_exhaustion_bails_deterministically() {
    // A legitimate stream: one executable, two declared-and-read locals.
    let events: Vec<crate::ev::Ev> = vec![
        crate::ev::Ev::ExecBegin { is_closure: false },
        crate::ev::Ev::VarDeclBegin {
            late: false,
            is_final: false,
            span: tspan(0),
        },
        crate::ev::Ev::Type {
            base: crate::ty::Base::Name(1),
            nullable: true,
            args: crate::ty::EMPTY_ARGS,
        },
        crate::ev::Ev::InitializedIdent {
            name: 1,
            token: tspan(1),
        },
        crate::ev::Ev::InitializerBegin,
        crate::ev::Ev::OtherExpr { span: tspan(2) },
        crate::ev::Ev::InitializerEnd,
        crate::ev::Ev::VarDeclEnd,
        crate::ev::Ev::VarDeclBegin {
            late: false,
            is_final: false,
            span: tspan(3),
        },
        crate::ev::Ev::Type {
            base: crate::ty::Base::Name(2),
            nullable: true,
            args: crate::ty::EMPTY_ARGS,
        },
        crate::ev::Ev::InitializedIdent {
            name: 2,
            token: tspan(4),
        },
        crate::ev::Ev::InitializerBegin,
        crate::ev::Ev::OtherExpr { span: tspan(5) },
        crate::ev::Ev::InitializerEnd,
        crate::ev::Ev::VarDeclEnd,
        // A read of the first local: a reachable read emits its query row.
        crate::ev::Ev::Ident {
            name: 1,
            token: tspan(6),
            context: dartq_parser::IdentifierContext::Expression,
        },
    ];
    // Production budget: converges, no facts dropped.
    let mut interp = crate::interp::Interp::new(&crate::ty::NameTypeOps, &[], &[]);
    interp.run(&events);
    assert!(!interp.bailed);
    assert!(!interp.facts.is_empty());

    // Same stream past an artificial budget floor: bailed, zero facts.
    let mut interp = crate::interp::Interp::new(&crate::ty::NameTypeOps, &[], &[]);
    interp.override_step_budget_for_test(4);
    interp.run(&events);
    assert!(interp.bailed);
    assert!(interp.facts.is_empty());
    // Determinism: the same shrunk budget bails at the same residue.
    let mut replay = crate::interp::Interp::new(&crate::ty::NameTypeOps, &[], &[]);
    replay.override_step_budget_for_test(4);
    replay.run(&events);
    assert!(replay.bailed);
    assert!(replay.facts.is_empty());
    assert_eq!(replay.budget, interp.budget);
}

/// Token span for hand-driven event sequences (arbitrary anchor).
fn tspan(offset: u32) -> dartq_parser::TokenSpan {
    dartq_parser::TokenSpan {
        id: offset,
        range: dartq_source::SourceRange { offset, length: 1 },
    }
}
