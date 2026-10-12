//! The provability predicates behind the null/never WARNING family: for a
//! `(static type, context)` pair, can the analyzer prove the diagnostic's
//! question at compile time?
//!
//! One face serves the seven WARNING codes (semantic emitters wire one code
//! to one row of the table below). All names are the *current* official code
//! names, verified in
//! `upstream Dart SDK source, pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart` —
//! `unnecessary_non_null_assertion` (:17504) and `unnecessary_type_check`
//! (:17642/:17655) are the renamed family members; the `is Null`/`is! Null`
//! warning's code name is `type_check_with_null` (:16358, unique names
//! `type_check_is_null`/`type_check_is_not_null`), not
//! `"unnecessary_type_check_with_null"`.
//!
//! | code | facet | official decision order |
//! |---|---|---|
//! | `unnecessary_cast` | provably equal (plus exclusions) | `_isUnnecessaryCast`, best_practices_verifier.dart:1708-1741, emitted :178 |
//! | `unnecessary_non_null_assertion` | provably non-null | `_checkForUnnecessaryNullAware`, error_verifier.dart:7668-7718 — the `isStrictlyNonNullable` gate |
//! | `unnecessary_type_check` | provably subtype (or `dynamic` on the right) | `_checkAllTypeChecks`, best_practices_verifier.dart:987-1041 |
//! | `type_check_with_null` | checked type is `Null` | same producer :1022-1036 |
//! | `null_check_always_fails` | provably null (`x!` on a `Null`-typed operand) | `visitNullAssertionExpression`, best_practices_verifier.dart:795-799 |
//! | `receiver_of_type_never` | provably never | property_element_resolver.dart:101-107 (also :436-440 and the assignment/increment faces) |
//! | `unnecessary_null_comparison` | provably non-null | `_checkForInvariantNullComparison`, best_practices_verifier.dart:1216-1281 |
//!
//! The three core facets delegate to the existing [`TypeSystem`] relations:
//! provably non-null is [`TypeSystem::is_strictly_non_nullable`] (the
//! `isStrictlyNonNullable` port, type_system.dart:1921-1951), provably never
//! is [`TypeSystem::is_bottom`], and provably subtype is
//! [`TypeSystem::is_subtype`]. This module adds only the per-code decision
//! orders around them (they are diagnostic-shaped: exclusion rules and
//! interplays the raw relations do not encode) plus
//! [`TypeSystem::can_be_equal`] — the constant-pattern face of the
//! pattern-space cluster (#507 P2), which needs
//! [`TypeSystem::eliminate_to_greatest`] (the `PatternGreatestClosureHelper`
//! port, least_greatest_closure.dart:98-130).
//!
//! AST-side nuances stay on the emitter side and are called out per method:
//! the `null`-literal refinements of the type-check and null-comparison
//! orders, the `super`/extension-override arms of the null-comparison order,
//! and the `hasPrimitiveEquality` gate plus the name-level
//! int-constant-vs-double-value cross in front of
//! [`TypeSystem::can_be_equal`].

use crate::ids::TypeId;
use crate::system::TypeSystem;
use crate::ty::{NamedMember, TypeKind, UninternedFunction, UninternedRecord, UninternedType};

/// The verdict of the `is`/`is!` check walk
/// (`_checkAllTypeChecks`, best_practices_verifier.dart:987-1041). The
/// caller maps a verdict plus the source polarity (`is` vs `is!`) to the
/// code: [`TypeCheckVerdict::AlwaysTrue`] fires `unnecessary_type_check` on
/// `is` and on `is!`-with-known-false it fires the `_false` variant
/// ([`TypeCheckVerdict::AlwaysFalse`]); [`TypeCheckVerdict::NullCheck`] is
/// the `type_check_with_null` family, downgraded by the caller to
/// [`TypeCheckVerdict::AlwaysTrue`]/[`TypeCheckVerdict::AlwaysFalse`] when
/// the operand is the `null` literal (an AST fact this face cannot see).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeCheckVerdict {
    /// The check is informative — no warning.
    Informative,
    /// The check's answer is a compile-time `true`.
    AlwaysTrue,
    /// The check's answer is a compile-time `false`.
    AlwaysFalse,
    /// The checked type is `Null`: the official reports
    /// `type_check_with_null` ("use `== null`/`!= null`") unless the operand
    /// is the `null` literal, in which case the check is constant and
    /// `unnecessary_type_check` fires instead.
    NullCheck,
}

impl TypeSystem<'_> {
    /// `unnecessary_cast`: `_isUnnecessaryCast`
    /// (best_practices_verifier.dart:1708-1741). Decision order: either side
    /// `Invalid` → no (already-reported errors); the operand type not equal
    /// to the target type → no (the cast is necessary); `dynamic` → no (the
    /// explicit cast is the recommended `avoid_dynamic_calls` form); bare
    /// `Function` → no (same recommendation). Equality is structural type
    /// identity including nullability (`x as int?` over `int` is necessary),
    /// which interned ids answer in O(1).
    #[must_use]
    pub fn is_unnecessary_cast(&self, operand: TypeId, target: TypeId) -> bool {
        let operand_ty = self.table().get(operand);
        let target_ty = self.table().get(target);
        if matches!(operand_ty.kind(), TypeKind::Invalid)
            || matches!(target_ty.kind(), TypeKind::Invalid)
        {
            return false;
        }
        if operand != target {
            return false;
        }
        if matches!(target_ty.kind(), TypeKind::Dynamic) {
            return false;
        }
        !self.is_bare_function_id(target)
    }

    /// `unnecessary_non_null_assertion`: `x!` on a target the analyzer can
    /// prove never null (error_verifier.dart:7668-7718 — the
    /// `_checkForUnnecessaryNullAware` gate `!isStrictlyNonNullable(target)
    /// → return`, emitted for the nullCheck kind :8823-8831). The `super`
    /// exemption (the check skips super targets) is an AST fact the emitter
    /// applies.
    #[must_use]
    pub fn is_unnecessary_null_assertion(&self, target: TypeId) -> bool {
        self.is_strictly_non_nullable(target)
    }

    /// The `is`/`is!` check walk (`_checkAllTypeChecks`,
    /// best_practices_verifier.dart:987-1041), in source order: `Invalid`
    /// operand → informative; `dynamic` checked type → always true;
    /// `Invalid` checked type → informative; `Null` checked type →
    /// [`TypeCheckVerdict::NullCheck`]; operand subtype of the checked type
    /// → the check is constant-true. A subtype-negative verdict does not
    /// exist in the official order — a non-matching pair is simply
    /// informative.
    #[must_use]
    pub fn type_check_verdict(&self, operand: TypeId, checked: TypeId) -> TypeCheckVerdict {
        let operand_ty = self.table().get(operand);
        if matches!(operand_ty.kind(), TypeKind::Invalid) {
            return TypeCheckVerdict::Informative;
        }
        let checked_ty = self.table().get(checked);
        if matches!(checked_ty.kind(), TypeKind::Dynamic) {
            return TypeCheckVerdict::AlwaysTrue;
        }
        if matches!(checked_ty.kind(), TypeKind::Invalid) {
            return TypeCheckVerdict::Informative;
        }
        if matches!(checked_ty.kind(), TypeKind::Null) {
            return TypeCheckVerdict::NullCheck;
        }
        if self.is_subtype(operand, checked) {
            return TypeCheckVerdict::AlwaysTrue;
        }
        TypeCheckVerdict::Informative
    }

    /// `null_check_always_fails`: `x!` where the operand's static type is
    /// `Null` (best_practices_verifier.dart:795-799 —
    /// `operand.typeOrThrow.isDartCoreNull`). `Never` is not enough: only
    /// the `Null` class fails the assertion, everything else either
    /// succeeds or the code is a different diagnostic.
    #[must_use]
    pub fn is_null_check_always_fails(&self, operand: TypeId) -> bool {
        self.is_null(operand)
    }

    /// `receiver_of_type_never`: a member access on a receiver whose static
    /// type is `Never` (property_element_resolver.dart:101-107 —
    /// `receiverType is NeverType && suffix none` after `resolveToBound`).
    /// M1 deviation: `resolveToBound` is the identity here — declared
    /// bounds are not modeled, so a bare type parameter cannot resolve to
    /// `Never` (the bound face arrives with M2 bounds).
    #[must_use]
    pub fn is_receiver_of_type_never(&self, receiver: TypeId) -> bool {
        self.is_bottom(receiver)
    }

    /// `unnecessary_null_comparison`: the operand side of `null == x` /
    /// `x == null` whose never-null-ness the analyzer can prove
    /// (best_practices_verifier.dart:1216-1281). The official tests
    /// `isStrictlyNonNullable` on the non-literal side (`super` answers
    /// `true` and an extension override `false` without consulting the
    /// type — AST facts the emitter applies; this face is the type-level
    /// arm).
    #[must_use]
    pub fn is_unnecessary_null_comparison(&self, operand: TypeId) -> bool {
        self.is_strictly_non_nullable(operand)
    }

    /// The constant-pattern face `_canBeEqual`
    /// (constant_verifier.dart:561-596): `false` proves
    /// `constant == value` always answers `false`, firing
    /// `constant_pattern_never_matches_value_type`. Decision order: the
    /// constant type must be an interface type (the analyzer's `Null` is
    /// one; the `hasPrimitiveEquality` gate upstream — int/double/String/
    /// bool/Null/enum constants — is an emitter-side fact); then
    /// interface value → subtype over the value's greatest closure;
    /// type-parameter value → `true` (M1 has no declared bounds, so this
    /// is the official's own boundless fallback: no warning); function or
    /// record value → nullable iff the constant is `Null`; every other
    /// shape → `true` (no warning).
    ///
    /// Emitter-side composition: the official's int-constant-vs-double-value
    /// cross (`constantType.isDartCoreInt && valueType.isDartCoreDouble`
    /// → `true`) is a NAME question — `CoreDecls` keeps declarations
    /// opaque — so the emitter applies it before consulting this face
    /// (`case 1` over `double` must not warn). Without the cross this face
    /// answers `is_subtype(int, double)` = `false` there.
    #[must_use]
    pub fn can_be_equal(&self, constant: TypeId, value: TypeId) -> bool {
        let constant_ty = self.table().get(constant);
        let value_ty = self.table().get(value);
        let constant_is_null = matches!(constant_ty.kind(), TypeKind::Null);
        if !constant_is_null && !matches!(constant_ty.kind(), TypeKind::Interface { .. }) {
            return true;
        }
        match value_ty.kind() {
            TypeKind::Interface { .. } => {
                let greatest = self.eliminate_to_greatest(value);
                self.is_subtype(constant, greatest)
            }
            TypeKind::Function { .. } | TypeKind::Record { .. } => {
                if constant_is_null {
                    self.is_nullable(value)
                } else {
                    false
                }
            }
            // The official recurses on a type parameter's (promoted) bound
            // when it is fully concrete; with no bounds modeled the
            // boundless fallback — like every other kind here — is the same
            // answer: no warning.
            _ => true,
        }
    }

    /// The pattern greatest closure (`PatternGreatestClosureHelper.
    /// eliminateToGreatest`, least_greatest_closure.dart:98-130): replaces
    /// every type-parameter type inside `t` — covariant positions with the
    /// top type (`Object?`), contravariant positions with the bottom type
    /// (`Never`) — so the result is a supertype of `t` for all values of
    /// the parameters. Variance flips once around a function type's whole
    /// parameter list (replacement_visitor.dart:242-260); interface
    /// arguments, `FutureOr` bases, and record fields stay covariant
    /// (`replacement_visitor.dart:360`/`:433` have no flip). A replacement
    /// unites nullabilities with the parameter's own `?`
    /// (least_greatest_closure.dart:123-128) — `T?` in a contravariant
    /// position is `Never?`, which interns as `Null` (type.dart:1207-1209).
    ///
    /// M1 deviations (bounds are M2): function-declared parameter bounds
    /// are not descended (replacement_visitor.dart:175-186) and extension
    /// types do not exist. Types free of parameters return unchanged
    /// without interning anything.
    #[must_use]
    pub fn eliminate_to_greatest(&self, t: TypeId) -> TypeId {
        self.eliminate(t, true)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "port of eliminateToGreatest's visitor; the arm order mirrors \
                  replacement_visitor.dart, so the body stays one function"
    )]
    fn eliminate(&self, t: TypeId, covariant: bool) -> TypeId {
        let ty = self.table().get(t);
        match ty.kind() {
            TypeKind::TypeParameter { .. } => {
                let replacement = if covariant {
                    self.object_question()
                } else {
                    self.table().intern(UninternedType::Never, false)
                };
                if ty.is_nullable() {
                    self.make_nullable(replacement)
                } else {
                    replacement
                }
            }
            TypeKind::Interface { decl, args } => {
                let mut rebuilt: Vec<TypeId> = Vec::new();
                let mut changed = false;
                for i in 0..args.len() {
                    let argument = self.table().arg(args.offset() + i);
                    let eliminated = self.eliminate(argument, covariant);
                    changed |= eliminated != argument;
                    rebuilt.push(eliminated);
                }
                if changed {
                    self.table().intern(
                        UninternedType::Interface {
                            decl,
                            args: rebuilt,
                        },
                        ty.is_nullable(),
                    )
                } else {
                    t
                }
            }
            TypeKind::FutureOr { base } => {
                let eliminated = self.eliminate(base, covariant);
                if eliminated == base {
                    t
                } else {
                    self.table().intern(
                        UninternedType::FutureOr { base: eliminated },
                        ty.is_nullable(),
                    )
                }
            }
            TypeKind::Function { signature } => {
                let sig = self.table().signature(signature);
                let return_type = self.eliminate(sig.return_type, covariant);
                // One flip around the whole parameter list
                // (replacement_visitor.dart:242-260): positional and named
                // parameters share the contravariant position.
                let positional: Vec<TypeId> = (0..sig.positional.len())
                    .map(|i| {
                        let parameter = self.table().arg(sig.positional.offset() + i);
                        self.eliminate(parameter, !covariant)
                    })
                    .collect();
                let named: Vec<NamedMember> = self
                    .table()
                    .named_members(sig.named)
                    .into_iter()
                    .map(|member| NamedMember {
                        name: member.name,
                        ty: self.eliminate(member.ty, !covariant),
                        required: member.required,
                    })
                    .collect();
                let changed = return_type != sig.return_type
                    || (0..sig.positional.len())
                        .any(|i| self.table().arg(sig.positional.offset() + i) != positional[i]);
                let named_changed = {
                    let members = self.table().named_members(sig.named);
                    members.len() != named.len()
                        || members
                            .iter()
                            .zip(&named)
                            .any(|(a, b)| a.name != b.name || a.ty != b.ty)
                };
                if !changed && !named_changed {
                    return t;
                }
                self.table()
                    .intern_function(
                        UninternedFunction {
                            return_type,
                            type_parameters: self
                                .table()
                                .function_type_parameters(sig.type_parameters),
                            positional,
                            named,
                            required_positional: sig.required_positional,
                        },
                        ty.is_nullable(),
                    )
                    .expect("the rebuilt signature keeps the source signature's validity")
            }
            TypeKind::Record { shape } => {
                let shape_data = self.table().record_shape(shape);
                let positional: Vec<TypeId> = (0..shape_data.positional.len())
                    .map(|i| {
                        let field = self.table().arg(shape_data.positional.offset() + i);
                        self.eliminate(field, covariant)
                    })
                    .collect();
                let named: Vec<NamedMember> = self
                    .table()
                    .named_members(shape_data.named)
                    .into_iter()
                    .map(|field| NamedMember {
                        name: field.name,
                        ty: self.eliminate(field.ty, covariant),
                        required: field.required,
                    })
                    .collect();
                let members = self.table().named_members(shape_data.named);
                let changed = (0..shape_data.positional.len())
                    .any(|i| self.table().arg(shape_data.positional.offset() + i) != positional[i])
                    || members.iter().zip(&named).any(|(a, b)| a.ty != b.ty);
                if !changed {
                    return t;
                }
                self.table()
                    .intern_record(UninternedRecord { positional, named }, ty.is_nullable())
                    .expect("the rebuilt shape keeps the source shape's validity")
            }
            _ => t,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hierarchy::{CoreDecls, TypeHierarchy};
    use crate::ids::{TypeDeclId, TypeParamId};
    use crate::table::TypeTable;

    const OBJECT: TypeDeclId = TypeDeclId::from_raw(1);
    const FUTURE: TypeDeclId = TypeDeclId::from_raw(2);
    const FUNCTION: TypeDeclId = TypeDeclId::from_raw(3);
    const RECORD: TypeDeclId = TypeDeclId::from_raw(4);
    const NUM: TypeDeclId = TypeDeclId::from_raw(10);
    const INT: TypeDeclId = TypeDeclId::from_raw(11);
    const STRING: TypeDeclId = TypeDeclId::from_raw(12);
    const SEALED: TypeDeclId = TypeDeclId::from_raw(20);
    const SUB_A: TypeDeclId = TypeDeclId::from_raw(21);
    const SUB_B: TypeDeclId = TypeDeclId::from_raw(22);
    const ENUM: TypeDeclId = TypeDeclId::from_raw(30);
    const OTHER: TypeDeclId = TypeDeclId::from_raw(40);

    fn itf(table: &TypeTable, decl: TypeDeclId, args: &[TypeId]) -> TypeId {
        table.intern(
            UninternedType::Interface {
                decl,
                args: args.to_vec(),
            },
            false,
        )
    }

    fn fixture() -> (TypeTable, TypeHierarchy) {
        let table = TypeTable::new();
        let param = TypeParamId::from_raw(1);
        let hierarchy = TypeHierarchy::builder(CoreDecls {
            object: OBJECT,
            future: FUTURE,
            function: FUNCTION,
            record: Some(RECORD),
        })
        .decl(OBJECT, Vec::new())
        .decl(FUTURE, vec![param])
        .decl(FUNCTION, Vec::new())
        .decl(RECORD, Vec::new())
        .decl(NUM, Vec::new())
        .decl(INT, Vec::new())
        .decl(STRING, Vec::new())
        .decl(SEALED, Vec::new())
        .decl(SUB_A, Vec::new())
        .decl(SUB_B, Vec::new())
        .decl(ENUM, Vec::new())
        .decl(OTHER, Vec::new())
        .supertype(NUM, itf(&table, OBJECT, &[]))
        .supertype(INT, itf(&table, NUM, &[]))
        .supertype(STRING, itf(&table, OBJECT, &[]))
        .supertype(SEALED, itf(&table, OBJECT, &[]))
        .supertype(SUB_A, itf(&table, SEALED, &[]))
        .supertype(SUB_B, itf(&table, SEALED, &[]))
        .supertype(ENUM, itf(&table, OBJECT, &[]))
        .supertype(OTHER, itf(&table, OBJECT, &[]))
        .sealed(SEALED)
        .enumeration(ENUM, 2)
        .build(&table)
        .expect("fixture hierarchy is valid");
        (table, hierarchy)
    }

    #[test]
    fn unnecessary_cast_pins() {
        let (table, hierarchy) = fixture();
        let sys = TypeSystem::new(&table, &hierarchy);
        let int = itf(&table, INT, &[]);
        let int_q = table.intern(
            UninternedType::Interface {
                decl: INT,
                args: vec![],
            },
            true,
        );
        let num = itf(&table, NUM, &[]);
        let dynamic = table.intern(UninternedType::Dynamic, false);
        let invalid = table.intern(UninternedType::Invalid, false);
        let bare_function = itf(&table, FUNCTION, &[]);

        // `int as int` — equal, no exclusions.
        assert!(sys.is_unnecessary_cast(int, int));
        // `int as int?` — nullability is part of identity; necessary.
        assert!(!sys.is_unnecessary_cast(int, int_q));
        // `int as num` — necessary.
        assert!(!sys.is_unnecessary_cast(int, num));
        // `x as dynamic` — the recommended `avoid_dynamic_calls` form.
        assert!(!sys.is_unnecessary_cast(int, dynamic));
        // `f as Function` — same recommendation.
        assert!(!sys.is_unnecessary_cast(bare_function, bare_function));
        // Already-reported errors on either side.
        assert!(!sys.is_unnecessary_cast(invalid, int));
        assert!(!sys.is_unnecessary_cast(int, invalid));
    }

    #[test]
    fn provably_non_null_facet_pins() {
        let (table, hierarchy) = fixture();
        let sys = TypeSystem::new(&table, &hierarchy);
        let int = itf(&table, INT, &[]);
        let int_q = table.intern(
            UninternedType::Interface {
                decl: INT,
                args: vec![],
            },
            true,
        );
        let dynamic = table.intern(UninternedType::Dynamic, false);
        let null = table.intern(UninternedType::Null, false);
        let never_q = table.intern(UninternedType::Never, true); // interned as Null
        let future_or_int = table.intern(UninternedType::FutureOr { base: int }, false);
        let future_or_int_q = table.intern(UninternedType::FutureOr { base: int_q }, false);
        let param = TypeParamId::from_raw(2);
        let t = table.intern(UninternedType::TypeParameter { parameter: param }, false);

        for (ty, expect) in [
            (int, true),
            (int_q, false),
            (dynamic, false),
            (null, false),
            (never_q, false),
            (future_or_int, true),
            (future_or_int_q, false),
            (t, false), // no declared bounds in M1
        ] {
            assert_eq!(sys.is_unnecessary_null_assertion(ty), expect, "{ty:?}");
            assert_eq!(sys.is_unnecessary_null_comparison(ty), expect, "{ty:?}");
        }
    }

    #[test]
    fn provably_null_and_never_facet_pins() {
        let (table, hierarchy) = fixture();
        let sys = TypeSystem::new(&table, &hierarchy);
        let int = itf(&table, INT, &[]);
        let int_q = table.intern(
            UninternedType::Interface {
                decl: INT,
                args: vec![],
            },
            true,
        );
        let null = table.intern(UninternedType::Null, false);
        let never = table.intern(UninternedType::Never, false);

        // `x!` fails only on the Null class; `Never` is a different code.
        assert!(sys.is_null_check_always_fails(null));
        assert!(!sys.is_null_check_always_fails(never));
        assert!(!sys.is_null_check_always_fails(int));
        assert!(!sys.is_null_check_always_fails(int_q));

        // A receiver is provably dead only at the bottom type. `Never?`
        // interns as `Null`, whose member accesses are a different face.
        assert!(sys.is_receiver_of_type_never(never));
        assert!(!sys.is_receiver_of_type_never(null));
        assert!(!sys.is_receiver_of_type_never(int));
    }

    #[test]
    fn type_check_verdict_pins() {
        let (table, hierarchy) = fixture();
        let sys = TypeSystem::new(&table, &hierarchy);
        let int = itf(&table, INT, &[]);
        let int_q = table.intern(
            UninternedType::Interface {
                decl: INT,
                args: vec![],
            },
            true,
        );
        let num = itf(&table, NUM, &[]);
        let dynamic = table.intern(UninternedType::Dynamic, false);
        let null = table.intern(UninternedType::Null, false);
        let invalid = table.intern(UninternedType::Invalid, false);

        assert_eq!(
            sys.type_check_verdict(int, int),
            TypeCheckVerdict::AlwaysTrue
        );
        assert_eq!(
            sys.type_check_verdict(int, num),
            TypeCheckVerdict::AlwaysTrue
        );
        // Downcast: informative.
        assert_eq!(
            sys.type_check_verdict(num, int),
            TypeCheckVerdict::Informative
        );
        // `is dynamic` — always true.
        assert_eq!(
            sys.type_check_verdict(int, dynamic),
            TypeCheckVerdict::AlwaysTrue
        );
        // `is Null` — the `type_check_with_null` family (the caller downgrades
        // to AlwaysTrue when the operand is the `null` literal).
        assert_eq!(
            sys.type_check_verdict(int, null),
            TypeCheckVerdict::NullCheck
        );
        assert_eq!(
            sys.type_check_verdict(null, null),
            TypeCheckVerdict::NullCheck
        );
        // Already-reported errors.
        assert_eq!(
            sys.type_check_verdict(invalid, num),
            TypeCheckVerdict::Informative
        );
        assert_eq!(
            sys.type_check_verdict(int, invalid),
            TypeCheckVerdict::Informative
        );
        // Nullable operand against non-nullable check: informative.
        assert_eq!(
            sys.type_check_verdict(int_q, int),
            TypeCheckVerdict::Informative
        );
    }

    #[test]
    fn can_be_equal_pins() {
        let (table, hierarchy) = fixture();
        let sys = TypeSystem::new(&table, &hierarchy);
        let int = itf(&table, INT, &[]);
        let int_q = table.intern(
            UninternedType::Interface {
                decl: INT,
                args: vec![],
            },
            true,
        );
        let string = itf(&table, STRING, &[]);
        let null = table.intern(UninternedType::Null, false);
        let enum_ty = itf(&table, ENUM, &[]);

        let param = TypeParamId::from_raw(3);
        let t = table.intern(UninternedType::TypeParameter { parameter: param }, false);
        let fn_ty = table
            .intern_function(
                UninternedFunction {
                    return_type: int,
                    type_parameters: Vec::new(),
                    positional: Vec::new(),
                    named: Vec::new(),
                    required_positional: 0,
                },
                false,
            )
            .expect("valid signature");
        let fn_q = table
            .intern_function(
                UninternedFunction {
                    return_type: int,
                    type_parameters: Vec::new(),
                    positional: Vec::new(),
                    named: Vec::new(),
                    required_positional: 0,
                },
                true,
            )
            .expect("valid signature");
        let record = table
            .intern_record(
                UninternedRecord {
                    positional: vec![int],
                    named: Vec::new(),
                },
                false,
            )
            .expect("valid shape");

        // Interface value: subtype over the greatest closure.
        assert!(sys.can_be_equal(int, int));
        assert!(!sys.can_be_equal(string, int));
        assert!(!sys.can_be_equal(int, string));
        // `case null` over a non-nullable value never matches; over `int?`
        // it can.
        assert!(!sys.can_be_equal(null, int));
        assert!(sys.can_be_equal(null, int_q));
        // Function/record values: match only `null` constants over nullable
        // function/record types.
        assert!(!sys.can_be_equal(null, fn_ty));
        assert!(sys.can_be_equal(null, fn_q));
        assert!(!sys.can_be_equal(null, record));
        // Type-parameter value: no bounds modeled — the official boundless
        // fallback (no warning).
        assert!(sys.can_be_equal(int, t));
        // FutureOr and every other value shape: conservative no-warning.
        assert!(sys.can_be_equal(
            int,
            table.intern(UninternedType::FutureOr { base: int }, false)
        ));
        // Enum constants carry primitive equality; the enum type as constant
        // type follows the interface arm.
        assert!(sys.can_be_equal(enum_ty, sys.hierarchy().sentinels().object_none));
        // Documented emitter-side contract: the int-constant-vs-double-value
        // cross is a NAME fact. Without it the face answers the plain
        // subtype question (false here); the emitter must apply the cross
        // by name before consulting this face.
        assert!(!sys.can_be_equal(int, string));
    }

    #[test]
    fn eliminate_to_greatest_pins() {
        let (table, hierarchy) = fixture();
        let sys = TypeSystem::new(&table, &hierarchy);
        let int = itf(&table, INT, &[]);
        let param = TypeParamId::from_raw(4);
        let t = table.intern(UninternedType::TypeParameter { parameter: param }, false);
        let t_q = table.intern(UninternedType::TypeParameter { parameter: param }, true);
        let object_question = table.intern(
            UninternedType::Interface {
                decl: OBJECT,
                args: vec![],
            },
            true,
        );
        let never = table.intern(UninternedType::Never, false);
        let null = table.intern(UninternedType::Null, false); // `Never?`

        // Covariant parameter → the top type; `T?` unites to the same `?`.
        assert_eq!(sys.eliminate_to_greatest(t), object_question);
        assert_eq!(sys.eliminate_to_greatest(t_q), object_question);

        // Contravariant position → the bottom type; `T?` in a contravariant
        // position is `Never?`, interned as `Null`.
        let fn_of = |parameter: TypeId, nullable: bool| {
            table
                .intern_function(
                    UninternedFunction {
                        return_type: int,
                        type_parameters: Vec::new(),
                        positional: vec![parameter],
                        named: Vec::new(),
                        required_positional: 1,
                    },
                    nullable,
                )
                .expect("valid signature")
        };
        assert_eq!(
            sys.eliminate_to_greatest(fn_of(t, false)),
            fn_of(never, false)
        );
        assert_eq!(
            sys.eliminate_to_greatest(fn_of(t_q, false)),
            fn_of(null, false)
        );

        // Variance flips once per function visit: `T` inside
        // `void Function(void Function(T))` sits in a covariant position
        // again (two flips).
        let outer = table
            .intern_function(
                UninternedFunction {
                    return_type: int,
                    type_parameters: Vec::new(),
                    positional: vec![fn_of(t, false)],
                    named: Vec::new(),
                    required_positional: 1,
                },
                false,
            )
            .expect("valid signature");
        let outer_expected = table
            .intern_function(
                UninternedFunction {
                    return_type: int,
                    type_parameters: Vec::new(),
                    positional: vec![fn_of(object_question, false)],
                    named: Vec::new(),
                    required_positional: 1,
                },
                false,
            )
            .expect("valid signature");
        assert_eq!(sys.eliminate_to_greatest(outer), outer_expected);

        // Parameter-free types pass through without interning anything.
        let int_q = table.intern(
            UninternedType::Interface {
                decl: INT,
                args: vec![],
            },
            true,
        );
        let before = table.len();
        assert_eq!(sys.eliminate_to_greatest(int), int);
        assert_eq!(sys.eliminate_to_greatest(int_q), int_q);
        assert_eq!(table.len(), before);
    }
}
