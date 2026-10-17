//! Generates the feature registry from the vendored
//! `data/experimental_features.yaml`.
//!
//! The yaml is parsed at build time and turned into `feature_registry.rs` in
//! [`OUT_DIR`], which `src/lib.rs` includes as its `generated` module. Entry
//! order in the generated tables follows yaml appearance order (bit index in
//! [`FeatureSet`](crate::FeatureSet) therefore depends on that order).

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

const DATA_FILE: &str = "data/experimental_features.yaml";
/// The generated bit set is a single `u128`.
const MAX_FEATURES: usize = 128;

#[derive(Deserialize)]
struct FeaturesFile {
    #[serde(rename = "current-version")]
    current_version: String,
    features: serde_yaml::Mapping,
}

/// One `features:` entry. Field names in the yaml are camelCase
/// (`enabledIn`, `experimentalReleaseVersion`). Unknown fields (`validation`,
/// `category`, `channels`, ...) are deliberately tolerated so newer SDK
/// snapshots keep building; they are simply not represented in the registry.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeatureDef {
    help: String,
    enabled_in: Option<String>,
    experimental_release_version: Option<String>,
    expired: Option<bool>,
}

fn main() {
    println!("cargo:rerun-if-changed={DATA_FILE}");
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo"));
    let raw = fs::read_to_string(manifest_dir.join(DATA_FILE))
        .unwrap_or_else(|e| panic!("failed to read vendored {DATA_FILE}: {e}"));
    let file: FeaturesFile = serde_yaml::from_str(&raw)
        .unwrap_or_else(|e| panic!("failed to parse vendored {DATA_FILE}: {e}"));

    let entries: Vec<(String, FeatureDef)> = file
        .features
        .into_iter()
        .map(|(key, value)| {
            let name = key
                .as_str()
                .unwrap_or_else(|| panic!("{DATA_FILE}: feature key is not a string"))
                .to_owned();
            let def: FeatureDef = serde_yaml::from_value(value)
                .unwrap_or_else(|e| panic!("{DATA_FILE}: invalid entry `{name}`: {e}"));
            validate_name(&name);
            (name, def)
        })
        .collect();
    assert!(
        !entries.is_empty() && entries.len() <= MAX_FEATURES,
        "{DATA_FILE}: expected 1..={MAX_FEATURES} feature entries, got {}",
        entries.len()
    );
    // The SDK never ships an expired flag without `enabledIn` (expiry means
    // "warn and ignore the flag", which presupposes version-based enablement
    // via a release version). Enforce it here so a re-vendored yaml that
    // breaks the invariant fails the build instead of silently invalidating
    // the "every expired entry has enabledIn" contract documented in
    // src/lib.rs.
    for (name, def) in &entries {
        assert!(
            !def.expired.unwrap_or(false) || def.enabled_in.is_some(),
            "{DATA_FILE}: expired feature `{name}` must also declare `enabledIn`"
        );
    }
    // Distinct kebab-case names can pascal-case to the same Rust variant
    // (`a-1` and `a1` both become `A1`), which would surface as a confusing
    // duplicate-variant compile error in the generated code. Enforce the
    // uniqueness invariant here so a re-vendored yaml that breaks it fails
    // the build with both offenders named instead.
    let mut variants: Vec<(String, &str)> = Vec::with_capacity(entries.len());
    for (name, _) in &entries {
        let variant = pascal_case(name);
        if let Some((_, first)) = variants.iter().find(|(v, _)| *v == variant) {
            panic!(
                "{DATA_FILE}: feature names `{first}` and `{name}` both pascal-case to `{variant}`"
            );
        }
        variants.push((variant, name.as_str()));
    }

    let default_version = parse_language_version(&file.current_version, "current-version");
    // The vendored data horizon: current-version raised to the newest
    // shipped entry. Feature-enablement comparisons clamp against it (see
    // `FeatureSet::for_language_version` in src/lib.rs) so a declared
    // version past the snapshot can only decide at the horizon.
    let newest_known = entries.iter().fold(default_version, |acc, (_, def)| {
        match def
            .enabled_in
            .as_deref()
            .map(|v| parse_language_version(v, "enabledIn"))
        {
            Some(v) if v > acc => v,
            _ => acc,
        }
    });
    let mut out = String::with_capacity(24 * 1024);
    write_prelude(&mut out, default_version, newest_known, entries.len());
    write_enum(&mut out, &entries);
    write_meta_table(&mut out, &entries);
    write_try_from(&mut out, &entries);

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    fs::write(out_dir.join("feature_registry.rs"), out)
        .expect("failed to write feature_registry.rs");
}

fn write_prelude(
    out: &mut String,
    default_version: (u32, u32),
    newest_known: (u32, u32),
    count: usize,
) {
    let _ = writeln!(
        out,
        "// @generated by build.rs from {DATA_FILE}. Do not edit;\
         \n// re-vendor the yaml and rebuild instead.\
         \n//\
         \n// Help prose is copied verbatim from the vendored yaml (rustdoc\
         \n// metacharacters escaped); `clippy::doc_markdown`\
         \n// is allowed on the enclosing module in src/lib.rs.\
         \n\
         \nuse crate::{{LanguageVersion, UnknownFeature}};\
         \n\
         \n/// Number of registry entries (bit index order = yaml appearance order).\
         \npub const FEATURE_COUNT: usize = {count};\
         \n\
         \n/// The SDK's current language version (`current-version` in the vendored yaml),\
         \n/// i.e. the version assumed for libraries that do not override it.\
         \npub const DEFAULT_LANGUAGE_VERSION: LanguageVersion = \
         LanguageVersion {{ major: {}, minor: {} }};\
         \n\
         \n/// The newest language version the vendored feature data knows about\
         \n/// (`max(current-version, max enabledIn)`): the horizon version-derived\
         \n/// feature enablement clamps to — see `FeatureSet::for_language_version`.\
         \npub const NEWEST_KNOWN_VERSION: LanguageVersion = \
         LanguageVersion {{ major: {}, minor: {} }};\
         \n\
         \n/// Static metadata for one registry entry.\
         \n#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]\
         \npub struct FeatureMeta {{\
         \n    /// Entry name in the yaml (also the `--enable-experiment=` flag string).\
         \n    pub name: &'static str,\
         \n    /// Flag string accepted on the command line.\
         \n    pub flag: &'static str,\
         \n    /// Shipping version from the yaml `enabledIn`; `None` means the feature\
         \n    /// is disabled by default and only enabled via the experiment flag.\
         \n    pub enabled_in: Option<LanguageVersion>,\
         \n    /// Version from which the experiment flag may enable the feature\
         \n    /// (yaml `experimentalReleaseVersion`).\
         \n    pub experimental_release_version: Option<LanguageVersion>,\
         \n    /// The flag is retired/rejected: tools keep running but only warn about\
         \n    /// it; it never changes version-based enablement.\
         \n    pub expired: bool,\
         \n    /// First sentence of the yaml `help` text.\
         \n    pub help: &'static str,\
         \n}}",
        default_version.0, default_version.1, newest_known.0, newest_known.1
    );
}

fn write_enum(out: &mut String, entries: &[(String, FeatureDef)]) {
    let _ = writeln!(
        out,
        "\
         \n\
         \n/// A Dart language feature governed by an experiment flag.\
         \n///\
         \n/// Variant order (and therefore the discriminant, which indexes\
         \n/// [`FEATURES`]) follows the vendored yaml appearance order.\
         \n#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]\
         \npub enum Feature {{"
    );
    for (name, def) in entries {
        let _ = writeln!(
            out,
            "    /// {}\
             \n    ///\
             \n    /// Flag: `{name}`.\
             \n    {},",
            escape_doc(first_sentence(&def.help)).replace('\n', " "),
            pascal_case(name)
        );
    }
    let _ = writeln!(out, "}}");
}

fn write_meta_table(out: &mut String, entries: &[(String, FeatureDef)]) {
    let _ = writeln!(
        out,
        "\
         \n\
         \nimpl Feature {{\
         \n    /// All registry entries, in yaml appearance order.\
         \n    pub const ALL: [Feature; FEATURE_COUNT] = ["
    );
    for (name, _) in entries {
        let _ = writeln!(out, "        Feature::{},", pascal_case(name));
    }
    let _ = writeln!(
        out,
        "    ];\
         \n\
         \n    /// Static metadata for this entry.\
         \n    #[must_use]\
         \n    pub fn meta(self) -> &'static FeatureMeta {{\
         \n        &FEATURES[self as usize]\
         \n    }}\
         \n}}\
         \n\
         \n/// Registry metadata, index-aligned with [`Feature`] discriminants.\
         \npub const FEATURES: [FeatureMeta; FEATURE_COUNT] = ["
    );
    for (name, def) in entries {
        let _ = writeln!(
            out,
            "    FeatureMeta {{\
             \n        name: {name:?},\
             \n        flag: {name:?},\
             \n        enabled_in: {},\
             \n        experimental_release_version: {},\
             \n        expired: {},\
             \n        help: {:?},\
             \n    }},",
            version_literal(def.enabled_in.as_deref()),
            version_literal(def.experimental_release_version.as_deref()),
            def.expired.unwrap_or(false),
            first_sentence(&def.help),
        );
    }
    let _ = writeln!(out, "];");
}

fn write_try_from(out: &mut String, entries: &[(String, FeatureDef)]) {
    let _ = writeln!(
        out,
        "\
         \n\
         \nimpl TryFrom<&str> for Feature {{\
         \n    type Error = UnknownFeature;\
         \n\
         \n    fn try_from(s: &str) -> Result<Self, Self::Error> {{\
         \n        match s {{"
    );
    for (name, _) in entries {
        let _ = writeln!(
            out,
            "            {name:?} => Ok(Self::{}),",
            pascal_case(name)
        );
    }
    let _ = writeln!(
        out,
        "            _ => Err(UnknownFeature(s.to_owned())),\
         \n        }}\
         \n    }}\
         \n}}"
    );
}

fn validate_name(name: &str) {
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && name.starts_with(|c: char| c.is_ascii_lowercase());
    assert!(
        valid,
        "{DATA_FILE}: feature name `{name}` is not kebab-case"
    );
}

/// `Some("3.10.0")` -> `Some(LanguageVersion { major: 3, minor: 10 })`.
fn version_literal(version: Option<&str>) -> String {
    match version {
        None => "None".to_owned(),
        Some(v) => {
            let (major, minor) = parse_language_version(v, "a version field");
            format!("Some(LanguageVersion {{ major: {major}, minor: {minor} }})")
        }
    }
}

/// Parses `major.minor` or `major.minor.patch` (the patch part is ignored;
/// language versions are `major.minor` only). Aligned with the runtime
/// `FromStr for LanguageVersion` (src/lib.rs): exactly 2 or 3 dot-separated
/// parts, each a bare run of ASCII digits — no sign, no whitespace — so a
/// 4-part (or otherwise malformed) yaml field fails the build instead of
/// being silently truncated, and a `+3.15` (which `u32::from_str` would
/// accept) fails the build instead of silently parsing.
fn parse_language_version(input: &str, field: &str) -> (u32, u32) {
    let mut parts = input.split('.');
    let parse = |s: Option<&str>| {
        s.and_then(parse_digits)
            .unwrap_or_else(|| panic!("{DATA_FILE}: {field} `{input}` is not <major>.<minor>"))
    };
    let major = parse(parts.next());
    let minor = parse(parts.next());
    match parts.next() {
        None => {}
        Some(patch) if parse_digits(patch).is_some() && parts.next().is_none() => {}
        Some(_) => panic!("{DATA_FILE}: {field} `{input}` is not <major>.<minor>[.<patch>]"),
    }
    (major, minor)
}

/// Parses a version-number part: a non-empty run of ASCII digits, nothing
/// else — the same bare-digits rule as the runtime `parse_digits` in
/// src/lib.rs; `u32::from_str` alone would also accept a leading `+`.
fn parse_digits(s: &str) -> Option<u32> {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        s.parse().ok()
    } else {
        None
    }
}

/// First sentence of a `help` string: text up to the first `". "`, inclusive.
fn first_sentence(help: &str) -> &str {
    let help = help.trim();
    match help.find(". ") {
        Some(pos) => &help[..=pos],
        None => help,
    }
}

/// Escapes rustdoc metacharacters so verbatim yaml help prose (which may
/// spell `<angle>` tag names or `[bracket]` references) renders as plain
/// text in the generated doc comments instead of parsing as HTML or links.
fn escape_doc(text: &str) -> String {
    text.replace('<', "\\<").replace('[', "\\[")
}

/// `dot-shorthands` -> `DotShorthands`, `inference-update-4` -> `InferenceUpdate4`.
fn pascal_case(kebab: &str) -> String {
    let mut out = String::with_capacity(kebab.len());
    for part in kebab.split('-') {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}
