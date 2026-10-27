//! Diagnostic data model: severity, type layering, source locations, and the
//! file-layout abstraction the output writers consume.

use std::cmp::Ordering;
use std::fmt;
use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;

use crate::code::CodeInfo;

/// Index of a file in the engine's file arena, and the half-open source span
/// `[offset, offset + length)` in UTF-16 code units.
///
/// Re-exported from `dartq-source` — the single source of truth for file
/// identity and the engine's position convention; diagnostics never define
/// their own.
pub use dartq_source::{FileId, SourceRange};

/// How severe a diagnostic is, ordered by increasing severity.
///
/// The derived [`Ord`] places `Info < Warning < Error`, matching the oracle's
/// severity escalation order.
///
/// Fact-check against the pinned oracle universe (no fourth variant): upstream
/// `DiagnosticSeverity` defines only NONE/INFO/WARNING/ERROR (upstream Dart
/// SDK source, `pkg/_fe_analyzer_shared/lib/src/base/errors.dart`; NONE is a
/// client sentinel, never carried by a code), the analysis server's LSP fold
/// maps ERROR/WARNING/INFO only (upstream Dart SDK source,
/// `pkg/analysis_server/lib/src/lsp/mapping.dart`), and the CLI exit code
/// consults Error/Warning/Information only (upstream Dart SDK source,
/// `pkg/dartdev/lib/src/commands/analyze.dart`). The former `HintCode` family
/// survives as `DiagnosticType.HINT` codes whose severity folds to INFO
/// (`DiagnosticCodeImpl.severity => type.severity`), so those rows are
/// [`Severity::Info`] here; `HINT` appears in oracle output only as the
/// machine-format `type` field, never as a severity string.
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
/// `COMPILE_TIME_ERROR`, `STATIC_WARNING`, `LINT`, `HINT`, `TODO`).
///
/// `Hint` and `Todo` carry their own machine `TYPE` bytes while folding to
/// [`Severity::Info`] — upstream `DiagnosticType.HINT`/`TODO` are declared
/// with `DiagnosticSeverity.INFO` (upstream Dart SDK source,
/// `pkg/_fe_analyzer_shared/lib/src/base/errors.dart`:284-296), and the
/// machine writer prints severity and type from separate columns
/// (`analyze.dart` `emitMachineFormat`: `INFO|HINT|...`, `INFO|TODO|...`).
///
/// [`DiagnosticType::Tool`] is dartq's own extension, used exclusively by
/// diagnostics in the `dartq/` namespace; it never appears for
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
    /// Extra analysis over code to follow best practices (`HINT`,
    /// INFO-severity upstream).
    Hint,
    /// `TODO`/`FIXME` comment marker (`TODO`, INFO-severity upstream).
    Todo,
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
            Self::Hint => "HINT",
            Self::Todo => "TODO",
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
    ///
    /// The upstream `HINT`/`TODO` types (`deprecated_member_use` and kin,
    /// `import_deferred_library_with_load_function`, `unnecessary_import`,
    /// `deprecated_colon_for_default_value`, `todo`; upstream Dart SDK
    /// source, `pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart` and
    /// `pkg/_fe_analyzer_shared/lib/src/base/errors.dart`:284-296) are
    /// INFO-severity the same way `LINT` is — the machine row keeps `HINT`
    /// /`TODO` in the TYPE column while SEVERITY prints `INFO`.
    #[must_use]
    pub const fn severity(self) -> Severity {
        match self {
            Self::StaticWarning => Severity::Warning,
            Self::Lint | Self::Hint | Self::Todo => Severity::Info,
            _ => Severity::Error,
        }
    }
}

/// The text of a diagnostic row's message or correction: a bare static
/// template or refcount-shared rendered text.
///
/// Sharing contract: string bytes are owned exactly once, at the row's
/// origin — [`Diagnostic::new`] wraps an interpolated `String` into an
/// `Arc<str>` (`Shared`), and static templates are stored as bare references
/// (`Static`, zero heap per row). Every later copy of the row — the
/// per-library result, the semdiag fold, the merged flat output — clones the
/// handle as a refcount bump, never a byte copy. That is what makes the
/// merged output's per-row flat copy cheap: a row clone copies the handles
/// in the struct, not the message and correction bytes behind them.
///
/// Equality and ordering follow the text, not the storage form, so rows
/// compare and sort byte-identically to the previous owned-`String` shape.
/// A dictionary intern table (hash lookup on every read) was considered and
/// rejected for this face: refcount sharing already collapses the
/// duplication without one.
#[derive(Debug, Clone)]
pub enum SharedStr {
    /// A template borrowed verbatim from the code table — zero heap per row.
    Static(&'static str),
    /// Rendered text; the `Arc` owns the bytes and clones are bumps.
    Shared(Arc<str>),
}

impl SharedStr {
    /// The text, independent of the storage form.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Static(text) => text,
            Self::Shared(text) => text,
        }
    }

    /// An owned `String` copy of the text — the wire-format seam
    /// (`PersistedDiagnostic`) and plugin-bridge conversions.
    #[must_use]
    pub fn into_owned(&self) -> String {
        self.as_str().to_owned()
    }
}

impl Deref for SharedStr {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for SharedStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl PartialEq for SharedStr {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for SharedStr {}

impl PartialOrd for SharedStr {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SharedStr {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl PartialEq<str> for SharedStr {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for SharedStr {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl From<&'static str> for SharedStr {
    fn from(text: &'static str) -> Self {
        Self::Static(text)
    }
}

impl From<String> for SharedStr {
    fn from(text: String) -> Self {
        Self::Shared(Arc::from(text))
    }
}

/// One context message attached to a diagnostic — the oracle's
/// `contextMessages` array element (upstream Dart SDK source,
/// `pkg/dartdev/lib/src/commands/analyze.dart` `emitJsonFormat`: each
/// element is `{location: {file, range}, message}`). The range uses the
/// parent diagnostic's convention: UTF-16 code-unit offsets into `file`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMessage {
    /// The context message text.
    pub message: SharedStr,
    /// File the context location points at.
    pub file: FileId,
    /// Location within [`ContextMessage::file`], in UTF-16 code-unit offsets.
    pub range: SourceRange,
}

/// One diagnostic: severity, stable code, message, and location.
///
/// The row is clone-heavy by design — the engine stores hold it behind
/// `Arc`s and the merged output clones it per rebuild — so its text fields
/// are shared handles ([`SharedStr`] message, `Arc<str>` correction):
/// cloning a row bumps refcounts and copies no text bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Stable code identifying the diagnostic.
    pub code: CodeInfo,
    /// problemMessage text. Static code-table templates are stored verbatim
    /// ([`SharedStr::Static`] — zero heap per row); interpolated rows carry
    /// the rendered form shared by refcount ([`SharedStr::Shared`]), wrapped
    /// once at the emit site by [`Diagnostic::new`].
    pub message: SharedStr,
    /// File the diagnostic points at; a sibling of [`Diagnostic::range`],
    /// per the shape.
    pub file: FileId,
    /// Location within [`Diagnostic::file`], in UTF-16 code-unit offsets.
    pub range: SourceRange,
    /// correctionMessage, when the diagnostic template provides one. Shared
    /// by refcount like the message — row clones bump, never copy.
    pub correction: Option<Arc<str>>,
    /// Related locations the oracle emits as `contextMessages`, in the
    /// order the producer reports them. `None` (the default) is the
    /// zero-cost no-context face: 8 bytes on the row, no allocation.
    /// `Some` rows share their element text by the same refcount
    /// discipline as [`Diagnostic::message`].
    pub context: Option<Box<[ContextMessage]>>,
}

impl Diagnostic {
    /// Creates a diagnostic located in `file` at `range`.
    ///
    /// `message` accepts a `&'static str` bare template (stored verbatim,
    /// zero allocation) or a rendered `String` (interpolated rows) — the
    /// emit site picks the cheapest form. Rendered text is wrapped into its
    /// `Arc<str>` exactly once, here at the origin, so every later row clone
    /// (the per-library result, the semdiag fold, the merged flat copy) is a
    /// refcount bump. `correction` takes the rendered `Option<String>` form
    /// and is wrapped the same way.
    ///
    /// `context` defaults to `None`; producers attach related locations
    /// through [`Diagnostic::with_context`].
    #[must_use]
    pub fn new(
        severity: Severity,
        code: CodeInfo,
        message: impl Into<SharedStr>,
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
            correction: correction.map(Arc::from),
            context: None,
        }
    }

    /// Attaches the `contextMessages` rows. An empty slice degrades to
    /// `None` — the oracle emits the key only when the array is non-empty
    /// (`analyze.dart` `emitJsonFormat`: `if (contextMessages.isNotEmpty)`),
    /// so the writer's skip posture and the model agree.
    #[must_use]
    pub fn with_context(mut self, context: Vec<ContextMessage>) -> Self {
        self.context = (!context.is_empty()).then(|| context.into_boxed_slice());
        self
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

    /// The analysis root the compact faces' file locator projects against
    /// (the workspace/context root the run analyzed within): files inside
    /// it are spelled relative to it, files outside it keep the absolute
    /// spelling. `None` (the default) is the no-root face — every file
    /// keeps its absolute spelling and the document carries no `root` key
    /// (multi-root merges, providers with no single root).
    fn analysis_root(&self) -> Option<&Path> {
        None
    }

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
