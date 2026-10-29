//! Serde model of the Dart VM Service Protocol wire shapes — only the
//! types this crate consumes are modeled, per the upstream spec
//! ("Dart VM Service Protocol", `runtime/vm/service/service.md`).
//!
//! Discriminated deserialization uses the protocol's `"type"` property
//! (the `@`-prefixed names are references). Unknown tags and unknown
//! enum variants degrade to `Unknown`/`Other` captures rather than
//! parse failures: adding event kinds and instance kinds is a
//! backwards-compatible protocol change, so a single unknown message
//! must never take the connection down.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Deserializes the `kind` string into a known variant or `Other` with
/// the unrecognized spelling preserved. Used for enums the spec marks
/// as backwards-compatibly extendable (`EventKind`, `InstanceKind`,
/// `SentinelKind`, `FrameKind`).
fn de_kind<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer)
}

/// The protocol `Version` object (`getVersion` result).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    /// Potentially-incompatible-change counter.
    pub major: u64,
    /// Backwards-compatible-change counter.
    pub minor: u64,
}

/// `Success` is the empty response (`{"type":"Success"}`); nothing to
/// deserialize beyond the tag.
#[derive(Debug, Clone, Copy)]
pub struct Success;

/// `SentinelKind`: `Collected`, `Expired`, `NotInitialized`,
/// `BeingInitialized`, `OptimizedOut`, `Free`, or a forwards-compatible
/// unknown kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SentinelKind {
    /// The referent was GC'd or its isolate exited.
    Collected,
    /// A temporary id lived past its id-zone eviction.
    Expired,
    /// A variable/field has not been initialized yet.
    NotInitialized,
    /// Deprecated.
    BeingInitialized,
    /// Eliminated by the optimizing compiler.
    OptimizedOut,
    /// Reserved.
    Free,
    /// A kind added in a newer minor protocol version.
    Other(String),
}

/// A `Sentinel` object: the normal response is not available, and this
/// is a normal condition rather than an error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sentinel {
    /// What kind of sentinel this is.
    pub kind: SentinelKind,
    /// Human-readable stand-in for the unavailable value.
    #[serde(default)]
    pub value_as_string: Option<String>,
}

impl<'de> Deserialize<'de> for SentinelKind {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match de_kind(deserializer)?.as_str() {
            "Collected" => Self::Collected,
            "Expired" => Self::Expired,
            "NotInitialized" => Self::NotInitialized,
            "BeingInitialized" => Self::BeingInitialized,
            "OptimizedOut" => Self::OptimizedOut,
            "Free" => Self::Free,
            other => Self::Other(other.to_owned()),
        })
    }
}

impl SentinelKind {
    /// The wire spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Collected => "Collected",
            Self::Expired => "Expired",
            Self::NotInitialized => "NotInitialized",
            Self::BeingInitialized => "BeingInitialized",
            Self::OptimizedOut => "OptimizedOut",
            Self::Free => "Free",
            Self::Other(other) => other,
        }
    }
}

impl Serialize for SentinelKind {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

/// `@Isolate` reference: enough information to address the isolate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IsolateRef {
    /// Opaque isolate id (getObject/getIsolate handle).
    pub id: String,
    /// Unique numeric id, as a string.
    #[serde(default)]
    pub number: Option<String>,
    /// Debugging name (not unique).
    #[serde(default)]
    pub name: Option<String>,
    /// Whether the VM spawned this isolate for internal use.
    #[serde(default)]
    pub is_system_isolate: bool,
    /// Opaque id of the isolate's group.
    #[serde(default)]
    pub isolate_group_id: Option<String>,
}

/// A full `Isolate` object (`getIsolate` result). Only the fields the
/// session consumes are modeled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Isolate {
    /// Opaque isolate id.
    pub id: String,
    /// Unique numeric id, as a string.
    #[serde(default)]
    pub number: Option<String>,
    /// Debugging name.
    #[serde(default)]
    pub name: Option<String>,
    /// Whether this is a VM-internal isolate.
    #[serde(default)]
    pub is_system_isolate: bool,
    /// Isolate group id.
    #[serde(default)]
    pub isolate_group_id: Option<String>,
    /// Runnable bit: debugger RPCs fail before this is true.
    #[serde(default)]
    pub runnable: bool,
    /// Whether the isolate pauses on exit.
    #[serde(default)]
    pub pause_on_exit: bool,
    /// The isolate's most recent pause/resume event.
    #[serde(default)]
    pub pause_event: Option<Event>,
    /// Root library reference (initialized at IsolateRunnable).
    #[serde(default)]
    pub root_lib: Option<LibraryRef>,
    /// All breakpoints currently set on this isolate.
    #[serde(default)]
    pub breakpoints: Vec<Breakpoint>,
    /// Current exception pause mode.
    #[serde(default)]
    pub exception_pause_mode: Option<ExceptionPauseMode>,
    /// The error that killed the isolate, if it is dying.
    #[serde(default)]
    pub error: Option<Value>,
}

/// `ExceptionPauseMode`: `None`, `Unhandled`, `All`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExceptionPauseMode {
    /// Do not pause on thrown exceptions.
    None,
    /// Pause on unhandled exceptions only.
    Unhandled,
    /// Pause on all thrown exceptions.
    All,
}

impl ExceptionPauseMode {
    /// The wire spelling used in `setIsolatePauseMode` params.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Unhandled => "Unhandled",
            Self::All => "All",
        }
    }
}

impl<'de> Deserialize<'de> for ExceptionPauseMode {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Unknown spellings fall back to `None` — the mode only widens
        // pause behavior, never narrows it.
        Ok(match de_kind(deserializer)?.as_str() {
            "Unhandled" => Self::Unhandled,
            "All" => Self::All,
            _ => Self::None,
        })
    }
}

impl serde::Serialize for ExceptionPauseMode {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

/// `StepOption`: `Into`, `Over`, `OverAsyncSuspension`, `Out`, `Rewind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepOption {
    /// Single step, entering function calls.
    Into,
    /// Single step, skipping over calls.
    Over,
    /// Step over an await/yield suspension point.
    OverAsyncSuspension,
    /// Step until the current function exits.
    Out,
    /// Exit the top frames without executing; paused at the call site.
    Rewind,
}

impl StepOption {
    /// The wire spelling used in `resume` params.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Into => "Into",
            Self::Over => "Over",
            Self::OverAsyncSuspension => "OverAsyncSuspension",
            Self::Out => "Out",
            Self::Rewind => "Rewind",
        }
    }
}

impl serde::Serialize for StepOption {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

/// `@Library` reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryRef {
    /// Opaque object id.
    pub id: String,
    /// Library name (may be empty for the anonymous root).
    #[serde(default)]
    pub name: Option<String>,
    /// `package:`/`file:`/`dart:` URI.
    #[serde(default)]
    pub uri: Option<String>,
}

/// `@Script` reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptRef {
    /// Opaque object id (fixed id; passed to getObject).
    pub id: String,
    /// Source URI.
    #[serde(default)]
    pub uri: Option<String>,
}

/// A `SourceLocation`: script plus token position, with optional
/// resolved line/column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceLocation {
    /// The script containing the location.
    #[serde(default)]
    pub script: Option<ScriptRef>,
    /// First token of the location.
    #[serde(default)]
    pub token_pos: Option<i64>,
    /// Last token, if this is a range.
    #[serde(default)]
    pub end_token_pos: Option<i64>,
    /// 1-based line (non-synthetic positions only).
    #[serde(default)]
    pub line: Option<i64>,
    /// 1-based column (non-synthetic positions only).
    #[serde(default)]
    pub column: Option<i64>,
}

/// An `UnresolvedSourceLocation`: the approximate location carried by
/// unresolved breakpoints. Either `script` or `script_uri` is present.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnresolvedSourceLocation {
    /// Script reference, when the script has been loaded.
    #[serde(default)]
    pub script: Option<ScriptRef>,
    /// Script URI, when the script has not been loaded yet.
    #[serde(default)]
    pub script_uri: Option<String>,
    /// Approximate token position.
    #[serde(default)]
    pub token_pos: Option<i64>,
    /// Approximate 1-based line.
    #[serde(default)]
    pub line: Option<i64>,
    /// Approximate 1-based column (present only when the breakpoint
    /// was column-specific).
    #[serde(default)]
    pub column: Option<i64>,
}

/// `Breakpoint.location`: resolved or unresolved.
#[derive(Debug, Clone, PartialEq)]
pub enum BreakpointLocation {
    /// A `SourceLocation` — the breakpoint resolved to a concrete
    /// program position.
    Resolved(SourceLocation),
    /// An `UnresolvedSourceLocation` — approximate position until the
    /// code is compiled/loaded.
    Unresolved(UnresolvedSourceLocation),
    /// Tag was absent or unrecognized.
    Unknown,
}

impl<'de> Deserialize<'de> for BreakpointLocation {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let tag = value.get("type").and_then(Value::as_str).unwrap_or("");
        // Remaining fields deserialize independently of the tag.
        let resolved = SourceLocation::deserialize(&value);
        let unresolved = UnresolvedSourceLocation::deserialize(&value);
        Ok(match tag {
            "SourceLocation" => Self::Resolved(resolved.unwrap_or(SourceLocation {
                script: None,
                token_pos: None,
                end_token_pos: None,
                line: None,
                column: None,
            })),
            "UnresolvedSourceLocation" => {
                Self::Unresolved(unresolved.unwrap_or(UnresolvedSourceLocation {
                    script: None,
                    script_uri: None,
                    token_pos: None,
                    line: None,
                    column: None,
                }))
            }
            _ => Self::Unknown,
        })
    }
}

impl Serialize for BreakpointLocation {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let (tag, mut value) = match self {
            Self::Resolved(location) => (
                Some("SourceLocation"),
                serde_json::to_value(location).map_err(serde::ser::Error::custom)?,
            ),
            Self::Unresolved(location) => (
                Some("UnresolvedSourceLocation"),
                serde_json::to_value(location).map_err(serde::ser::Error::custom)?,
            ),
            Self::Unknown => (None, Value::Null),
        };
        if let (Some(object), Some(tag)) = (value.as_object_mut(), tag) {
            object.insert("type".to_owned(), Value::String(tag.to_owned()));
        }
        value.serialize(serializer)
    }
}

/// A `Breakpoint` object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Breakpoint {
    /// Opaque object id (`breakpoints/<n>`; used by removeBreakpoint).
    pub id: String,
    /// User-facing breakpoint number.
    #[serde(default)]
    pub breakpoint_number: i64,
    /// Whether the breakpoint is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Whether it resolved to a concrete program location.
    #[serde(default)]
    pub resolved: bool,
    /// Where the breakpoint sits (resolved or approximate).
    #[serde(default)]
    pub location: Option<BreakpointLocation>,
}

/// `FrameKind`: `Regular`, `AsyncCausal`, `AsyncSuspensionMarker`,
/// `AsyncActivation` (deprecated), or newer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameKind {
    /// A synchronous frame.
    Regular,
    /// An async continuation frame.
    AsyncCausal,
    /// Boundary marker between async frames.
    AsyncSuspensionMarker,
    /// Deprecated since protocol 4.7.
    AsyncActivation,
    /// A kind added in a newer minor protocol version.
    Other(String),
}

impl<'de> Deserialize<'de> for FrameKind {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match de_kind(deserializer)?.as_str() {
            "Regular" => Self::Regular,
            "AsyncCausal" => Self::AsyncCausal,
            "AsyncSuspensionMarker" => Self::AsyncSuspensionMarker,
            "AsyncActivation" => Self::AsyncActivation,
            other => Self::Other(other.to_owned()),
        })
    }
}

impl FrameKind {
    /// The wire spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Regular => "Regular",
            Self::AsyncCausal => "AsyncCausal",
            Self::AsyncSuspensionMarker => "AsyncSuspensionMarker",
            Self::AsyncActivation => "AsyncActivation",
            Self::Other(other) => other,
        }
    }
}

impl Serialize for FrameKind {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

/// `@Function` reference embedded in frames.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionRef {
    /// Opaque object id.
    pub id: String,
    /// Function name.
    #[serde(default)]
    pub name: Option<String>,
}

/// One `Frame` in a `Stack`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    /// Frame index; 0 is the top (most recent) frame.
    pub index: u32,
    /// The executing function.
    #[serde(default)]
    pub function: Option<FunctionRef>,
    /// Where execution sits in this frame.
    #[serde(default)]
    pub location: Option<SourceLocation>,
    /// Local variables bound in this frame.
    #[serde(default)]
    pub vars: Vec<BoundVariable>,
    /// Frame kind (absent means Regular).
    #[serde(default)]
    pub kind: Option<FrameKind>,
}

/// A `BoundVariable`: a local with its value (reference or sentinel).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundVariable {
    /// Variable name.
    pub name: String,
    /// The bound value — instance reference or sentinel.
    #[serde(default)]
    pub value: Option<ValueOrSentinel>,
    /// The declared static type.
    #[serde(default)]
    pub static_type: Option<InstanceRef>,
}

/// An `@Instance|Sentinel` slot (BoundVariable.value, BoundField.value,
/// MapAssociation key/value, elements entries).
#[derive(Debug, Clone, PartialEq)]
pub enum ValueOrSentinel {
    /// An instance reference (possibly a full `Instance`).
    Instance(Box<Instance>),
    /// A sentinel (`Collected`, `NotInitialized`, `OptimizedOut`, …).
    Sentinel(Sentinel),
    /// Anything else — keeps a bad wire message from killing the read.
    Other(Value),
}

impl<'de> Deserialize<'de> for ValueOrSentinel {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        Ok(match value.get("type").and_then(Value::as_str) {
            Some("Sentinel") => serde_json::from_value(value)
                .map(Self::Sentinel)
                .unwrap_or_else(|_| {
                    Self::Sentinel(Sentinel {
                        kind: SentinelKind::Other("Unparseable".into()),
                        value_as_string: None,
                    })
                }),
            // Parse from a borrow so a shape the model does not accept
            // still reaches the caller verbatim.
            Some(tag) if tag.contains("Instance") => match Instance::deserialize(&value) {
                Ok(instance) => Self::Instance(Box::new(instance)),
                Err(_) => Self::Other(value),
            },
            _ => Self::Other(value),
        })
    }
}

impl Serialize for ValueOrSentinel {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Instance(instance) => {
                let mut value =
                    serde_json::to_value(instance).map_err(serde::ser::Error::custom)?;
                if let Some(object) = value.as_object_mut() {
                    object.insert("type".to_owned(), Value::String("Instance".to_owned()));
                }
                value.serialize(serializer)
            }
            Self::Sentinel(sentinel) => {
                let mut value =
                    serde_json::to_value(sentinel).map_err(serde::ser::Error::custom)?;
                if let Some(object) = value.as_object_mut() {
                    object.insert("type".to_owned(), Value::String("Sentinel".to_owned()));
                }
                value.serialize(serializer)
            }
            Self::Other(value) => value.serialize(serializer),
        }
    }
}

/// `InstanceKind`, with an `Other` catch-all for kinds added by newer
/// protocol versions (spec: treat unrecognized kinds as PlainInstance).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceKind {
    /// A plain Dart object.
    PlainInstance,
    /// `null`.
    Null,
    /// `true`/`false`.
    Bool,
    /// A double.
    Double,
    /// An int.
    Int,
    /// A string.
    String,
    /// A built-in list.
    List,
    /// A built-in map.
    Map,
    /// A built-in set.
    Set,
    /// Vector kinds.
    Float32x4,
    /// Vector kinds.
    Float64x2,
    /// Vector kinds.
    Int32x4,
    /// TypedData kinds.
    Uint8ClampedList,
    /// TypedData kinds.
    Uint8List,
    /// TypedData kinds.
    Uint16List,
    /// TypedData kinds.
    Uint32List,
    /// TypedData kinds.
    Uint64List,
    /// TypedData kinds.
    Int8List,
    /// TypedData kinds.
    Int16List,
    /// TypedData kinds.
    Int32List,
    /// TypedData kinds.
    Int64List,
    /// TypedData kinds.
    Float32List,
    /// TypedData kinds.
    Float64List,
    /// TypedData kinds.
    Int32x4List,
    /// TypedData kinds.
    Float32x4List,
    /// TypedData kinds.
    Float64x2List,
    /// A record.
    Record,
    /// A stack trace.
    StackTrace,
    /// A closure.
    Closure,
    /// A mirror reference.
    MirrorReference,
    /// A regular expression.
    RegExp,
    /// A weak property.
    WeakProperty,
    /// A weak reference.
    WeakReference,
    /// A type literal.
    Type,
    /// A type parameter.
    TypeParameter,
    /// Deprecated type ref.
    TypeRef,
    /// A function type.
    FunctionType,
    /// A record type.
    RecordType,
    /// A bounded type.
    BoundedType,
    /// A receive port.
    ReceivePort,
    /// A user tag.
    UserTag,
    /// A finalizer.
    Finalizer,
    /// A native finalizer.
    NativeFinalizer,
    /// A finalizer entry.
    FinalizerEntry,
    /// A dart:ffi pointer.
    Pointer,
    /// A kind added in a newer minor protocol version — the spec says
    /// to treat these as `PlainInstance`.
    Other(String),
}

impl<'de> Deserialize<'de> for InstanceKind {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match de_kind(deserializer)?.as_str() {
            "PlainInstance" => Self::PlainInstance,
            "Null" => Self::Null,
            "Bool" => Self::Bool,
            "Double" => Self::Double,
            "Int" => Self::Int,
            "String" => Self::String,
            "List" => Self::List,
            "Map" | "MapLiteral" => Self::Map,
            "Set" => Self::Set,
            "Float32x4" => Self::Float32x4,
            "Float64x2" => Self::Float64x2,
            "Int32x4" => Self::Int32x4,
            "Uint8ClampedList" => Self::Uint8ClampedList,
            "Uint8List" => Self::Uint8List,
            "Uint16List" => Self::Uint16List,
            "Uint32List" => Self::Uint32List,
            "Uint64List" => Self::Uint64List,
            "Int8List" => Self::Int8List,
            "Int16List" => Self::Int16List,
            "Int32List" => Self::Int32List,
            "Int64List" => Self::Int64List,
            "Float32List" => Self::Float32List,
            "Float64List" => Self::Float64List,
            "Int32x4List" => Self::Int32x4List,
            "Float32x4List" => Self::Float32x4List,
            "Float64x2List" => Self::Float64x2List,
            "Record" => Self::Record,
            "StackTrace" => Self::StackTrace,
            "Closure" => Self::Closure,
            "MirrorReference" => Self::MirrorReference,
            "RegExp" => Self::RegExp,
            "WeakProperty" => Self::WeakProperty,
            "WeakReference" => Self::WeakReference,
            "Type" => Self::Type,
            "TypeParameter" => Self::TypeParameter,
            "TypeRef" => Self::TypeRef,
            "FunctionType" => Self::FunctionType,
            "RecordType" => Self::RecordType,
            "BoundedType" => Self::BoundedType,
            "ReceivePort" => Self::ReceivePort,
            "UserTag" => Self::UserTag,
            "Finalizer" => Self::Finalizer,
            "NativeFinalizer" => Self::NativeFinalizer,
            "FinalizerEntry" => Self::FinalizerEntry,
            "Pointer" => Self::Pointer,
            other => Self::Other(other.to_owned()),
        })
    }
}

impl InstanceKind {
    /// The wire spelling (`Map` covers the legacy `MapLiteral`
    /// spelling on the deserialize side).
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::PlainInstance => "PlainInstance",
            Self::Null => "Null",
            Self::Bool => "Bool",
            Self::Double => "Double",
            Self::Int => "Int",
            Self::String => "String",
            Self::List => "List",
            Self::Map => "Map",
            Self::Set => "Set",
            Self::Float32x4 => "Float32x4",
            Self::Float64x2 => "Float64x2",
            Self::Int32x4 => "Int32x4",
            Self::Uint8ClampedList => "Uint8ClampedList",
            Self::Uint8List => "Uint8List",
            Self::Uint16List => "Uint16List",
            Self::Uint32List => "Uint32List",
            Self::Uint64List => "Uint64List",
            Self::Int8List => "Int8List",
            Self::Int16List => "Int16List",
            Self::Int32List => "Int32List",
            Self::Int64List => "Int64List",
            Self::Float32List => "Float32List",
            Self::Float64List => "Float64List",
            Self::Int32x4List => "Int32x4List",
            Self::Float32x4List => "Float32x4List",
            Self::Float64x2List => "Float64x2List",
            Self::Record => "Record",
            Self::StackTrace => "StackTrace",
            Self::Closure => "Closure",
            Self::MirrorReference => "MirrorReference",
            Self::RegExp => "RegExp",
            Self::WeakProperty => "WeakProperty",
            Self::WeakReference => "WeakReference",
            Self::Type => "Type",
            Self::TypeParameter => "TypeParameter",
            Self::TypeRef => "TypeRef",
            Self::FunctionType => "FunctionType",
            Self::RecordType => "RecordType",
            Self::BoundedType => "BoundedType",
            Self::ReceivePort => "ReceivePort",
            Self::UserTag => "UserTag",
            Self::Finalizer => "Finalizer",
            Self::NativeFinalizer => "NativeFinalizer",
            Self::FinalizerEntry => "FinalizerEntry",
            Self::Pointer => "Pointer",
            Self::Other(other) => other,
        }
    }
}

impl Serialize for InstanceKind {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

/// A `BoundField`: one instance field with its value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundField {
    /// The field's declaration reference (deprecated upstream).
    #[serde(default)]
    pub decl: Option<Value>,
    /// Field name — `string` for ordinary fields, `int` for record
    /// positional fields.
    #[serde(default)]
    pub name: Option<Value>,
    /// The bound value.
    #[serde(default)]
    pub value: Option<ValueOrSentinel>,
}

/// A `MapAssociation`: one key/value entry of a `Map` instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapAssociation {
    /// The key.
    #[serde(default)]
    pub key: Option<ValueOrSentinel>,
    /// The value.
    #[serde(default)]
    pub value: Option<ValueOrSentinel>,
}

/// An `@Instance` or `Instance` — the object-graph node. Both the
/// reference and full shapes deserialize into this struct; fields that
/// exist only on the full shape are optional.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Instance {
    /// Opaque object id; the lazy-expansion handle.
    #[serde(default)]
    pub id: String,
    /// Instance kind.
    pub kind: InstanceKind,
    /// The instance's class (present on references and objects).
    #[serde(default, rename = "class")]
    pub class: Option<ClassRef>,
    /// Primitive/string render (kinds Null, Bool, Double, Int, String,
    /// StackTrace, Pointer, vector kinds).
    #[serde(default)]
    pub value_as_string: Option<String>,
    /// Set when `value_as_string` was truncated.
    #[serde(default)]
    pub value_as_string_is_truncated: bool,
    /// Field count / element count / codeunit count, per kind.
    #[serde(default)]
    pub length: Option<u64>,
    /// `Type` name.
    #[serde(default)]
    pub name: Option<String>,
    /// `PlainInstance`/`Record` fields (full objects only).
    #[serde(default)]
    pub fields: Vec<BoundField>,
    /// `List`/`Set` elements.
    #[serde(default)]
    pub elements: Vec<ValueOrSentinel>,
    /// `Map` associations.
    #[serde(default)]
    pub associations: Vec<MapAssociation>,
    /// Base64 bytes for TypedData kinds.
    #[serde(default)]
    pub bytes: Option<String>,
    /// `Closure` function.
    #[serde(default)]
    pub closure_function: Option<FunctionRef>,
    /// `UserTag` label.
    #[serde(default)]
    pub label: Option<String>,
    /// `ReceivePort` port id.
    #[serde(default)]
    pub port_id: Option<i64>,
    /// `RegExp` pattern.
    #[serde(default)]
    pub pattern: Option<Box<Instance>>,
    /// `Type` resolved class.
    #[serde(default)]
    pub type_class: Option<ClassRef>,
    /// `TypeParameter` parameterized class.
    #[serde(default)]
    pub parameterized_class: Option<ClassRef>,
    /// `TypeParameter` index.
    #[serde(default)]
    pub parameter_index: Option<i64>,
    /// `BoundedType` target type.
    #[serde(default)]
    pub target_type: Option<Box<Instance>>,
    /// `BoundedType`/`TypeParameter` bound.
    #[serde(default)]
    pub bound: Option<Box<Instance>>,
    /// `Type` type arguments.
    #[serde(default)]
    pub type_arguments: Option<Value>,
    /// `FunctionType` return type.
    #[serde(default)]
    pub return_type: Option<Box<Instance>>,
    /// `FunctionType` parameters.
    #[serde(default)]
    pub parameters: Vec<Value>,
    /// `MirrorReference` referent.
    #[serde(default)]
    pub mirror_referent: Option<Value>,
    /// `WeakProperty` key.
    #[serde(default)]
    pub property_key: Option<Value>,
    /// `WeakProperty` value.
    #[serde(default)]
    pub property_value: Option<Value>,
    /// `WeakReference` target.
    #[serde(default)]
    pub target: Option<Value>,
    /// `Closure` context.
    #[serde(default)]
    pub closure_context: Option<Value>,
    /// `Closure` tear-off receiver.
    #[serde(default)]
    pub closure_receiver: Option<Box<Instance>>,
    /// `ReceivePort` allocation stack.
    #[serde(default)]
    pub allocation_location: Option<Box<Instance>>,
    /// `ReceivePort` debug name.
    #[serde(default)]
    pub debug_name: Option<String>,
    /// `Finalizer`/`NativeFinalizer` callback.
    #[serde(default)]
    pub callback: Option<Box<Instance>>,
    /// `NativeFinalizer` callback address.
    #[serde(default)]
    pub callback_address: Option<Box<Instance>>,
    /// `Finalizer`/`NativeFinalizer` entries set.
    #[serde(default)]
    pub all_entries: Option<Box<Instance>>,
    /// `FinalizerEntry` watched value.
    #[serde(default)]
    pub value: Option<Box<Instance>>,
    /// `FinalizerEntry` token.
    #[serde(default)]
    pub token: Option<Box<Instance>>,
    /// `FinalizerEntry` detach key.
    #[serde(default)]
    pub detach: Option<Box<Instance>>,
    /// `RegExp` case sensitivity.
    #[serde(default)]
    pub is_case_sensitive: Option<bool>,
    /// `RegExp` multiline flag.
    #[serde(default)]
    pub is_multi_line: Option<bool>,
}

/// Alias kept for the signature slots where the spec writes
/// `@Instance`.
pub type InstanceRef = Instance;

/// `@Class` reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassRef {
    /// Opaque object id.
    pub id: String,
    /// Class name.
    #[serde(default)]
    pub name: Option<String>,
}

/// `EventKind` — the full published enum plus `Other` for
/// backwards-compatible additions (spec: clients must ignore
/// unrecognized events).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    /// `setVMName` notification.
    VmUpdate,
    /// `setFlag` notification.
    VmFlagUpdate,
    /// A new isolate started.
    IsolateStart,
    /// An isolate is ready to run.
    IsolateRunnable,
    /// An isolate exited.
    IsolateExit,
    /// Isolate name changed.
    IsolateUpdate,
    /// An isolate was reloaded.
    IsolateReload,
    /// An isolate registered a service extension RPC.
    ServiceExtensionAdded,
    /// Paused at start (`--pause-isolates-on-start`).
    PauseStart,
    /// Paused at exit.
    PauseExit,
    /// Paused at a breakpoint or by stepping.
    PauseBreakpoint,
    /// Paused by a `pause` RPC interrupt.
    PauseInterrupted,
    /// Paused on an exception.
    PauseException,
    /// Paused after a service request (e.g. reload).
    PausePostRequest,
    /// Started or resumed execution.
    Resume,
    /// Not yet runnable (never delivered over a stream).
    None,
    /// A breakpoint was added.
    BreakpointAdded,
    /// An unresolved breakpoint resolved.
    BreakpointResolved,
    /// A breakpoint was removed.
    BreakpointRemoved,
    /// A breakpoint was updated.
    BreakpointUpdated,
    /// GC event.
    Gc,
    /// Stdout/stderr bytes (WriteEvent).
    WriteEvent,
    /// `dart:developer.inspect` argument.
    Inspect,
    /// `dart:developer.postEvent` payload.
    Extension,
    /// `dart:developer.log` record.
    Logging,
    /// Timer fired significantly late.
    TimerSignificantlyOverdue,
    /// A block of timeline events completed.
    TimelineEvents,
    /// `setVMTimelineFlags` acknowledgement.
    TimelineStreamSubscriptionsUpdate,
    /// A service was registered by another client.
    ServiceRegistered,
    /// A service was unregistered.
    ServiceUnregistered,
    /// Isolate user tag changed.
    UserTagChanged,
    /// CPU samples block.
    CpuSamples,
    /// A kind added in a newer minor protocol version.
    Other(String),
}

impl<'de> Deserialize<'de> for EventKind {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match de_kind(deserializer)?.as_str() {
            "VMUpdate" => Self::VmUpdate,
            "VMFlagUpdate" => Self::VmFlagUpdate,
            "IsolateStart" => Self::IsolateStart,
            "IsolateRunnable" => Self::IsolateRunnable,
            "IsolateExit" => Self::IsolateExit,
            "IsolateUpdate" => Self::IsolateUpdate,
            "IsolateReload" => Self::IsolateReload,
            "ServiceExtensionAdded" => Self::ServiceExtensionAdded,
            "PauseStart" => Self::PauseStart,
            "PauseExit" => Self::PauseExit,
            "PauseBreakpoint" => Self::PauseBreakpoint,
            "PauseInterrupted" => Self::PauseInterrupted,
            "PauseException" => Self::PauseException,
            "PausePostRequest" => Self::PausePostRequest,
            "Resume" => Self::Resume,
            "None" => Self::None,
            "BreakpointAdded" => Self::BreakpointAdded,
            "BreakpointResolved" => Self::BreakpointResolved,
            "BreakpointRemoved" => Self::BreakpointRemoved,
            "BreakpointUpdated" => Self::BreakpointUpdated,
            "GC" => Self::Gc,
            "WriteEvent" => Self::WriteEvent,
            "Inspect" => Self::Inspect,
            "Extension" => Self::Extension,
            "Logging" => Self::Logging,
            "TimerSignificantlyOverdue" => Self::TimerSignificantlyOverdue,
            "TimelineEvents" => Self::TimelineEvents,
            "TimelineStreamSubscriptionsUpdate" => Self::TimelineStreamSubscriptionsUpdate,
            "ServiceRegistered" => Self::ServiceRegistered,
            "ServiceUnregistered" => Self::ServiceUnregistered,
            "UserTagChanged" => Self::UserTagChanged,
            "CpuSamples" => Self::CpuSamples,
            other => Self::Other(other.to_owned()),
        })
    }
}

impl EventKind {
    /// The wire spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::VmUpdate => "VMUpdate",
            Self::VmFlagUpdate => "VMFlagUpdate",
            Self::IsolateStart => "IsolateStart",
            Self::IsolateRunnable => "IsolateRunnable",
            Self::IsolateExit => "IsolateExit",
            Self::IsolateUpdate => "IsolateUpdate",
            Self::IsolateReload => "IsolateReload",
            Self::ServiceExtensionAdded => "ServiceExtensionAdded",
            Self::PauseStart => "PauseStart",
            Self::PauseExit => "PauseExit",
            Self::PauseBreakpoint => "PauseBreakpoint",
            Self::PauseInterrupted => "PauseInterrupted",
            Self::PauseException => "PauseException",
            Self::PausePostRequest => "PausePostRequest",
            Self::Resume => "Resume",
            Self::None => "None",
            Self::BreakpointAdded => "BreakpointAdded",
            Self::BreakpointResolved => "BreakpointResolved",
            Self::BreakpointRemoved => "BreakpointRemoved",
            Self::BreakpointUpdated => "BreakpointUpdated",
            Self::Gc => "GC",
            Self::WriteEvent => "WriteEvent",
            Self::Inspect => "Inspect",
            Self::Extension => "Extension",
            Self::Logging => "Logging",
            Self::TimerSignificantlyOverdue => "TimerSignificantlyOverdue",
            Self::TimelineEvents => "TimelineEvents",
            Self::TimelineStreamSubscriptionsUpdate => "TimelineStreamSubscriptionsUpdate",
            Self::ServiceRegistered => "ServiceRegistered",
            Self::ServiceUnregistered => "ServiceUnregistered",
            Self::UserTagChanged => "UserTagChanged",
            Self::CpuSamples => "CpuSamples",
            Self::Other(other) => other,
        }
    }
}

impl Serialize for EventKind {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

/// A stream `Event` (`streamNotify` params `event` member). Only the
/// fields the session consumes are modeled; everything else stays in
/// the raw value the event was parsed from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    /// What kind of event this is.
    pub kind: EventKind,
    /// Associated isolate reference.
    #[serde(default)]
    pub isolate: Option<IsolateRef>,
    /// Associated isolate group reference.
    #[serde(default)]
    pub isolate_group: Option<Value>,
    /// Associated VM reference.
    #[serde(default)]
    pub vm: Option<Value>,
    /// Event timestamp, ms since epoch. Required by the spec — a
    /// missing field is a protocol violation, not a default.
    pub timestamp: i64,
    /// Breakpoint for `PauseBreakpoint`, `BreakpointAdded`,
    /// `BreakpointRemoved`, `BreakpointResolved`, `BreakpointUpdated`.
    #[serde(default)]
    pub breakpoint: Option<Breakpoint>,
    /// All breakpoints at the current `PauseBreakpoint` position.
    #[serde(default)]
    pub pause_breakpoints: Vec<Breakpoint>,
    /// Top stack frame for pause events and Resume.
    #[serde(default)]
    pub top_frame: Option<Frame>,
    /// Thrown exception for `PauseException`.
    #[serde(default)]
    pub exception: Option<Instance>,
    /// Base64 bytes for `WriteEvent`.
    #[serde(default)]
    pub bytes: Option<String>,
    /// `dart:developer.inspect` argument.
    #[serde(default)]
    pub inspectee: Option<Instance>,
    /// Whether the pause sits at an await/yield suspension.
    #[serde(default)]
    pub at_async_suspension: bool,
    /// Reload success/failure status (`IsolateReload`).
    #[serde(default)]
    pub status: Option<String>,
    /// Why a reload failed (`IsolateReload`).
    #[serde(default)]
    pub reload_failure_reason: Option<String>,
    /// `dart:developer.log` record (`Logging`).
    #[serde(default)]
    pub log_record: Option<Value>,
    /// New extension RPC name (`ServiceExtensionAdded`).
    #[serde(default)]
    pub extension_rpc: Option<String>,
    /// Extension kind (`Extension`).
    #[serde(default)]
    pub extension_kind: Option<String>,
    /// Extension payload (`Extension`).
    #[serde(default)]
    pub extension_data: Option<Value>,
    /// Service name (`ServiceRegistered`/`ServiceUnregistered`).
    #[serde(default)]
    pub service: Option<String>,
    /// RPC method name (`ServiceRegistered`/`ServiceUnregistered`).
    #[serde(default)]
    pub method: Option<String>,
    /// Registered-service alias (`ServiceRegistered`).
    #[serde(default)]
    pub alias: Option<String>,
    /// Flag name (`VMFlagUpdate`).
    #[serde(default)]
    pub flag: Option<String>,
    /// Flag value (`VMFlagUpdate`).
    #[serde(default)]
    pub new_value: Option<String>,
    /// Whether a HeapSnapshot group ended (`HeapSnapshot`).
    #[serde(default)]
    pub last: Option<bool>,
    /// Overtime detail (`TimerSignificantlyOverdue`).
    #[serde(default)]
    pub details: Option<String>,
}

/// A `Stack` (`getStack` result).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stack {
    /// Synchronous frames, top first.
    #[serde(default)]
    pub frames: Vec<Frame>,
    /// Synchronous + async-causal frames with suspension markers, when
    /// async code is in flight.
    #[serde(default)]
    pub async_causal_frames: Vec<Frame>,
    /// Whether the stack was truncated.
    #[serde(default)]
    pub truncated: bool,
}

/// A `ScriptList` (`getScripts` result).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptList {
    /// All scripts loaded in the isolate.
    #[serde(default)]
    pub scripts: Vec<ScriptRef>,
}

/// A `ReloadReport` (`reloadSources` result).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReloadReport {
    /// Whether the reload succeeded.
    pub success: bool,
}

/// A full `VM` (`getVM` result).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Vm {
    /// VM debugging name.
    #[serde(default)]
    pub name: Option<String>,
    /// Word length (32/64).
    #[serde(default)]
    pub architecture_bits: u32,
    /// Host CPU name.
    #[serde(default)]
    pub host_cpu: Option<String>,
    /// OS name.
    #[serde(default)]
    pub operating_system: Option<String>,
    /// Target CPU name.
    #[serde(default)]
    pub target_cpu: Option<String>,
    /// Dart VM version string.
    #[serde(default)]
    pub version: Option<String>,
    /// Process id.
    #[serde(default)]
    pub pid: u64,
    /// Start timestamp (ms since epoch).
    #[serde(default)]
    pub start_time: i64,
    /// User isolates.
    #[serde(default)]
    pub isolates: Vec<IsolateRef>,
    /// Isolate groups.
    #[serde(default)]
    pub isolate_groups: Vec<Value>,
    /// System isolates.
    #[serde(default)]
    pub system_isolates: Vec<IsolateRef>,
    /// System isolate groups.
    #[serde(default)]
    pub system_isolate_groups: Vec<Value>,
}

/// The tagged union returned by `getObject` and by `evaluate`-family
/// RPCs (`@Instance`, `Instance`, `Sentinel`, `Error`, or any other
/// Object subtype — kept as the raw value with its tag).
#[derive(Debug, Clone, PartialEq)]
pub enum VmObject {
    /// An instance (reference or full object).
    Instance(Box<Instance>),
    /// A sentinel (`Collected`/`Expired`/…).
    Sentinel(Sentinel),
    /// A Dart-level `Error` object (unhandled exception during
    /// evaluation, etc.).
    Error {
        /// The error kind string.
        kind: String,
        /// The error message.
        message: String,
    },
    /// Any other response subtype (Script, Class, Library, Code, …) —
    /// kept as the raw JSON for callers that only need a subset.
    Other(Value),
}

impl VmObject {
    /// Parses one typed response value (the `result` member) into the
    /// union. A missing/unrecognized `type` tag yields `Other`.
    #[must_use]
    pub fn from_value(value: Value) -> Self {
        match value.get("type").and_then(Value::as_str).unwrap_or("") {
            "Sentinel" => serde_json::from_value(value)
                .map(Self::Sentinel)
                .unwrap_or_else(|_| {
                    Self::Sentinel(Sentinel {
                        kind: SentinelKind::Other("Unparseable".into()),
                        value_as_string: None,
                    })
                }),
            "Error" | "@Error" => Self::Error {
                kind: value
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("Unknown")
                    .to_owned(),
                message: value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            },
            tag if tag.ends_with("Instance") => match Instance::deserialize(&value) {
                Ok(instance) => Self::Instance(Box::new(instance)),
                Err(_) => Self::Other(value),
            },
            _ => Self::Other(value),
        }
    }
}
