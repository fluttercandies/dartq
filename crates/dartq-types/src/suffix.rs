//! The nullability suffix dimension.

use crate::error::TypeError;

/// Suffix indicating the nullability of a type, mirroring the analyzer's
/// `NullabilitySuffix`
/// (`_fe_analyzer_shared/lib/src/type_inference/nullability_suffix.dart:20`).
///
/// The suffix is a *syntactic* dimension and is not the same as "nullable":
/// `Null`, `dynamic`, and `void` carry no `?` yet behave as nullable in the
/// subtyping rules, and `S?` reads as `S ∪ Null`
/// (nullability_suffix.dart:9-13). Internally a dartq type stores the
/// two-state projection as a `bool` (spec §1.2/§4.1); this enum exists at the
/// API boundary for analyzer parity.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum NullabilitySuffix {
    /// The canonical representation ends with `?`; the type is unioned with
    /// `Null`.
    Question,
    /// The legacy pre-2.12 suffix. Never stored in a dartq type — the
    /// compiler rejects language versions below 2.12 — see
    /// [`TypeError::LegacyNullability`].
    Star,
    /// No suffix.
    None,
}

impl NullabilitySuffix {
    /// Maps the suffix onto the internal two-state nullability dimension.
    ///
    /// # Errors
    /// Returns [`TypeError::LegacyNullability`] for
    /// [`NullabilitySuffix::Star`]: dartq is a null-safe-only compiler
    /// (PRD §3.7), so legacy types never exist.
    pub fn as_nullable(self) -> Result<bool, TypeError> {
        match self {
            Self::Question => Ok(true),
            Self::None => Ok(false),
            Self::Star => Err(TypeError::LegacyNullability),
        }
    }
}
