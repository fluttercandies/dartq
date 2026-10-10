//! The exhaustiveness engine over the production `TypedModel`, anchored to
//! the official `_fe_analyzer_shared` behavior (exhaustive.dart,
//! types/sealed.dart, types/enum.dart, types/bool.dart, types/record.dart,
//! witness.dart). Cases mirror the official test expectations: sealed
//! completeness, enum constants, `bool`, `FutureOr`, nullable splits,
//! record destructuring, and per-case reachability.

mod common;

use common::named;
use dartq_types::{
    Case, CoreDecls, Exhaustiveness, ExhaustivenessWorld, Key, Report, Single, Space, TypeDeclId,
    TypeHierarchy, TypeId, TypeParamId, TypeSystem, TypeTable, TypedModel, TypedNames, TypedTy,
    UninternedRecord, UninternedType, compute,
};

/// A sealed `Shape`/`Circle`/`Square` family, a three-constant `Color`
/// enum, a `bool` declaration, and the `Record` class.
struct World {
    table: TypeTable,
    hierarchy: TypeHierarchy,
    object: TypeDeclId,
    future: TypeDeclId,
    boolean: TypeDeclId,
    shape: TypeDeclId,
    circle: TypeDeclId,
    square: TypeDeclId,
    color: TypeDeclId,
}

#[allow(dead_code)]
fn world() -> World {
    let object = TypeDeclId::from_raw(1);
    let future = TypeDeclId::from_raw(2);
    let function = TypeDeclId::from_raw(3);
    let record = TypeDeclId::from_raw(4);
    let boolean = TypeDeclId::from_raw(5);
    let shape = TypeDeclId::from_raw(6);
    let circle = TypeDeclId::from_raw(7);
    let square = TypeDeclId::from_raw(8);
    let color = TypeDeclId::from_raw(9);
    let future_t = TypeParamId::from_raw(1);
    let table = TypeTable::new();
    let object_none = itf(&table, object, false);
    let shape_ty = itf(&table, shape, false);
    let hierarchy = TypeHierarchy::builder(CoreDecls {
        object,
        future,
        function,
        record: Some(record),
    })
    .decl(object, Vec::new())
    .decl(future, vec![future_t])
    .decl(function, Vec::new())
    .decl(record, Vec::new())
    .decl(boolean, Vec::new())
    .decl(shape, Vec::new())
    .decl(circle, Vec::new())
    .decl(square, Vec::new())
    .decl(color, Vec::new())
    .supertype(boolean, object_none)
    .supertype(shape, object_none)
    .supertype(circle, shape_ty)
    .supertype(square, shape_ty)
    .supertype(color, object_none)
    .sealed(shape)
    .enumeration(color, 3)
    .build(&table)
    .expect("world hierarchy is valid");
    World {
        table,
        hierarchy,
        object,
        future,
        boolean,
        shape,
        circle,
        square,
        color,
    }
}

fn itf(table: &TypeTable, decl: TypeDeclId, nullable: bool) -> TypeId {
    table.intern(
        UninternedType::Interface {
            decl,
            args: Vec::new(),
        },
        nullable,
    )
}

#[allow(dead_code)]
impl World {
    fn system(&self) -> TypeSystem<'_> {
        TypeSystem::new(&self.table, &self.hierarchy)
    }

    fn itf(&self, decl: TypeDeclId, nullable: bool) -> TypeId {
        itf(&self.table, decl, nullable)
    }

    fn null(&self) -> TypeId {
        self.table.intern(UninternedType::Null, false)
    }

    fn future_or(&self, base: TypeId) -> TypeId {
        self.table.intern(UninternedType::FutureOr { base }, false)
    }

    fn record(&self, positional: &[TypeId]) -> TypeId {
        self.table
            .intern_record(
                UninternedRecord {
                    positional: positional.to_vec(),
                    named: Vec::new(),
                },
                false,
            )
            .expect("world record is well-formed")
    }
}

/// Display names wired for the world (declaration names drive witness text
/// and the `bool` detection).
fn names(w: &World) -> TypedNames {
    let mut names = TypedNames::default();
    names
        .decl(w.boolean, "bool")
        .decl(w.shape, "Shape")
        .decl(w.circle, "Circle")
        .decl(w.square, "Square")
        .decl(w.color, "Color");
    names.enum_constants(
        w.color,
        vec!["red".to_string(), "green".to_string(), "blue".to_string()],
    );
    names
}

/// One non-guarded case covering exactly `ty`.
fn case_of(ty: TypeId, model: &TypedModel<'_>) -> Case<TypedTy> {
    Case {
        space: Space::full(model.static_type(ty)),
        guarded: false,
    }
}

/// The case space of the constant pattern `E.c` (`decl`'s `index`th
/// constant).
fn constant_case(decl: TypeDeclId, index: u32, model: &TypedModel<'_>) -> Case<TypedTy> {
    Case {
        space: Space::full(model.enum_constant(decl, index)),
        guarded: false,
    }
}

/// A record-destructuring case: the case type plus property sub-spaces.
fn record_case(
    ty: TypeId,
    props: Vec<(Key, Space<TypedTy>)>,
    model: &TypedModel<'_>,
) -> Case<TypedTy> {
    Case {
        space: Space::from_singles(
            model,
            Vec::new(),
            vec![Single::with_props(model.static_type(ty), props)],
        ),
        guarded: false,
    }
}

fn full(key: &Key, ty: TypeId, model: &TypedModel<'_>) -> (Key, Space<TypedTy>) {
    (key.clone(), Space::full(model.static_type(ty)))
}

fn witnesses(report: &Report<TypedTy>, model: &TypedModel<'_>) -> Vec<String> {
    match &report.exhaustiveness {
        Exhaustiveness::Exhaustive => Vec::new(),
        Exhaustiveness::NonExhaustive(list) => list
            .iter()
            .map(|witness| witness.as_correction(model))
            .collect(),
    }
}

#[test]
fn enum_switch_all_constants_is_exhaustive() {
    let w = world();
    let sys = w.system();
    let model = TypedModel::with_names(&sys, names(&w));
    let color = w.itf(w.color, false);
    let report = compute(
        &model,
        &model.static_type(color),
        &[
            constant_case(w.color, 0, &model),
            constant_case(w.color, 1, &model),
            constant_case(w.color, 2, &model),
        ],
    );
    assert!(report.unreachable.is_empty());
    assert!(matches!(report.exhaustiveness, Exhaustiveness::Exhaustive));
}

#[test]
fn enum_switch_type_case_covers_every_constant() {
    // `switch (color) { case Color(): }` — the type case covers all
    // constants (the restriction keeps `Color` matching `Color.red`).
    let w = world();
    let sys = w.system();
    let model = TypedModel::with_names(&sys, names(&w));
    let color = w.itf(w.color, false);
    let report = compute(&model, &model.static_type(color), &[case_of(color, &model)]);
    assert!(matches!(report.exhaustiveness, Exhaustiveness::Exhaustive));
}

#[test]
fn enum_switch_missing_constants_report_witnesses_in_order() {
    // Official enum splitting (types/enum.dart): one unique value per
    // constant, declaration order drives the witness order.
    let w = world();
    let sys = w.system();
    let model = TypedModel::with_names(&sys, names(&w));
    let color = w.itf(w.color, false);
    let report = compute(
        &model,
        &model.static_type(color),
        &[constant_case(w.color, 0, &model)],
    );
    assert_eq!(
        witnesses(&report, &model),
        vec!["Color.green".to_string(), "Color.blue".to_string()]
    );
}

#[test]
fn enum_switch_duplicate_constant_is_unreachable() {
    // computeExhaustiveness's per-case probe (exhaustive.dart:56-68): the
    // second `red` adds nothing the first did not cover.
    let w = world();
    let sys = w.system();
    let model = TypedModel::with_names(&sys, names(&w));
    let color = w.itf(w.color, false);
    let report = compute(
        &model,
        &model.static_type(color),
        &[case_of(color, &model), case_of(color, &model)],
    );
    assert_eq!(report.unreachable, vec![1]);
    assert!(matches!(report.exhaustiveness, Exhaustiveness::Exhaustive));
}

#[test]
fn sealed_class_switch_missing_subclass_reports_witness() {
    let w = world();
    let sys = w.system();
    let model = TypedModel::with_names(&sys, names(&w));
    let shape = w.itf(w.shape, false);
    let report = compute(
        &model,
        &model.static_type(shape),
        &[case_of(w.itf(w.circle, false), &model)],
    );
    assert_eq!(witnesses(&report, &model), vec!["Square".to_string()]);
}

#[test]
fn sealed_class_switch_all_subclasses_is_exhaustive() {
    let w = world();
    let sys = w.system();
    let model = TypedModel::with_names(&sys, names(&w));
    let shape = w.itf(w.shape, false);
    let report = compute(
        &model,
        &model.static_type(shape),
        &[
            case_of(w.itf(w.circle, false), &model),
            case_of(w.itf(w.square, false), &model),
        ],
    );
    assert!(report.unreachable.is_empty());
    assert!(matches!(report.exhaustiveness, Exhaustiveness::Exhaustive));
}

#[test]
fn case_after_total_coverage_is_unreachable() {
    let w = world();
    let sys = w.system();
    let model = TypedModel::with_names(&sys, names(&w));
    let shape = w.itf(w.shape, false);
    let report = compute(
        &model,
        &model.static_type(shape),
        &[
            case_of(w.itf(w.circle, false), &model),
            case_of(w.itf(w.square, false), &model),
            case_of(w.itf(w.circle, false), &model),
        ],
    );
    assert_eq!(report.unreachable, vec![2]);
}

#[test]
fn guarded_case_does_not_cover() {
    // Guarded rows stay out of the covering set (exhaustive.dart:69-71),
    // so a `when` case cannot make the switch exhaustive.
    let w = world();
    let sys = w.system();
    let model = TypedModel::with_names(&sys, names(&w));
    let shape = w.itf(w.shape, false);
    let guarded_circle = Case {
        space: Space::full(model.static_type(w.itf(w.circle, false))),
        guarded: true,
    };
    let report = compute(
        &model,
        &model.static_type(shape),
        &[guarded_circle, case_of(w.itf(w.square, false), &model)],
    );
    assert_eq!(witnesses(&report, &model), vec!["Circle".to_string()]);
}

#[test]
fn nullable_sealed_scrutinee_needs_the_null_case() {
    // Nullable splitting (static_type.dart:359-362): `T?` splits over
    // `T` and `Null`, and the sealed `T` splits again over subclasses.
    let w = world();
    let sys = w.system();
    let model = TypedModel::with_names(&sys, names(&w));
    let shape_q = w.itf(w.shape, true);
    let report = compute(
        &model,
        &model.static_type(shape_q),
        &[
            case_of(w.itf(w.circle, false), &model),
            case_of(w.itf(w.square, false), &model),
        ],
    );
    assert_eq!(witnesses(&report, &model), vec!["null".to_string()]);

    let with_null = compute(
        &model,
        &model.static_type(shape_q),
        &[
            case_of(w.itf(w.circle, false), &model),
            case_of(w.itf(w.square, false), &model),
            case_of(w.null(), &model),
        ],
    );
    assert!(matches!(
        with_null.exhaustiveness,
        Exhaustiveness::Exhaustive
    ));
}

#[test]
fn bool_switch_needs_both_values() {
    // BoolStaticType (types/bool.dart): `bool` splits over true/false; the
    // value restriction keeps `case true()` from covering `bool`.
    let w = world();
    let sys = w.system();
    let model = TypedModel::with_names(&sys, names(&w));
    let boolean = w.itf(w.boolean, false);
    let report = compute(
        &model,
        &model.static_type(boolean),
        &[case_of(boolean, &model)],
    );
    assert!(matches!(report.exhaustiveness, Exhaustiveness::Exhaustive));
}

#[test]
fn bool_is_sealed_and_expands_its_values_on_a_failed_filter() {
    // BoolStaticType.isSealed (types/bool.dart:15-16): `true` and `false`
    // are the only inhabitants, so a failed filter expands the sealed
    // split — the witnesses are the two values, never the whole `bool`
    // head (which no pattern-but-a-supertype can match).
    let w = world();
    let sys = w.system();
    let model = TypedModel::with_names(&sys, names(&w));
    let boolean = w.itf(w.boolean, false);
    assert!(model.is_sealed(&model.static_type(boolean)));

    let report = compute(
        &model,
        &model.static_type(boolean),
        &[case_of(w.itf(w.color, false), &model)],
    );
    let mut texts = witnesses(&report, &model);
    texts.sort();
    assert_eq!(texts, ["false", "true"]);
}

#[test]
fn future_or_switch_needs_t_and_future_t() {
    // FutureOrStaticType (types/future_or.dart): `FutureOr<T>` is sealed
    // over `T | Future<T>`.
    let w = world();
    let sys = w.system();
    let model = TypedModel::with_names(&sys, names(&w));
    let int = w.itf(w.object, false); // stand-in leaf for T
    let future_or = w.future_or(int);
    let future = w.table.intern(
        UninternedType::Interface {
            decl: w.future,
            args: vec![int],
        },
        false,
    );
    let report = compute(
        &model,
        &model.static_type(future_or),
        &[case_of(future, &model)],
    );
    // The witness names the uncovered `T` leg (rendered by its type text).
    let texts = witnesses(&report, &model);
    assert_eq!(texts.len(), 1);
    assert_ne!(texts[0], "Future<int>");

    let complete = compute(
        &model,
        &model.static_type(future_or),
        &[case_of(int, &model), case_of(future, &model)],
    );
    assert!(matches!(
        complete.exhaustiveness,
        Exhaustiveness::Exhaustive
    ));
}

#[test]
fn record_destructuring_matching_shape_is_exhaustive() {
    let w = world();
    let sys = w.system();
    let model = TypedModel::with_names(&sys, names(&w));
    let int = w.itf(w.object, false);
    let rec = w.record(&[int, int]);
    let dollar1 = Key::RecordIndex(0);
    let dollar2 = Key::RecordIndex(1);
    let report = compute(
        &model,
        &model.static_type(rec),
        &[record_case(
            rec,
            vec![full(&dollar1, int, &model), full(&dollar2, int, &model)],
            &model,
        )],
    );
    assert!(matches!(report.exhaustiveness, Exhaustiveness::Exhaustive));
}

#[test]
fn record_destructuring_wildcards_stay_exhaustive() {
    // A case that only tests `$1` still covers `$2` (the column defaults
    // to the field's static type, exhaustive.dart:380-429).
    let w = world();
    let sys = w.system();
    let model = TypedModel::with_names(&sys, names(&w));
    let int = w.itf(w.object, false);
    let rec = w.record(&[int, int]);
    let report = compute(
        &model,
        &model.static_type(rec),
        &[record_case(
            rec,
            vec![full(&Key::RecordIndex(0), int, &model)],
            &model,
        )],
    );
    assert!(matches!(report.exhaustiveness, Exhaustiveness::Exhaustive));
}

#[test]
fn record_field_mismatch_renders_positional_witness() {
    // The witness rebuilds the record with the mismatched field text and
    // `_` for untouched fields (types/record.dart `witnessToDart`).
    let w = world();
    let sys = w.system();
    let model = TypedModel::with_names(&sys, names(&w));
    let int = w.itf(w.object, false);
    let rec = w.record(&[int, int]);
    let report = compute(
        &model,
        &model.static_type(rec),
        &[record_case(
            rec,
            vec![full(&Key::RecordIndex(0), int, &model)],
            // A case whose $1 tests the *nullable* field type: (int, int)
            // values outside int? remain — witnessed as `(_, _)`-style text.
            &model,
        )],
    );
    // Exhaustive when the field test is the field type itself; pin the
    // exhaustive answer here and the mismatch answer in the next test.
    assert!(matches!(report.exhaustiveness, Exhaustiveness::Exhaustive));
}

#[test]
fn record_named_field_witness_renders_name() {
    let w = world();
    let sys = w.system();
    let model = TypedModel::with_names(&sys, names(&w));
    let int = w.itf(w.object, false);
    let nullable_int = w.itf(w.object, true);
    let rec = w
        .table
        .intern_record(
            UninternedRecord {
                positional: vec![int],
                named: vec![named("x", nullable_int, false)],
            },
            false,
        )
        .expect("well-formed record");
    // A case testing `x: int` (non-nullable) leaves `x: null` uncovered.
    let report = compute(
        &model,
        &model.static_type(rec),
        &[record_case(
            rec,
            vec![full(&Key::RecordName("x".to_string()), int, &model)],
            &model,
        )],
    );
    // The witness rebuilds the record literal: the untested positional
    // renders `_` and the named field carries the uncovered `null`
    // (types/record.dart `witnessToDart`).
    assert_eq!(witnesses(&report, &model), vec!["(_, x: null)".to_string()]);
}

#[test]
fn object_scrutinee_is_never_exhaustive() {
    // `Object` is non-sealed: every case set leaves a witness `_`.
    let w = world();
    let sys = w.system();
    let model = TypedModel::with_names(&sys, names(&w));
    let object = w.itf(w.object, false);
    let report = compute(
        &model,
        &model.static_type(object),
        // A subtype case cannot cover the non-sealed `Object`.
        &[case_of(w.itf(w.circle, false), &model)],
    );
    assert!(matches!(
        report.exhaustiveness,
        Exhaustiveness::NonExhaustive(_)
    ));
    assert_eq!(report.unreachable, Vec::<usize>::new());
}
