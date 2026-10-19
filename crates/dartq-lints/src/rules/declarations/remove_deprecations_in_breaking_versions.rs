use super::{Check, NodeKind};
use crate::registry::{Group, RuleSpec};
use crate::rules::not_dispatched;

/// Official: `pkg/linter/lib/src/rules/remove_deprecations_in_breaking_version.dart`
/// — when the package's pubspec version is a breaking release
/// (`isBreakingVersion`, :22-26), every `@deprecated`/`@Deprecated`
/// annotation reports at its name (`visitAnnotation`, :49-55): the
/// breaking release was the promised cleanup point, the deprecations
/// should be gone by now. Registration is gated on the pubspec version
/// (:36-48 — a missing or unparseable version never registers).
///
/// `kinds: &[]` — the gate fact is the containing package's `version:`
/// value, which the engine's [`Unit`][crate::Unit] does not carry (the
/// pubspec family's mounting seam, `rules/pubspec.rs`). The body is the
/// two pure faces the mounter joins: [`is_breaking_version`] over the
/// pubspec value and [`check_annotation`] over each declaration's
/// metadata row. Batch-close note: the #1153 promotion pins this name
/// into `STRUCTURALLY_SILENT` (`tests/registry_pin.rs`) beside the
/// freeze regen. Message: `diagnostic.g.dart`
/// `removeDeprecationsInBreakingVersions`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "remove_deprecations_in_breaking_versions",
    group: Group::Standalone,
    problem: "Remove deprecated elements in breaking versions.",
    correction: "Try removing the deprecated element.",
    kinds: &[],
    check: not_dispatched,
};

/// The official `isBreakingVersion` (:22-26) over the pubspec
/// `version:` value: the build metadata must be absent (a prerelease
/// still counts — `2.0.0-dev` ships before the stable major and is
/// expected to clean up), and the release must sit on a zero-segment
/// boundary: `x.0.0` or `0.x.0`. An unparseable value is not breaking
/// (the registration bail, :44-46).
#[must_use]
pub fn is_breaking_version(version: &str) -> bool {
    let Some((major, minor, patch, has_build)) = parse(version) else {
        return false;
    };
    !has_build && (major == 0 || minor == 0) && patch == 0
}

/// `pub_semver`'s `major.minor.patch[-prerelease][+build]` shape, the
/// four facts the verdict reads. A prerelease suffix is cut at the
/// first `-`; anything past the third numeric segment fails. Upstream
/// pub_semver 2.2.1 requires at least one identifier character per
/// optional segment, so `1.0.0-` and `1.0.0+` fail `Version.parse`
/// (the official gate never registers) — they fail closed here too,
/// rather than reading as a bare breaking `1.0.0`.
fn parse(version: &str) -> Option<(u64, u64, u64, bool)> {
    let (core, has_build) = match version.split_once('+') {
        Some((core, build)) if !build.is_empty() => (core, true),
        Some(_) => return None,
        None => (version, false),
    };
    let numbers = match core.split_once('-') {
        Some((numbers, prerelease)) if !prerelease.is_empty() => numbers,
        Some(_) => return None,
        None => core,
    };
    let mut parts = numbers.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch, has_build))
}

/// The official annotation face (`visitAnnotation`, :49-55) over one
/// `Metadata` row: on a breaking version, a deprecation annotation
/// reports at its name identifier (`reportAtNode(node.name)` — the
/// corpus `lint(1, 10)` for `@deprecated foo() {}`). The mounter walks
/// the declaration rows' metadata with the version gate applied first
/// (:36-48). `None` stays silent.
#[must_use]
pub fn check_annotation(
    c: &Check<'_>,
    version: &str,
    meta: u32,
) -> Option<core::ops::Range<u32>> {
    if !is_breaking_version(version) {
        return None;
    }
    if !super::is_deprecated_metadata(c, meta) {
        return None;
    }
    let name = c
        .collect_direct(meta)
        .into_iter()
        .find(|&piece| c.kind(piece) == NodeKind::Identifier)?;
    Some(c.utf16_span(name))
}
