//! dartiq-types — the Dart type system's minimal M1 surface: the ten-shape
//! type skeleton, nullability as a dimension, canonical interning, the
//! subtype relation, assignability, and the least upper bound.
//!
//! Fact sources (semantic behavior is taken verbatim from these, never from
//! memory): `docs/facts/types-nullability-spec.md` and the vendored SDK under
//! `.third/dart-sdk`, chiefly `pkg/analyzer/lib/src/dart/element/type.dart`
//! ("type.dart"), `subtype.dart`, `least_upper_bound.dart`, and
//! `pkg/analyzer/lib/src/dart/element/type_system.dart` (`type_system.dart`).
//!
//! # Design
//!
//! ## Nullability is a dimension, not a variant
//!
//! Every type carries the two-state `?` projection; `T?` is the same kind
//! with the question flag (`withNullability`, type.dart:1558/1567-1569).
//! This mirrors the official model exactly — the suffix is per-class state,
//! not an enum variant (type.dart:1558). The unit kinds normalize at
//! interning: `dynamic`/`void`/`Invalid`/`Null` are stored non-`?` (their
//! nullability is semantic; type.dart:90-92), and `Never?` is interned as
//! `Null` (official `Never?` behaves as `Null`, type.dart:1207-1209). The
//! legacy `*` suffix does not exist here: dartiq rejects language versions
//! below 2.12, so no legacy types can exist ([`NullabilitySuffix::Star`] is
//! an explicit construction error).
//!
//! ## Canonical interning: O(1) equality (spec §1.3/§4.3)
//!
//! The analyzer's semantic baseline is pure structural equality
//! (`InterfaceTypeImpl.==`, type.dart:876-889; `isStructurallyEqualTo`
//! delegates to `==`, type.dart:1589). dartiq interns on that same structure:
//! the canonical key *is* the structure, so [`TypeId`] equality and hashing
//! are structural equality and hashing, in O(1). This is a dartiq
//! engineering decision (PRD §10.4/§10.5) — it does not change any subtype,
//! assignability, or LUB answer, so byte-for-byte differential testing
//! against `dart analyze` is unaffected. Interned children make structural
//! hashing/equality of compound types shallow: equal children are
//! guaranteed to share one id.
//!
//! Construction only through [`TypeTable::intern`] and friends — the
//! dartiq counterpart of the analyzer's warning against low-level
//! `withNullability` manipulation (type.dart:1608-1612). Bulk data (type
//! arguments, parameters, signatures) lives in append-only side tables so a
//! stored kind stays within the 16-32 byte budget (spec §4.2, PRD §10.5).
//!
//! ## The hierarchy is injected, not discovered
//!
//! Declarations are opaque [`TypeDeclId`] handles; supertype edges arrive as
//! an immutable [`TypeHierarchy`] snapshot built and validated by the layer
//! above. This keeps the dependency direction `types ← semantic` (spec §6,
//! R5): no reverse dependency, no cycles.
//!
//! ## M1 boundaries (spec §5)
//!
//! Implemented: `T?` syntax and the two-state suffix, the subtype rules
//! 0a/0c/1-7/10-13 of `subtype.dart:262-523` (rule 14 ships as the record
//! stub: every non-identical record/record pair is `false`,
//! subtype.dart:512-519), `isAssignableTo`
//! (type_system.dart:891-925) with the `strict-casts` gate, the `FutureOr`
//! rules (subtype.dart:372-385/418-449), the interface walk with invariant
//! arity/covariant arguments (subtype.dart:44-83, class parameters are
//! covariant, element.dart:11560-11561), function subtyping with positional
//! name-sorted correlation (subtype.dart:86-182), `isStrictlyNonNullable`
//! (type_system.dart:1921-1951), and the LUB face (interface chain +
//! nullability merge + `FutureOr` merging).
//!
//! Not implemented (M2): promoted bounds (flow), declared bounds and
//! variance annotations, extension types, `UnknownInferredType`, record
//! subtype/LUB rules (records are construction + equality skeletons),
//! function/function LUB merging (falls back to bare `Function`), and
//! implicit `call` tear-offs in assignability. Each fallback is documented
//! at its site and matches an official fallback path where one exists.
//!
//! The table is single-threaded (interior `RefCell`); the M2 arena story
//! revisits parallelism.

mod error;
mod hierarchy;
mod ids;
mod lub;
mod suffix;
mod system;
mod table;
mod ty;

pub use error::TypeError;
pub use hierarchy::{CoreDecls, DeclInfo, TypeHierarchy, TypeHierarchyBuilder};
pub use ids::{FunctionSigId, RecordShapeId, TypeDeclId, TypeId, TypeParamId};
pub use suffix::NullabilitySuffix;
pub use system::TypeSystem;
pub use table::TypeTable;
pub use ty::{
    FunctionSigData, NamedMember, PoolRange, RecordShapeData, Type, TypeKind, UninternedFunction,
    UninternedRecord, UninternedType,
};
