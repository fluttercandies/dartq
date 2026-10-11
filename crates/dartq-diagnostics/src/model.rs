//! Diagnostic data model: severity, type layering, source locations, and the
//! file-layout abstraction the output writers consume.

use std::borrow::Cow;
use std::fmt;
use std::path::Path;

use crate::code::CodeInfo;

/// Index of a file in the engine's file arena, and the half-open source span
/// `[offset, offset + length)` in UTF-16 code units (PRD §6/§7/§10.4).
///
/// Re-exported from `dartq-source` — the single source of truth for file
/// identity and the engine's position convention; diagnostics never define
/// their own.
pub use dartq_source::{FileId, SourceRange};

/// How severe a diagnostic is, ordered by increasing severity.
///
/// The derived [`Ord`] places `Info < Warning < Error`, matching the oracle's
/// severity escalation order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Informational (lints, hints); never fatal.
    Info,
    /// Warning; fatal for `dart analyze` by default.
    Warning,
    /// Error; compilation cannot proceed.
    Error,
}

impl Severity {
    /// `UPPERCASE` form used by the machine format and the JSON `severity`
    /// field (both are uppercase in the oracle).
    #[must_use]
    pub const fn as_machine_str(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warning => "WARNING",
            Self::Error => "ERROR",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Info => "Info",
            Self::Warning => "Warning",
            Self::Error => "Error",
        })
    }
}

/// The analyzer diagnostic family a code belongs to — the machine format's
/// `TYPE` field value set as measured from the oracle (`SYNTACTIC_ERROR`,
/// `COMPILE_TIME_ERROR`, `STATIC_WARNING`, `LINT`).
///
/// [`DiagnosticType::Tool`] is dartq's own extension, used exclusively by
/// diagnostics in the `dartq/` namespace (PRD §13.2); it never appears for
/// official codes and therefore never enters the oracle differential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DiagnosticType {
    /// Syntax-level failure produced by scanning/parsing recovery.
    SyntacticError,
    /// Static semantic violation that makes the program invalid.
    CompileTimeError,
    /// Suspicious-but-legal construct.
    StaticWarning,
    /// Style/best-practice rule violation.
    Lint,
    /// dartq-own runtime/tooling diagnostic (`dartq/` namespace only).
    Tool,
}

impl DiagnosticType {
    /// `UPPERCASE_UNDERSCORE` form used by the machine format and the JSON
    /// `type` field (both are uppercase in the oracle).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SyntacticError => "SYNTACTIC_ERROR",
            Self::CompileTimeError => "COMPILE_TIME_ERROR",
            Self::StaticWarning => "STATIC_WARNING",
            Self::Lint => "LINT",
            Self::Tool => "TOOL",
        }
    }
}

impl fmt::Display for DiagnosticType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl DiagnosticType {
    /// Oracle severity for a registry-true family (the
    /// `dtype_matches_oracle_severity` invariant: `COMPILE_TIME_ERROR` rows
    /// are oracle-ERROR, `STATIC_WARNING` rows oracle-WARNING, `LINT` rows
    /// oracle-INFO). The single derivation of its mapping — dartq-semantic's
    /// per-code `severity` folds onto this method (`dtype().severity()`).
    /// `Tool` is the dartq-own fallback and maps to ERROR.
    #[must_use]
    pub const fn severity(self) -> Severity {
        match self {
            Self::StaticWarning => Severity::Warning,
            Self::Lint => Severity::Info,
            _ => Severity::Error,
        }
    }
}

/// One diagnostic: severity, stable code, message, and location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Stable code identifying the diagnostic (PRD §13.2 naming discipline).
    pub code: CodeInfo,
    /// problemMessage text. `Borrowed` rows carry a bare template verbatim
    /// (no interpolation — zero heap per row); `Owned` rows carry the
    /// interpolated form, rendered once at the emit site (a scratch buffer
    /// can be moved in, no copy). Moving to the engine-wide string intern
    /// table (`StringId`) remains the recorded follow-up.
    pub message: Cow<'static, str>,
    /// File the diagnostic points at; a sibling of [`Diagnostic::range`],
    /// per the PRD §13.1 shape.
    pub file: FileId,
    /// Location within [`Diagnostic::file`], in UTF-16 code-unit offsets.
    pub range: SourceRange,
    /// correctionMessage, when the diagnostic template provides one.
    pub correction: Option<String>,
}

impl Diagnostic {
    /// Creates a diagnostic located in `file` at `range`.
    ///
    /// `message` accepts a `&'static str` bare template (borrowed verbatim,
    /// zero allocation) or an owned rendered `String` (interpolated rows) —
    /// the emit site picks the cheapest form; `Cow::Owned` also accepts a
    /// scratch buffer moved in via `mem::take`, so per-row-group rendering
    /// reuses one buffer's capacity.
    #[must_use]
    pub fn new(
        severity: Severity,
        code: CodeInfo,
        message: impl Into<Cow<'static, str>>,
        file: FileId,
        range: SourceRange,
        correction: Option<String>,
    ) -> Self {
        Self {
            severity,
            code,
            message: message.into(),
            file,
            range,
            correction,
        }
    }
}

/// Read-side view of the file set the diagnostics refer to.
///
/// Implemented by the caller (CLI) on top of `dartq-source`'s line index; the
/// output writers depend only on this trait, never on the scanner.
pub trait FileLayoutProvider {
    /// Path of the file as emitted in output. The oracle prints canonical
    /// absolute paths (symlinks resolved); implementations must supply the
    /// same normalization.
    ///
    /// # Panics
    /// Implementations may panic on an unknown `FileId`; callers contractually
    /// only pass IDs that participate in the analysis.
    fn file_path(&self, file: FileId) -> &Path;

    /// 1-based `(line, column)` of a UTF-16 code-unit offset.
    ///
    /// # Panics
    /// Implementations may panic on an offset past the end of the file.
    fn line_col(&self, file: FileId, utf16_offset: u32) -> (u32, u32);
}

/// Everything the output writers consume: the diagnostic list plus the file
/// layout needed to render locations.
#[derive(Clone, Copy)]
pub struct AnalysisOutput<'a> {
    pub diagnostics: &'a [Diagnostic],
    pub files: &'a dyn FileLayoutProvider,
}
