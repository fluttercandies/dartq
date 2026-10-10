//! Generic function types: instantiation, the top/bottom replacement walk,
//! and type-argument boundedness.
//!
//! Fact sources (behavior anchored, form native):
//!
//! * `pkg/analyzer/lib/src/dart/element/type.dart:308-333` —
//!   `FunctionTypeImpl.instantiate`: exact arity (the official throws
//!   `ArgumentError`), an empty substitution returns the receiver,
//!   otherwise the signature's type parameters are substituted away and
//!   the result carries none.
//! * `pkg/analyzer/lib/src/dart/element/replace_top_bottom_visitor.dart:13-122`
//!   — `replaceTopAndBottom`: a bottom type (`Never`) in a contravariant
//!   position becomes the top (`Object?`); a top type in any other
//!   position becomes the bottom (`Never`); function parameters flip the
//!   variance (`variance.combine(Variance.contravariant)`, :49-53);
//!   interface arguments are processed with the *incoming* variance — the
//!   official does not fold the parameter's own variance here (:97-100) —
//!   and record types have no arm, so they pass through unchanged (:43).
//!   The FutureOr mapping: the official models `FutureOr<T>` as an
//!   interface type with one argument (type.dart:518), which is exactly
//!   the `FutureOr` arm below.
//! * `pkg/analyzer/lib/src/dart/element/well_bounded.dart:62-129` —
//!   `TypeBoundedHelper`: the regular-bounded pass substitutes the type
//!   arguments into each parameter's bound and demands
//!   `argument <: bound` (:74-119); the super-bounded pass replaces top
//!   and bottom in the whole type and re-runs the regular pass on the
//!   inverted arguments (:121-129) — a type that fails regular but passes
//!   inverted is *super-bounded* (spec §15.2).
//! * `pkg/analyzer/lib/src/error/type_arguments_verifier.dart:476-535` —
//!   the reporting flow the semantic layer wires to
//!   `type_argument_not_matching_bounds` (diagnostic.g.dart:16320-16331):
//!   without super-boundedness every regular issue reports; with it, a
//!   failing regular pass re-checks the inverted arguments and reports
//!   *those* (the inverted argument text is what lands in the message).

use crate::error::TypeError;
use crate::ids::{TypeId, TypeParamId};
use crate::system::TypeSystem;
use crate::table::TypeTable;
use crate::ty::{TypeKind, UninternedFunction, UninternedType};
use crate::variance::{ParamFactsTable, Variance};

/// `FunctionTypeImpl.instantiate` (`type.dart:308-333`): substitutes the
/// signature's declared type parameters with `arguments` throughout the
/// return type and every parameter type, producing a non-generic function
/// type. The interning table's substitution never rewrites a function
/// type's *own* binders (ids are unique per declaration), which is exactly
/// the scoping the official substitution relies on for this shape.
///
/// # Errors
/// [`TypeError::NotFunctionType`] when `fn_ty` is not a function type,
/// [`TypeError::FunctionInstantiationArity`] when
/// `arguments.len() != typeParameters.length` (the official
/// `ArgumentError`, type.dart:311-315).
pub fn instantiate_function(
    table: &TypeTable,
    fn_ty: TypeId,
    arguments: &[TypeId],
) -> Result<TypeId, TypeError> {
    let TypeKind::Function { signature } = table.get(fn_ty).kind() else {
        return Err(TypeError::NotFunctionType {
            actual: table.get(fn_ty).kind(),
        });
    };
    let sig = table.signature(signature);
    let parameters = table.function_type_parameters(sig.type_parameters);
    if arguments.len() != parameters.len() {
        return Err(TypeError::FunctionInstantiationArity {
            expected: parameters.len(),
            actual: arguments.len(),
        });
    }
    if arguments.is_empty() {
        return Ok(fn_ty);
    }
    let map: Vec<(TypeParamId, TypeId)> =
        parameters.into_iter().zip(arguments.iter().copied()).collect();
    let positional: Vec<TypeId> = (0..sig.positional.len())
        .map(|i| table.arg(sig.positional.offset() + i))
        .map(|ty| table.substitute(ty, &map))
        .collect();
    let named = table
        .named_members(sig.named)
        .into_iter()
        .map(|mut member| {
            member.ty = table.substitute(member.ty, &map);
            member
        })
        .collect();
    table.intern_function(
        UninternedFunction {
            return_type: table.substitute(sig.return_type, &map),
            // The substitution instantiates the quantifier away
            // (type.dart:325: `typeParameters: const []`).
            type_parameters: Vec::new(),
            positional,
            named,
            required_positional: sig.required_positional,
        },
        table.get(fn_ty).is_nullable(),
    )
}

/// `replaceTopAndBottom` (`type_system.dart:1629-1636` wrapping
/// `ReplaceTopBottomVisitor.run`, replace_top_bottom_visitor.dart:113-121):
/// top becomes `Never` and bottom becomes `Object?` per the position
/// variance. See the module docs for the anchored decision order.
#[must_use]
pub fn replace_top_bottom(sys: &TypeSystem<'_>, ty: TypeId) -> TypeId {
    let object_question = sys.table().intern(
        UninternedType::Interface {
            decl: sys.hierarchy().core().object,
            args: Vec::new(),
        },
        true,
    );
    let never = sys.table().intern(UninternedType::Never, false);
    process_top_bottom(sys, ty, Variance::Covariant, object_question, never, 0)
}

fn process_top_bottom(
    sys: &TypeSystem<'_>,
    ty: TypeId,
    variance: Variance,
    top: TypeId,
    bottom: TypeId,
    depth: u32,
) -> TypeId {
    const DEPTH_LIMIT: u32 = crate::system::DEPTH_LIMIT;
    if depth > DEPTH_LIMIT {
        return ty;
    }
    // The position probe (replace_top_bottom_visitor.dart:20-33).
    if variance == Variance::Contravariant {
        if sys.is_subtype(ty, bottom) {
            return top;
        }
    } else if sys.is_top(ty) {
        return bottom;
    }

    match sys.table().get(ty).kind() {
        TypeKind::Interface { decl, args } => {
            if args.is_empty() {
                return ty;
            }
            let new_args: Vec<TypeId> = (0..args.len())
                .map(|i| {
                    let arg = sys.table().arg(args.offset() + i);
                    // The incoming variance, unchanged per argument — the
                    // official interface arm ignores the parameter's own
                    // variance (:97-100).
                    process_top_bottom(sys, arg, variance, top, bottom, depth + 1)
                })
                .collect();
            sys.table().intern(
                UninternedType::Interface { decl, args: new_args },
                sys.table().get(ty).is_nullable(),
            )
        }
        // `FutureOr<T>` is the official interface shape with one argument
        // (type.dart:518): the base is processed with the incoming
        // variance.
        TypeKind::FutureOr { base } => {
            let new_base = process_top_bottom(sys, base, variance, top, bottom, depth + 1);
            if new_base == base {
                ty
            } else {
                sys.table()
                    .intern(UninternedType::FutureOr { base: new_base }, sys.table().get(ty).is_nullable())
            }
        }
        TypeKind::Function { signature } => {
            let sig = sys.table().signature(signature);
            let new_return =
                process_top_bottom(sys, sig.return_type, variance, top, bottom, depth + 1);
            let new_positional: Vec<TypeId> = (0..sig.positional.len())
                .map(|i| {
                    let p = sys.table().arg(sig.positional.offset() + i);
                    process_top_bottom(
                        sys,
                        p,
                        variance.combine(Variance::Contravariant),
                        top,
                        bottom,
                        depth + 1,
                    )
                })
                .collect();
            let new_named = sys
                .table()
                .named_members(sig.named)
                .into_iter()
                .map(|mut member| {
                    member.ty = process_top_bottom(
                        sys,
                        member.ty,
                        variance.combine(Variance::Contravariant),
                        top,
                        bottom,
                        depth + 1,
                    );
                    member
                })
                .collect();
            sys.table()
                .intern_function(
                    UninternedFunction {
                        return_type: new_return,
                        // The signature's own parameters are kept verbatim
                        // (:56 — `typeParameters: type.typeParameters`).
                        type_parameters: sys.table().function_type_parameters(sig.type_parameters),
                        positional: new_positional,
                        named: new_named,
                        required_positional: sig.required_positional,
                    },
                    sys.table().get(ty).is_nullable(),
                )
                .expect("the rebuilt signature preserves the validated shape")
        }
        // Records have no arm in the official visitor
        // (replace_top_bottom_visitor.dart:43) — pass through unchanged.
        _ => ty,
    }
}

/// One violated bound (the official `TypeArgumentIssue`,
/// well_bounded.dart:27-45).
#[derive(Clone, Copy, Debug)]
pub struct TypeArgumentIssue {
    /// The argument's position in the instantiation.
    pub index: usize,
    /// The parameter whose bound was violated.
    pub parameter: TypeParamId,
    /// The bound after substituting the type arguments in.
    pub bound: TypeId,
    /// The violating argument.
    pub argument: TypeId,
}

/// `_isRegularBounded`'s issue collection (`well_bounded.dart:74-119`):
/// for each parameter with a declared bound, substitute the arguments in
/// and demand `argument <: bound`. dartq carries no alias kinds, so the
/// official alias arm (well_bounded.dart:80-85) is unreachable here and
/// only interface instantiations carry parameters — other kinds answer
/// with no issues (well_bounded.dart:89-91).
#[must_use]
pub fn regular_bounded_issues(
    sys: &TypeSystem<'_>,
    facts: &ParamFactsTable,
    ty: TypeId,
) -> Vec<TypeArgumentIssue> {
    let TypeKind::Interface { decl, args } = sys.table().get(ty).kind() else {
        return Vec::new();
    };
    let parameters = sys
        .hierarchy()
        .decl(decl)
        .expect("declared owner")
        .type_parameters
        .clone();
    let arguments: Vec<TypeId> = (0..args.len()).map(|i| sys.table().arg(args.offset() + i)).collect();
    let map: Vec<(TypeParamId, TypeId)> =
        parameters.iter().copied().zip(arguments.iter().copied()).collect();
    let mut issues = Vec::new();
    for (i, (&parameter, &argument)) in parameters.iter().zip(arguments.iter()).enumerate() {
        let Some(declared_bound) = facts.get(parameter).bound else {
            continue;
        };
        let bound = sys.table().substitute(declared_bound, &map);
        if !sys.is_subtype(argument, bound) {
            issues.push(TypeArgumentIssue {
                index: i,
                parameter,
                bound,
                argument,
            });
        }
    }
    issues
}

/// `TypeBoundedHelper.isWellBounded` (`well_bounded.dart:62-72`): the
/// regular pass, then — where super-bounded types are allowed — the
/// inverted pass over `replaceTopAndBottom` of the whole type. The caller
/// decides `allow_super_bounded` exactly where the official does
/// (`_shouldAllowSuperBoundedTypes`, type_arguments_verifier.dart:736-753:
/// supertype clauses, type aliases, and extension types forbid it).
#[must_use]
pub fn is_well_bounded(
    sys: &TypeSystem<'_>,
    facts: &ParamFactsTable,
    ty: TypeId,
    allow_super_bounded: bool,
) -> Boundedness {
    let issues = regular_bounded_issues(sys, facts, ty);
    if issues.is_empty() {
        return Boundedness::RegularBounded;
    }
    if !allow_super_bounded {
        return Boundedness::NotWellBounded(issues);
    }
    // `_isSuperBounded` (well_bounded.dart:121-129): re-run the regular
    // pass on the inverted type. A non-interface inversion (the type was
    // itself top) reads regular-bounded, i.e. super-bounded.
    let inverted = replace_top_bottom(sys, ty);
    let inverted_issues = regular_bounded_issues(sys, facts, inverted);
    if inverted_issues.is_empty() {
        Boundedness::SuperBounded
    } else {
        // The verifier reports the inverted arguments
        // (type_arguments_verifier.dart:502-535).
        Boundedness::NotWellBounded(inverted_issues)
    }
}

/// The `TypeBoundedResult` family (well_bounded.dart:12-25, 132-134).
#[derive(Clone, Debug)]
pub enum Boundedness {
    /// `RegularBoundedTypeResult` — every argument satisfies its bound.
    RegularBounded,
    /// `SuperBoundedTypeResult` — regular fails, the inverted pass passes.
    SuperBounded,
    /// `NotWellBoundedTypeResult` — the failing pass's issues; with
    /// super-boundedness allowed these come from the *inverted*
    /// arguments.
    NotWellBounded(Vec<TypeArgumentIssue>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hierarchy::{CoreDecls, TypeHierarchy};
    use crate::variance::ParamFacts;
    use crate::ids::TypeDeclId;

    const OBJECT: TypeDeclId = TypeDeclId::from_raw(1);
    const FUTURE: TypeDeclId = TypeDeclId::from_raw(2);
    const FUNCTION: TypeDeclId = TypeDeclId::from_raw(3);
    const INT: TypeDeclId = TypeDeclId::from_raw(4);
    const STRING: TypeDeclId = TypeDeclId::from_raw(5);
    const G: TypeDeclId = TypeDeclId::from_raw(6);

    fn fixture() -> (TypeTable, TypeHierarchy) {
        let table = TypeTable::new();
        let hierarchy = TypeHierarchy::builder(CoreDecls {
            object: OBJECT,
            future: FUTURE,
            function: FUNCTION,
            record: None,
        })
        .decl(OBJECT, Vec::new())
        .decl(FUTURE, vec![TypeParamId::from_raw(1)])
        .decl(FUNCTION, Vec::new())
        .decl(INT, Vec::new())
        .decl(STRING, Vec::new())
        .decl(G, vec![TypeParamId::from_raw(2)])
        .build(&table)
        .expect("fixture hierarchy is valid");
        (table, hierarchy)
    }

    fn itf(table: &TypeTable, decl: TypeDeclId, args: &[TypeId]) -> TypeId {
        table.intern(
            UninternedType::Interface {
                decl,
                args: args.to_vec(),
            },
            false,
        )
    }

    /// `T Function(T)` instantiated at `int` is `int Function(int)` with
    /// the quantifier gone (type.dart:308-333).
    #[test]
    fn gft_instantiation_substitutes_and_drops_the_binder() {
        let (table, _) = fixture();
        let t = TypeParamId::from_raw(10);
        let t_ty = table.intern(UninternedType::TypeParameter { parameter: t }, false);
        let int = itf(&table, INT, &[]);
        let identity = table
            .intern_function(
                UninternedFunction {
                    return_type: t_ty,
                    type_parameters: vec![t],
                    positional: vec![t_ty],
                    named: Vec::new(),
                    required_positional: 1,
                },
                false,
            )
            .expect("signature is well-formed");
        let instantiated = instantiate_function(&table, identity, &[int]).expect("arity matches");
        let sig = match table.get(instantiated).kind() {
            TypeKind::Function { signature } => table.signature(signature),
            other => panic!("expected a function kind, got {other:?}"),
        };
        assert!(sig.type_parameters.is_empty(), "the binder must be gone");
        assert!(matches!(
            table.get(sig.return_type).kind(),
            TypeKind::Interface { decl: INT, .. }
        ));
        assert!(matches!(
            table.get(table.arg(sig.positional.offset())).kind(),
            TypeKind::Interface { decl: INT, .. }
        ));
    }

    #[test]
    fn gft_instantiation_rejects_wrong_arity_and_non_functions() {
        let (table, _) = fixture();
        let t = TypeParamId::from_raw(11);
        let t_ty = table.intern(UninternedType::TypeParameter { parameter: t }, false);
        let identity = table
            .intern_function(
                UninternedFunction {
                    return_type: t_ty,
                    type_parameters: vec![t],
                    positional: Vec::new(),
                    named: Vec::new(),
                    required_positional: 0,
                },
                false,
            )
            .expect("signature is well-formed");
        let int = itf(&table, INT, &[]);
        assert!(matches!(
            instantiate_function(&table, identity, &[int, int]),
            Err(TypeError::FunctionInstantiationArity { expected: 1, actual: 2 })
        ));
        assert!(matches!(
            instantiate_function(&table, int, &[int]),
            Err(TypeError::NotFunctionType { .. })
        ));
        // The arity check precedes the empty short-circuit
        // (type.dart:311-318): zero arguments against one type parameter
        // is the ArgumentError.
        assert!(matches!(
            instantiate_function(&table, identity, &[]),
            Err(TypeError::FunctionInstantiationArity { expected: 1, actual: 0 })
        ));
        // The empty short-circuit (type.dart:316-318) fires only for
        // non-generic functions: the receiver returns itself.
        let non_generic = table
            .intern_function(
                UninternedFunction {
                    return_type: int,
                    type_parameters: Vec::new(),
                    positional: vec![int],
                    named: Vec::new(),
                    required_positional: 1,
                },
                false,
            )
            .expect("signature is well-formed");
        assert!(matches!(
            instantiate_function(&table, non_generic, &[]),
            Ok(ty) if ty == non_generic
        ));
    }

    /// The replacement walk (replace_top_bottom_visitor.dart:20-61): tops
    /// in covariant positions become `Never`, `Never` in contravariant
    /// positions becomes `Object?`, and the function parameter position
    /// flips the variance.
    #[test]
    fn replace_top_bottom_flips_tops_and_bottoms_by_position() {
        let (table, hierarchy) = fixture();
        let sys = TypeSystem::new(&table, &hierarchy);
        let dynamic = table.intern(UninternedType::Dynamic, false);
        let void = table.intern(UninternedType::Void, false);
        let never = table.intern(UninternedType::Never, false);

        // Top in a covariant position → Never.
        assert!(matches!(
            table.get(replace_top_bottom(&sys, dynamic)).kind(),
            TypeKind::Never
        ));
        assert!(matches!(
            table.get(replace_top_bottom(&sys, void)).kind(),
            TypeKind::Never
        ));
        // Bottom (`Never`) stays: not a top, covariant position has no
        // bottom rule.
        assert!(matches!(
            table.get(replace_top_bottom(&sys, never)).kind(),
            TypeKind::Never
        ));

        // `dynamic Function(Never)` → `Never Function(Object?)`: the
        // return is a top in covariant position; the parameter is a
        // bottom in contravariant position.
        let fn_ty = table
            .intern_function(
                UninternedFunction {
                    return_type: dynamic,
                    type_parameters: Vec::new(),
                    positional: vec![never],
                    named: Vec::new(),
                    required_positional: 1,
                },
                false,
            )
            .expect("signature is well-formed");
        let inverted = replace_top_bottom(&sys, fn_ty);
        let sig = match table.get(inverted).kind() {
            TypeKind::Function { signature } => table.signature(signature),
            other => panic!("expected a function kind, got {other:?}"),
        };
        assert!(matches!(table.get(sig.return_type).kind(), TypeKind::Never));
        assert!(matches!(
            table.get(table.arg(sig.positional.offset())).kind(),
            TypeKind::Interface { decl: OBJECT, .. } if table.get(table.arg(sig.positional.offset())).is_nullable()
        ));

        // Nesting: `FutureOr<dynamic>` is top as a whole → Never (the
        // position probe fires before the structure walk).
        let future_or = table.intern(UninternedType::FutureOr { base: dynamic }, false);
        assert!(matches!(
            table.get(replace_top_bottom(&sys, future_or)).kind(),
            TypeKind::Never
        ));

        // Records pass through unchanged (no arm,
        // replace_top_bottom_visitor.dart:43).
        let record = table
            .intern_record(
                crate::ty::UninternedRecord {
                    positional: vec![dynamic],
                    named: Vec::new(),
                },
                false,
            )
            .expect("record shape is well-formed");
        assert_eq!(replace_top_bottom(&sys, record), record);
    }

    /// The regular-bounded pass (well_bounded.dart:74-119): arguments are
    /// checked against their substituted bounds; violations carry the
    /// index, parameter, substituted bound, and argument.
    #[test]
    fn regular_bounded_collects_violations_with_substituted_bounds() {
        let (table, hierarchy) = fixture();
        let sys = TypeSystem::new(&table, &hierarchy);
        let mut facts = ParamFactsTable::new();
        let g_t = TypeParamId::from_raw(2);
        facts.set(
            g_t,
            ParamFacts {
                variance: Variance::Covariant,
                bound: Some(itf(&table, INT, &[])),
            },
        );
        let int = itf(&table, INT, &[]);
        let string = itf(&table, STRING, &[]);
        assert!(regular_bounded_issues(&sys, &facts, itf(&table, G, &[int])).is_empty());
        let issues = regular_bounded_issues(&sys, &facts, itf(&table, G, &[string]));
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].index, 0);
        assert_eq!(issues[0].parameter, g_t);
        assert!(matches!(
            table.get(issues[0].argument).kind(),
            TypeKind::Interface { decl: STRING, .. }
        ));
    }

    /// `isWellBounded` (well_bounded.dart:62-72): without super-boundedness
    /// the regular issues report; with it, the inverted pass decides — a
    /// bound-violating argument that is itself neither top nor bottom
    /// stays violated after inversion.
    #[test]
    fn well_bounded_runs_the_super_pass_only_where_allowed() {
        let (table, hierarchy) = fixture();
        let sys = TypeSystem::new(&table, &hierarchy);
        let mut facts = ParamFactsTable::new();
        let g_t = TypeParamId::from_raw(2);
        facts.set(
            g_t,
            ParamFacts {
                variance: Variance::Covariant,
                bound: Some(itf(&table, INT, &[])),
            },
        );
        let string = itf(&table, STRING, &[]);
        let bad = itf(&table, G, &[string]);
        assert!(matches!(
            is_well_bounded(&sys, &facts, bad, false),
            Boundedness::NotWellBounded(_)
        ));
        // The inverted type `G<String>` carries the same (unchanged)
        // argument, so the super pass fails too.
        assert!(matches!(
            is_well_bounded(&sys, &facts, bad, true),
            Boundedness::NotWellBounded(_)
        ));
        // A satisfied instantiation is regular-bounded without entering
        // the super pass.
        let good = itf(&table, G, &[itf(&table, INT, &[])]);
        assert!(matches!(
            is_well_bounded(&sys, &facts, good, true),
            Boundedness::RegularBounded
        ));
    }
}
