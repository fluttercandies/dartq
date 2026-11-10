//! The #2493 pins: the null-aware invocation's deadness on a receiver whose
//! static type is null-or-equivalent — the drift corpus row
//! `sqlite3/database.dart 92:20` (`DB? _database` under
//! `Sqlite3Delegate<DB extends CommonDatabase>` with the bound unresolvable
//! in the per-file oracle context).
//!
//! The oracle path (bisection-verified against `dart analyze`): a
//! `?.`-receiver typed `T?` over a type parameter whose bound is the
//! invalid face subtypes `Null` vacuously (`T? <: Null` iff `T <: Null`
//! iff `B <: Null`, `B` invalid — upstream Dart SDK source,
//! pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:9214-9216
//! sets the access path unreachable;
//! pkg/analyzer/lib/src/dart/resolver/flow_analysis_visitor.dart:615-632
//! classifies through the subtype question). The dead-code verifier records
//! the interval at the selector name (`recordDeadIntervalAt(node,
//! selector.name)`, type_analyzer.dart:3995) and reports it at the chain's
//! close — one row anchored at the member name, never at the `?.` operator.
//!
//! The field-vs-local trap: an INSTANCE FIELD read fires exactly one row
//! (fields are not promotable elements, so the official's
//! `_verifyUnassignedVariable` never adds its operator-anchored row); a
//! LOCAL declared without an initializer fires TWO rows (the unassigned
//! face adds the `?.`-operator anchor). The field shape is the corpus row.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use crate::ty::{Base, EMPTY_ARGS, Ty, TypeOps};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// A name-level ops that answers [`TypeOps::is_invalid_typed`] for exactly
/// one interned spelling — the stand-in for the resolved back-end's
/// unresolved-name verdict (`CommonDatabase` under the corpus copy's
/// per-file analysis context). Everything else delegates to
/// [`crate::ty::NameTypeOps`].
struct InvalidBoundOps {
    /// The interned id of the invalid bound's lexeme (`u32::MAX` = the
    /// resolved world: nothing is invalid).
    und: std::cell::Cell<u32>,
}

impl TypeOps for InvalidBoundOps {
    fn is_subtype(&self, left: Ty, right: Ty) -> bool {
        crate::ty::NameTypeOps.is_subtype(left, right)
    }

    fn factor(&self, from: Ty, what: Ty) -> Ty {
        crate::ty::NameTypeOps.factor(from, what)
    }

    fn try_promote_to(&self, to: Ty, from: Ty) -> Option<Ty> {
        crate::ty::NameTypeOps.try_promote_to(to, from)
    }

    fn promote_to_non_null(&self, t: Ty) -> Ty {
        crate::ty::NameTypeOps.promote_to_non_null(t)
    }

    fn classify(&self, t: Ty) -> crate::ty::Classification {
        crate::ty::NameTypeOps.classify(t)
    }

    fn is_bottom(&self, t: Ty) -> bool {
        crate::ty::NameTypeOps.is_bottom(t)
    }

    fn is_invalid_typed(
        &self,
        ty: Ty,
        args: u32,
        _arena: &[Vec<crate::ty::ArgTy>],
        _names: &[Box<str>],
    ) -> bool {
        ty.base == Base::Name(self.und.get()) && !ty.nullable && args == EMPTY_ARGS
    }
}

/// Dead-code facts of `source` as source text. `invalid` is the bound
/// lexeme the ops stub certifies unresolved (`None` = the resolved world).
/// `stage` runs before the parse with the interned `DB`/invalid ids — the
/// semantic layer's staging channel (`Flow::stage_this_member_type`,
/// keyed on the read's UTF-16 offset; the sources here are ASCII, so
/// byte offsets stand in).
fn dead_of_with(
    source: &'static str,
    invalid: Option<&str>,
    stage: impl FnOnce(&mut Flow, u32, u32),
) -> Vec<String> {
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
    let und = {
        let mut recorder = flow.0.borrow_mut();
        let db = recorder.intern_name("DB");
        let und = match invalid {
            Some(lexeme) => recorder.intern_name(lexeme),
            None => u32::MAX,
        };
        let ops: &'static dyn TypeOps = Box::leak(Box::new(InvalidBoundOps {
            und: std::cell::Cell::new(und),
        }));
        recorder.set_ops(ops);
        stage(&mut recorder, db, und);
        und
    };
    let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
    parser.parse_unit();
    drop(parser);
    let _ = und;
    flow.0
        .borrow_mut()
        .take_facts()
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::DeadCode { span } => Some(
                source
                    [span.range.offset as usize..(span.range.offset + span.range.length) as usize]
                    .to_string(),
            ),
            _ => None,
        })
        .collect()
}

#[test]
fn null_aware_field_on_invalid_bound_param_dies_at_member_name() {
    // The corpus shape (drift `sqlite3/database.dart:92`): an instance
    // field `DB? _database` under `<DB extends CommonDatabase>` with the
    // bound unresolvable, read null-aware inside a catch block. The
    // declared type rides the semantic staging channel (the implicit-this
    // arm stages exactly this `DB? _database` spelling,
    // diagnose/listener.rs). Oracle 3.13.1: one `dead_code` row at the
    // member name — `close()` — never at the `?.` operator.
    let src = "class C<DB extends Und> {\n  DB? x;\n  void f() {\n    x?.close();\n  }\n}\n";
    let read_offset = src.find("x?.close()").expect("read in source") as u32;
    let rows = dead_of_with(src, Some("Und"), |flow, db, _und| {
        flow.stage_this_member_type(
            read_offset,
            Ty {
                base: Base::Name(db),
                nullable: true,
            },
        );
    });
    assert_eq!(rows, vec!["close()".to_string()]);
}

#[test]
fn null_aware_field_after_await_write_dies_in_catch() {
    // The full corpus method shape (async + await assignment + try/catch +
    // rethrow): the write is irrelevant to the deadness (bisection proved
    // the oracle fires with the assignment deleted) — the row comes from
    // the receiver's classification alone. Same single name-anchored row.
    let src = "class C<DB extends Und> {\n  DB? x;\n  Future<void> f() async {\n    x = await g();\n    try {\n      h();\n    } catch (e) {\n      x?.close();\n      x = null;\n      rethrow;\n    }\n  }\n  Future<DB> g() async => throw 0;\n  void h() {}\n}\n";
    let read_offset = src.find("x?.close()").expect("read in source") as u32;
    let rows = dead_of_with(src, Some("Und"), |flow, db, _und| {
        flow.stage_this_member_type(
            read_offset,
            Ty {
                base: Base::Name(db),
                nullable: true,
            },
        );
    });
    assert_eq!(rows, vec!["close()".to_string()]);
}

#[test]
fn null_aware_param_on_invalid_bound_param_dies_at_member_name() {
    // The parameter twin (oracle probe: one row at the name): the declared
    // type reaches the fold through the formal's own recorded type — no
    // staging channel needed. The promotable-read face never adds an
    // operator row here (only the unassigned-local face would, and a
    // parameter is assigned).
    let src = "class C<DB extends Und> {\n  void g(DB? y) {\n    y?.close();\n  }\n}\n";
    let rows = dead_of_with(src, Some("Und"), |_, _, _| {});
    assert_eq!(rows, vec!["close()".to_string()]);
}

#[test]
fn null_aware_read_on_resolved_bound_stays_live() {
    // The A-side: the same spelling with a RESOLVED bound (the full
    // oracle world where the bound's class is reachable) classifies
    // potentially-nullable — the access path stays live and no row fires
    // (bisection: the pristine drift file in its package context is
    // clean). The staging channel still runs; only the bound's validity
    // differs.
    let src = "class C<DB extends Res> {\n  DB? x;\n  void f() {\n    x?.close();\n  }\n}\n";
    let read_offset = src.find("x?.close()").expect("read in source") as u32;
    let rows = dead_of_with(src, None, |flow, db, _und| {
        flow.stage_this_member_type(
            read_offset,
            Ty {
                base: Base::Name(db),
                nullable: true,
            },
        );
    });
    assert_eq!(rows, Vec::<String>::new());
}
