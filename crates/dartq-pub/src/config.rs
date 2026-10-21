//! `.dart_tool/package_config.json` parsing.
//!
//! Consumer, not writer: unknown top-level and per-entry keys are tolerated
//! and preserved (`pub` appends `flutterRoot`/`flutterVersion`/`pubCache`),
//! entry order is kept exactly as written (pub's sort discipline is its own
//! contract), and `rootUri` resolution treats the config's own directory
//! (`.dart_tool/`) as the relative base — never the package root.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::PubError;
use crate::version::{LanguageVersion, Version};
use crate::workspace::normalize;

/// Top-level keys owned by the package_config schema; everything else lands
/// in [`PackageConfig::additional_properties`].
const RESERVED_KEYS: [&str; 5] = [
    "configVersion",
    "packages",
    "generated",
    "generator",
    "generatorVersion",
];

/// A parsed `package_config.json` (config version 2).
///
/// The `Serialize` derive feeds exactly one consumer: [`crate::graph`]'s
/// `graph_id`, which hashes the canonical re-serialization. That form uses
/// snake_case struct-field keys (`root_uri`, `generator_version`) — it is
/// **not** the package_config.json dialect (camelCase `rootUri`,
/// `generatorVersion`) and must never be used to write a config file.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PackageConfig {
    /// Entries in file order (lock packages sorted by name, then workspace
    /// members in `transitiveWorkspace` DFS order).
    pub packages: Vec<PackageConfigEntry>,
    /// Always `"pub"` for pub-generated files.
    pub generator: Option<String>,
    /// The Dart SDK version of the generating `pub`.
    pub generator_version: Option<Version>,
    /// Unknown top-level keys, preserved verbatim. This is where
    /// `flutterRoot`/`flutterVersion`/`pubCache` live; freshness compares
    /// them against the current environment.
    pub additional_properties: BTreeMap<String, serde_json::Value>,
}

/// One `packages[]` entry.
///
/// Like [`PackageConfig`], the `Serialize` form exists only for `graph_id`'s
/// canonical hash: snake_case keys (`root_uri`, `package_uri`), never the
/// camelCase package_config.json spelling — never write a config from it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PackageConfigEntry {
    pub name: String,
    /// The `rootUri` string as written: an absolute `file:` URI for
    /// pub-cache packages, or a relative URI path based at the config's
    /// directory (`"../"` for the root, `"../pkg_b"` for members).
    pub root_uri: String,
    /// Subdirectory inside the package root holding Dart sources; `None`
    /// when the entry was written without `packageUri`. The reader contract
    /// (`package_config_json.dart:151-155`) resolves `packageUri` onto the
    /// root and **absent `packageUri` means the package root itself is the
    /// source root** — never a `lib/` default. Pub's writer always emits
    /// `packageUri: "lib/"` (entrypoint.dart:511), so real pub-generated
    /// configs carry it; an absent `packageUri` is a hand-written or
    /// foreign-generator shape, and the oracle still reads it.
    pub package_uri: Option<String>,
    /// The package's language version (`^\d+\.\d+$`), pub's materialization
    /// of that package's own pubspec sdk lower bound.
    pub language_version: Option<LanguageVersion>,
    /// Unknown per-entry keys, preserved verbatim.
    pub additional_properties: BTreeMap<String, serde_json::Value>,
}

impl PackageConfig {
    /// Parses `package_config.json` text.
    ///
    /// # Errors
    ///
    /// Returns [`PubError::PackageConfig`] for invalid JSON, a
    /// `configVersion` other than `2`, malformed entries, or a repeated
    /// package name. Every one of these is a whole-config defect: the
    /// oracle's loader (`PackageConfig.parseString` under the catching
    /// `parsePackageConfigJsonFile`) throws on the first one and degrades
    /// to a silent `Packages.empty` — valid siblings do NOT survive
    /// (mixed-entry probes #2434: a bad name, a duplicate name, a missing
    /// `rootUri`, or a typed-null `packageUri`/`languageVersion` floor the
    /// good entries along with the bad one). Only entry-level content the
    /// oracle never validates — percent-escape spellings, `file:` URL
    /// authorities, the free-form `generator` fields — resolves entry by
    /// entry; those shapes are consumed, not rejected, here as well.
    pub fn parse(json: &str) -> Result<PackageConfig, PubError> {
        let invalid = |what: String| PubError::PackageConfig(what);
        let root: serde_json::Value =
            serde_json::from_str(json).map_err(|e| invalid(e.to_string()))?;
        let root = root
            .as_object()
            .ok_or_else(|| invalid("package_config.json must be a JSON object".into()))?;

        let config_version = root
            .get("configVersion")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| invalid("configVersion must be an integer".into()))?;
        if config_version != 2 {
            return Err(invalid(format!(
                "configVersion must be 2, got {config_version}"
            )));
        }

        let packages_raw = root
            .get("packages")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| invalid("packages must be a list".into()))?;
        let mut packages = Vec::with_capacity(packages_raw.len());
        let mut seen_names = BTreeSet::new();
        for entry in packages_raw {
            let entry = parse_entry(entry)?;
            // A repeated name floors the whole config: `_validatePackages`
            // reports the duplicate through `onError` and the analyzer's
            // default handler throws (the loop's `continue` never runs) —
            // the loader catch turns that into a silent `Packages.empty`
            // (mixed-entry probe #2434: two entries sharing a name, with
            // distinct resolvable roots, leave BOTH unresolved).
            if !seen_names.insert(entry.name.clone()) {
                return Err(invalid(format!("duplicate package name `{}`", entry.name)));
            }
            packages.push(entry);
        }

        // The generator fields are free-form: the oracle's parse reads only
        // `configVersion` and `packages` as schema, everything else rides
        // `extraData` unvalidated (probed #2434: `generator: 123` and
        // `generatorVersion: "garbage"` resolve fine). dartq keeps the
        // typed faces for well-formed values — the freshness face compares
        // `generatorVersion` against the running SDK — and reads any other
        // JSON shape as absent rather than rejecting the config.
        let generator = match root.get("generator") {
            Some(serde_json::Value::String(text)) => Some(text.clone()),
            _ => None,
        };
        let generator_version = match root.get("generatorVersion") {
            Some(serde_json::Value::String(text)) => Version::parse(text).ok(),
            _ => None,
        };

        let additional_properties = root
            .iter()
            .filter(|(key, _)| !RESERVED_KEYS.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();

        Ok(PackageConfig {
            packages,
            generator,
            generator_version,
            additional_properties,
        })
    }

    /// Looks an entry up by package name.
    ///
    /// Names are unique: [`Self::parse`] rejects a repeated name outright
    /// (the oracle floors such a config), so the lookup answers with the
    /// one effective entry.
    #[must_use]
    pub fn entry(&self, name: &str) -> Option<&PackageConfigEntry> {
        self.packages.iter().find(|entry| entry.name == name)
    }

    /// The `pubCache` additional property as a filesystem path — pub writes
    /// it as a `file:` URI. `None` when absent or not a decodable `file:`
    /// URI; freshness then treats every entry as outside the cache, i.e.
    /// mtime-checked (conservative).
    #[must_use]
    pub fn pub_cache_root(&self) -> Option<PathBuf> {
        let uri = self.additional_properties.get("pubCache")?.as_str()?;
        let rest = uri.strip_prefix("file://")?;
        if !rest.starts_with('/') {
            return None;
        }
        Some(PathBuf::from(percent_decode(rest)))
    }
}

impl PackageConfigEntry {
    /// True when `rootUri` is an absolute URI (any scheme): it starts with
    /// an RFC 3986 scheme (`ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`
    /// followed by `:`). This is only a *shape* property, used by
    /// [`Self::resolve_root`] to reject non-`file` schemes — pub writes
    /// pub-cache packages as absolute `file:` URIs, but so are absolute
    /// path dependencies (`relative: false`), which live outside the cache
    /// and must stay freshness-checked. Scheme detection is
    /// prefix-anchored, not a `://` search: a relative path that merely
    /// contains a literal `://` (`../weird://name`) stays relative.
    #[must_use]
    pub fn is_absolute_uri(&self) -> bool {
        let Some((scheme, _)) = self.root_uri.split_once(':') else {
            return false;
        };
        let mut chars = scheme.chars();
        matches!(chars.next(), Some(first) if first.is_ascii_alphabetic())
            && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    }

    /// Resolves `rootUri` against `config_dir`, the directory holding
    /// `package_config.json` (normally `.dart_tool/`). Works for absolute
    /// `file:` URIs — `file:///abs` (the only shape pub writes), the
    /// single-slash `file:/abs`, and `file://host/abs` whose authority the
    /// oracle's path face ignores (probed #2434: the entry resolves at
    /// `/cache/a`) — and relative URI paths, with or without a trailing
    /// slash (hosted cache dirs have none, git checkouts do).
    ///
    /// Percent escapes decode into the path text (`%20` → space, and a
    /// `%2F` → a real separator, both oracle-probed); a `%` with no two hex
    /// digits after it stays a literal `%` (the oracle's `Uri.parse`
    /// re-encodes the bare `%` as `%25` and the path face decodes that
    /// back, so `../x%zz` resolves the directory literally named `x%zz`).
    ///
    /// The result is only lexically joined; callers that need a canonical
    /// filesystem path should canonicalize it themselves.
    ///
    /// # Errors
    ///
    /// Returns [`PubError::PackageConfig`] for a non-`file` absolute URI
    /// scheme. The oracle crashes its own analysis server on that shape
    /// (`fileUriToNormalizedPath`: `Uri http://... must have scheme
    /// 'file:'` — probed #2434), an unmatchable failure; dartq's
    /// structured error keeps the corrupt-config silent floor instead.
    pub fn resolve_root(&self, config_dir: &Path) -> Result<PathBuf, PubError> {
        let uri = &self.root_uri;
        if let Some(rest) = uri.strip_prefix("file:") {
            if let Some(after_authority) = rest.strip_prefix("//") {
                // `file:///abs/path` and `file://host/abs/path` alike: the
                // path is everything from the first `/` — an authority is
                // not part of the filesystem path (the oracle's
                // `PosixStyle.pathFromUri` reads `uri.path` only). An
                // authority with no path (`file://host`) names the root.
                let path = after_authority
                    .find('/')
                    .map_or("/", |index| &after_authority[index..]);
                return Ok(PathBuf::from(percent_decode(path)));
            }
            // The single-slash absolute form `file:/abs` (the leading `/`
            // stays part of the path); a `file:relative` reference falls
            // through and resolves against the config directory like any
            // relative path.
            if rest.starts_with('/') {
                return Ok(PathBuf::from(percent_decode(rest)));
            }
            return Ok(config_dir.join(percent_decode(rest)));
        } else if self.is_absolute_uri() {
            return Err(PubError::PackageConfig(format!(
                "unsupported rootUri scheme `{uri}`"
            )));
        }
        Ok(config_dir.join(percent_decode(uri)))
    }
}

/// One config entry resolved to its normalized filesystem directories — the
/// single root/lib-dir face both package tables consume (dartq-package's
/// identity table, dartq-resolution's `package:` face), so the two never
/// grow diverging copies of the policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPackage {
    /// The entry's name (validated against `is_valid_config_name`).
    pub name: String,
    /// `rootUri` resolved against the config's directory and lexically
    /// normalized ([`crate::workspace::normalize`]).
    pub root: PathBuf,
    /// The `packageUri` directory joined onto [`Self::root`], normalized the
    /// same way, and validated to stay inside the root.
    pub lib_dir: PathBuf,
    /// The entry's declared language version, if any.
    pub language_version: Option<LanguageVersion>,
}

impl PackageConfig {
    /// Resolves every entry to its normalized, subtree-validated
    /// directories — the one policy per question, enforced once:
    ///
    /// - **Name grammar is the loose reader charset** — every
    ///   `_validPackageNameCharacters` character with at least one non-`.`
    ///   (`package_config-3.0.0` `util.dart:12-36`), so config-written names
    ///   like `1abc`/`-foo` are consumed as pub wrote them. Deliberately
    ///   looser than the pubspec layer's strict grammar
    ///   (`pubspec.rs`, `is_valid_package_name`): the config is read, not
    ///   re-published.
    /// - **`lib_dir` is normalized and subtree-validated** — a normalized
    ///   `packageUri` directory must stay inside its package root
    ///   (`package_config-3.0.0` `package_config.dart:838`); after
    ///   normalization a `../lib` spelling escapes and is rejected loudly
    ///   instead of silently mis-resolving.
    ///
    /// - **Names are already unique here** — [`Self::parse`] rejects a
    ///   repeated name outright (the oracle floors such a config), so no
    ///   duplicate reaches the resolution.
    ///
    /// `config_dir` must be absolute — the engine's assembly step passes the
    /// absolute `.dart_tool/` directory, and a relative one would silently
    /// produce wrong roots (build is one-shot; no later step could recover).
    ///
    /// # Errors
    ///
    /// [`PubError::PackageConfig`] for a relative `config_dir`, a name
    /// outside the reader grammar, an unresolvable `rootUri`, or a
    /// `packageUri` directory escaping its root.
    pub fn resolve(&self, config_dir: &Path) -> Result<Vec<ResolvedPackage>, PubError> {
        if !config_dir.is_absolute() {
            return Err(PubError::PackageConfig(format!(
                "config_dir must be absolute, got `{}`",
                config_dir.display()
            )));
        }
        let mut resolved = Vec::with_capacity(self.packages.len());
        for entry in &self.packages {
            if !is_valid_config_name(&entry.name) {
                return Err(PubError::PackageConfig(format!(
                    "invalid package name `{name}`: expected characters \
                     `!$&'()*+,-.0-9;=@A-Z_a-z~` with at least one non-dot character",
                    name = entry.name
                )));
            }
            let root = normalize(&entry.resolve_root(config_dir)?);
            let lib_dir = match &entry.package_uri {
                // packageUri absent: the package root itself is the source
                // root — the reader contract (package_config_json.dart:152
                // `packageRoot = root` when `packageUri == null`), not the
                // `lib/` shape pub's writer emits.
                None => root.clone(),
                Some(uri) => normalize(&root.join(uri)),
            };
            if lib_dir != root && lib_dir.strip_prefix(&root).is_err() {
                return Err(PubError::PackageConfig(format!(
                    "package `{name}`: the package URI root `{lib}` is not below \
                     the package root `{root}`",
                    name = entry.name,
                    lib = lib_dir.display(),
                    root = root.display(),
                )));
            }
            resolved.push(ResolvedPackage {
                name: entry.name.clone(),
                root,
                lib_dir,
                language_version: entry.language_version,
            });
        }
        Ok(resolved)
    }
}

/// The package_config reader's loose name grammar — every character in
/// the reader charset with at least one non-`.` character
/// (`package_config-3.0.0` `util.dart:12-36`, `_validPackageNameCharacters`
/// + `checkPackageName`).
fn is_valid_config_name(name: &str) -> bool {
    const CHARSET: &str =
        "!$&'()*+,-.;=@0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ_abcdefghijklmnopqrstuvwxyz~";
    let mut non_dot = false;
    for c in name.chars() {
        if !CHARSET.contains(c) {
            return false;
        }
        non_dot |= c != '.';
    }
    non_dot
}

fn parse_entry(entry: &serde_json::Value) -> Result<PackageConfigEntry, PubError> {
    let invalid = |what: String| PubError::PackageConfig(what);
    let object = entry
        .as_object()
        .ok_or_else(|| invalid("package entries must be objects".into()))?;
    let name = object
        .get("name")
        .and_then(serde_json::Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| invalid("package entry needs a non-empty `name`".into()))?;
    let root_uri = object
        .get("rootUri")
        .and_then(serde_json::Value::as_str)
        .filter(|uri| !uri.is_empty())
        .ok_or_else(|| invalid(format!("package `{name}` needs a non-empty `rootUri`")))?;

    // The read side normalizes a written `packageUri` with a trailing
    // slash; absent `packageUri` stays `None` so `resolve` can anchor the
    // source root at `rootUri` itself — the oracle's reader contract
    // (package_config_json.dart:151-155), not the `lib/` default pub's
    // writer emits. A typed null is NOT absent: the oracle's `checkType`
    // rejects a JSON `null` where a string is due and the loader floors
    // (probed #2434), so both fields error here.
    let package_uri = match object.get("packageUri") {
        None => None,
        Some(serde_json::Value::Null) => {
            return Err(invalid("packageUri must be a string, if given".into()));
        }
        Some(value) => {
            let text = value
                .as_str()
                .ok_or_else(|| invalid("packageUri must be a string".into()))?;
            if text.ends_with('/') {
                Some(text.to_owned())
            } else {
                Some(format!("{text}/"))
            }
        }
    };

    let language_version = match object.get("languageVersion") {
        None => None,
        Some(serde_json::Value::Null) => {
            return Err(invalid(format!(
                "package `{name}` languageVersion must be a string"
            )));
        }
        Some(value) => {
            let text = value.as_str().ok_or_else(|| {
                invalid(format!("package `{name}` languageVersion must be a string"))
            })?;
            Some(LanguageVersion::parse(text).map_err(|e| {
                PubError::PackageConfig(format!("package `{name}` languageVersion: {e}"))
            })?)
        }
    };

    let additional_properties = object
        .iter()
        .filter(|(key, _)| {
            !matches!(
                key.as_str(),
                "name" | "rootUri" | "packageUri" | "languageVersion"
            )
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();

    Ok(PackageConfigEntry {
        name: name.to_owned(),
        root_uri: root_uri.to_owned(),
        package_uri,
        language_version,
        additional_properties,
    })
}

/// Decodes `%XX` escapes into the path text they name (URIs carry
/// percent-encoded paths, e.g. spaces as `%20`). A `%` with no two hex
/// digits after it stays a literal `%` — the oracle's `Uri.parse` re-encodes
/// a bare `%` as `%25` and its path face decodes that back, so the net
/// meaning of `x%zz` is the literal directory name `x%zz` (probed #2434).
/// Bytes that do not form UTF-8 become `U+FFFD`, the way Dart's
/// `decodeComponent` yields a String for any input.
fn percent_decode(text: &str) -> String {
    if !text.contains('%') {
        return text.to_owned();
    }
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let (Some(&high), Some(&low)) = (bytes.get(index + 1), bytes.get(index + 2))
            && let (Some(high), Some(low)) = (hex_nibble(high), hex_nibble(low))
        {
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "configVersion": 2,
      "packages": [
        {
          "name": "path",
          "rootUri": "file:///tmp/pubfacts/pcache/hosted/pub.dev/path-1.9.1",
          "packageUri": "lib/",
          "languageVersion": "3.4"
        },
        {
          "name": "mini_a",
          "rootUri": "../",
          "packageUri": "lib/",
          "languageVersion": "3.5"
        },
        {
          "name": "git_dep",
          "rootUri": "../../git/gitdep-9482c6eb905b42f4ee1b08626d56a17366135764/",
          "languageVersion": "3.4"
        }
      ],
      "generator": "pub",
      "generatorVersion": "3.13.1",
      "flutterRoot": "file:///home/x/fvm/versions/3.47.1",
      "flutterVersion": "3.47.1",
      "pubCache": "file:///tmp/pubfacts/pcache",
      "generated": "2026-10-08T00:00:00.000Z"
    }"#;

    #[test]
    fn parses_full_fixture() {
        let config = PackageConfig::parse(FIXTURE).expect("valid config");
        assert_eq!(config.packages.len(), 3);
        assert_eq!(config.generator.as_deref(), Some("pub"));
        assert_eq!(
            config
                .generator_version
                .as_ref()
                .expect("version")
                .to_string(),
            "3.13.1"
        );
        // Unknown/env keys preserved, reserved keys excluded.
        assert_eq!(config.additional_properties.len(), 3);
        assert_eq!(
            config.additional_properties["flutterVersion"].as_str(),
            Some("3.47.1")
        );
        // Entries keep file order.
        assert_eq!(config.packages[0].name, "path");
        assert_eq!(config.packages[1].name, "mini_a");
        // languageVersion parsed into major/minor.
        assert_eq!(
            config.packages[0].language_version,
            Some(LanguageVersion::new(3, 4))
        );
        assert!(config.entry("mini_a").is_some());
        assert!(config.entry("nope").is_none());
    }

    #[test]
    fn rejects_wrong_or_missing_config_version() {
        let v1 = FIXTURE.replace("\"configVersion\": 2", "\"configVersion\": 1");
        assert!(matches!(
            PackageConfig::parse(&v1),
            Err(PubError::PackageConfig(_))
        ));
        let missing = r#"{"packages": []}"#;
        assert!(PackageConfig::parse(missing).is_err());
        assert!(PackageConfig::parse("not json").is_err());
    }

    #[test]
    fn rejects_malformed_entries() {
        let no_name = r#"{"configVersion": 2, "packages": [{"rootUri": "../"}]}"#;
        assert!(PackageConfig::parse(no_name).is_err());
        let bad_lv = r#"{"configVersion": 2, "packages": [
            {"name": "a", "rootUri": "../", "languageVersion": "3"}]}"#;
        assert!(PackageConfig::parse(bad_lv).is_err());
        let no_uri = r#"{"configVersion": 2, "packages": [{"name": "a"}]}"#;
        assert!(PackageConfig::parse(no_uri).is_err());
        // A typed null is not absence: the oracle's `checkType` rejects a
        // JSON null where a string is due and the loader floors the whole
        // config (mixed-entry probe #2434 — the good sibling dies too).
        let null_uri = r#"{"configVersion": 2, "packages": [{"name": "a", "rootUri": null}]}"#;
        assert!(PackageConfig::parse(null_uri).is_err());
        let null_package_uri = r#"{"configVersion": 2, "packages": [
            {"name": "a", "rootUri": "../", "packageUri": null}]}"#;
        assert!(PackageConfig::parse(null_package_uri).is_err());
        let null_language_version = r#"{"configVersion": 2, "packages": [
            {"name": "a", "rootUri": "../", "languageVersion": null}]}"#;
        assert!(PackageConfig::parse(null_language_version).is_err());
    }

    /// A repeated name floors the whole config: `_validatePackages`
    /// reports the duplicate through `onError` and the analyzer's default
    /// handler throws — the loader catch degrades to a silent
    /// `Packages.empty`. Probed #2434 with distinct resolvable roots
    /// (first-wins folding would have resolved one of them): neither
    /// resolves.
    #[test]
    fn duplicate_names_floor_the_config() {
        let duplicated = r#"{"configVersion": 2, "packages": [
            {"name": "app", "rootUri": "../"},
            {"name": "app", "rootUri": "../other"}]}"#;
        let error = PackageConfig::parse(duplicated).expect_err("duplicate name");
        assert!(
            error.to_string().contains("duplicate package name `app`"),
            "unexpected error: {error}"
        );
    }

    /// The generator fields are free-form extraData for the oracle (probed
    /// #2434: `generator: 123` and `generatorVersion: "garbage"` resolve
    /// fine — its parse validates only `configVersion` and `packages`), so
    /// any JSON shape is consumed: well-formed values keep the typed faces
    /// (the freshness comparison), anything else reads as absent.
    #[test]
    fn generator_fields_tolerate_any_json() {
        let garbage = r#"{"configVersion": 2, "generator": 123, "generatorVersion": "garbage",
            "packages": [{"name": "a", "rootUri": "../"}]}"#;
        let config = PackageConfig::parse(garbage).expect("tolerated");
        assert_eq!(config.generator, None);
        assert_eq!(config.generator_version, None);
        let well_formed = r#"{"configVersion": 2, "generator": "pub", "generatorVersion": "3.13.1",
            "packages": []}"#;
        let config = PackageConfig::parse(well_formed).expect("typed");
        assert_eq!(config.generator.as_deref(), Some("pub"));
        assert_eq!(
            config
                .generator_version
                .as_ref()
                .expect("version")
                .to_string(),
            "3.13.1"
        );
    }

    #[test]
    fn package_uri_normalized() {
        let with_slash = r#"{"configVersion": 2, "packages": [
            {"name": "a", "rootUri": "../", "packageUri": "lib/"}]}"#;
        let config = PackageConfig::parse(with_slash).expect("ok");
        assert_eq!(config.packages[0].package_uri.as_deref(), Some("lib/"));
        let without_slash = with_slash.replace("lib/", "src");
        let config = PackageConfig::parse(&without_slash).expect("ok");
        assert_eq!(config.packages[0].package_uri.as_deref(), Some("src/"));
        // Absent stays `None` — the reader's `packageRoot = root` face,
        // never a `lib/` default.
        let absent = r#"{"configVersion": 2, "packages": [{"name": "a", "rootUri": "../"}]}"#;
        let config = PackageConfig::parse(absent).expect("ok");
        assert_eq!(config.packages[0].package_uri, None);
    }

    #[test]
    fn pub_cache_root_decodes_to_a_path() {
        let config = PackageConfig::parse(FIXTURE).expect("ok");
        assert_eq!(
            config.pub_cache_root(),
            Some(PathBuf::from("/tmp/pubfacts/pcache"))
        );
        // Absent, or a plain path instead of a file URI: not decodable.
        let absent = r#"{"configVersion": 2, "packages": []}"#;
        assert_eq!(
            PackageConfig::parse(absent).expect("ok").pub_cache_root(),
            None
        );
        let plain = FIXTURE.replace(
            "\"pubCache\": \"file:///tmp/pubfacts/pcache\"",
            "\"pubCache\": \"/tmp/pubfacts/pcache\"",
        );
        assert_eq!(
            PackageConfig::parse(&plain).expect("ok").pub_cache_root(),
            None
        );
    }

    #[test]
    fn resolve_root_handles_relative_and_file_uris() {
        let config = PackageConfig::parse(FIXTURE).expect("ok");
        let config_dir = Path::new("/proj/.dart_tool");
        // Relative base is the config directory, not the package root.
        assert_eq!(
            config
                .entry("mini_a")
                .expect("entry")
                .resolve_root(config_dir)
                .expect("ok"),
            PathBuf::from("/proj/.dart_tool/..")
        );
        // Git-style trailing slash, hosted-style without: both resolve.
        let git = config
            .entry("git_dep")
            .expect("entry")
            .resolve_root(config_dir)
            .expect("ok");
        assert!(git.ends_with("gitdep-9482c6eb905b42f4ee1b08626d56a17366135764"));
        let hosted = config
            .entry("path")
            .expect("entry")
            .resolve_root(config_dir)
            .expect("ok");
        assert!(hosted.starts_with("/tmp/pubfacts/pcache/"));
    }

    #[test]
    fn resolve_root_percent_decodes() {
        let odd = r#"{"configVersion": 2, "packages": [
            {"name": "a", "rootUri": "../my%20pkg"}]}"#;
        let config = PackageConfig::parse(odd).expect("ok");
        // Lexically joined; normalization is the caller's job.
        assert_eq!(
            config.packages[0]
                .resolve_root(Path::new("/p/.dart_tool"))
                .expect("ok"),
            PathBuf::from("/p/.dart_tool/../my pkg")
        );
        assert!(!config.packages[0].is_absolute_uri());
        let hosted = r#"{"configVersion": 2, "packages": [
            {"name": "a", "rootUri": "file:///cache/a-1.0.0"}]}"#;
        let config = PackageConfig::parse(hosted).expect("ok");
        assert!(config.packages[0].is_absolute_uri());
        // file:/// yields an absolute path; a non-empty authority is
        // skipped — the oracle's path face reads `uri.path` only (probed
        // #2434: `file://host/cache/a` resolves at `/cache/a`).
        assert_eq!(
            config.packages[0]
                .resolve_root(Path::new("/anywhere"))
                .expect("ok"),
            PathBuf::from("/cache/a-1.0.0")
        );
        let bad_host = r#"{"configVersion": 2, "packages": [
            {"name": "a", "rootUri": "file://host/cache/a-1.0.0"}]}"#;
        let config = PackageConfig::parse(bad_host).expect("ok");
        assert_eq!(
            config.packages[0]
                .resolve_root(Path::new("/p"))
                .expect("the authority is ignored"),
            PathBuf::from("/cache/a-1.0.0")
        );
        // An authority with no path names the filesystem root.
        let bare_host = r#"{"configVersion": 2, "packages": [
            {"name": "a", "rootUri": "file://host"}]}"#;
        let config = PackageConfig::parse(bare_host).expect("ok");
        assert_eq!(
            config.packages[0]
                .resolve_root(Path::new("/p"))
                .expect("ok"),
            PathBuf::from("/")
        );
    }

    /// A `%` with no two hex digits after it is NOT a defect: the oracle's
    /// `Uri.parse` re-encodes the bare `%` as `%25` and its path face
    /// decodes that back, so `../x%zz` resolves the directory literally
    /// named `x%zz` (probed #2434 — the entry survives, valid siblings
    /// resolve, and a real `x%zz` directory on disk matches). A `%2F`
    /// decodes into a real path separator, like any escape. Bytes that do
    /// not form UTF-8 become `U+FFFD`, `decodeComponent`'s face.
    #[test]
    fn invalid_escapes_resolve_as_literal_text() {
        let escaped = r#"{"configVersion": 2, "packages": [
            {"name": "esc", "rootUri": "../x%zz"}]}"#;
        let config = PackageConfig::parse(escaped).expect("the escape is not a defect");
        assert_eq!(
            config.packages[0]
                .resolve_root(Path::new("/p/.dart_tool"))
                .expect("ok"),
            PathBuf::from("/p/.dart_tool/../x%zz")
        );
        // Truncated escape: same literal face.
        let truncated = r#"{"configVersion": 2, "packages": [
            {"name": "esc", "rootUri": "../x%2"}]}"#;
        let config = PackageConfig::parse(truncated).expect("ok");
        assert_eq!(
            config.packages[0]
                .resolve_root(Path::new("/p/.dart_tool"))
                .expect("ok"),
            PathBuf::from("/p/.dart_tool/../x%2")
        );
        // Valid escapes keep decoding, `%2F` included — a real separator
        // (oracle-probed: the entry resolves into the split directory).
        let slash = r#"{"configVersion": 2, "packages": [
            {"name": "esc", "rootUri": "../x%2Fsub"}]}"#;
        let config = PackageConfig::parse(slash).expect("ok");
        assert_eq!(
            config.packages[0]
                .resolve_root(Path::new("/p/.dart_tool"))
                .expect("ok"),
            PathBuf::from("/p/.dart_tool/../x/sub")
        );
        // Invalid UTF-8 escapes degrade to `U+FFFD`, never an error.
        let binary = r#"{"configVersion": 2, "packages": [
            {"name": "esc", "rootUri": "../x%FF"}]}"#;
        let config = PackageConfig::parse(binary).expect("ok");
        assert_eq!(
            config.packages[0]
                .resolve_root(Path::new("/p/.dart_tool"))
                .expect("ok"),
            PathBuf::from("/p/.dart_tool/../x\u{fffd}")
        );
    }

    #[test]
    fn resolve_root_accepts_single_slash_file_uris() {
        // `file:/abs` — the single-slash form Dart's `Uri.parse` +
        // `toFilePath` accept as `/abs` — resolves absolute, never as a
        // config-relative join; the authority-ful empty-authority form
        // keeps resolving, and a real authority stays rejected.
        let single = r#"{"configVersion": 2, "packages": [
            {"name": "a", "rootUri": "file:/cache/a-1.0.0"}]}"#;
        let config = PackageConfig::parse(single).expect("ok");
        assert!(config.packages[0].is_absolute_uri());
        assert_eq!(
            config.packages[0]
                .resolve_root(Path::new("/p/.dart_tool"))
                .expect("ok"),
            PathBuf::from("/cache/a-1.0.0")
        );
        let hosted = r#"{"configVersion": 2, "packages": [
            {"name": "a", "rootUri": "file:///cache/a-1.0.0"}]}"#;
        let config = PackageConfig::parse(hosted).expect("ok");
        assert_eq!(
            config.packages[0]
                .resolve_root(Path::new("/p/.dart_tool"))
                .expect("ok"),
            PathBuf::from("/cache/a-1.0.0")
        );
        let bad_host = r#"{"configVersion": 2, "packages": [
            {"name": "a", "rootUri": "file://host/cache/a-1.0.0"}]}"#;
        let config = PackageConfig::parse(bad_host).expect("ok");
        assert_eq!(
            config.packages[0]
                .resolve_root(Path::new("/p"))
                .expect("the authority is ignored"),
            PathBuf::from("/cache/a-1.0.0")
        );
    }

    #[test]
    fn scheme_detection_is_prefix_anchored() {
        // A relative path that merely contains a literal `://` stays
        // relative (the scheme grammar is anchored at the start,
        // RFC 3986 §3.1), while a real non-file scheme is still rejected.
        let tricky = r#"{"configVersion": 2, "packages": [
            {"name": "a", "rootUri": "../weird://name"}]}"#;
        let config = PackageConfig::parse(tricky).expect("ok");
        assert!(!config.packages[0].is_absolute_uri());
        assert_eq!(
            config.packages[0]
                .resolve_root(Path::new("/p/.dart_tool"))
                .expect("ok"),
            PathBuf::from("/p/.dart_tool/../weird://name")
        );
        let http = r#"{"configVersion": 2, "packages": [
            {"name": "a", "rootUri": "http://cache/a-1.0.0"}]}"#;
        let config = PackageConfig::parse(http).expect("ok");
        assert!(config.packages[0].resolve_root(Path::new("/p")).is_err());
    }

    fn resolve_ok(json: &str) -> Vec<ResolvedPackage> {
        PackageConfig::parse(json)
            .expect("valid config")
            .resolve(Path::new("/proj/.dart_tool"))
            .expect("resolves")
    }

    #[test]
    fn resolve_normalizes_root_and_lib_dirs() {
        let resolved = resolve_ok(
            r#"{
              "configVersion": 2,
              "packages": [
                {"name": "app", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.5"},
                {"name": "dep", "rootUri": "../deps/../deps/dep", "packageUri": "src/"}
              ],
              "generator": "pub"
            }"#,
        );
        assert_eq!(resolved[0].root, PathBuf::from("/proj"));
        assert_eq!(resolved[0].lib_dir, PathBuf::from("/proj/lib"));
        assert_eq!(
            resolved[0].language_version,
            Some(LanguageVersion::new(3, 5))
        );
        // Dot segments in rootUri and the trailing slash in packageUri
        // collapse lexically.
        assert_eq!(resolved[1].root, PathBuf::from("/proj/deps/dep"));
        assert_eq!(resolved[1].lib_dir, PathBuf::from("/proj/deps/dep/src"));
    }

    /// `packageUri` absent: the reader anchors `packageUriRoot` at the
    /// `rootUri` itself (package_config_json.dart:152 — `packageRoot = root`
    /// when `packageUri == null`), so `lib_dir == root`. Pub's writer always
    /// emits `packageUri: "lib/"` (entrypoint.dart:511), so this shape only
    /// arises from hand-written or foreign-generated configs, but the
    /// analyzer still reads it — and `package:meta/meta.dart` under such a
    /// config must look under the package root, not under `lib/`.
    #[test]
    fn resolve_anchors_lib_dir_at_root_when_package_uri_absent() {
        let resolved = resolve_ok(
            r#"{
              "configVersion": 2,
              "packages": [
                {"name": "app", "rootUri": "../"},
                {"name": "dep", "rootUri": "../deps/dep", "packageUri": "src/"}
              ],
              "generator": "pub"
            }"#,
        );
        assert_eq!(resolved[0].root, PathBuf::from("/proj"));
        assert_eq!(resolved[0].lib_dir, PathBuf::from("/proj"));
        assert_eq!(resolved[1].root, PathBuf::from("/proj/deps/dep"));
        assert_eq!(resolved[1].lib_dir, PathBuf::from("/proj/deps/dep/src"));
    }

    #[test]
    fn parse_rejects_malformed_json() {
        // Malformed JSON throws at this layer — the oracle's
        // package_config-layer throw (`PackageConfig.parseString`); the
        // analyzer's catch (upstream Dart SDK source,
        // pkg/analyzer/lib/src/context/packages.dart
        // `parsePackageConfigJsonFile` — the read and the parse share one
        // catch) is the ASSEMBLY's silent `Packages.empty` degrade, so the
        // reader stays strict and the zero-row floor lives in the corrupt
        // floor posture (#2427).
        let malformed = [
            // Bare word.
            "not json",
            // Truncated JSON.
            r#"{"configVersion": 2, "packages": [{"name": "x""#,
            // Trailing comma.
            r#"{"configVersion": 2, "packages": [],}"#,
            // Unquoted key.
            r#"{configVersion: 2, "packages": []}"#,
            // Empty file.
            "",
        ];
        for text in malformed {
            assert!(
                PackageConfig::parse(text).is_err(),
                "malformed JSON must not parse: {text:?}"
            );
        }
    }

    #[test]
    fn resolve_rejects_names_outside_the_reader_grammar() {
        // A space is outside even the reader's loose charset
        // (package_config-3.0.0 util.dart:12-36).
        let invalid = r#"{"configVersion": 2, "packages": [
            {"name": "foo bar", "rootUri": "../"}]}"#;
        let error = PackageConfig::parse(invalid)
            .expect("parses fine at the config layer")
            .resolve(Path::new("/proj/.dart_tool"))
            .expect_err("invalid name");
        assert!(
            error.to_string().contains("invalid package name `foo bar`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn resolve_rejects_package_uri_outside_root() {
        // A `../lib` packageUri escapes the package root after
        // normalization (package_config-3.0.0 package_config.dart:838).
        let escaping = r#"{"configVersion": 2, "packages": [
            {"name": "app", "rootUri": "../", "packageUri": "../lib"}]}"#;
        let error = PackageConfig::parse(escaping)
            .expect("parses fine at the config layer")
            .resolve(Path::new("/proj/.dart_tool"))
            .expect_err("escaping packageUri");
        let text = error.to_string();
        assert!(text.contains("not below the package root"), "{text}");
        assert!(text.contains("`/lib`"), "{text}");
        assert!(text.contains("`/proj`"), "{text}");
    }

    #[test]
    fn resolve_rejects_relative_config_dir() {
        let config = PackageConfig::parse(
            r#"{"configVersion": 2, "packages": [{"name": "app", "rootUri": "../"}]}"#,
        )
        .expect("valid config");
        let error = config
            .resolve(Path::new(".dart_tool"))
            .expect_err("relative config dir");
        assert!(
            error
                .to_string()
                .contains("config_dir must be absolute, got `.dart_tool`"),
            "unexpected error: {error}"
        );
    }
}
