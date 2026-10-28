// Extraction core for the official-corpus porter.
//
// Walks the analyzer package's test sources, finds every inline-diagnostic
// test call, and turns it into a standalone case: clean code (expectation
// markers removed, semantic language-feature directives lowered), expected
// diagnostics (code + position), and companion files.
library;

import 'package:analyzer/dart/analysis/features.dart';
import 'package:analyzer/dart/analysis/utilities.dart';
import 'package:analyzer/dart/ast/ast.dart';
import 'package:analyzer/dart/ast/visitor.dart';
import 'package:analyzer/src/dart/analysis/experiments.dart';
import 'package:pub_semver/pub_semver.dart';

/// The test-entry methods whose first argument carries the code snippet.
const callNames = {
  'resolveTestCodeWithDiagnostics',
  'resolveTestCode',
  'assertTestCodeWithStrictCastsDiagnostics',
};

final _caretPattern = RegExp(r'^[ \t]*//[ \t]*\^+[ \t]*$');
final _expectationPattern = RegExp(
  r'^[ \t]*//[ \t]*\[(?:diag\.[A-Za-z_][A-Za-z0-9_]*|context[ \t]+[0-9]+)\]',
);
final _diagLinePattern = RegExp(
  r'^[ \t]*//[ \t]*\[diag\.([A-Za-z_][A-Za-z0-9_]*)\]',
);
final _explicitLocationPattern = RegExp(
  r'\[column ([0-9]+)\]\[length ([0-9]+)\]',
);

final _languageVersionOverridePattern = RegExp(
  r'^[ \t]*//[ \t]*@dart[ \t]*=',
  multiLine: true,
);
final _beforeLanguageFeaturePattern = RegExp(
  r'^([ \t]*)//[ \t]*%before-language-feature:[ \t]*'
  r'([a-z0-9-]+)[ \t]*(?=\r?$)',
  multiLine: true,
);

/// Whether [name] is a class-level setup mixin the corpus does not model.
bool isSkippedSetupName(String name) =>
    name.contains('StrictCasts') ||
    name.contains('WithoutNullSafety') ||
    RegExp('Language2[0-9]').hasMatch(name) ||
    name.contains('Experiment');

class ExpectedDiagnostic {
  final String code;
  final int line;
  final int column;
  final int length;

  ExpectedDiagnostic(this.code, this.line, this.column, this.length);

  Map<String, Object> toJson() => {
    'code': code,
    'line': line,
    'column': column,
    'length': length,
  };
}

class CompanionFile {
  /// Path relative to the case root, e.g. `a.dart` or `lib/a.dart`.
  final String relativePath;
  final String content;

  CompanionFile(this.relativePath, this.content);
}

class ExtractedCase {
  /// `<test file>/<class>/<method>#<n>` — the single case identity.
  final String id;

  /// Clean code (markers removed, feature directive lowered).
  final String code;
  final List<ExpectedDiagnostic> expected;
  final List<CompanionFile> companions;
  final bool needsMeta;
  final bool needsStrictCasts;

  ExtractedCase(
    this.id,
    this.code,
    this.expected,
    this.companions,
    this.needsMeta,
    this.needsStrictCasts,
  );
}

class SkippedCase {
  final String id;
  final String reason;

  SkippedCase(this.id, this.reason);
}

class FileExtraction {
  final List<ExtractedCase> cases;
  final List<SkippedCase> skips;

  FileExtraction(this.cases, this.skips);
}

/// Removes diagnostic expectation marker lines, keeping each retained line's
/// own terminator, with terminators only between retained lines.
String removeDiagnosticExpectations(String content) {
  var lines = _parseLines(content);
  var kept = lines.where((l) => !_isMarkerLine(l.text)).toList();
  var buffer = StringBuffer();
  for (var i = 0; i < kept.length; i++) {
    buffer.write(kept[i].text);
    if (i < kept.length - 1) {
      buffer.write(kept[i].terminator);
    }
  }
  return buffer.toString();
}

class _Line {
  final String text;
  final String terminator;
  _Line(this.text, this.terminator);
}

List<_Line> _parseLines(String content) {
  var result = <_Line>[];
  var lineStart = 0;
  for (var i = 0; i < content.length; i++) {
    var cu = content.codeUnitAt(i);
    if (cu == 0x0D || cu == 0x0A) {
      var terminator = content.substring(i, i + 1);
      if (cu == 0x0D && i + 1 < content.length && content.codeUnitAt(i + 1) == 0x0A) {
        terminator = content.substring(i, i + 2);
        i++;
      }
      result.add(_Line(content.substring(lineStart, i), terminator));
      lineStart = i + 1;
    }
  }
  result.add(_Line(content.substring(lineStart), ''));
  return result;
}

bool _isMarkerLine(String text) =>
    _caretPattern.hasMatch(text) || _expectationPattern.hasMatch(text);

/// Extracts expected diagnostics from marker lines in [content].
///
/// A diagnostic's position comes from the explicit `[column N][length M]`
/// metadata when present, otherwise from the nearest preceding caret line.
/// The 0-based line is the nearest preceding non-marker line's index in the
/// marker-stripped content.
List<ExpectedDiagnostic> extractExpectations(String content) {
  var lines = _parseLines(content);
  var result = <ExpectedDiagnostic>[];
  var keptCount = 0;
  int? caretColumn;
  int? caretLength;
  for (var line in lines) {
    var text = line.text;
    if (_caretPattern.hasMatch(text)) {
      var first = text.indexOf('^');
      caretColumn = first;
      caretLength = text.replaceAll(RegExp('[^^]'), '').length;
      continue;
    }
    var diag = _diagLinePattern.firstMatch(text);
    if (diag != null) {
      int column;
      int length;
      var loc = _explicitLocationPattern.firstMatch(text);
      if (loc != null) {
        column = int.parse(loc.group(1)!) - 1;
        length = int.parse(loc.group(2)!);
      } else if (caretColumn != null) {
        column = caretColumn;
        length = caretLength!;
      } else {
        // No position available; record at origin so the case still surfaces.
        column = 0;
        length = 0;
      }
      result.add(
        ExpectedDiagnostic(
          screamingSnake(diag.group(1)!),
          keptCount - 1,
          column,
          length,
        ),
      );
      continue;
    }
    if (_expectationPattern.hasMatch(text)) {
      continue; // context-message marker
    }
    keptCount++;
    caretColumn = null;
    caretLength = null;
  }
  return result;
}

/// Converts a camelCase diagnostic marker name to the analyzer's lowercase
/// snake_case unique code name (the `code` field of machine output). The
/// official harness derives marker names from code names via
/// `toCamelCase()`, so this inverts that mapping.
String screamingSnake(String camel) {
  var buffer = StringBuffer();
  for (var i = 0; i < camel.length; i++) {
    var c = camel[i];
    var isUpper = c.toUpperCase() == c && c.toLowerCase() != c;
    if (isUpper && i > 0) {
      buffer.write('_');
    }
    buffer.write(c.toLowerCase());
  }
  return buffer.toString();
}

/// Port of the official harness's `LanguageFeatureDirectiveLowering`: rewrites
/// a `%before-language-feature: X` directive into a `// @dart = V` override.
///
/// Throws [FormatException] where the official lowering throws.
String lowerLanguageFeatureDirective(String code) {
  var matches = _beforeLanguageFeaturePattern.allMatches(code).toList();
  if (matches.isEmpty) {
    return code;
  }
  if (matches.length > 1) {
    throw const FormatException(
      'Only one %before-language-feature directive is supported per file.',
    );
  }
  if (_languageVersionOverridePattern.hasMatch(code)) {
    throw const FormatException(
      '%before-language-feature combined with @dart override.',
    );
  }
  var match = matches.single;
  var featureName = match.group(2)!;
  var feature = ExperimentStatus.knownFeatures[featureName];
  if (feature == null) {
    throw FormatException("Unknown language feature '$featureName'.");
  }
  var version = _languageVersionBefore(feature);
  return code.replaceRange(
    match.start,
    match.end,
    '${match.group(1)}// @dart = ${version.major}.${version.minor}',
  );
}

Version _languageVersionBefore(ExperimentalFeature feature) {
  var enabledVersion =
      feature.releaseVersion ??
      feature.experimentalReleaseVersion ??
      ExperimentStatus.currentVersion;
  if (enabledVersion.minor > 0) {
    return Version(enabledVersion.major, enabledVersion.minor - 1, 0);
  }
  Version? previousKnownVersion;
  for (var knownFeature in ExperimentStatus.knownFeatures.values) {
    for (var version in [
      knownFeature.releaseVersion,
      knownFeature.experimentalReleaseVersion,
    ]) {
      if (version != null &&
          version < enabledVersion &&
          (previousKnownVersion == null || version > previousKnownVersion)) {
        previousKnownVersion = version;
      }
    }
  }
  if (previousKnownVersion == null) {
    throw StateError(
      "Cannot determine a language version before '$enabledVersion'.",
    );
  }
  return previousKnownVersion;
}

/// Extracts every portable case from one test file.
FileExtraction extractFile(String relativePath, String content) {
  var cases = <ExtractedCase>[];
  var skips = <SkippedCase>[];
  var baseName = relativePath.endsWith('_test.dart')
      ? relativePath.substring(0, relativePath.length - '_test.dart'.length)
      : relativePath;
  var parseResult = parseString(
    content: content,
    featureSet: FeatureSet.latestLanguageVersion(),
    throwIfDiagnostics: false,
  );
  var unit = parseResult.unit;

  for (var declaration in unit.declarations) {
    String? className;
    List<String> setupNames = const [];
    List<MethodDeclaration> methods;
    if (declaration is ClassDeclaration) {
      className = declaration.namePart.typeName.lexeme;
      setupNames = _headerNames(
        declaration.extendsClause?.superclass,
        declaration.withClause?.mixinTypes,
      );
      methods = declaration.body.members.whereType<MethodDeclaration>().toList();
    } else if (declaration is ClassTypeAlias) {
      className = declaration.name.lexeme;
      setupNames = _headerNames(
        declaration.superclass,
        declaration.withClause.mixinTypes,
      );
      methods = const [];
    } else if (declaration is MixinDeclaration) {
      className = declaration.name.lexeme;
      setupNames = _headerNames(
        null,
        declaration.onClause?.superclassConstraints,
      );
      methods = declaration.body.members.whereType<MethodDeclaration>().toList();
    } else {
      continue;
    }

    String? classSkipReason;
    for (var name in setupNames) {
      if (isSkippedSetupName(name)) {
        classSkipReason = 'class_setup_$name';
        break;
      }
    }

    for (var method in methods) {
      var methodName = method.name.lexeme;
      var calls = <MethodInvocation>[];
      method.body.accept(_CallCollector(calls));
      var targetIndex = 0;
      for (var call in calls) {
        if (!callNames.contains(call.methodName.name)) {
          continue;
        }
        var id = '$baseName/$className/$methodName#$targetIndex';
        targetIndex++;
        var (skip, built) = _buildCase(
          id: id,
          call: call,
          callsBefore: calls.where((c) => c.offset < call.offset).toList(),
          classSkipReason: classSkipReason,
        );
        if (skip != null) {
          skips.add(SkippedCase(id, skip));
        } else {
          cases.add(built!);
        }
      }
    }
  }
  return FileExtraction(cases, skips);
}

/// Builds one case; returns `(skipReason, case)` with exactly one side set.
(String?, ExtractedCase?) _buildCase({
  required String id,
  required MethodInvocation call,
  required List<MethodInvocation> callsBefore,
  required String? classSkipReason,
}) {
  (String?, ExtractedCase?) fail(String reason) => (reason, null);
  (String?, ExtractedCase?) ok(ExtractedCase c) => (null, c);

  if (classSkipReason != null) {
    return fail(classSkipReason);
  }

  var args = call.argumentList.arguments;
  if (args.isEmpty) {
    return fail('non_literal');
  }
  var code = _literalString(args.first.argumentExpression);
  if (code == null) {
    return fail('non_literal');
  }

  var companions = <CompanionFile>[];
  var needsMeta = false;
  var needsStrictCasts = call.methodName.name ==
      'assertTestCodeWithStrictCastsDiagnostics';
  String? configSkip;
  var customOptions = false;

  for (var setup in callsBefore) {
    var setupName = setup.methodName.name;
    var setupArgs = setup.argumentList.arguments;
    switch (setupName) {
      case 'newFile' when setupArgs.length >= 2:
        var path = _companionPath(setupArgs.first.argumentExpression);
        if (path == null) {
          return fail('companion_outside_package');
        }
        var content = _literalString(setupArgs[1].argumentExpression);
        if (content == null) {
          return fail('companion_non_literal');
        }
        companions.add(CompanionFile(path, content));
      case 'writeTestPackageConfigWithMeta':
        needsMeta = true;
      case 'writeTestPackageConfig':
        for (var arg in setupArgs) {
          if (arg is NamedArgument) {
            var label = arg.name.lexeme;
            var value = arg.argumentExpression;
            if (label == 'languageVersion') {
              configSkip ??= 'config_languageVersion';
            } else if (value is BooleanLiteral && value.value) {
              switch (label) {
                case 'meta':
                  needsMeta = true;
                case 'flutter':
                case 'ffi':
                case 'angularMeta':
                  configSkip ??= 'config_$label';
              }
            }
          }
        }
      case 'writeTestPackageAnalysisOptionsFile':
      case 'createAnalysisOptionsFile':
        customOptions = true;
    }
  }
  if (configSkip != null) {
    return fail(configSkip);
  }
  if (customOptions) {
    return fail('custom_options');
  }

  String clean;
  try {
    clean = removeDiagnosticExpectations(code);
    clean = lowerLanguageFeatureDirective(clean);
  } on FormatException {
    return fail('lowering_error');
  } on StateError {
    return fail('lowering_error');
  }

  return ok(
    ExtractedCase(
      id,
      clean,
      extractExpectations(code),
      companions,
      needsMeta,
      needsStrictCasts,
    ),
  );
}

/// Returns the value of [e] if it is a string literal (a single simple literal
/// or adjacent simple literals), else `null`.
String? _literalString(Expression e) {
  if (e is SimpleStringLiteral) {
    return e.value;
  }
  if (e is AdjacentStrings) {
    var buffer = StringBuffer();
    for (var part in e.strings) {
      if (part is! SimpleStringLiteral) {
        return null;
      }
      buffer.write(part.value);
    }
    return buffer.toString();
  }
  return null;
}

/// Returns the case-root-relative path of a `newFile` first argument rooted at
/// `$testPackageLibPath` or `$testPackageRootPath`, else `null`.
String? _companionPath(Expression e) {
  if (e is! StringInterpolation) {
    return null;
  }
  bool? isLibPath;
  var rest = StringBuffer();
  for (var element in e.elements) {
    if (element is InterpolationString) {
      rest.write(element.value);
      continue;
    }
    if (element is! InterpolationExpression) {
      return null;
    }
    var expr = element.expression;
    if (expr is! SimpleIdentifier || isLibPath != null) {
      return null;
    }
    switch (expr.name) {
      case 'testPackageLibPath' when rest.isEmpty:
        isLibPath = true;
      case 'testPackageRootPath' when rest.isEmpty:
        isLibPath = false;
      default:
        return null;
    }
  }
  if (isLibPath == null) {
    return null;
  }
  var restText = rest.toString();
  if (!restText.startsWith('/') || restText.length < 2) {
    return null;
  }
  return restText.substring(1);
}

List<String> _headerNames(NamedType? superclass, List<NamedType>? mixins) {
  return [
    if (superclass != null) superclass.name.lexeme,
    ...?mixins?.map((t) => t.name.lexeme),
  ];
}

class _CallCollector extends RecursiveAstVisitor<void> {
  final List<MethodInvocation> calls;
  _CallCollector(this.calls);

  @override
  void visitMethodInvocation(MethodInvocation node) {
    calls.add(node);
    node.visitChildren(this);
  }
}
