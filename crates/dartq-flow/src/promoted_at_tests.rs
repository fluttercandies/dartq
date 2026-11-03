//! The `Flow::promoted_type_at` query pins: the write-target reads a plain
//! assignment's retraction removes from the fact stream stay answerable at
//! their read offset (the if-null composite's `LUB(NonNull(readType),
//! rhsType)` consumer).

use crate::forward::SharedFlow;
use crate::record::Flow;
use crate::ty::{Base, Ty};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// Parses `source` and answers `(promoted_type_at(offset), expected label)`
/// for the write target's read offset.
fn promoted_at(source: &str, lexeme: &str) -> Option<Ty> {
    let result = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        dartq_source::FileTable::new().intern(std::path::Path::new("test.dart")),
    );
    assert!(result.diagnostics.is_empty(), "{source:?}");
    let tokens: &'static TokenTable = Box::leak(Box::new(result.tokens));
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let mut flow = SharedFlow(std::rc::Rc::new(std::cell::RefCell::new(Flow::new(
        tokens, source,
    ))));
    flow.0
        .borrow_mut()
        .set_sound_flow(features.sound_flow_analysis);
    let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
    parser.parse_unit();
    drop(parser);
    let offset = source.find(lexeme).expect("needle") as u32;
    flow.0.borrow().promoted_type_at(offset)
}

fn int_ty(flow_ty: Option<Ty>, nullable: bool) -> Ty {
    // The base id is interned by the analysis itself; `int` is one of the
    // literal spellings `exec_end` interns, so the comparison rebuilds the
    // face through the same table by parsing it in a probe unit — simplest
    // is to compare (base kind, nullability) through a marker name check.
    let ty = flow_ty.expect("write-target read face");
    assert!(matches!(ty.base, Base::Name(_)), "{ty:?}");
    assert_eq!(ty.nullable, nullable, "{ty:?}");
    ty
}

/// `x ??= 5` on an unpromoted `int? x`: the retracted target read answers
/// its declared nullable face.
#[test]
fn if_null_target_read_answers_declared_face() {
    let source = "void f() {\n  int? x;\n  x ??= 5;\n}\n";
    let ty = promoted_at(source, "x ??=");
    int_ty(ty, true);
}

/// The promoted twin: inside `if (x != null)`, the target read of
/// `x ??= 5` answers the promoted non-null face.
#[test]
fn if_null_target_read_answers_promoted_face() {
    let source = "void f() {\n  int? x;\n  if (x != null) {\n    x ??= 5;\n  }\n}\n";
    let ty = promoted_at(source, "x ??= ");
    int_ty(ty, false);
}

/// An ordinary read is no retracted target: its face rides the fact
/// stream, and the side table answers nothing.
#[test]
fn ordinary_read_answers_nothing() {
    let source = "void f() {\n  int? x;\n  var y = x;\n}\n";
    assert_eq!(promoted_at(source, "x;\n}"), None);
}
