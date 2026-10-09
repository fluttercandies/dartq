//! Semantic versions, `pub_semver` constraint syntax, and language versions.
//!
//! This is the exact subset of `pub_semver` 2.x grammar that pub itself
//! accepts (`.third/pub` + `pub_semver-2.2.0`): `any`, bare versions,
//! caret `^` (which must occupy the whole string), and space-separated
//! `<`/`>`/`<=`/`>=` comparisons that intersect. There is **no** `~`/`~>`
//! syntax and no comma lists — pub's `startComparison` is `^[<>]=?` only.

use std::cmp::Ordering;
use std::fmt;

use dartq_language::DEFAULT_LANGUAGE_VERSION;

use crate::PubError;

/// A semantic version: `major.minor.patch[-pre][+build]`.
///
/// Ordering follows pub_semver: numeric fields first, then a version with
/// a pre-release sorts *before* one without, and build metadata sorts
/// *after* no build — so [`Ord`] and [`PartialEq`] (which includes build,
/// as pub's `==` does) agree.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// Dot-separated pre-release identifiers, e.g. `"0"` in `3.0.0-0`.
    pub pre_release: Option<String>,
    /// Dot-separated build metadata; sorts after no build.
    pub build: Option<String>,
}

impl Version {
    /// Parses a complete version string; the whole input must match.
    ///
    /// # Errors
    ///
    /// Returns [`PubError::Version`] unless the text is exactly
    /// `\d+\.\d+\.\d+(-ident(._ident)*)?(\+ident(._ident)*)?`.
    pub fn parse(text: &str) -> Result<Version, PubError> {
        let invalid = || PubError::Version(format!("invalid version `{text}`"));

        let (rest, build) = match text.split_once('+') {
            Some((rest, build)) => (rest, Some(build)),
            None => (text, None),
        };
        let (core, pre_release) = match rest.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (rest, None),
        };

        let mut nums = core.split('.');
        let (major, minor, patch) = match (nums.next(), nums.next(), nums.next(), nums.next()) {
            (Some(m), Some(n), Some(p), None) => {
                (parse_number(m), parse_number(n), parse_number(p))
            }
            _ => return Err(invalid()),
        };
        let (Some(major), Some(minor), Some(patch)) = (major, minor, patch) else {
            return Err(invalid());
        };
        if let Some(pre) = pre_release
            && !valid_identifiers(pre)
        {
            return Err(invalid());
        }
        if let Some(build) = build
            && !valid_identifiers(build)
        {
            return Err(invalid());
        }
        Ok(Version {
            major,
            minor,
            patch,
            pre_release: pre_release.map(str::to_owned),
            build: build.map(str::to_owned),
        })
    }

    /// pub's `nextBreaking`: bump major when nonzero, else minor; drops any
    /// pre-release/build suffix (`pub_semver` `version.dart:243`).
    #[must_use]
    pub fn next_breaking(&self) -> Version {
        if self.major == 0 {
            Version::new(self.major, self.minor + 1, 0)
        } else {
            Version::new(self.major + 1, 0, 0)
        }
    }

    /// pub's `firstPreRelease`: the lowest pre-release of this version,
    /// used as the exclusive upper bound of caret ranges.
    #[must_use]
    pub fn first_pre_release(&self) -> Version {
        Version {
            major: self.major,
            minor: self.minor,
            patch: self.patch,
            pre_release: Some("0".to_owned()),
            build: None,
        }
    }

    /// `major.minor.patch` without suffixes.
    #[must_use]
    pub const fn new(major: u64, minor: u64, patch: u64) -> Version {
        Version {
            major,
            minor,
            patch,
            pre_release: None,
            build: None,
        }
    }
}

fn parse_number(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn valid_identifiers(text: &str) -> bool {
    !text.is_empty()
        && text
            .split('.')
            .all(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

fn compare_pre_release(a: Option<&String>, b: Option<&String>) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater, // a release outranks any pre-release
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => {
            let mut xs = x.split('.');
            let mut ys = y.split('.');
            loop {
                match (xs.next(), ys.next()) {
                    (None, None) => return Ordering::Equal,
                    (None, Some(_)) => return Ordering::Less,
                    (Some(_), None) => return Ordering::Greater,
                    (Some(xi), Some(yi)) => {
                        let ord = match (parse_number(xi), parse_number(yi)) {
                            (Some(xn), Some(yn)) => xn.cmp(&yn),
                            // Numeric identifiers sort below alphanumeric ones.
                            (Some(_), None) => Ordering::Less,
                            (None, Some(_)) => Ordering::Greater,
                            (None, None) => xi.cmp(yi),
                        };
                        if ord != Ordering::Equal {
                            return ord;
                        }
                    }
                }
            }
        }
    }
}

// Build metadata participates in ordering after pre-release, exactly as
// pub_semver's `compareTo` (builds always come after no build string;
// version.dart:241-243) and its `==` (which includes build; 168-173), so
// `Ord` and `Eq` agree.
impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then_with(|| self.minor.cmp(&other.minor))
            .then_with(|| self.patch.cmp(&other.patch))
            .then_with(|| {
                compare_pre_release(self.pre_release.as_ref(), other.pre_release.as_ref())
            })
            .then_with(|| compare_build(self.build.as_ref(), other.build.as_ref()))
    }
}

/// Orders the build part: a version with a build sorts after one without,
/// and two builds compare by their dot-separated identifiers (pub_semver's
/// `_compareLists` — the same identifier rules as pre-release).
fn compare_build(a: Option<&String>, b: Option<&String>) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(x), Some(y)) => compare_pre_release(Some(x), Some(y)),
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(pre) = &self.pre_release {
            write!(f, "-{pre}")?;
        }
        if let Some(build) = &self.build {
            write!(f, "+{build}")?;
        }
        Ok(())
    }
}

// Canonical serialization for the graph id: the plain version string.
impl serde::Serialize for Version {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// A version range with optional bounds — the only constraint shape pub's
/// parser produces besides exact versions, `any`, and the empty constraint.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct VersionRange {
    pub min: Option<Version>,
    pub include_min: bool,
    pub max: Option<Version>,
    pub include_max: bool,
}

/// A parsed `pub_semver` constraint.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum VersionConstraint {
    /// `any` — allows every version.
    Any,
    /// A bare version such as `1.2.3` — allows exactly that version.
    Exact(Version),
    /// A range of allowed versions.
    Range(VersionRange),
    /// Contradicting bounds; allows nothing (pub's `VersionConstraint.empty`).
    Empty,
}

impl VersionConstraint {
    /// Parses the pub_semver constraint grammar (see the module docs).
    ///
    /// # Errors
    ///
    /// Returns [`PubError::Version`] for anything outside the grammar,
    /// including `~`/`~>` (not in pub's syntax) and comma lists.
    pub fn parse(text: &str) -> Result<VersionConstraint, PubError> {
        let text = text.trim();
        if text == "any" {
            return Ok(VersionConstraint::Any);
        }

        // Caret must occupy the entire string; Version::parse already
        // requires its whole input to match, so this rejects `^1.0.0 <2.0.0`.
        if let Some(rest) = text.strip_prefix('^') {
            let version = Version::parse(rest.trim())?;
            let max = version.next_breaking().first_pre_release();
            return Ok(VersionConstraint::Range(VersionRange {
                min: Some(version),
                include_min: true,
                max: Some(max),
                include_max: false,
            }));
        }

        // Space-separated bare versions and comparisons intersect: the
        // tightest min/max win; on ties the exclusive bound wins
        // (pub_semver `version_constraint.dart:143-163`).
        let mut min: Option<(Version, bool)> = None;
        let mut max: Option<(Version, bool)> = None;
        let mut rest = text;
        loop {
            rest = rest.trim_start();
            if rest.is_empty() {
                break;
            }
            if let Some((lo, hi, remainder)) = scan_comparison(rest) {
                if let Some(lo) = lo {
                    merge_bound(&mut min, lo, true);
                }
                if let Some(hi) = hi {
                    // Each comparison term is its own `VersionRange` in pub,
                    // whose constructor lowers an exclusive bare release to
                    // its first pre-release before any merge happens
                    // (version_range.dart:64-74) — `<3.0.0` is `<3.0.0-0`
                    // from the start.
                    merge_bound(&mut max, lower_exclusive_max(hi), false);
                }
                rest = remainder;
                continue;
            }
            if let Some((version, remainder)) = scan_bare_version(rest) {
                // A bare version is both bounds, inclusive: it collapses to
                // an exact constraint when nothing tightens either side.
                merge_bound(&mut min, (version.clone(), true), true);
                merge_bound(&mut max, (version, true), false);
                rest = remainder;
                continue;
            }
            return Err(PubError::Version(format!(
                "could not parse constraint `{text}`"
            )));
        }

        if min.is_none() && max.is_none() {
            return Err(PubError::Version("empty constraint string".into()));
        }
        // pub's final assembly (version_constraint.dart:155-170): empty on
        // crossed bounds; on equal bounds only `>=v <=v` survives, as an
        // exact constraint. The per-term lowering above is what turns
        // `>=1.2.3 <1.2.3` into a crossed pair (1.2.3 > 1.2.3-0).
        if let (Some((lo, inc_lo)), Some((hi, inc_hi))) = (&min, &max) {
            if lo > hi {
                return Ok(VersionConstraint::Empty);
            }
            if lo == hi {
                return Ok(if *inc_lo && *inc_hi {
                    VersionConstraint::Exact(lo.clone())
                } else {
                    VersionConstraint::Empty
                });
            }
        }
        Ok(VersionConstraint::Range(VersionRange {
            min: min.as_ref().map(|(v, _)| v.clone()),
            include_min: min.as_ref().is_some_and(|(_, i)| *i),
            max: max.as_ref().map(|(v, _)| v.clone()),
            include_max: max.as_ref().is_some_and(|(_, i)| *i),
        }))
    }

    /// The lower bound of the constraint, if it has one — the sole input to
    /// language version inference (`language_version.dart:37-57`).
    #[must_use]
    pub fn lower_bound(&self) -> Option<&Version> {
        match self {
            VersionConstraint::Exact(v) => Some(v),
            VersionConstraint::Range(range) => range.min.as_ref(),
            VersionConstraint::Any | VersionConstraint::Empty => None,
        }
    }
}

/// A version bound plus whether it is inclusive.
type Bound = (Version, bool);

/// Scans `op version` at the start of `text`; returns the min-side and/or
/// max-side bound it contributes plus the unconsumed remainder.
fn scan_comparison(text: &str) -> Option<(Option<Bound>, Option<Bound>, &str)> {
    let (op, rest) = if let Some(r) = text.strip_prefix("<=") {
        ("<=", r)
    } else if let Some(r) = text.strip_prefix(">=") {
        (">=", r)
    } else if let Some(r) = text.strip_prefix('<') {
        ("<", r)
    } else if let Some(r) = text.strip_prefix('>') {
        (">", r)
    } else {
        return None;
    };
    let (version, remainder) = scan_bare_version(rest.trim_start())?;
    let bound = match op {
        "<=" => (None, Some((version, true))),
        "<" => (None, Some((version, false))),
        ">=" => (Some((version, true)), None),
        _ => (Some((version, false)), None),
    };
    Some((bound.0, bound.1, remainder))
}

/// Scans a bare version at the start of `text` (three numeric dot groups,
/// then optional pre-release/build), returning it with the remainder.
fn scan_bare_version(text: &str) -> Option<(Version, &str)> {
    let bytes = text.as_bytes();
    let mut parts = [""; 3];
    let mut end = 0;
    for (i, slot) in parts.iter_mut().enumerate() {
        let start = end;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end == start {
            return None;
        }
        *slot = &text[start..end];
        if i < 2 {
            if bytes.get(end) != Some(&b'.') {
                return None;
            }
            end += 1;
        }
    }
    let mut version = Version {
        major: parse_number(parts[0])?,
        minor: parse_number(parts[1])?,
        patch: parse_number(parts[2])?,
        pre_release: None,
        build: None,
    };
    let mut rest = &text[end..];
    if let Some(stripped) = rest.strip_prefix('-') {
        let end = identifier_end(stripped);
        let pre = &stripped[..end];
        if !valid_identifiers(pre) {
            return None;
        }
        version.pre_release = Some(pre.to_owned());
        rest = &stripped[end..];
    }
    if let Some(stripped) = rest.strip_prefix('+') {
        let end = identifier_end(stripped);
        let build = &stripped[..end];
        if !valid_identifiers(build) {
            return None;
        }
        version.build = Some(build.to_owned());
        rest = &stripped[end..];
    }
    Some((version, rest))
}

/// Length of the leading run of pre-release/build identifier characters.
fn identifier_end(text: &str) -> usize {
    text.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '.'))
        .unwrap_or(text.len())
}

/// Lowers an exclusive bound that is a bare release (no pre-release, no
/// build) to its first pre-release — pub_semver's `VersionRange` constructor
/// rule for a term with no min (version_range.dart:64-74; a scanned
/// comparison never carries a min, so the min-shield cannot fire here).
/// `<3.0.0` therefore allows `3.0.0-*` pre-releases exactly as little as
/// pub does.
fn lower_exclusive_max(mut bound: Bound) -> Bound {
    if !bound.1 && bound.0.pre_release.is_none() && bound.0.build.is_none() {
        bound.0 = bound.0.first_pre_release();
    }
    bound
}

/// Keeps the tighter bound; on equal bounds an exclusion wins over inclusion
/// (mirrors pub's `includeMin`/`includeMax` intersection).
fn merge_bound(current: &mut Option<Bound>, new: Bound, is_min: bool) {
    let Some((ref mut old_version, ref mut old_include)) = *current else {
        *current = Some(new);
        return;
    };
    let tighten = if is_min {
        new.0 > *old_version
    } else {
        new.0 < *old_version
    };
    if tighten {
        *old_version = new.0;
        *old_include = new.1;
    } else if new.0 == *old_version && !new.1 {
        *old_include = false;
    }
}

/// A Dart language version `X.Y` (`language_version.dart`).
///
/// A newtype over [`dartq_language::LanguageVersion`] — the single source
/// of truth for language versioning — so this crate's public API bridges to
/// it directly while adding pub-side behavior (parse, the sdk-constraint
/// rule, serde for canonical config serialization; dartq-language's type
/// carries no serde impls).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LanguageVersion(dartq_language::LanguageVersion);

impl LanguageVersion {
    /// pub's fallback when a constraint has no lower bound
    /// (`defaultLanguageVersion`).
    pub const PUB_DEFAULT: LanguageVersion = LanguageVersion::new(2, 7);
    /// `firstVersionWithWorkspaces` — gates `workspace:`/`resolution:`.
    pub const FIRST_WITH_WORKSPACES: LanguageVersion = LanguageVersion::new(3, 5);
    /// `firstVersionWithWorkspaceGlobs` — gates glob expansion of
    /// `workspace:` entries (language_version.dart:115).
    pub const FIRST_WITH_WORKSPACE_GLOBS: LanguageVersion = LanguageVersion::new(3, 11);
    /// dartq's chain bottom (PRD §8.3 level 4): the current SDK default,
    /// taken from the vendored registry's `DEFAULT_LANGUAGE_VERSION` (3.15).
    pub const SDK_DEFAULT: LanguageVersion = LanguageVersion::new(
        DEFAULT_LANGUAGE_VERSION.major,
        DEFAULT_LANGUAGE_VERSION.minor,
    );

    #[must_use]
    pub const fn new(major: u32, minor: u32) -> LanguageVersion {
        LanguageVersion(dartq_language::LanguageVersion::new(major, minor))
    }

    /// The major component.
    #[must_use]
    pub const fn major(self) -> u32 {
        self.0.major
    }

    /// The minor component.
    #[must_use]
    pub const fn minor(self) -> u32 {
        self.0.minor
    }

    /// The underlying dartq-language value.
    #[must_use]
    pub const fn into_inner(self) -> dartq_language::LanguageVersion {
        self.0
    }

    /// Parses `X.Y` (regex `^\d+\.\d+$`).
    ///
    /// # Errors
    ///
    /// Returns [`PubError::Version`] for anything else.
    pub fn parse(text: &str) -> Result<LanguageVersion, PubError> {
        let invalid = || PubError::Version(format!("invalid language version `{text}`"));
        let Some((major, minor)) = text.split_once('.') else {
            return Err(invalid());
        };
        let (Some(major), Some(minor)) = (parse_number_u32(major), parse_number_u32(minor)) else {
            return Err(invalid());
        };
        Ok(LanguageVersion::new(major, minor))
    }

    /// pub's `LanguageVersion.fromSdkConstraint`: the constraint's lower
    /// bound major.minor; `2.7` when absent or bound-less (facts §5-L1).
    #[must_use]
    pub fn from_sdk_constraint(constraint: Option<&VersionConstraint>) -> LanguageVersion {
        constraint
            .and_then(VersionConstraint::lower_bound)
            .map_or(Self::PUB_DEFAULT, Self::of_version)
    }

    /// The major.minor of a version, saturating at `u32::MAX`.
    fn of_version(version: &Version) -> LanguageVersion {
        LanguageVersion::new(
            u32::try_from(version.major).unwrap_or(u32::MAX),
            u32::try_from(version.minor).unwrap_or(u32::MAX),
        )
    }
}

impl From<dartq_language::LanguageVersion> for LanguageVersion {
    fn from(inner: dartq_language::LanguageVersion) -> LanguageVersion {
        LanguageVersion(inner)
    }
}

fn parse_number_u32(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

impl fmt::Display for LanguageVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.0.major, self.0.minor)
    }
}

// Canonical string form (`"X.Y"`), matching how pub writes the field; the
// config's derived `Serialize` relies on this for its graph-id canonical bytes.
impl serde::Serialize for LanguageVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).expect("valid version")
    }

    fn c(text: &str) -> VersionConstraint {
        VersionConstraint::parse(text).expect("valid constraint")
    }

    #[test]
    fn version_parse_round_trip() {
        let parsed = v("1.9.1");
        assert_eq!((parsed.major, parsed.minor, parsed.patch), (1, 9, 1));
        assert_eq!(parsed.to_string(), "1.9.1");
        let fancy = v("3.0.0-0+meta");
        assert_eq!(fancy.pre_release.as_deref(), Some("0"));
        assert_eq!(fancy.build.as_deref(), Some("meta"));
        assert_eq!(fancy.to_string(), "3.0.0-0+meta");
    }

    #[test]
    fn version_parse_rejects_garbage() {
        for bad in [
            "1.2",
            "v1.2.3",
            "1.2.3-",
            "1.2.3+",
            "1.2.3.4",
            "",
            "01.2.3-x!",
        ] {
            assert!(Version::parse(bad).is_err(), "`{bad}` must not parse");
        }
    }

    #[test]
    fn version_ordering_semver() {
        let mut list = [
            v("1.0.0"),
            v("1.0.0-alpha"),
            v("1.0.0-alpha.1"),
            v("1.0.0-alpha.beta"),
            v("1.0.0-beta"),
            v("1.0.0+build"),
            v("2.0.0"),
        ];
        list.sort();
        let expected = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0",
            "1.0.0+build",
            "2.0.0",
        ];
        let rendered: Vec<String> = list.iter().map(ToString::to_string).collect();
        assert_eq!(rendered, expected);
    }

    #[test]
    fn build_metadata_participates_in_ordering() {
        // pub_semver: builds always come after no build string, and `==`
        // includes build — `Ord` and `Eq` must agree (no cmp-Equal-but-!=).
        assert!(v("1.0.0") < v("1.0.0+a"));
        assert!(v("1.0.0+a") < v("1.0.0+b"));
        assert_eq!(v("1.0.0+a"), v("1.0.0+a"));

        // A build-bearing bound pair stays a range: with build in the
        // ordering, the bounds no longer compare equal, so pub's final
        // assembly must not collapse this to an exact constraint.
        assert_eq!(
            c(">=1.0.0+a <=1.0.0+b"),
            VersionConstraint::Range(VersionRange {
                min: Some(v("1.0.0+a")),
                include_min: true,
                max: Some(v("1.0.0+b")),
                include_max: true,
            })
        );
        // Without builds, equal inclusive bounds still collapse to exact.
        assert_eq!(c(">=1.0.0 <=1.0.0"), VersionConstraint::Exact(v("1.0.0")));
    }

    #[test]
    fn constraint_any_exact_and_range() {
        assert_eq!(c("any"), VersionConstraint::Any);
        assert_eq!(c("1.2.3"), VersionConstraint::Exact(v("1.2.3")));
        // The exclusive max is pub's normalized form: `<3.0.0` stores
        // `3.0.0-0`, as pub_semver's VersionRange constructor does
        // (version_range.dart:64-74).
        assert_eq!(
            c(">=2.0.0 <3.0.0"),
            VersionConstraint::Range(VersionRange {
                min: Some(v("2.0.0")),
                include_min: true,
                max: Some(v("3.0.0-0")),
                include_max: false,
            })
        );
    }

    #[test]
    fn exclusive_max_lowers_bare_releases() {
        // `<3.0.0` behaves as `<3.0.0-0`.
        assert_eq!(
            c("<3.0.0"),
            VersionConstraint::Range(VersionRange {
                max: Some(v("3.0.0-0")),
                include_max: false,
                ..VersionRange::default()
            })
        );
        // Inclusive max stays.
        assert_eq!(
            c("<=3.0.0"),
            VersionConstraint::Range(VersionRange {
                max: Some(v("3.0.0")),
                include_max: true,
                ..VersionRange::default()
            })
        );
        // A pre-release or build suffix on the bound blocks the lowering.
        assert!(
            matches!(c("<3.0.0-rc"), VersionConstraint::Range(r) if r.max == Some(v("3.0.0-rc")))
        );
        assert!(
            matches!(c("<3.0.0+build"), VersionConstraint::Range(r) if r.max == Some(v("3.0.0+build")))
        );
    }

    #[test]
    fn equal_bounds_collapse_to_empty() {
        // pub lowers `<1.2.3` to `1.2.3-0` per term, so this is a crossed
        // pair (1.2.3 > 1.2.3-0) and collapses to VersionConstraint.empty.
        assert_eq!(c(">=1.2.3 <1.2.3"), VersionConstraint::Empty);
        // Same for an exclusive min, and for a min that is a pre-release of
        // the max: each term is normalized independently, so the bounds meet
        // at 1.2.3-0 with the max exclusive.
        assert_eq!(c(">1.2.3 <1.2.3"), VersionConstraint::Empty);
        assert_eq!(c(">=1.2.3-0 <1.2.3"), VersionConstraint::Empty);
    }

    #[test]
    fn caret_bounds_follow_pub_semver() {
        // ^3.5.0 => >=3.5.0 <4.0.0-0
        assert_eq!(
            c("^3.5.0"),
            VersionConstraint::Range(VersionRange {
                min: Some(v("3.5.0")),
                include_min: true,
                max: Some(v("4.0.0-0")),
                include_max: false,
            })
        );
        // 0.x carets bump the minor (pub_semver nextBreaking).
        assert_eq!(c("^0.2.3").lower_bound(), Some(&v("0.2.3")),);
        assert!(matches!(c("^0.2.3"), VersionConstraint::Range(r) if r.max == Some(v("0.3.0-0"))));
        // Caret must be the whole constraint.
        assert!(VersionConstraint::parse("^1.2.3 <2.0.0").is_err());
    }

    #[test]
    fn constraint_intersection_picks_tightest() {
        assert_eq!(
            c(">=2.0.0 >=3.0.0 <4.0.0 <3.5.0").lower_bound(),
            Some(&v("3.0.0"))
        );
        // Contradiction collapses to Empty (pub returns VersionConstraint.empty).
        assert_eq!(c(">=3.0.0 <2.0.0"), VersionConstraint::Empty);
        // Equal bounds inclusive on both sides => exact.
        assert_eq!(c(">=1.2.3 <=1.2.3"), VersionConstraint::Exact(v("1.2.3")));
    }

    #[test]
    fn no_tilde_syntax_accepted_fact() {
        // facts §2: pub has no `~`/`~>` syntax and no comma lists.
        for bad in ["~1.2.3", "~>1.2.3", ">=1.0.0, <2.0.0"] {
            assert!(
                VersionConstraint::parse(bad).is_err(),
                "`{bad}` must not parse"
            );
        }
    }

    #[test]
    fn language_version_from_sdk_constraint() {
        use LanguageVersion as Lv;
        // facts §5-L1 measured cases.
        assert_eq!(
            Lv::from_sdk_constraint(Some(&c(">=3.0.0 <4.0.0"))),
            Lv::new(3, 0)
        );
        assert_eq!(Lv::from_sdk_constraint(Some(&c("^3.5.0"))), Lv::new(3, 5));
        assert_eq!(Lv::from_sdk_constraint(Some(&c("any"))), Lv::PUB_DEFAULT);
        assert_eq!(Lv::from_sdk_constraint(Some(&c("<3.0.0"))), Lv::PUB_DEFAULT);
        assert_eq!(Lv::from_sdk_constraint(None), Lv::PUB_DEFAULT);
        assert_eq!(Lv::from_sdk_constraint(Some(&c("2.7.0"))), Lv::new(2, 7));
        // The empty collapse feeds pub's `sdkConstraint.isEmpty` check
        // (language_version.dart:38-40): a contradictory constraint has no
        // lower bound, so the 2.7 default applies.
        assert_eq!(
            Lv::from_sdk_constraint(Some(&c(">=1.2.3 <1.2.3"))),
            Lv::PUB_DEFAULT
        );
    }

    #[test]
    fn language_version_parse_strict() {
        assert_eq!(
            LanguageVersion::parse("3.5").expect("ok"),
            LanguageVersion::new(3, 5)
        );
        for bad in ["3", "3.5.0", "v3.5", "3.x", "", "3.-1"] {
            assert!(
                LanguageVersion::parse(bad).is_err(),
                "`{bad}` must not parse"
            );
        }
    }

}
