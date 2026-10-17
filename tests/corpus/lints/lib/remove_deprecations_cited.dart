// remove_deprecations_cited — late-join citation face (#1381, F8).
// The rule remove_deprecations_in_breaking_versions is dartq's
// LATE_JOIN_RULES seam (#1353): its rows join at the shell stage and
// `// ignore:` citations must suppress them. Upstream shape (Dart SDK
// source, pkg/linter/lib/src/rules/
// remove_deprecations_in_breaking_version.dart): the visitor reports at
// every deprecation annotation (lines 63-71, reportAtNode on the
// annotation name), gated at lines 44-57 on a PubPackage whose pubspec
// carries a version passing isBreakingVersion (lines 19-24: x.0.0 or
// 0.x.0). This corpus root's pubspec has NO `version:` field, so
// pubspec.version is null and the gate returns before the visitor is
// registered; the rule is also not named in analysis_options.yaml.
// Zero rows by design on both legs — dartq and the dart analyze oracle.
// ANY row on this file in either leg at the serial run is a regression
// of the #1353 seam (a row joined without the version gate or without
// citation matching); N5 serial adjudication owns the verdict.
// Citation window pins (analyzer suppression machinery, Dart SDK
// source, pkg/analyzer/lib/src/ignore_comments/ignore_info.dart:
// 130-133 — an own-line citation covers only the next line, a trailing
// citation only its own line):
//   cited — citation directly above the annotation line, inside the
//   window; suppresses when the gate is on.
//   misplacedControl — citation one line below the annotation; its
//   own-line window starts at the declaration line, so a gated row on
//   the annotation line above is never suppressed. Both citations are
//   documented per document_ignores (pkg/linter document_ignores.dart:
//   59-104): this face by its comment line above, the control by
//   trailing free text.
// No uses of the deprecated members on purpose: upstream rows land at
// the annotation, not at use sites, so uses would only couple the
// zero-row posture to unrelated rules' membership.
// Expected rows: 0 — expect-empty on both sides.
class Legacy {
  // Cited: the citation sits on the line immediately preceding the
  // annotation.
  // ignore: remove_deprecations_in_breaking_versions
  @Deprecated('Use current instead.')
  String cited() => '';

  // Control: the citation is one line too low.
  @Deprecated('Use current instead.')
  // ignore: remove_deprecations_in_breaking_versions, one line too low
  String misplacedControl() => '';
}
