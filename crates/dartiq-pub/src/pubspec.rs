//! `pubspec.yaml` consumption (facts §2): the minimal field set dartiq
//! reads — identity, environment, workspace/resolution, and the three
//! dependency sections in their hosted/git/path/sdk forms.
//!
//! dartiq does no version solving, so only *structural* validity is
//! enforced here, matching pub's parse-time errors (unknown `resolution`
//! values, the `>=3.5` gate on `workspace`/`resolution`, `dart` as an
//! environment key). Dependency merging from `pubspec_overrides.yaml` is
//! out of scope, but the discovery-critical `workspace`/`resolution` keys
//! are honored via [`Pubspec::parse_with_overrides`] — pub merges them with
//! the overrides file taking precedence (pubspec.dart:86-88, 107-109).

use std::collections::BTreeMap;

use crate::PubError;
use crate::version::{LanguageVersion, Version, VersionConstraint};

/// `environment:` constraints by SDK name (`sdk` maps to Dart; anything
/// else, e.g. `flutter`, keeps its raw parsed constraint).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Environment {
    /// The `sdk` key — the Dart SDK constraint driving language version
    /// inference.
    pub sdk: Option<VersionConstraint>,
    /// Additional SDK flavors (`flutter: ...`, ...).
    pub flavors: BTreeMap<String, VersionConstraint>,
}

/// The `resolution:` mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Resolution {
    /// No `resolution:` key.
    #[default]
    Unspecified,
    /// Member of a pub workspace; resolution happens at the root.
    Workspace,
    Local,
    External,
}

impl Resolution {
    /// pub's enum name, as quoted in its workspace-discovery errors
    /// (entrypoint.dart:149-152: `resolution.name`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Resolution::Unspecified => "none",
            Resolution::Workspace => "workspace",
            Resolution::Local => "local",
            Resolution::External => "external",
        }
    }
}

/// One dependency entry, in the source form pub accepts (facts §2).
#[derive(Clone, Debug, PartialEq)]
pub enum DependencySpec {
    /// `foo: ^1.2.3` or `{hosted: ..., version: ...}`; absent `version`
    /// means `any`.
    Hosted {
        /// `hosted:` value when the map form names a custom server.
        url: Option<String>,
        /// Nested `name:` inside a hosted map.
        name: Option<String>,
        version: Option<VersionConstraint>,
    },
    /// `git: <url>` or `{git: {url, ref?, path?, tag-pattern?}}`.
    Git {
        url: String,
        /// Branch, commit or tag; mutually exclusive with `tag_pattern`.
        reference: Option<String>,
        tag_pattern: Option<String>,
        /// Subdirectory inside the repository.
        path: Option<String>,
    },
    /// `path: <relative-or-absolute>`; resolved against the containing
    /// pubspec by the caller.
    Path { path: String },
    /// `sdk: <sdk-id>`, e.g. `flutter`.
    Sdk { sdk: String },
}

/// A parsed `pubspec.yaml`.
#[derive(Clone, Debug, PartialEq)]
pub struct Pubspec {
    pub name: String,
    /// Absent in many packages; pub then treats it as `0.0.0`
    /// (`pubspec_parse.dart:47-52`), so callers compare with `Option`.
    pub version: Option<Version>,
    pub environment: Environment,
    /// `workspace:` member paths as written (relative subdirectories),
    /// empty when absent.
    pub workspace: Vec<String>,
    pub resolution: Resolution,
    pub dependencies: BTreeMap<String, DependencySpec>,
    pub dev_dependencies: BTreeMap<String, DependencySpec>,
    pub dependency_overrides: BTreeMap<String, DependencySpec>,
}

impl Pubspec {
    /// Parses `pubspec.yaml` text.
    ///
    /// # Errors
    ///
    /// Returns [`PubError::Pubspec`] for invalid YAML, a missing `name`,
    /// constraint syntax errors, unknown `resolution` values, or
    /// `workspace:`/`resolution:` on a package whose language version is
    /// below 3.5 (facts §2, `pubspec.dart:71-133`).
    pub fn parse(yaml: &str) -> Result<Pubspec, PubError> {
        Self::parse_with_overrides(yaml, None)
    }

    /// Parses `pubspec.yaml` text, letting an existing `pubspec_overrides.yaml`
    /// override the discovery-critical `workspace` and `resolution` keys —
    /// pub loads every pubspec on the root walk with `allowOverridesFile:
    /// true` (entrypoint.dart:95-101) and merges those two keys with the
    /// overrides file winning (pubspec.dart:87-91, 108-112). Other overrides
    /// keys (dependency merging) are not consumed here.
    ///
    /// `overrides` is the overrides file's text, or `None` when the file
    /// does not exist.
    ///
    /// # Errors
    ///
    /// As [`Self::parse`], plus [`PubError::Pubspec`] when `overrides` is
    /// given but is not valid YAML mapping text.
    pub fn parse_with_overrides(yaml: &str, overrides: Option<&str>) -> Result<Pubspec, PubError> {
        let overrides = match overrides {
            None => None,
            Some(text) => {
                let value: serde_yaml::Value = serde_yaml::from_str(text)
                    .map_err(|e| PubError::Pubspec(format!("pubspec_overrides.yaml: {e}")))?;
                match value {
                    serde_yaml::Value::Null => None, // empty overrides file
                    serde_yaml::Value::Mapping(map) => Some(map),
                    _ => {
                        return Err(PubError::Pubspec(
                            "pubspec_overrides.yaml must be a mapping".into(),
                        ));
                    }
                }
            }
        };
        Self::parse_mapping(yaml, overrides.as_ref())
    }

    fn parse_mapping(
        yaml: &str,
        overrides: Option<&serde_yaml::Mapping>,
    ) -> Result<Pubspec, PubError> {
        let invalid = |what: String| PubError::Pubspec(what);
        let root: serde_yaml::Value =
            serde_yaml::from_str(yaml).map_err(|e| invalid(e.to_string()))?;
        let root = root
            .as_mapping()
            .ok_or_else(|| invalid("pubspec.yaml must be a mapping".into()))?;

        let name = optional_field(root, "name")?
            .filter(|value| !value.is_empty())
            .ok_or_else(|| invalid("pubspec.yaml needs a non-empty `name`".into()))?;

        let version = match optional_field(root, "version")? {
            None => None,
            Some(text) => Some(
                Version::parse(&text).map_err(|e| PubError::Pubspec(format!("version: {e}")))?,
            ),
        };

        let environment = parse_environment(root.get("environment"), &invalid)?;

        // `workspace`/`resolution` require language >= 3.5, where the
        // language version is this pubspec's own sdk lower bound.
        let language_version = LanguageVersion::from_sdk_constraint(environment.sdk.as_ref());
        let workspace_node = discovery_field(root, overrides, "workspace");
        let workspace = match workspace_node {
            None | Some(serde_yaml::Value::Null) => Vec::new(),
            Some(_) => {
                require_workspaces(language_version)?;
                parse_workspace(workspace_node, &invalid)?
            }
        };
        let resolution = match discovery_field(root, overrides, "resolution") {
            None | Some(serde_yaml::Value::Null) => Resolution::Unspecified,
            Some(text) => {
                require_workspaces(language_version)?;
                let text = text
                    .as_str()
                    .ok_or_else(|| invalid("`resolution` must be a string".into()))?;
                match text {
                    "workspace" => Resolution::Workspace,
                    "local" => Resolution::Local,
                    "external" => Resolution::External,
                    other => {
                        return Err(invalid(format!(
                            "`resolution` must be one of `workspace`, `local`, `external`, got `{other}`"
                        )));
                    }
                }
            }
        };

        let dep = |key: &str| -> Result<BTreeMap<String, DependencySpec>, PubError> {
            parse_dependency_map(root.get(key), key, &invalid)
        };
        let dependencies = dep("dependencies")?;
        let dev_dependencies = dep("dev_dependencies")?;
        let dependency_overrides = dep("dependency_overrides")?;

        Ok(Pubspec {
            name,
            version,
            environment,
            workspace,
            resolution,
            dependencies,
            dev_dependencies,
            dependency_overrides,
        })
    }
}

fn require_workspaces(language_version: LanguageVersion) -> Result<(), PubError> {
    if language_version < LanguageVersion::FIRST_WITH_WORKSPACES {
        return Err(PubError::Pubspec(format!(
            "`workspace` and `resolution` require at least language version {}",
            LanguageVersion::FIRST_WITH_WORKSPACES
        )));
    }
    Ok(())
}

/// The effective value of a discovery key (`workspace`, `resolution`):
/// the overrides mapping wins when it carries the key
/// (`_overridesFileFields?.nodes[key] ?? fields.nodes[key]`,
/// pubspec.dart:87-91, 108-112).
fn discovery_field<'a>(
    root: &'a serde_yaml::Mapping,
    overrides: Option<&'a serde_yaml::Mapping>,
    key: &str,
) -> Option<&'a serde_yaml::Value> {
    overrides
        .and_then(|map| map.get(key))
        .or_else(|| root.get(key))
}

/// Reads a scalar string field, erroring when present but not a string.
fn optional_field(root: &serde_yaml::Mapping, key: &str) -> Result<Option<String>, PubError> {
    let value = root.get(key);
    match value {
        None | Some(serde_yaml::Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(str::to_owned)
            .map(Some)
            .ok_or_else(|| PubError::Pubspec(format!("`{key}` must be a string"))),
    }
}

fn parse_environment(
    value: Option<&serde_yaml::Value>,
    invalid: &dyn Fn(String) -> PubError,
) -> Result<Environment, PubError> {
    let Some(value) = value else {
        return Ok(Environment::default());
    };
    let mapping = value
        .as_mapping()
        .ok_or_else(|| invalid("`environment` must be a mapping".into()))?;
    let mut environment = Environment::default();
    for (key, constraint) in mapping {
        let name = key
            .as_str()
            .ok_or_else(|| invalid("SDK names must be strings".into()))?;
        let constraint_text = constraint
            .as_str()
            .ok_or_else(|| invalid(format!("environment `{name}` must be a string")))?;
        let parsed = VersionConstraint::parse(constraint_text)
            .map_err(|e| PubError::Pubspec(format!("environment `{name}`: {e}")))?;
        match name {
            // pub rejects the `dart` alias explicitly (pubspec.dart:241-243).
            "dart" => {
                return Err(invalid("use \"sdk\" for the Dart SDK constraint".into()));
            }
            "sdk" => environment.sdk = Some(parsed),
            other => {
                environment.flavors.insert(other.to_owned(), parsed);
            }
        }
    }
    Ok(environment)
}

fn parse_workspace(
    value: Option<&serde_yaml::Value>,
    invalid: &dyn Fn(String) -> PubError,
) -> Result<Vec<String>, PubError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let list = value
        .as_sequence()
        .ok_or_else(|| invalid("`workspace` must be a list of strings".into()))?;
    let mut members = Vec::with_capacity(list.len());
    for entry in list {
        let path = entry
            .as_str()
            .ok_or_else(|| invalid("`workspace` must be a list of strings".into()))?;
        // pub: relative, within '.', and not '.' itself (pubspec.dart:89-101).
        if path.is_empty()
            || path.starts_with('/')
            || path == "."
            || path.split('/').any(|part| part.is_empty() || part == "..")
        {
            return Err(invalid(format!(
                "`workspace` members must be subdirectory paths, got `{path}`"
            )));
        }
        members.push(path.to_owned());
    }
    Ok(members)
}

fn parse_dependency_map(
    value: Option<&serde_yaml::Value>,
    section: &str,
    invalid: &dyn Fn(String) -> PubError,
) -> Result<BTreeMap<String, DependencySpec>, PubError> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let mapping = value
        .as_mapping()
        .ok_or_else(|| invalid(format!("`{section}` must be a mapping")))?;
    let mut deps = BTreeMap::new();
    for (key, spec) in mapping {
        let name = key
            .as_str()
            .ok_or_else(|| invalid("dependency names must be strings".into()))?;
        let parsed = parse_dependency_spec(spec, invalid)?;
        deps.insert(name.to_owned(), parsed);
    }
    Ok(deps)
}

fn parse_dependency_spec(
    spec: &serde_yaml::Value,
    invalid: &dyn Fn(String) -> PubError,
) -> Result<DependencySpec, PubError> {
    match spec {
        // Short hosted form: `foo: ^1.2.3` (or null for unconstrained).
        serde_yaml::Value::Null => Ok(DependencySpec::Hosted {
            url: None,
            name: None,
            version: None,
        }),
        serde_yaml::Value::String(text) => {
            let version = VersionConstraint::parse(text)
                .map_err(|e| PubError::Pubspec(format!("dependency version: {e}")))?;
            Ok(DependencySpec::Hosted {
                url: None,
                name: None,
                version: Some(version),
            })
        }
        serde_yaml::Value::Mapping(map) => {
            let version = match map.get("version") {
                None | Some(serde_yaml::Value::Null) => None,
                Some(text) => {
                    let text = text
                        .as_str()
                        .ok_or_else(|| invalid("`version` must be a string".into()))?;
                    Some(
                        VersionConstraint::parse(text)
                            .map_err(|e| PubError::Pubspec(format!("version: {e}")))?,
                    )
                }
            };

            let sources: Vec<(&str, &serde_yaml::Value)> = map
                .iter()
                .filter(|(key, _)| key.as_str() != Some("version"))
                .map(|(key, value)| (key.as_str().unwrap_or(""), value))
                .collect();
            match sources.as_slice() {
                // Map form without a source key defaults to hosted.
                [] => Ok(DependencySpec::Hosted {
                    url: None,
                    name: None,
                    version,
                }),
                [("hosted", value)] => parse_hosted(value, version, invalid),
                [("git", value)] => parse_git(value, invalid),
                [("path", value)] => {
                    let path = value
                        .as_str()
                        .ok_or_else(|| invalid("`path` dependency must be a string".to_owned()))?;
                    Ok(DependencySpec::Path {
                        path: path.to_owned(),
                    })
                }
                [("sdk", value)] => {
                    let sdk = value
                        .as_str()
                        .ok_or_else(|| invalid("`sdk` dependency must be a string".to_owned()))?;
                    Ok(DependencySpec::Sdk {
                        sdk: sdk.to_owned(),
                    })
                }
                _ => Err(invalid("a dependency may only have one source".into())),
            }
        }
        _ => Err(invalid(
            "a dependency specification must be a string or a mapping".into(),
        )),
    }
}

fn parse_hosted(
    value: &serde_yaml::Value,
    version: Option<VersionConstraint>,
    invalid: &dyn Fn(String) -> PubError,
) -> Result<DependencySpec, PubError> {
    match value {
        // `hosted: https://some-pub.com`
        serde_yaml::Value::String(url) => Ok(DependencySpec::Hosted {
            url: Some(url.clone()),
            name: None,
            version,
        }),
        serde_yaml::Value::Mapping(map) => {
            let url = optional_field(map, "url")?;
            let name = optional_field(map, "name")?;
            Ok(DependencySpec::Hosted { url, name, version })
        }
        _ => Err(invalid("`hosted` must be a URL string or a mapping".into())),
    }
}

fn parse_git(
    value: &serde_yaml::Value,
    invalid: &dyn Fn(String) -> PubError,
) -> Result<DependencySpec, PubError> {
    let (url, reference, tag_pattern, path) = match value {
        // `git: <url>` shorthand.
        serde_yaml::Value::String(url) => (url.clone(), None, None, None),
        serde_yaml::Value::Mapping(map) => {
            let url = optional_field(map, "url")?
                .ok_or_else(|| invalid("git dependency needs a `url`".into()))?;
            let reference = optional_field(map, "ref")?;
            let tag_pattern = optional_field(map, "tag-pattern")?;
            if reference.is_some() && tag_pattern.is_some() {
                return Err(invalid(
                    "a git dependency may not have both `ref` and `tag-pattern`".into(),
                ));
            }
            let path = optional_field(map, "path")?;
            (url, reference, tag_pattern, path)
        }
        _ => return Err(invalid("`git` must be a URL string or a mapping".into())),
    };
    Ok(DependencySpec::Git {
        url,
        reference,
        tag_pattern,
        path,
    })
}

impl DependencySpec {
    /// Hosted custom-server URL, if the map form provided one.
    #[must_use]
    pub fn url(&self) -> Option<&str> {
        match self {
            DependencySpec::Hosted { url, .. } => url.as_deref(),
            _ => None,
        }
    }

    /// Nested hosted package name, if given.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        match self {
            DependencySpec::Hosted { name, .. } => name.as_deref(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_fields() {
        let spec = Pubspec::parse("name: mini_a\n").expect("ok");
        assert_eq!(spec.name, "mini_a");
        assert!(spec.version.is_none());
        assert!(spec.environment.sdk.is_none());
        assert!(spec.workspace.is_empty());
        assert_eq!(spec.resolution, Resolution::Unspecified);
        assert!(spec.dependencies.is_empty());
    }

    #[test]
    fn parses_environment_and_version() {
        let yaml = "name: a\nversion: 1.2.3\nenvironment:\n  sdk: '>=3.0.0 <4.0.0'\n  flutter: '>=3.3.0'\n";
        let spec = Pubspec::parse(yaml).expect("ok");
        assert_eq!(spec.version.expect("version").to_string(), "1.2.3");
        assert_eq!(
            spec.environment.sdk.expect("sdk"),
            VersionConstraint::parse(">=3.0.0 <4.0.0").expect("constraint")
        );
        assert!(spec.environment.flavors.contains_key("flutter"));
    }

    #[test]
    fn rejects_dart_environment_key() {
        let yaml = "name: a\nenvironment:\n  dart: '>=3.0.0'\n";
        assert!(matches!(Pubspec::parse(yaml), Err(PubError::Pubspec(_))));
    }

    #[test]
    fn dependency_source_forms() {
        let yaml = "\
name: a
dependencies:
  hosted_short: ^1.2.3
  hosted_map:
    hosted: https://pub.example.com
    version: '>=1.0.0 <2.0.0'
  hosted_named:
    hosted:
      url: https://pub.example.com
      name: real_name
  git_dep:
    git:
      url: /tmp/gitdep
      ref: main
  git_short:
    git: https://github.com/x/y.git
  path_dep:
    path: ../local
  sdk_dep:
    sdk: flutter
dev_dependencies:
  dev_only: 1.0.0
";
        let spec = Pubspec::parse(yaml).expect("ok");
        assert_eq!(
            spec.dependencies["hosted_short"],
            DependencySpec::Hosted {
                url: None,
                name: None,
                version: Some(VersionConstraint::parse("^1.2.3").expect("c"))
            }
        );
        assert_eq!(
            spec.dependencies["hosted_map"].url(),
            Some("https://pub.example.com")
        );
        assert_eq!(spec.dependencies["hosted_named"].name(), Some("real_name"));
        let git = match &spec.dependencies["git_dep"] {
            DependencySpec::Git { url, reference, .. } => (url.as_str(), reference.as_deref()),
            other => panic!("expected git, got {other:?}"),
        };
        assert_eq!(git, ("/tmp/gitdep", Some("main")));
        assert!(matches!(
            spec.dependencies["git_short"],
            DependencySpec::Git {
                reference: None,
                ..
            }
        ));
        assert_eq!(
            spec.dependencies["path_dep"],
            DependencySpec::Path {
                path: "../local".into()
            }
        );
        assert_eq!(
            spec.dependencies["sdk_dep"],
            DependencySpec::Sdk {
                sdk: "flutter".into()
            }
        );
        assert_eq!(
            spec.dev_dependencies["dev_only"],
            DependencySpec::Hosted {
                url: None,
                name: None,
                version: Some(VersionConstraint::Exact(Version::new(1, 0, 0)))
            }
        );
    }

    #[test]
    fn rejects_multiple_sources_and_bad_ref_combo() {
        let two_sources = "name: a\ndependencies:\n  x:\n    hosted: https://a\n    path: ../b\n";
        assert!(matches!(
            Pubspec::parse(two_sources),
            Err(PubError::Pubspec(_))
        ));
        let ref_and_tag = "name: a\ndependencies:\n  x:\n    git:\n      url: /r\n      ref: main\n      tag-pattern: 'v*'\n";
        assert!(matches!(
            Pubspec::parse(ref_and_tag),
            Err(PubError::Pubspec(_))
        ));
        let unknown_source = "name: a\ndependencies:\n  x:\n    cvs: whatever\n";
        assert!(matches!(
            Pubspec::parse(unknown_source),
            Err(PubError::Pubspec(_))
        ));
    }

    #[test]
    fn workspace_and_resolution_gate_on_language_version() {
        // >=3.5: accepted.
        let ok =
            "name: r\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - pkg_b\nresolution: workspace\n";
        let spec = Pubspec::parse(ok).expect("ok");
        assert_eq!(spec.workspace, vec!["pkg_b"]);
        assert_eq!(spec.resolution, Resolution::Workspace);

        // <3.5: both keys rejected.
        let old = "name: r\nenvironment:\n  sdk: '>=3.0.0 <4.0.0'\nresolution: workspace\n";
        assert!(matches!(Pubspec::parse(old), Err(PubError::Pubspec(_))));
        // No sdk constraint: pub default 2.7 < 3.5 -> rejected.
        let no_sdk = "name: r\nworkspace:\n  - pkg_b\n";
        assert!(matches!(Pubspec::parse(no_sdk), Err(PubError::Pubspec(_))));
    }

    #[test]
    fn rejects_unknown_resolution_and_bad_members() {
        let unknown = "name: r\nenvironment:\n  sdk: ^3.5.0\nresolution: megaverse\n";
        assert!(matches!(Pubspec::parse(unknown), Err(PubError::Pubspec(_))));
        let dot = "name: r\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - '.'\n";
        assert!(matches!(Pubspec::parse(dot), Err(PubError::Pubspec(_))));
        let escape = "name: r\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - '../outside'\n";
        assert!(matches!(Pubspec::parse(escape), Err(PubError::Pubspec(_))));
        let absolute = "name: r\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - /abs\n";
        assert!(matches!(
            Pubspec::parse(absolute),
            Err(PubError::Pubspec(_))
        ));
    }

    #[test]
    fn overrides_file_wins_on_discovery_keys() {
        let yaml = "name: m\nenvironment:\n  sdk: ^3.5.0\nresolution: local\nworkspace:\n  - a\n";
        let spec = Pubspec::parse(yaml).expect("ok");
        assert_eq!(spec.resolution, Resolution::Local);
        assert_eq!(spec.workspace, vec!["a"]);

        // Both keys together: the overrides values win.
        let both =
            Pubspec::parse_with_overrides(yaml, Some("resolution: workspace\nworkspace:\n  - b\n"))
                .expect("ok");
        assert_eq!(both.resolution, Resolution::Workspace);
        assert_eq!(both.workspace, vec!["b"]);

        // A key absent from the overrides file stays at the pubspec value.
        let partial =
            Pubspec::parse_with_overrides(yaml, Some("resolution: external\n")).expect("ok");
        assert_eq!(partial.resolution, Resolution::External);
        assert_eq!(partial.workspace, vec!["a"]);

        // Keys the pubspec lacks can be introduced by the overrides file.
        let introduced = Pubspec::parse_with_overrides(
            "name: m\nenvironment:\n  sdk: ^3.5.0\n",
            Some("resolution: workspace\n"),
        )
        .expect("ok");
        assert_eq!(introduced.resolution, Resolution::Workspace);
    }

    #[test]
    fn overrides_file_is_validated_like_the_pubspec() {
        // Unknown resolution values and bad member shapes are rejected.
        assert!(matches!(
            Pubspec::parse_with_overrides("name: m\n", Some("resolution: megaverse\n")),
            Err(PubError::Pubspec(_))
        ));
        assert!(matches!(
            Pubspec::parse_with_overrides("name: m\n", Some("workspace:\n  - '../out'\n")),
            Err(PubError::Pubspec(_))
        ));
        // The >=3.5 gate applies to keys coming from the overrides file too.
        assert!(matches!(
            Pubspec::parse_with_overrides(
                "name: m\nenvironment:\n  sdk: '>=3.0.0 <4.0.0'\n",
                Some("resolution: workspace\n"),
            ),
            Err(PubError::Pubspec(_))
        ));
        // Non-mapping overrides text is rejected; empty text is no keys.
        assert!(Pubspec::parse_with_overrides("name: m\n", Some("- a\n")).is_err());
        let empty = Pubspec::parse_with_overrides("name: m\n", Some("")).expect("ok");
        assert_eq!(empty.resolution, Resolution::Unspecified);
        assert!(empty.workspace.is_empty());
    }

    #[test]
    fn requires_name_and_mapping() {
        assert!(Pubspec::parse("version: 1.0.0\n").is_err());
        assert!(Pubspec::parse("").is_err());
        assert!(Pubspec::parse("- a\n- b\n").is_err());
    }
}
