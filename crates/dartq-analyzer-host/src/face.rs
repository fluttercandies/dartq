//! The rendered-output face: the final stdout bytes of one `dartq analyze`
//! invocation, produced inside the daemon so a resident ask ships ready-to-
//! print bytes instead of a report the client re-renders.
//!
//! The daemon process is the same `dartq` binary as the CLI, so the face
//! writer itself lives in `dartq-commands` (the one-writer contract — the
//! cold in-process path renders through the identical function, never a
//! second renderer). [`crate::daemon::serve`] receives it as a
//! [`FaceRenderer`] fn pointer at start, keeping this crate free of a
//! `dartq-commands` dependency.
//!
//! The wire splits deterministic bytes from run facts: the daemon renders
//! the face payload (cacheable), while everything that is not a pure
//! function of the error map — timings, `--stats`, the `--memory` RSS
//! splice — stays client-side. The rc travels beside the bytes in
//! [`FaceMeta`]'s counts (severity presence over the report rows, the
//! modelable-diagnostic count for the omitted-errors note gate, the
//! unmodelable rows the client prints as `note:` advisories), so the
//! client's fatal-infos/fatal-warnings arithmetic is unchanged.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::report::Row;

/// The output face one `dartq analyze` invocation asks for: the format
/// plus every input the bytes depend on that is not the error map
/// (currently just the analysis root, which the `file`/`heading`
/// spellings relativize against). `serde(default)` keeps pre-face asks
/// (a missing key) on the report path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Face {
    /// The `dart analyze --format` face: `lon` (default), `json`, `text`.
    pub format: FaceFormat,
    /// The run's analysis root — the lon `root:`/`file` and text heading
    /// locator base; `None` spells absolute paths and carries no `root`
    /// key (the multi-root and degenerate-root face).
    #[serde(default)]
    pub root: Option<PathBuf>,
}

/// The `dartq analyze --format` spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FaceFormat {
    /// The canonical LON document (default).
    Lon,
    /// The oracle `--format json` envelope.
    Json,
    /// The grouped human text face.
    Text,
}

/// The counts the client needs to finish the run beside the rendered
/// bytes: the severity exit ladder's inputs, the note gate's modelable
/// count, and the unmodelable rows it prints as advisories. Sent as the
/// `meta` member of the `Answer::Face` header line.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaceMeta {
    /// Any `ERROR`-severity report row (priority and unmodelable rows
    /// included — the exit ladder sees the report, not the printed set).
    pub has_error: bool,
    /// Any `WARNING`-severity report row.
    pub has_warning: bool,
    /// Any `INFO`-severity report row.
    pub has_info: bool,
    /// `priority_errors`: ERROR rows of `pubspec.yaml` /
    /// `analysis_options.yaml` that the faces leave out but the exit code
    /// counts.
    pub priority_errors: u32,
    /// Whether any report row is a modelable `ERROR` diagnostic — the
    /// "found errors its machine and json formats omit" note fires when
    /// the exit is `3` yet this is empty.
    pub has_diag_error: bool,
    /// Report rows the diagnostic model cannot shape (unmodelable
    /// severity or type), each already serialized for the client's
    /// `note: unparsed dart analyze output:` line.
    #[serde(default)]
    pub unparsed: Vec<String>,
}

/// One render's product: the client-facing counts and the stdout bytes
/// (without the trailing newline the line discipline adds — the face
/// payload itself is line-complete: the json envelope and the lon line
/// carry their own `\n`, and the text face's last tail line does).
#[derive(Debug)]
pub struct FaceRender {
    /// The rc and advisory counts beside the bytes.
    pub meta: FaceMeta,
    /// The deterministic stdout payload.
    pub bytes: Vec<u8>,
}

/// The face writer injected into the daemon: one function, the same one
/// the cold in-process path calls, over the report's kept rows in dartdev
/// order. Rows arrive as references so a render never clones row data.
pub type FaceRenderer = fn(&[&Row], &Face) -> FaceRender;
