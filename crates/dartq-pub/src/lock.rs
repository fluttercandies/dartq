//! `pubspec.lock` parsing (facts §3).
//!
//! Consumer notes baked in:
//! - `sdks` holds pub's **effective** constraints (R6-rewritten), never to
//!   be diffed against a pubspec's original text (facts §8.1);
//! - the legacy top-level `sdk` key from pub 1.14-1.18 is folded into
//!   `sdks["dart"]`;
//! - `dependency` classification only reflects the root package's sections
//!   and is absent in old locks;
//! - absolute path dependencies serialize `relative: 'false'` — the
//!   *string* `"false"`, a pub quirk that must parse as `false`
//!   (facts §3/§8.4).

use std::collections::BTreeMap;

use serde::Serialize;

use crate::PubError;
use crate::version::Version;

/// Which source a locked package (or config entry) came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum SourceKind {
    Hosted,
    Git,
    Path,
    Sdk,
}

impl SourceKind {
    fn parse(text: &str) -> Result<SourceKind, PubError> {
        match text {
            "hosted" => Ok(SourceKind::Hosted),
            "git" => Ok(SourceKind::Git),
            "path" => Ok(SourceKind::Path),
            "sdk" => Ok(SourceKind::Sdk),
            other => Err(PubError::LockFile(format!("unknown source `{other}`"))),
        }
    }
}

/// The root-package classification pub writes into `dependency:`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum DependencyKind {
    /// `"direct main"`.
    DirectMain,
    /// `"direct dev"`.
    DirectDev,
    /// `"direct overridden"` — only in `dependency_overrides` (facts §7).
    DirectOverridden,
    /// `transitive` — including workspace members' direct deps, which pub
    /// still records as transitive (facts §8.2).
    Transitive,
}

impl DependencyKind {
    fn parse(text: &str) -> Result<DependencyKind, PubError> {
        match text {
            "direct main" => Ok(DependencyKind::DirectMain),
            "direct dev" => Ok(DependencyKind::DirectDev),
            "direct overridden" => Ok(DependencyKind::DirectOverridden),
            "transitive" => Ok(DependencyKind::Transitive),
            other => Err(PubError::LockFile(format!(
                "unknown dependency classification `{other}`"
            ))),
        }
    }
}

/// The `description:` of a locked package, per source.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum LockDescription {
    Hosted {
        /// Must equal the lock entry's key (pub validates this).
        name: String,
        url: String,
        sha256: Option<String>,
    },
    Git {
        url: String,
        /// Branch/commit/tag; mutually exclusive with `tag_pattern`.
        reference: Option<String>,
        tag_pattern: Option<String>,
        /// The git commit actually checked out.
        resolved_ref: String,
        /// Subdirectory inside the repository; `"."` for the repo root.
        path: String,
    },
    Path {
        /// Path as written in the lock: relative to the lock's directory
        /// when `relative`, absolute otherwise.
        path: String,
        relative: bool,
    },
    /// A bare sdk name, e.g. `"flutter"` (sdk.dart:203-205).
    Sdk(String),
}

/// One `packages:` entry.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LockedPackage {
    pub version: Version,
    pub source: SourceKind,
    pub description: LockDescription,
    /// Absent in locks written by older pub versions.
    pub dependency: Option<DependencyKind>,
}

/// A parsed `pubspec.lock`.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct LockFile {
    /// SDK name -> **effective** constraint string (facts §8.1: do not
    /// compare against pubspec originals).
    pub sdks: BTreeMap<String, String>,
    /// Locked packages by name; the root package is never here.
    pub packages: BTreeMap<String, LockedPackage>,
}

impl LockFile {
    /// An empty lock, equivalent to parsing an empty file.
    #[must_use]
    pub fn empty() -> LockFile {
        LockFile::default()
    }

    /// Parses `pubspec.lock` text.
    ///
    /// # Errors
    ///
    /// Returns [`PubError::LockFile`] for invalid YAML or malformed
    /// package entries.
    pub fn parse(yaml: &str) -> Result<LockFile, PubError> {
        // pub: an empty file is an empty lock (lock_file.dart:104).
        if yaml.trim().is_empty() {
            return Ok(LockFile::empty());
        }
        let invalid = |what: String| PubError::LockFile(what);
        let root: serde_yaml::Value =
            serde_yaml::from_str(yaml).map_err(|e| invalid(e.to_string()))?;
        let root = root
            .as_mapping()
            .ok_or_else(|| invalid("pubspec.lock must be a YAML mapping".into()))?;

        let mut sdks = BTreeMap::new();
        // Legacy top-level `sdk` key (pub 1.14-1.18 lockfiles).
        if let Some(legacy) = root.get("sdk") {
            let text = legacy
                .as_str()
                .ok_or_else(|| invalid("legacy `sdk` must be a string".into()))?;
            sdks.insert("dart".to_owned(), text.to_owned());
        }
        if let Some(map) = root.get("sdks") {
            let map = map
                .as_mapping()
                .ok_or_else(|| invalid("`sdks` must be a mapping".into()))?;
            for (name, constraint) in map {
                let name = name
                    .as_str()
                    .ok_or_else(|| invalid("sdk names must be strings".into()))?;
                let text = constraint
                    .as_str()
                    .ok_or_else(|| invalid(format!("sdks.{name} must be a string")))?;
                sdks.insert(name.to_owned(), text.to_owned());
            }
        }

        let mut packages = BTreeMap::new();
        if let Some(map) = root.get("packages") {
            let map = map
                .as_mapping()
                .ok_or_else(|| invalid("`packages` must be a mapping".into()))?;
            for (name, spec) in map {
                let name = name
                    .as_str()
                    .ok_or_else(|| invalid("package names must be strings".into()))?;
                let spec = spec
                    .as_mapping()
                    .ok_or_else(|| invalid(format!("package `{name}` entry must be a mapping")))?;
                packages.insert(name.to_owned(), parse_package(name, spec)?);
            }
        }

        Ok(LockFile { sdks, packages })
    }

    /// Looks a locked package up by name.
    #[must_use]
    pub fn package(&self, name: &str) -> Option<&LockedPackage> {
        self.packages.get(name)
    }
}

fn parse_package(name: &str, spec: &serde_yaml::Mapping) -> Result<LockedPackage, PubError> {
    let invalid = |what: String| PubError::LockFile(what);
    let version_text = spec
        .get("version")
        .and_then(serde_yaml::Value::as_str)
        .ok_or_else(|| invalid(format!("package `{name}` needs a string `version`")))?;
    let version = Version::parse(version_text)
        .map_err(|e| PubError::LockFile(format!("package `{name}` version: {e}")))?;

    let source_text = spec
        .get("source")
        .and_then(serde_yaml::Value::as_str)
        .ok_or_else(|| invalid(format!("package `{name}` needs a string `source`")))?;
    let source = SourceKind::parse(source_text)?;

    let description = parse_description(name, source, spec.get("description"), &invalid)?;

    let dependency =
        match spec.get("dependency") {
            None | Some(serde_yaml::Value::Null) => None,
            Some(text) => Some(
                DependencyKind::parse(text.as_str().ok_or_else(|| {
                    invalid(format!("package `{name}` dependency must be a string"))
                })?)
                .map_err(|e| PubError::LockFile(format!("package `{name}`: {e}")))?,
            ),
        };

    Ok(LockedPackage {
        version,
        source,
        description,
        dependency,
    })
}

fn parse_description(
    name: &str,
    source: SourceKind,
    value: Option<&serde_yaml::Value>,
    invalid: &dyn Fn(String) -> PubError,
) -> Result<LockDescription, PubError> {
    let missing = || invalid(format!("package `{name}` needs a description"));
    match source {
        // sdk descriptions are a bare scalar (lock_file.dart:180-184).
        SourceKind::Sdk => {
            let text = value
                .and_then(serde_yaml::Value::as_str)
                .ok_or_else(missing)?;
            Ok(LockDescription::Sdk(text.to_owned()))
        }
        SourceKind::Hosted | SourceKind::Git | SourceKind::Path => {
            let map = value
                .and_then(serde_yaml::Value::as_mapping)
                .ok_or_else(missing)?;
            parse_mapped_description(name, source, map, invalid)
        }
    }
}

fn parse_mapped_description(
    name: &str,
    source: SourceKind,
    map: &serde_yaml::Mapping,
    invalid: &dyn Fn(String) -> PubError,
) -> Result<LockDescription, PubError> {
    let string_field = |key: &str| -> Result<Option<String>, PubError> {
        match map.get(key) {
            None | Some(serde_yaml::Value::Null) => Ok(None),
            Some(value) => value.as_str().map(str::to_owned).map(Some).ok_or_else(|| {
                invalid(format!(
                    "package `{name}` description `{key}` must be a string"
                ))
            }),
        }
    };
    let required = |key: &str| -> Result<String, PubError> {
        string_field(key)?
            .ok_or_else(|| invalid(format!("package `{name}` description needs `{key}`")))
    };

    match source {
        SourceKind::Sdk => unreachable!("handled by caller"),
        SourceKind::Hosted => {
            let described = required("name")?;
            if described != name {
                return Err(invalid(format!(
                    "package name {name} doesn't match description name {described}"
                )));
            }
            Ok(LockDescription::Hosted {
                name: described,
                url: required("url")?,
                sha256: string_field("sha256")?,
            })
        }
        SourceKind::Git => {
            let reference = string_field("ref")?;
            let tag_pattern = string_field("tag-pattern")?;
            if reference.is_some() && tag_pattern.is_some() {
                return Err(invalid(format!(
                    "package `{name}` description may not have both `ref` and `tag-pattern`"
                )));
            }
            Ok(LockDescription::Git {
                url: required("url")?,
                reference,
                tag_pattern,
                resolved_ref: required("resolved-ref")?,
                path: string_field("path")?.unwrap_or_else(|| ".".to_owned()),
            })
        }
        SourceKind::Path => {
            // `relative` is `true`/`false` for relative paths and the
            // *string* `'false'` for absolute ones (path.dart:329-338);
            // both spellings normalize to bool here.
            let relative = match map.get("relative") {
                Some(serde_yaml::Value::Bool(value)) => *value,
                Some(serde_yaml::Value::String(text)) if text == "true" => true,
                Some(serde_yaml::Value::String(text)) if text == "false" => false,
                None => false,
                Some(other) => {
                    return Err(invalid(format!(
                        "package `{name}` description `relative` must be a bool, got {other:?}"
                    )));
                }
            };
            Ok(LockDescription::Path {
                path: required("path")?,
                relative,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "\
# Generated by pub
# See https://dart.dev/tools/pub/glossary#lockfile
packages:
  path:
    dependency: \"direct main\"
    description:
      name: path
      sha256: \"75cca69d1490965be98c73ceaea117e8a04dd21217b37b292c9ddbec0d955bc5\"
      url: \"https://pub.dev\"
    source: hosted
    version: \"1.9.1\"
  git_dep:
    dependency: transitive
    description:
      path: \".\"
      ref: main
      resolved-ref: \"9482c6eb905b42f4ee1b08626d56a17366135764\"
      url: \"/tmp/pubfacts/gitdep\"
    source: git
    version: \"0.2.0\"
  local_dep:
    dependency: \"direct overridden\"
    description:
      path: \"../gitdep\"
      relative: true
    source: path
    version: \"0.2.0\"
  flutter_pkgs:
    dependency: \"direct dev\"
    description: flutter
    source: sdk
    version: \"0.0.0\"
sdks:
  dart: \">=3.5.0 <4.0.0\"
  flutter: \">=3.3.0\"
";

    #[test]
    fn parses_all_four_sources() {
        let lock = LockFile::parse(FIXTURE).expect("valid lock");
        assert_eq!(lock.packages.len(), 4);
        assert_eq!(lock.sdks["dart"], ">=3.5.0 <4.0.0");
        assert_eq!(lock.sdks["flutter"], ">=3.3.0");

        let hosted = lock.package("path").expect("entry");
        assert_eq!(hosted.version.to_string(), "1.9.1");
        assert_eq!(hosted.dependency, Some(DependencyKind::DirectMain));
        assert_eq!(
            hosted.description,
            LockDescription::Hosted {
                name: "path".into(),
                url: "https://pub.dev".into(),
                sha256: Some(
                    "75cca69d1490965be98c73ceaea117e8a04dd21217b37b292c9ddbec0d955bc5".into()
                ),
            }
        );

        let git = lock.package("git_dep").expect("entry");
        assert_eq!(git.dependency, Some(DependencyKind::Transitive));
        assert!(matches!(
            git.description,
            LockDescription::Git { ref resolved_ref, ref reference, path: ref p, .. }
            if resolved_ref == "9482c6eb905b42f4ee1b08626d56a17366135764"
                && reference.as_deref() == Some("main")
                && p == "."
        ));

        let sdk = lock.package("flutter_pkgs").expect("entry");
        assert_eq!(sdk.description, LockDescription::Sdk("flutter".into()));
        assert_eq!(sdk.dependency, Some(DependencyKind::DirectDev));
    }

    #[test]
    fn path_absolute_dependency_string_false_quirk() {
        // facts §8.4: `relative: 'false'` is the string "false".
        let yaml = "\
packages:
  abs_dep:
    dependency: \"direct main\"
    description:
      path: /elsewhere/pkg
      relative: 'false'
    source: path
    version: \"1.0.0\"
";
        let lock = LockFile::parse(yaml).expect("valid lock");
        assert_eq!(
            lock.package("abs_dep").expect("entry").description,
            LockDescription::Path {
                path: "/elsewhere/pkg".into(),
                relative: false
            }
        );
    }

    #[test]
    fn empty_file_is_empty_lock_and_legacy_sdk_key() {
        assert_eq!(LockFile::parse("   \n").expect("ok"), LockFile::empty());
        let legacy = "sdk: \">=2.19.0 <3.0.0\"\npackages: {}\n";
        let lock = LockFile::parse(legacy).expect("ok");
        assert_eq!(
            lock.sdks.get("dart").map(String::as_str),
            Some(">=2.19.0 <3.0.0")
        );
        assert!(lock.packages.is_empty());
    }

    #[test]
    fn hosted_name_mismatch_rejected() {
        let yaml = "\
packages:
  a:
    dependency: \"direct main\"
    description:
      name: b
      url: \"https://pub.dev\"
    source: hosted
    version: \"1.0.0\"
";
        assert!(matches!(LockFile::parse(yaml), Err(PubError::LockFile(_))));
    }

    #[test]
    fn rejects_unknown_source_and_dependency() {
        let unknown_source = "packages:\n  a:\n    dependency: \"direct main\"\n    description: {}\n    source: cvs\n    version: \"1.0.0\"\n";
        assert!(matches!(
            LockFile::parse(unknown_source),
            Err(PubError::LockFile(_))
        ));
        let unknown_dep = "packages:\n  a:\n    dependency: \"maybe\"\n    description: {}\n    source: hosted\n    version: \"1.0.0\"\n";
        assert!(matches!(
            LockFile::parse(unknown_dep),
            Err(PubError::LockFile(_))
        ));
    }

    #[test]
    fn dependency_field_is_optional() {
        let yaml = "\
packages:
  a:
    description:
      name: a
      url: \"https://pub.dev\"
    source: hosted
    version: \"1.0.0\"
";
        let lock = LockFile::parse(yaml).expect("ok");
        assert_eq!(lock.package("a").expect("entry").dependency, None);
    }
}
