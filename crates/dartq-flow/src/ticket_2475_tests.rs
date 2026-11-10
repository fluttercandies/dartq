//! The #2475 pins: a NON-instantiated object pattern on a generic sealed
//! scrutinee promotes the variable to the INSTANTIATED case type —
//! `case AsyncData():` over `AsyncValue<ValueT> value` promotes to
//! `AsyncData<ValueT>`, the oracle's `requiredType` spelling
//! (`downwardInferObjectPatternRequiredType` → `_inferTypeArguments`'s
//! `GenericInferrer.constrainReturnType(declaredType, contextType)`
//! solving the head's parameters against the matched type along the
//! superclass chain — upstream Dart SDK source,
//! pkg/_fe_analyzer_shared/lib/src/type_inference/type_analyzer.dart:
//! 1497-1513, pkg/analyzer/lib/src/generated/resolver.dart:869-913,
//! 4596-4620). The riverpod corpus' four ATNA rows (`case AsyncData():` /
//! `case AsyncError():` over `AsyncValue<ValueT>`) trace to the
//! two-superclass-hop shapes the ticket-era direct-edge-only inference
//! could not climb (`AsyncData` → `AsyncResult` → `AsyncValue`); the
//! transitive walk answers `AsyncData<ValueT>` where the direct edge
//! alone saw only `AsyncResult<ValueT>`. `AsyncLoading` — one hop —
//! never appeared in the rows.
//!
//! The ops stand-in below carries the seam the real resolved back-end
//! answers ([`TypeOps::downcast_infer`]'s superclass-chain walk + the
//! `is_subtype` hierarchy): a directed edge list `sub → super` whose
//! transitive closure is the subtype relation, and a downward inference
//! that returns the matched type's own written arguments beside the
//! tested head — the single-parameter chain the repro shapes share
//! (`AsyncData<T> extends AsyncResult<T> extends AsyncValue<T>`: the
//! solution `T ↦ ValueT` IS the scrutinee's argument list).

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use crate::ty::{ArgTy, Base, EMPTY_ARGS, Ty, TypeOps};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// A name-level ops with two hierarchy answers the spelling alone cannot
/// carry: the transitive `sub → super` subtype closure and the object
/// pattern's downward-inferred required type (the real back-end's
/// registry walk). Everything else delegates to
/// [`crate::ty::NameTypeOps`]; [`TypeOps::try_promote_to_typed`] keeps
/// the official strict-subtype gate.
struct HierarchyOps {
    /// Interned name ids, seeded after the recorder interns them (before
    /// `set_ops` mounts this): the edge pairs index into this list.
    ids: std::cell::RefCell<Vec<u32>>,
    /// `(sub_index, super_index)` pairs into `ids` — one row per DIRECT
    /// `extends` edge (`AsyncData → AsyncResult`, `AsyncResult →
    /// AsyncValue`, `AsyncLoading → AsyncValue`). The transitive closure
    /// answers the bare subtype question.
    edges: &'static [(usize, usize)],
}

impl HierarchyOps {
    /// Binds the run's own interned ids — the same seed-and-mount order
    /// `BoolToObjectOps` uses.
    fn seed(&self, ids: Vec<u32>) {
        *self.ids.borrow_mut() = ids;
    }

    /// The direct superclass of `sub`, per edge — one hop.
    fn direct_supers(&self, sub: u32) -> Vec<u32> {
        let ids = self.ids.borrow();
        self.edges
            .iter()
            .filter(|(s, _)| ids.get(*s) == Some(&sub))
            .filter_map(|(_, t)| ids.get(*t).copied())
            .collect()
    }

    /// Whether `sub` reaches `target` along the edge transitive closure —
    /// `AsyncData` reaches `AsyncValue` through `AsyncResult`.
    fn reaches(&self, sub: u32, target: u32) -> bool {
        let mut seen = rustc_hash::FxHashSet::default();
        let mut queue = std::collections::VecDeque::from([sub]);
        while let Some(cur) = queue.pop_front() {
            if cur == target {
                return true;
            }
            if !seen.insert(cur) {
                continue;
            }
            queue.extend(self.direct_supers(cur));
        }
        false
    }
}

impl TypeOps for HierarchyOps {
    fn is_subtype(&self, left: Ty, right: Ty) -> bool {
        if let (Base::Name(l), Base::Name(r)) = (left.base, right.base)
            && self.reaches(l, r)
        {
            // The hierarchy carries the names; nullability still folds
            // the name-level way (a non-null left feeds every right).
            return !left.nullable || right.nullable;
        }
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

    /// The raw pattern's required type solves against the matched type's
    /// own arguments when the tested head reaches the matched head along
    /// the chain (`case AsyncData():` over `AsyncValue<ValueT>` reads
    /// `AsyncData<ValueT>`). The real back-end composes each hop's
    /// substitution into a fresh arena row; the single-parameter chains
    /// the repro shapes share make the answer the scrutinee's own list
    /// verbatim, so the fake returns `from_args` — the same index the
    /// resolved impl hands back for this shape.
    fn downcast_infer(
        &self,
        tested: Ty,
        tested_args: u32,
        from: Ty,
        from_args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> Option<(Ty, u32)> {
        let _ = names;
        if tested_args != EMPTY_ARGS || from_args == EMPTY_ARGS {
            return None;
        }
        let (Base::Name(t), Base::Name(f)) = (tested.base, from.base) else {
            return None;
        };
        if !self.reaches(t, f) {
            return None;
        }
        let _ = arena.get(from_args as usize)?;
        Some((tested, from_args))
    }
}

/// Scans and parses `source` with the recorder attached. `intern` names
/// are interned on the recorder's own table before parsing; `ops`, when
/// given, is seeded with the interned ids and mounted as the replay's
/// back-end (None = the name-level default). Returns the interned ids,
/// the flow (for `type_args_at`/`name_lexeme` reads of the arg trees),
/// and the taken facts.
fn analyze<'t>(
    source: &'static str,
    intern: &[&str],
    ops: Option<&'t HierarchyOps>,
) -> (Vec<u32>, SharedFlow<'t>, Vec<FlowFact>) {
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
    let ids = {
        let mut recorder = flow.0.borrow_mut();
        let ids: Vec<u32> = intern
            .iter()
            .map(|lexeme| recorder.intern_name(lexeme))
            .collect();
        if let Some(ops) = ops {
            ops.seed(ids.clone());
            recorder.set_ops(ops);
        }
        ids
    };
    let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
    parser.parse_unit();
    drop(parser);
    let facts = flow.0.borrow_mut().take_facts();
    (ids, flow, facts)
}

/// `(snippet, ty, args)` of every `ReadType`/`PromotedRead` row — the
/// triple the instantiated-face assertions need (the `args` arena index
/// distinguishes the raw `AsyncData` head from the inferred
/// `AsyncData<ValueT>` spelling).
fn reads<'a>(
    source: &'a str,
    facts: &[FlowFact],
) -> (Vec<(&'a str, Ty, u32)>, Vec<(&'a str, Ty, u32)>) {
    let at = |span: dartq_source::SourceRange| {
        &source[span.offset as usize..(span.offset + span.length) as usize]
    };
    let mut read = Vec::new();
    let mut promoted = Vec::new();
    for fact in facts {
        match fact {
            FlowFact::ReadType { span, ty, args } => read.push((at(span.range), *ty, *args)),
            FlowFact::PromotedRead { span, ty, args } => {
                promoted.push((at(span.range), *ty, *args))
            }
            _ => {}
        }
    }
    (read, promoted)
}

/// The instantiated-face check: the row's `args` arena reads as exactly
/// the `ValueT` argument list — `[ArgTy { base: Name(ValueT), .. }]`.
fn arg_is(flow: &Flow<'_>, args: u32, vt: u32) -> bool {
    matches!(
        flow.type_args_at(args),
        [ArgTy {
            base: Base::Name(id),
            nullable: false,
            ..
        }] if *id == vt
    )
}

/// The headline repro (riverpod `element.dart:96-98`,
/// `override_with_value.dart:154-156`): `case AsyncData():` /
/// `case AsyncError():` (two superclass hops) and `case AsyncLoading():`
/// (one hop) over `AsyncValue<ValueT> value` each promote `value` to the
/// instantiated head — `AsyncData<ValueT>` / `AsyncError<ValueT>` /
/// `AsyncLoading<ValueT>`, not the raw face. The post-switch read falls
/// back to the declared `AsyncValue<ValueT>`: the failed-match arms join
/// the unmatched chain.
#[test]
fn raw_object_pattern_promotes_to_the_instantiated_head() {
    let source = "void f<ValueT>(AsyncValue<ValueT> value) {\n\
                  \x20 switch (value) {\n\
                  \x20   case AsyncData():\n\
                  \x20     value;\n\
                  \x20   case AsyncError():\n\
                  \x20     value;\n\
                  \x20   case AsyncLoading():\n\
                  \x20     value;\n\
                  \x20 }\n\
                  \x20 value;\n\
                  }";
    // Direct extends edges: AsyncData/AsyncError → AsyncResult,
    // AsyncResult/AsyncLoading → AsyncValue (indices into `intern`).
    let ops = HierarchyOps {
        ids: std::cell::RefCell::new(Vec::new()),
        edges: &[(2, 1), (3, 1), (1, 0), (4, 0)],
    };
    let (ids, flow, facts) = analyze(
        source,
        &[
            "AsyncValue",
            "AsyncResult",
            "AsyncData",
            "AsyncError",
            "AsyncLoading",
            "ValueT",
        ],
        Some(&ops),
    );
    let (av, ad, ae, al, vt) = (ids[0], ids[2], ids[3], ids[4], ids[5]);
    let (read, promoted) = reads(source, &facts);
    let names: Vec<Base> = read
        .iter()
        .filter(|(snippet, _, _)| *snippet == "value")
        .map(|(_, ty, _)| ty.base)
        .collect();
    assert_eq!(
        names,
        [
            Base::Name(av),
            Base::Name(ad),
            Base::Name(ae),
            Base::Name(al),
            Base::Name(av)
        ],
        "the scrutinee/case-body/post-switch reads: {read:?}"
    );
    let flow = flow.0.borrow();
    for (_, _, args) in read
        .iter()
        .filter(|(snippet, ty, _)| *snippet == "value" && ty.base != Base::Name(av))
    {
        assert!(
            arg_is(&flow, *args, vt),
            "the promoted read carries the instantiated argument list: {read:?}"
        );
    }
    for head in [ad, ae, al] {
        assert!(
            promoted
                .iter()
                .any(|(snippet, ty, args)| *snippet == "value"
                    && ty.base == Base::Name(head)
                    && !ty.nullable
                    && arg_is(&flow, *args, vt)),
            "the `AsyncX<ValueT>` promotion row stands on `value`: {promoted:?}"
        );
    }
}

/// The written-argument face needs no inference (regression):
/// `case AsyncData<ValueT>():` spells the required type itself — the
/// call site's `args == EMPTY_ARGS` gate skips `downcast_infer` and the
/// promotion stores the WRITTEN tree. Same read face, different
/// provenance.
#[test]
fn written_object_pattern_args_promote_the_instantiated_head() {
    let source = "void f<ValueT>(AsyncValue<ValueT> value) {\n\
                  \x20 switch (value) {\n\
                  \x20   case AsyncData<ValueT>():\n\
                  \x20     value;\n\
                  \x20 }\n\
                  }";
    let ops = HierarchyOps {
        ids: std::cell::RefCell::new(Vec::new()),
        edges: &[(2, 1), (1, 0)],
    };
    let (ids, flow, facts) = analyze(
        source,
        &["AsyncValue", "AsyncResult", "AsyncData", "ValueT"],
        Some(&ops),
    );
    let (ad, vt) = (ids[2], ids[3]);
    let (read, promoted) = reads(source, &facts);
    let flow = flow.0.borrow();
    let body = read
        .iter()
        .rev()
        .find(|(snippet, _, _)| *snippet == "value")
        .expect("the case-body read carries a row");
    assert!(
        body.1.base == Base::Name(ad) && !body.1.nullable,
        "the written-argument pattern promotes `value` to `AsyncData`: {read:?}"
    );
    assert!(
        arg_is(&flow, body.2, vt),
        "the row carries the WRITTEN `ValueT` list: {read:?}"
    );
    assert!(
        promoted
            .iter()
            .any(|(snippet, ty, args)| *snippet == "value"
                && ty.base == Base::Name(ad)
                && arg_is(&flow, *args, vt)),
        "{promoted:?}"
    );
}

/// A scrutinee whose face carries no arguments takes no inference arm
/// (`from_args == EMPTY_ARGS` refuses — the oracle's
/// `_inferTypeArguments` has no context arguments to solve against):
/// `Object o` under `case AsyncData():` still promotes — the name-level
/// optimistic cross-base arm — but to the RAW head (`AsyncData` with
/// `EMPTY_ARGS`, the `Name<dynamic>` convention), never an invented
/// argument list. A non-generic unrelated scrutinee (`Foo`) behaves the
/// same: no edge, no arguments, the raw-face promotion.
#[test]
fn non_generic_scrutinee_promotes_to_the_raw_head() {
    let source = "void g(Object o) {\n\
                  \x20 switch (o) {\n\
                  \x20   case AsyncData():\n\
                  \x20     o;\n\
                  \x20 }\n\
                  }\n\
                  void h(Foo foo) {\n\
                  \x20 switch (foo) {\n\
                  \x20   case AsyncData():\n\
                  \x20     foo;\n\
                  \x20 }\n\
                  }";
    let ops = HierarchyOps {
        ids: std::cell::RefCell::new(Vec::new()),
        edges: &[(1, 0)],
    };
    let (ids, _flow, facts) = analyze(
        source,
        &["AsyncValue", "AsyncResult", "AsyncData", "Object", "Foo"],
        Some(&ops),
    );
    let ad = ids[2];
    let (read, promoted) = reads(source, &facts);
    for var in ["o", "foo"] {
        let body = read
            .iter()
            .rev()
            .find(|(snippet, _, _)| *snippet == var)
            .unwrap_or_else(|| panic!("the `{var}` body read carries a row: {read:?}"));
        assert!(
            body.1.base == Base::Name(ad) && body.2 == EMPTY_ARGS,
            "`{var}` promotes to the raw `AsyncData` head: {read:?}"
        );
        assert!(
            promoted.iter().any(|(snippet, ty, args)| *snippet == var
                && ty.base == Base::Name(ad)
                && *args == EMPTY_ARGS),
            "{promoted:?}"
        );
    }
}
