//! Loading an [`SdkSnapshot`] from an on-disk SDK layout.
//!
//! Step order follows the SDK loader contract §5: version file →
//! revision → `libraries.json` (with target resolution) →
//! `allowed_experiments.json` → canonical id.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::str::FromStr;

use serde::Deserialize;

use dartq_language::LanguageVersion;
use dartq_source::content_hash;

use crate::identity;
use crate::metadata::{
    AllowedExperiments, Importability, SdkIdentity, SdkLibraries, SdkLibrary, SdkSnapshot,
    TargetLibraries,
};

impl SdkSnapshot {
    /// Loads an SDK snapshot from `dart_sdk_root` (a provisioned Dart SDK
    /// containing `version`, `lib/libraries.json`, … — not a source
    /// checkout).
    ///
    /// All metadata is read eagerly; `dart:` source bodies are resolved
    /// lazily via [`uri_to_path`](SdkSnapshot::uri_to_path).
    ///
    /// `flutter_sdk_root` is the reserved seam for Flutter SDK support
    /// (M2): folding in `bin/cache/flutter.version.json` and replacing the
    /// library table with the `sky_engine` `_embedder.yaml` mapping, with
    /// analyzer `EmbedderSdk` semantics (full table replacement, not
    /// overlay — a flutter project's `dart:core` and `dart:ui` come from
    /// `sky_engine`, the SDK loader contract §4). Passing a root
    /// before that lands is a hard error rather than a silently partial
    /// load, and `None` is fully self-sufficient: the dart-sdk
    /// `libraries.json` covers every non-embedder `dart:` library.
    ///
    /// # Errors
    ///
    /// Hard-errors ([`SdkError`]) on any missing or malformed metadata —
    /// including an empty library table, a malformed include chain, or an
    /// unusable `allowed_experiments.json` — where the analyzer would
    /// silently degrade; see the crate docs.
    pub fn load(
        dart_sdk_root: &Path,
        flutter_sdk_root: Option<&Path>,
    ) -> Result<SdkSnapshot, SdkError> {
        if let Some(flutter_root) = flutter_sdk_root {
            return Err(SdkError::FlutterUnsupported {
                root: flutter_root.to_path_buf(),
            });
        }

        let version_path = dart_sdk_root.join("version");
        let version_text = read_required(&version_path)?;
        let version = version_text.trim();
        // The version file may carry a trailing release annotation
        // (`3.13.1 (stable) (Tue …) …`); the official reader consumes only
        // the leading numeric parts of the split (sdk.dart:25-30), so the
        // tail after the leading `major.minor[.patch]` run is ignored
        // rather than rejected. The identity keeps the file's declared
        // text verbatim.
        let parseable = leading_version_run(version);
        let language_metadata =
            LanguageVersion::from_str(parseable).map_err(|_| SdkError::InvalidVersion {
                path: version_path.clone(),
                value: version.to_owned(),
            })?;

        let revision_path = dart_sdk_root.join("revision");
        let revision = match fs::read_to_string(&revision_path) {
            Ok(text) => Some(text.trim().to_owned()),
            Err(err) if err.kind() == ErrorKind::NotFound => None,
            Err(err) => {
                return Err(SdkError::Io {
                    path: revision_path,
                    source: err,
                });
            }
        };

        let libraries_path = dart_sdk_root.join("lib").join("libraries.json");
        let libraries_text = read_required(&libraries_path)?;
        let sdk_libraries = parse_libraries_json(&libraries_text).map_err(|message| {
            SdkError::InvalidLibrariesJson {
                path: libraries_path.clone(),
                message,
            }
        })?;
        if sdk_libraries
            .targets
            .values()
            .all(|target| target.libraries.is_empty())
        {
            return Err(SdkError::EmptyLibraries {
                path: libraries_path,
            });
        }

        let experiments_path = dart_sdk_root
            .join("lib")
            .join("_internal")
            .join("allowed_experiments.json");
        let experiments_text = read_required(&experiments_path)?;
        let platform_metadata =
            parse_allowed_experiments(&experiments_text).map_err(|message| {
                SdkError::InvalidAllowedExperiments {
                    path: experiments_path,
                    message,
                }
            })?;

        let dart = SdkIdentity {
            version: version.to_owned(),
            revision,
            libraries_json_hash: content_hash(libraries_text.as_bytes()),
        };
        let id =
            identity::snapshot_id(&dart, language_metadata, &sdk_libraries, &platform_metadata);

        Ok(SdkSnapshot {
            id,
            dart,
            sdk_libraries,
            platform_metadata,
            language_metadata,
            dart_sdk_root: dart_sdk_root.to_path_buf(),
        })
    }
}

/// Failure modes of [`SdkSnapshot::load`].
///
/// Every variant is a hard error: the analyzer's two silent degradations
/// (empty library map, all-experiments-off on malformed
/// `allowed_experiments.json`) are deliberate anti-patterns here, so a
/// crippled SDK can never pass for a valid analysis baseline.
#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    /// A required metadata file is absent (`version`, `libraries.json`,
    /// `allowed_experiments.json`).
    #[error("required SDK file missing: {path}")]
    MissingFile { path: std::path::PathBuf },
    /// A required metadata file exists but could not be read (permissions,
    /// non-UTF-8 content, …).
    #[error("cannot read SDK file {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    /// The `version` file does not parse as `major.minor[.patch]`.
    #[error("invalid SDK version file {path}: expected `major.minor[.patch]`, found `{value}`")]
    InvalidVersion {
        path: std::path::PathBuf,
        value: String,
    },
    /// `libraries.json` is not valid JSON of the expected shape, or its
    /// include chain is malformed (unknown target / cycle / cross-file
    /// `path` include).
    #[error("invalid libraries.json {path}: {message}")]
    InvalidLibrariesJson {
        path: std::path::PathBuf,
        message: String,
    },
    /// `libraries.json` parses but defines no library in any segment — no
    /// usable `dart:` surface.
    #[error("libraries.json {path} defines no library in any target")]
    EmptyLibraries { path: std::path::PathBuf },
    /// `allowed_experiments.json` is not valid JSON of the expected shape,
    /// or references an undefined experiment set.
    #[error("invalid allowed_experiments.json {path}: {message}")]
    InvalidAllowedExperiments {
        path: std::path::PathBuf,
        message: String,
    },
    /// A flutter SDK root was supplied. Flutter (embedder) support is
    /// reserved for M2 — see [`SdkSnapshot::load`].
    #[error(
        "flutter SDK support is not implemented yet (root: {root}); \
         load the dart SDK at `<flutter>/bin/cache/dart-sdk` with \
         `flutter_sdk_root: None` instead"
    )]
    FlutterUnsupported { root: std::path::PathBuf },
}

/// Reads a required UTF-8 file, distinguishing absence
/// ([`SdkError::MissingFile`]) from other IO failures.
fn read_required(path: &Path) -> Result<String, SdkError> {
    fs::read_to_string(path).map_err(|err| match err.kind() {
        ErrorKind::NotFound => SdkError::MissingFile {
            path: path.to_path_buf(),
        },
        _ => SdkError::Io {
            path: path.to_path_buf(),
            source: err,
        },
    })
}

/// The leading `major.minor[.patch]` digit run of the version file's
/// content. Digits and their separating dots are consumed; the scan stops
/// at the first other byte, and a final dot with no digits after it is
/// dropped. Empty when the text does not start with a digit.
fn leading_version_run(text: &str) -> &str {
    let mut end = 0;
    let mut after_dot = false;
    for (i, byte) in text.bytes().enumerate() {
        match byte {
            b'0'..=b'9' => {
                end = i + 1;
                after_dot = false;
            }
            b'.' if end > 0 && !after_dot => {
                end = i + 1;
                after_dot = true;
            }
            _ => break,
        }
    }
    if after_dot {
        end -= 1;
    }
    &text[..end]
}

// --- libraries.json ---

/// One `libraries.json` segment as written: `include` chain plus raw
/// library entries. Unknown keys are tolerated (the format grows across
/// versions); `comment:*` siblings never reach this type.
#[derive(Deserialize)]
struct RawSegment {
    #[serde(default)]
    include: Vec<RawInclude>,
    libraries: BTreeMap<String, RawLibrary>,
}

/// One `libraries.json` `include` entry. `path` (an include from another
/// libraries.json file, per the official `libraries_specification.dart`
/// format) is captured only to reject it explicitly: serde would otherwise
/// drop the key and silently resolve `target` from the current file.
#[derive(Deserialize)]
struct RawInclude {
    path: Option<String>,
    target: String,
}

/// One library entry as written: `patches` is a string, `null`, or an
/// array of strings, normalized to a list during conversion.
#[derive(Deserialize)]
struct RawLibrary {
    uri: String,
    #[serde(default)]
    patches: PatchList,
    support_conditional_import: Option<bool>,
    support_direct_import: Option<String>,
}

#[derive(Default)]
struct PatchList(Vec<String>);

impl<'de> Deserialize<'de> for PatchList {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PatchListVisitor;

        impl<'de> serde::de::Visitor<'de> for PatchListVisitor {
            type Value = PatchList;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a patch path string or an array of patch path strings")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(PatchList(vec![value.to_owned()]))
            }

            // `patches: null` is an explicit empty list, not a type error
            // (libraries_specification.dart:333-335: `data['patches'] ==
            // null` yields `const []`).
            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(PatchList(Vec::new()))
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(PatchList(Vec::new()))
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut patches = Vec::new();
                while let Some(patch) = seq.next_element::<String>()? {
                    patches.push(patch);
                }
                Ok(PatchList(patches))
            }
        }

        deserializer.deserialize_any(PatchListVisitor)
    }
}

/// Parses `libraries.json` into include-expanded segments.
///
/// `comment:*` keys are generation annotations, not segments (the official
/// specification parser skips them the same way). All other keys must be
/// segment objects; unknown keys *inside* a segment or library entry are
/// tolerated — except `include[].path`, the cross-file include form of the
/// official format, which is rejected explicitly (see [`RawInclude`]).
/// Returns a message on any structural violation.
pub(crate) fn parse_libraries_json(text: &str) -> Result<SdkLibraries, String> {
    let raw: BTreeMap<String, serde_json::Value> =
        serde_json::from_str(text).map_err(|err| format!("not valid JSON: {err}"))?;
    let mut segments = BTreeMap::new();
    for (name, value) in &raw {
        if name.starts_with("comment:") {
            continue;
        }
        let segment: RawSegment = serde_json::from_value(value.clone())
            .map_err(|err| format!("invalid target segment `{name}`: {err}"))?;
        for include in &segment.include {
            if include.path.is_some() {
                return Err(format!(
                    "unsupported cross-file `include` in target `{name}`: an entry with a \
                     `path` key loads another libraries.json file (the official \
                     `libraries_specification.dart` multi-file format), which is not \
                     supported; includes must reference targets of the current file"
                ));
            }
        }
        segments.insert(name.clone(), segment);
    }

    let mut targets = BTreeMap::new();
    for name in segments.keys() {
        let mut libraries = BTreeMap::new();
        let mut stack = Vec::new();
        let mut visited = BTreeSet::new();
        resolve_segment(&segments, name, &mut stack, &mut visited, &mut libraries)?;
        targets.insert(name.clone(), TargetLibraries { libraries });
    }
    Ok(SdkLibraries { targets })
}

/// Depth-first segment resolution: inherited segments contribute first, own
/// definitions overwrite (`include` merges, self wins). `stack` detects
/// cycles; `visited` skips re-processing a segment already merged through
/// another include path (a diamond contributes once — its entries are
/// identical either way), one set per root target so each root still
/// merges the full chain into its own `libraries`.
fn resolve_segment(
    segments: &BTreeMap<String, RawSegment>,
    name: &str,
    stack: &mut Vec<String>,
    visited: &mut BTreeSet<String>,
    libraries: &mut BTreeMap<String, SdkLibrary>,
) -> Result<(), String> {
    if stack.iter().any(|entry| entry == name) {
        let mut cycle = stack.clone();
        cycle.push(name.to_owned());
        return Err(format!("`include` cycle: {}", cycle.join(" -> ")));
    }
    if !visited.insert(name.to_owned()) {
        return Ok(());
    }
    let Some(segment) = segments.get(name) else {
        return Err(format!(
            "`include` references unknown target `{name}` (known: {})",
            segments.keys().cloned().collect::<Vec<_>>().join(", ")
        ));
    };
    stack.push(name.to_owned());
    for include in &segment.include {
        resolve_segment(segments, &include.target, stack, visited, libraries)?;
    }
    for (library_name, raw) in &segment.libraries {
        libraries.insert(library_name.clone(), convert_library(library_name, raw)?);
    }
    stack.pop();
    Ok(())
}

/// Normalizes one raw entry per the specification semantics: patches string
/// → list, conditional-import default `true` and forced `false` for
/// `_`-prefixed names, direct-import default `always`.
fn convert_library(name: &str, raw: &RawLibrary) -> Result<SdkLibrary, String> {
    check_uri_scheme("uri", name, &raw.uri)?;
    for patch in &raw.patches.0 {
        check_uri_scheme("patches", name, patch)?;
    }
    let support_direct_import = match &raw.support_direct_import {
        Some(value) => Importability::from_spec(value)?,
        None => Importability::Always,
    };
    Ok(SdkLibrary {
        uri: raw.uri.clone(),
        patches: raw.patches.0.clone(),
        support_conditional_import: raw.support_conditional_import.unwrap_or(true)
            && !name.starts_with('_'),
        support_direct_import,
    })
}

/// The official `checkAndResolve` scheme gate
/// (libraries_specification.dart:300-317): a `uri`/`patches` value is either
/// scheme-less (a path relative to the SDK `lib/` root) or a `file:` URI;
/// any other scheme is a parse-time hard error, never a silently
/// mis-resolved path. Scheme detection mirrors Dart's `Uri.parse`: an
/// `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ) ":"` prefix; anything else
/// before the first `:` is not a scheme at all.
fn check_uri_scheme(field: &str, library: &str, value: &str) -> Result<(), String> {
    let Some(scheme) = value.find(':').map(|colon| &value[..colon]) else {
        return Ok(());
    };
    let has_scheme = scheme.starts_with(|ch: char| ch.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'));
    if has_scheme && !scheme.eq_ignore_ascii_case("file") {
        return Err(format!(
            "unsupported URI scheme `{scheme}:` in `{field}` of library `{library}`: \
             only scheme-less paths and `file:` URIs are supported"
        ));
    }
    Ok(())
}

// --- allowed_experiments.json ---

/// Parses `allowed_experiments.json`, resolving every experiment-set
/// reference at parse time (an undefined reference is an error, matching
/// the official parser).
pub(crate) fn parse_allowed_experiments(text: &str) -> Result<AllowedExperiments, String> {
    let root: serde_json::Value =
        serde_json::from_str(text).map_err(|err| format!("not valid JSON: {err}"))?;
    let root = root.as_object().ok_or("root must be a JSON object")?;

    match root.get("version") {
        Some(version) if version.as_i64() == Some(1) => {}
        other => {
            return Err(format!(
                "expected field `version` with integer value 1, found {other:?}"
            ));
        }
    }

    let sets_value = root
        .get("experimentSets")
        .ok_or("missing required field `experimentSets`")?;
    let sets = sets_value
        .as_object()
        .ok_or("`experimentSets` must be an object")?;
    let mut experiment_sets = BTreeMap::new();
    for (name, value) in sets {
        let list = value.as_array().ok_or_else(|| {
            format!("`experimentSets.{name}` must be an array of experiment names")
        })?;
        let mut set = std::collections::BTreeSet::new();
        for experiment in list {
            let experiment = experiment
                .as_str()
                .ok_or_else(|| format!("`experimentSets.{name}` must contain only strings"))?;
            set.insert(experiment.to_owned());
        }
        experiment_sets.insert(name.clone(), set);
    }
    let resolve =
        |reference: &str, at: &str| -> Result<std::collections::BTreeSet<String>, String> {
            experiment_sets
                .get(reference)
                .cloned()
                .ok_or_else(|| format!("`{at}` references undefined experiment set `{reference}`"))
        };

    let sdk = root.get("sdk").ok_or("missing required field `sdk`")?;
    let sdk = sdk.as_object().ok_or("`sdk` must be an object")?;
    let default = sdk
        .get("default")
        .ok_or("missing required field `sdk.default`")?;
    let default = default
        .as_object()
        .ok_or("`sdk.default` must be an object")?;
    let default_set = required_experiment_set(default, "sdk.default")?;
    let sdk_default = resolve(default_set, "sdk.default")?;

    let mut sdk_libraries = BTreeMap::new();
    if let Some(libraries) = sdk.get("libraries") {
        let libraries = libraries
            .as_object()
            .ok_or("`sdk.libraries` must be an object")?;
        for (name, value) in libraries {
            let entry = value.as_object().ok_or_else(|| {
                format!("`sdk.libraries.{name}` must be an object with an `experimentSet`")
            })?;
            let set_name = required_experiment_set(entry, &format!("sdk.libraries.{name}"))?;
            sdk_libraries.insert(name.clone(), resolve(set_name, "sdk.libraries")?);
        }
    }

    let mut packages = BTreeMap::new();
    if let Some(packages_value) = root.get("packages") {
        let packages_value = packages_value
            .as_object()
            .ok_or("`packages` must be an object")?;
        for (name, value) in packages_value {
            let entry = value.as_object().ok_or_else(|| {
                format!("`packages.{name}` must be an object with an `experimentSet`")
            })?;
            let set_name = required_experiment_set(entry, &format!("packages.{name}"))?;
            packages.insert(name.clone(), resolve(set_name, "packages")?);
        }
    }

    Ok(AllowedExperiments::new(
        sdk_default,
        sdk_libraries,
        packages,
    ))
}

/// Reads the mandatory `experimentSet` string out of a grant object.
fn required_experiment_set<'a>(
    entry: &'a serde_json::Map<String, serde_json::Value>,
    at: &str,
) -> Result<&'a str, String> {
    let value = entry
        .get("experimentSet")
        .ok_or_else(|| format!("missing required field `{at}.experimentSet`"))?;
    value
        .as_str()
        .ok_or_else(|| format!("`{at}.experimentSet` must be a string"))
}

#[cfg(test)]
mod tests;
