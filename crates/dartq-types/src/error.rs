//! Explicit errors for type construction and hierarchy snapshot validation.

use crate::ids::{TypeDeclId, TypeParamId};
use crate::ty::TypeKind;

/// Errors raised when interning types or building a [`crate::TypeHierarchy`].
///
/// Every variant is a caller-contract violation, not a recoverable runtime
/// condition: the table refuses to intern malformed input so that every stored
/// [`crate::Type`] is well-formed by construction.
///
/// Deliberately not `PartialEq`: [`TypeError::SupertypeNotInterface`] embeds
/// a [`TypeKind`], and kinds hold pool positions, not values (see the
/// [`TypeKind`] docs). Match on the variant instead.
#[derive(Debug, Clone, thiserror::Error)]
pub enum TypeError {
    /// The legacy `*` suffix (pre-2.12 libraries; see
    /// `nullability_suffix.dart:20-35`) cannot appear in a dartq type: the
    /// compiler rejects language versions below 2.12 (PRD §3.7), so legacy
    /// types never exist.
    #[error(
        "legacy `*` nullability suffix is not supported: dartq only accepts \
         null-safe libraries (SDK < 2.12 is rejected)"
    )]
    LegacyNullability,

    /// A supertype edge references a declaration that was never declared.
    #[error("supertype references undeclared type declaration #{0}")]
    UndeclaredDecl(TypeDeclId),

    /// The same declaration was declared twice in the builder.
    #[error("type declaration #{0} is declared more than once")]
    DuplicateDecl(TypeDeclId),

    /// Supertype edges must be interface types.
    #[error("supertype of declaration #{decl} must be an interface type, got {actual:?}")]
    SupertypeNotInterface {
        /// The declaration whose supertype edge is malformed.
        decl: TypeDeclId,
        /// The kind that was supplied instead.
        actual: TypeKind,
    },

    /// An interface supertype carries the wrong number of type arguments.
    #[error(
        "supertype of declaration #{decl} expects {expected} type argument(s), \
         got {actual}"
    )]
    ArityMismatch {
        /// The declaration instantiated with the wrong arity.
        decl: TypeDeclId,
        /// Arity declared by the target declaration.
        expected: usize,
        /// Arity of the supplied supertype.
        actual: usize,
    },

    /// A supertype edge references a type parameter that the declaring class
    /// does not own; instantiation at query time could not bind it.
    #[error(
        "supertype of declaration #{decl} references type parameter #{param}, \
         which is not declared by #{decl}"
    )]
    ForeignTypeParameter {
        /// The declaration owning the supertype edge.
        decl: TypeDeclId,
        /// The unbound type parameter referenced by the edge.
        param: TypeParamId,
    },

    /// The core `Object` declaration received a supertype edge. `Object` is
    /// the hierarchy root — it must reach `build` without supertypes so the
    /// LUB rule can anchor `Object` at unique depth 1
    /// (`least_upper_bound.dart:327-357`).
    #[error("core `Object` declaration #{0} cannot declare a supertype")]
    ObjectSupertype(TypeDeclId),

    /// A supertype edge carried the `?` projection. Edges feed the LUB
    /// supertype closures verbatim, and a nullable id there would break the
    /// unique-depth invariant the max-unique-depth rule relies on
    /// (`least_upper_bound.dart:327-357`).
    #[error("supertype of declaration #{0} must not be nullable (`?`)")]
    NullableSupertype(TypeDeclId),

    /// The supertype graph contains a cycle; subtype walks and
    /// longest-inheritance-path computation would not terminate.
    #[error("supertype graph contains a cycle: {0:?}")]
    CyclicHierarchy(Vec<TypeDeclId>),

    /// Two named members of one function or record share a name.
    #[error("duplicate named member `{0}`")]
    DuplicateNamedMember(String),

    /// The required positional parameter count exceeds the number of
    /// positional parameters (`requiredPositionalParameterCount`,
    /// `type.dart:132`).
    #[error(
        "required positional parameter count {required} exceeds the {total} \
         positional parameters"
    )]
    RequiredPositionalOutOfBounds {
        /// The supplied required count.
        required: u32,
        /// The number of positional parameters.
        total: usize,
    },

    /// A generic-function instantiation was handed a type that is not a
    /// function type (`FunctionTypeImpl.instantiate` is a function-type
    /// method, `type.dart:308`).
    #[error("generic instantiation requires a function type, got {actual:?}")]
    NotFunctionType {
        /// The kind that was supplied instead.
        actual: TypeKind,
    },

    /// A generic-function instantiation received the wrong number of type
    /// arguments — the official `ArgumentError`
    /// (`type.dart:311-315`).
    #[error(
        "type argument count {actual} does not match the {expected} type \
         parameter(s) of the function type"
    )]
    FunctionInstantiationArity {
        /// The signature's type-parameter count.
        expected: usize,
        /// The supplied argument count.
        actual: usize,
    },
}
