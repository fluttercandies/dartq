//! `pubspec.yaml` consumption (facts §2): the minimal field set dartq
//! reads — identity, environment, workspace/resolution, and the three
//! dependency sections in their hosted/git/path/sdk forms.
//!
//! dartq does no version solving, so only *structural* validity is
//! enforced here, matching pub's parse-time errors (unknown `resolution`
//! values, the `>=3.5` gate on `workspace`/`resolution`, `dart` as an
//! environment key, `packageNameRegExp` + reserved-word checks on `name`,
//! and `packageNameRegExp` + self-rejection on every dependency name). The
//! discovery-critical `workspace`/`resolution` keys and the whole
//! `dependency_overrides` section are honored via
//! [`Pubspec::parse_with_overrides`] — the overrides file wins where its key
//! is present (pubspec.dart:86-112, 169-193), and its key set is whitelisted
//! (pubspec.dart:393-407).

use std::collections::BTreeMap;
use std::path::Path;

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
    /// override the discovery-critical `workspace` and `resolution` keys and
    /// supply the whole `dependency_overrides` section — pub loads every
    /// pubspec on the root walk with `allowOverridesFile: true`
    /// (entrypoint.dart:95-101) and takes each key from the overrides file
    /// when present (pubspec.dart:87-91, 108-112, 169-193). The overrides
    /// file's key set is whitelisted (pubspec.dart:393-407).
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
                    serde_yaml::Value::Mapping(map) => {
                        // pub rejects any key outside the whitelist
                        // (pubspec.dart:393-407).
                        for key in map.keys() {
                            if !matches!(
                                key.as_str(),
                                Some("dependency_overrides" | "resolution" | "workspace")
                            ) {
                                return Err(PubError::Pubspec(
                                    "pubspec_overrides.yaml only supports the \
                                     `dependency_overrides`, `resolution` and `workspace` fields."
                                        .into(),
                                ));
                            }
                        }
                        Some(map)
                    }
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
        validate_package_name(&name, &invalid)?;

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
            parse_dependency_map(root.get(key), key, &name, &invalid)
        };
        let dependencies = dep("dependencies")?;
        let dev_dependencies = dep("dev_dependencies")?;
        // The overrides file's `dependency_overrides` replaces the pubspec's
        // entirely when present (pubspec.dart:169-193), and the root's
        // effective map is what `validateWorkspace` and the graph see
        // (package.dart:531-555). Both origins are checked against the
        // pubspec's own name (pubspec.dart:179, 190).
        let dependency_overrides = match overrides.and_then(|map| map.get("dependency_overrides")) {
            Some(node) => {
                parse_dependency_map(Some(node), "dependency_overrides", &name, &invalid)?
            }
            None => dep("dependency_overrides")?,
        };

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
        // pub validates the lexically normalized path: relative, not '.', and
        // within '.' (pubspec.dart:89-101 — `p.isRelative`, `p.equals`,
        // `p.isWithin`, all normalization-based, so `a/../b` -> `b` and
        // `pkg_a/` -> `pkg_a` are valid; only '.' itself and paths whose
        // normalization keeps a leading `..` fail). The raw spelling is kept
        // (pubspec.dart:100).
        if path.starts_with('/') {
            return Err(invalid(
                "\"workspace\" members must be relative paths".into(),
            ));
        }
        let normalized = crate::workspace::normalize(Path::new(path));
        let escapes = normalized
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir));
        if normalized.as_os_str().is_empty() || normalized == Path::new(".") || escapes {
            return Err(invalid(
                "\"workspace\" members must be subdirectories".into(),
            ));
        }
        members.push(path.to_owned());
    }
    Ok(members)
}

/// pub's `packageNameRegExp` (`^[a-zA-Z_]\w*(\.[a-zA-Z_]\w*)*$`,
/// pubspec_parse.dart:17-19 over the ASCII `identifierRegExp` of
/// utils.dart:31): dot-separated Dart identifiers, fully anchored.
fn is_valid_package_name(name: &str) -> bool {
    name.split('.').all(is_dart_identifier)
}

/// pub's name-field validation (`expectPackageNameField`,
/// utils.dart:795-808): `packageNameRegExp`, then the Dart reserved-word
/// rejection (utils.dart:38-107 — the commented-out words there are
/// deliberately allowed). Dependency names get the regexp only
/// (pubspec.dart:596-598).
fn validate_package_name(name: &str, invalid: &dyn Fn(String) -> PubError) -> Result<(), PubError> {
    if !is_valid_package_name(name) {
        return Err(invalid(
            "\"name\" field must be a valid Dart identifier.".into(),
        ));
    }
    if RESERVED_WORDS.contains(&name.to_lowercase().as_str()) {
        return Err(invalid(
            "\"name\" field may not be a Dart reserved word.".into(),
        ));
    }
    Ok(())
}

fn is_dart_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Dart reserved words, ported verbatim from utils.dart:38-107 (including its
/// commented-out allowances: `async`, `base`, `dynamic`, `extension`,
/// `factory`, `get`, `show`, `static`, `sync`, `typedef`, `when` stay legal).
const RESERVED_WORDS: [&str; 57] = [
    "abstract",
    "as",
    "assert",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "covariant",
    "default",
    "deferred",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "external",
    "false",
    "final",
    "finally",
    "for",
    "hide",
    "if",
    "implements",
    "import",
    "in",
    "inline",
    "interface",
    "is",
    "late",
    "library",
    "mixin",
    "new",
    "null",
    "of",
    "on",
    "operator",
    "part",
    "required",
    "rethrow",
    "return",
    "sealed",
    "set",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "type",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

fn parse_dependency_map(
    value: Option<&serde_yaml::Value>,
    section: &str,
    package_name: &str,
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
            .ok_or_else(|| invalid("A dependency name must be a string.".into()))?;
        // Every dependency name — regular, dev and override, whatever the
        // source — is checked before its spec is parsed
        // (pubspec.dart:596-598). Reserved words are NOT rejected here;
        // that check applies to the `name:` field only (utils.dart:795-808).
        if !is_valid_package_name(name) {
            return Err(invalid("Not a valid package name.".into()));
        }
        // pubspec.dart:599-602: a package may not list itself as a
        // dependency — checked per entry against the containing pubspec's
        // own `name` (`_packageName`, pubspec.dart:45). pub reads
        // `fields['name']` lazily but before any spec parsing;
        // `parse_mapping` likewise reads `name` before the dependency
        // sections, so the ordering matches. pub's `packageName != null`
        // guard covers nameless pubspecs, which our parser rejects up
        // front — the check always runs here.
        if name == package_name {
            return Err(invalid(
                "A package may not list itself as a dependency.".into(),
            ));
        }
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

    #[test]
    fn validates_package_names_like_pub() {
        // pubspec_parse.dart:17-19: dot-separated Dart identifiers.
        for valid in ["a", "mini_a", "_hidden", "a.b", "f1_2.x_y"] {
            assert!(
                Pubspec::parse(&format!("name: {valid}\n")).is_ok(),
                "{valid}"
            );
        }
        for invalid in ["1abc", "-a", "a-", "a..b", "a b", "a.b."] {
            let err = Pubspec::parse(&format!("name: \"{invalid}\"\n"))
                .expect_err(&format!("`{invalid}`"));
            assert!(
                err.to_string().contains("valid Dart identifier"),
                "unexpected error: {err}"
            );
        }
    }

    #[test]
    fn rejects_dart_reserved_words_as_names() {
        let err = Pubspec::parse("name: class\n").expect_err("reserved");
        assert!(
            err.to_string().contains("may not be a Dart reserved word"),
            "unexpected error: {err}"
        );
        // Matching is case-insensitive (utils.dart:802).
        assert!(Pubspec::parse("name: While\n").is_err());
        // Words pub deliberately allows (the commented-out entries of
        // utils.dart:38-107).
        for allowed in [
            "async",
            "base",
            "dynamic",
            "extension",
            "factory",
            "get",
            "show",
            "static",
            "sync",
            "typedef",
            "when",
        ] {
            assert!(
                Pubspec::parse(&format!("name: {allowed}\n")).is_ok(),
                "{allowed}"
            );
        }
    }

    #[test]
    fn validates_dependency_names_like_pub() {
        // pubspec.dart:596-598: the same `packageNameRegExp` as the `name`
        // field, with the message "Not a valid package name." — applied to
        // all three sections and every source form.
        let ok = "name: a\ndependencies:\n  a.b_c: ^1.0.0\n";
        let spec = Pubspec::parse(ok).expect("ok");
        assert!(spec.dependencies.contains_key("a.b_c"));

        for section in ["dependencies", "dev_dependencies", "dependency_overrides"] {
            let yaml = format!("name: a\n{section}:\n  1abc: ^1.0.0\n");
            let err = Pubspec::parse(&yaml).expect_err(section);
            assert!(
                err.to_string().contains("Not a valid package name."),
                "unexpected error: {err}"
            );
        }

        // The name is checked before the spec, so the source form is
        // irrelevant (pubspec.dart:596 precedes the source dispatch).
        let git_bad =
            "name: a\ndependencies:\n  a-:\n    git:\n      url: /tmp/r\n      ref: main\n";
        let err = Pubspec::parse(git_bad).expect_err("git bad name");
        assert!(
            err.to_string().contains("Not a valid package name."),
            "unexpected error: {err}"
        );

        // Reserved words are legal dependency names: that rejection lives
        // only in `expectPackageNameField` (utils.dart:795-808), not in
        // `_parseDependencies`.
        let reserved = "name: a\ndependencies:\n  class: ^1.0.0\n";
        let spec = Pubspec::parse(reserved).expect("reserved words are allowed as dep names");
        assert!(spec.dependencies.contains_key("class"));
    }

    #[test]
    fn overrides_file_dependency_names_are_validated() {
        let err = Pubspec::parse_with_overrides(
            "name: m\n",
            Some("dependency_overrides:\n  a..b: ^1.0.0\n"),
        )
        .expect_err("bad overrides dep name");
        assert!(
            err.to_string().contains("Not a valid package name."),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn rejects_self_dependency_in_every_section() {
        // pubspec.dart:599-602 via `_packageName` (pubspec.dart:45): the
        // check passes the pubspec's own name into all three sections
        // (pubspec.dart:142, 155, 179, 190), including `dependency_overrides`
        // served from pubspec_overrides.yaml.
        let message = "A package may not list itself as a dependency.";
        for section in ["dependencies", "dev_dependencies", "dependency_overrides"] {
            let yaml = format!("name: a\n{section}:\n  a: ^1.0.0\n");
            let err = Pubspec::parse(&yaml).expect_err(section);
            assert!(err.to_string().contains(message), "unexpected error: {err}");
        }
        let err = Pubspec::parse_with_overrides(
            "name: m\n",
            Some("dependency_overrides:\n  m: ^1.0.0\n"),
        )
        .expect_err("overrides self");
        assert!(err.to_string().contains(message), "unexpected error: {err}");

        // The self check precedes spec parsing (pubspec.dart:599 precedes
        // the source dispatch), so an unparsable source form on the self
        // entry reports the self-dependency message.
        let err = Pubspec::parse("name: a\ndependencies:\n  a:\n    cvs: whatever\n")
            .expect_err("self before spec");
        assert!(err.to_string().contains(message), "unexpected error: {err}");
    }

    #[test]
    fn allows_non_self_dependency_names() {
        // pubspec.dart:600 compares exact string equality, so `a.b` and
        // `ab` do not conflict with `a`.
        let yaml = "\
name: a
dependencies:
  a.b: ^1.0.0
  ab: ^2.0.0
dev_dependencies:
  b: ^1.0.0
dependency_overrides:
  c: ^1.0.0
";
        let spec = Pubspec::parse(yaml).expect("ok");
        assert_eq!(spec.dependencies.len(), 2);
        assert_eq!(spec.dev_dependencies.len(), 1);
        assert_eq!(spec.dependency_overrides.len(), 1);
    }

    #[test]
    fn rejects_unknown_overrides_keys() {
        // pubspec.dart:393-407: only three keys are allowed.
        let err = Pubspec::parse_with_overrides("name: m\n", Some("name: other\n"))
            .expect_err("name key");
        assert!(
            err.to_string()
                .contains("pubspec_overrides.yaml only supports the `dependency_overrides`, `resolution` and `workspace` fields"),
            "unexpected error: {err}"
        );
        assert!(
            Pubspec::parse_with_overrides("name: m\n", Some("environment:\n  sdk: ^3.5.0\n"))
                .is_err()
        );
        // All three whitelisted keys together are fine.
        let spec = Pubspec::parse_with_overrides(
            "name: m\nenvironment:\n  sdk: ^3.5.0\n",
            Some(
                "dependency_overrides:\n  foo: ^1.0.0\nresolution: workspace\nworkspace:\n  - a\n",
            ),
        )
        .expect("ok");
        assert_eq!(spec.resolution, Resolution::Workspace);
    }

    #[test]
    fn overrides_dependency_overrides_replace_the_pubspecs() {
        // The sdk floor is load-bearing here: the overrides-file
        // `resolution` key below is checked against the pubspec's language
        // version, which defaults to 2.7 without an sdk constraint
        // (pubspec.dart:106-116, language_version.dart:111).
        let yaml =
            "name: m\nenvironment:\n  sdk: ^3.5.0\ndependency_overrides:\n  a: ^1.0.0\n  b: ^2.0.0\n";
        // No overrides file: the pubspec map stands.
        let spec = Pubspec::parse(yaml).expect("ok");
        assert_eq!(spec.dependency_overrides.len(), 2);
        // Overrides file carrying `dependency_overrides` replaces it wholly
        // (pubspec.dart:169-193).
        let replaced =
            Pubspec::parse_with_overrides(yaml, Some("dependency_overrides:\n  c: ^3.0.0\n"))
                .expect("ok");
        assert_eq!(replaced.dependency_overrides.len(), 1);
        assert!(replaced.dependency_overrides.contains_key("c"));
        // Key absent from the overrides file: pubspec map stands.
        let untouched =
            Pubspec::parse_with_overrides(yaml, Some("resolution: workspace\n")).expect("ok");
        assert_eq!(untouched.dependency_overrides.len(), 2);
    }

    #[test]
    fn workspace_members_validate_on_normalized_paths() {
        // pubspec.dart:89-101 via package:path normalization: inner `..`,
        // `.` segments and trailing slashes are legal spellings.
        for valid in ["a/../b", "pkg_a/", "./a", "a/./b", "a//b", "a/b/"] {
            let spec = Pubspec::parse(&format!(
                "name: r\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - \"{valid}\"\n"
            ))
            .unwrap_or_else(|e| panic!("`{valid}`: {e}"));
            assert_eq!(spec.workspace, vec![valid], "raw spelling is kept");
        }
        // Only '.' itself and paths escaping the root fail.
        for invalid in [".", "..", "../out", "a/../../b", "/abs"] {
            assert!(
                Pubspec::parse(&format!(
                    "name: r\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - \"{invalid}\"\n"
                ))
                .is_err(),
                "`{invalid}`"
            );
        }
    }
}
