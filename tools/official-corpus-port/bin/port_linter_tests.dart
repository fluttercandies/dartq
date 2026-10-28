// Ports the upstream Dart SDK linter rule test suite (pkg/linter/test/rules)
// into a materialized analysis corpus: one package per rule, one directory per
// test case, with expected diagnostics recorded as (code, offset, length)
// rows. The corpus reproduces the official harness environment: a package
// named `test` whose `lib/test.dart` holds the case code, analysis options
// enabling the single rule under test, and per-class setup groups (meta
// dependency, language version 2.19, experiments).
//
// Usage:
//   dart run bin/port_linter_tests.dart port <rulesDir> <corpusDir>
//   dart run bin/port_linter_tests.dart run <corpusDir> <outDir> \
//       --dart <dartBin> --dartq <dartqBin> --sdk <sdkDir> [--only <pkgName>]
//   dart run bin/port_linter_tests.dart report <outDir/results.json> <outMd>
//
// `port` parses the upstream test sources, materializes the corpus packages
// and writes <corpusDir>/manifest.json. `run` executes the oracle and dartq
// once per package (serially, with an RSS watchdog) and compares rows per
// case: fidelity (expectations vs oracle) and parity (dartq vs oracle, no
// filtering). `report` renders the markdown baseline report.

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:analyzer/dart/analysis/utilities.dart';
import 'package:analyzer/dart/ast/ast.dart';
import 'package:analyzer/dart/ast/visitor.dart';

const _usage =
    'usage: port_linter_tests.dart port <rulesDir> <corpusDir> | '
    'run <corpusDir> <outDir> --dart <bin> --dartq <bin> --sdk <dir> '
    '[--only <pkg>] | report <results.json> <outMd>';

/// The `dart` executable running this script.
String dartBinPath() => Platform.executable;

Future<void> main(List<String> args) async {
  if (args.isEmpty) _fail(_usage);
  final watch = Stopwatch()..start();
  switch (args[0]) {
    case 'port':
      if (args.length != 3) _fail(_usage);
      port(args[1], args[2]);
    case 'run':
      await runCmd(args.sublist(1), watch);
    case 'report':
      if (args.length != 3) _fail(_usage);
      report(args[1], args[2]);
    default:
      _fail(_usage);
  }
}

Never _fail(String message) {
  stderr.writeln(message);
  exit(2);
}

// ---------------------------------------------------------------------------
// Markup format (ported from the upstream test_code_format.dart grammar):
//   position      ::= '/*' int '*/'        shorthand '^' (position 0)
//   rangeStart    ::= '/*[' int '*/'       shorthand '[!' (range 0 start)
//   rangeEnd      ::= '/*' int ']*/'       shorthand '!]' (range 0 end)
//   zeroWidth     ::= '/**/' (removed)     '^/**/' -> literal '^'
// Patterns are tried at each position in the upstream order.
// ---------------------------------------------------------------------------

class MarkupRange {
  final int offset;
  final int length;
  const MarkupRange(this.offset, this.length);
}

class ParsedMarkup {
  final String code;
  final List<MarkupRange> ranges;
  const ParsedMarkup(this.code, this.ranges);
}

final _rePosition = RegExp(r'/\*(\d+)\*/');
final _reRangeStart = RegExp(r'/\*\[(\d+)\*/');
final _reRangeEnd = RegExp(r'/\*(\d+)\]\*/');

ParsedMarkup? parseMarkup(String marked) {
  final code = StringBuffer();
  final posOffsets = <int, int>{};
  final rangeStart = <int, int>{};
  final rangeEnd = <int, int>{};
  var i = 0;
  int start() => code.length;
  bool scan(Pattern p) {
    final m = p.matchAsPrefix(marked, i);
    if (m == null) return false;
    i = m.end;
    return true;
  }

  while (i < marked.length) {
    if (scan('^/**/')) {
      code.write('^');
    } else if (scan('^')) {
      if (posOffsets.containsKey(0)) return null;
      posOffsets[0] = start();
    } else if (_rePosition.matchAsPrefix(marked, i) case final m?) {
      final n = int.parse(m.group(1)!);
      if (posOffsets.containsKey(n) || n > posOffsets.length) return null;
      posOffsets[n] = start();
      i = m.end;
    } else if (scan('[!')) {
      if (rangeStart.containsKey(0)) return null;
      rangeStart[0] = start();
    } else if (scan('!]')) {
      if (!rangeStart.containsKey(0)) return null;
      rangeEnd[0] = start();
    } else if (_reRangeStart.matchAsPrefix(marked, i) case final m?) {
      final n = int.parse(m.group(1)!);
      if (rangeStart.containsKey(n) || n > rangeStart.length) return null;
      rangeStart[n] = start();
      i = m.end;
    } else if (_reRangeEnd.matchAsPrefix(marked, i) case final m?) {
      final n = int.parse(m.group(1)!);
      if (!rangeStart.containsKey(n) || rangeEnd.containsKey(n)) return null;
      rangeEnd[n] = start();
      i = m.end;
    } else if (scan('/**/')) {
      // removed
    } else {
      code.writeCharCode(marked.codeUnitAt(i));
      i++;
    }
  }
  for (final n in rangeStart.keys) {
    if (!rangeEnd.containsKey(n)) return null;
  }
  final text = code.toString();
  final ranges = <MarkupRange>[];
  for (final n in rangeStart.keys) {
    ranges.add(MarkupRange(rangeStart[n]!, rangeEnd[n]! - rangeStart[n]!));
  }
  ranges.sort((a, b) => a.offset.compareTo(b.offset));
  return ParsedMarkup(text, ranges);
}

// ---------------------------------------------------------------------------
// Extraction models
// ---------------------------------------------------------------------------

class ExpRow {
  final String code;
  final int offset;
  final int length;
  const ExpRow(this.code, this.offset, this.length);
}

enum TargetKind { main, libFile, testDir, hook, bin, rootFile }

class CaseTarget {
  final TargetKind kind;
  final String rel;
  const CaseTarget(this.kind, this.rel);
}

class PortCase {
  final String source; // 'File.dart#Class.method'
  final CaseTarget target;
  final String code;
  final List<ExpRow> rows;
  final Map<String, String> companions; // libRel -> content
  const PortCase(
    this.source,
    this.target,
    this.code,
    this.rows,
    this.companions,
  );
}

class ClassInfo {
  final String className;
  final String rule;
  final bool meta;
  final bool v219;
  final List<String> experiments;
  final Map<String, String> classCompanions; // libRel -> content
  ClassInfo(
    this.className,
    this.rule,
    this.meta,
    this.v219,
    this.experiments,
    this.classCompanions,
  );
}

class ExtractResult {
  final List<(ClassInfo, PortCase)> cases = [];
  final Map<String, int> caseSkips = {};
  final Map<String, int> classSkips = {};
  final List<String> parseErrorFiles = [];
  final Set<String> rules = {};
  int files;
  ExtractResult(this.files);
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

const _pathVars = {
  'testPackageLibPath': 'lib',
  'testPackageRootPath': 'root',
  'testPackageTestPath': 'testdir',
  'testPackagePubspecPath': 'pubspec',
};

const _denyMethods = {
  'newPackage': 'setup_new_package',
  'PackageConfigFileBuilder': 'setup_packageconfig',
  'newPackageConfigJsonFileFromBuilder': 'setup_packageconfig',
  'writeTestPackageConfig': 'setup_packageconfig',
  'writeTestPackageConfig2': 'setup_packageconfig',
  'resolveFile': 'unsupported_api',
  'getResolvedUnit': 'unsupported_api',
  'assertDiagnosticsInUnits': 'unsupported_api',
  'applyPendingFileChanges': 'unsupported_api',
  'PhysicalResourceProvider': 'setup_external',
};

/// Resolves a `newFile(path, content)` first argument to a logical location.
/// Returns (kind, rel) or null when unresolvable.
(String, String)? resolveNewFilePath(Expression arg) {
  if (arg is SimpleStringLiteral) {
    final p = arg.value;
    if (p.startsWith('/home/test/lib/')) return ('lib', p.substring(15));
    if (p.startsWith('/home/test/')) return ('root', p.substring(11));
    return null;
  }
  if (arg is StringInterpolation) {
    final els = arg.elements;
    if (els.isEmpty) return null;
    String prefixVar = '';
    var idx = 0;
    if (els[0] is InterpolationString) {
      final s = (els[0] as InterpolationString).value;
      if (s.isNotEmpty) return null;
      idx = 1;
    }
    if (idx >= els.length || els[idx] is! InterpolationExpression) return null;
    final expr = (els[idx] as InterpolationExpression).expression;
    if (expr is! SimpleIdentifier || !_pathVars.containsKey(expr.name)) {
      return null;
    }
    prefixVar = _pathVars[expr.name]!;
    idx++;
    final suffix = StringBuffer();
    for (; idx < els.length; idx++) {
      final el = els[idx];
      if (el is InterpolationString) {
        suffix.write(el.value);
      } else {
        return null;
      }
    }
    var rel = suffix.toString();
    // The suffix continues the path with '/', e.g. '$testPackageLibPath/a.dart'.
    while (rel.startsWith('/')) {
      rel = rel.substring(1);
    }
    return switch (prefixVar) {
      'lib' => ('lib', rel),
      'root' => ('root', rel),
      'testdir' => ('testdir', rel),
      'pubspec' => ('pubspec', rel),
      _ => null,
    };
  }
  if (arg is SimpleIdentifier) {
    if (arg.name == 'testFilePath') return ('main', 'test.dart');
    return null;
  }
  return null;
}

String? stringLitValue(
  Expression e,
  String skipInterpolated,
  Map<String, int> skips,
  String skipKey,
) {
  if (e is SimpleStringLiteral) return e.stringValue;
  if (e is AdjacentStrings) return e.stringValue;
  if (e is StringInterpolation) {
    skips[skipInterpolated] = (skips[skipInterpolated] ?? 0) + 1;
  } else {
    skips[skipKey] = (skips[skipKey] ?? 0) + 1;
  }
  return null;
}

String camelToSnake(String name) {
  final b = StringBuffer();
  for (var i = 0; i < name.length; i++) {
    final c = name[i];
    final isUp = c.toUpperCase() == c && c.toLowerCase() != c;
    if (isUp && i > 0) {
      final prev = name[i - 1];
      final prevLower = prev.toLowerCase() != prev;
      final next = i + 1 < name.length ? name[i + 1] : '';
      final nextLower = next.isNotEmpty && next.toLowerCase() != next;
      if (!prevLower || (prevLower && nextLower)) b.write('_');
    }
    b.write(c.toLowerCase());
  }
  return b.toString();
}

ExtractResult extractRules(String rulesDir) {
  final res = ExtractResult(0);
  final files =
      Directory(rulesDir)
          .listSync()
          .whereType<File>()
          .where((f) => f.path.endsWith('_test.dart'))
          .toList()
        ..sort((a, b) => a.path.compareTo(b.path));
  for (final file in files) {
    res.files++;
    final source = File(file.path).readAsStringSync();
    final unit = parseString(content: source, throwIfDiagnostics: false);
    if (unit.errors.isNotEmpty) {
      res.parseErrorFiles.add(file.uri.pathSegments.last);
    }

    final classes = <String, ClassDeclaration>{};
    for (final decl in unit.unit.declarations) {
      if (decl is ClassDeclaration) {
        classes[decl.namePart.typeName.lexeme] = decl;
      }
    }

    for (final decl in classes.values) {
      final chain = _baseChain(decl, classes);
      if (chain == null) {
        res.classSkips['base_not_lint_rule_test'] =
            (res.classSkips['base_not_lint_rule_test'] ?? 0) + 1;
        continue;
      }
      final info = _classInfo(
        chain,
        res.classSkips,
        file.uri.pathSegments.last,
      );
      if (info == null) continue;
      res.rules.add(info.rule);
      for (final member in decl.body.members.whereType<MethodDeclaration>()) {
        final name = member.name.lexeme;
        if (!name.startsWith('test_')) continue;
        if (member.parameters != null &&
            member.parameters!.parameters.any(
              (p) => p.isNamed || p.isRequired,
            )) {
          res.caseSkips['helper_signature'] =
              (res.caseSkips['helper_signature'] ?? 0) + 1;
          continue;
        }
        if (member.metadata.any(
          (m) =>
              m.name.name.endsWith('FailingTest') ||
              m.name.name.endsWith('SkippedTest'),
        )) {
          res.caseSkips['failing_test'] =
              (res.caseSkips['failing_test'] ?? 0) + 1;
          continue;
        }
        final kase = _extractCase(info, name, member, res.caseSkips);
        if (kase != null) res.cases.add((info, kase));
      }
    }
  }
  return res;
}

List<ClassDeclaration>? _baseChain(
  ClassDeclaration decl,
  Map<String, ClassDeclaration> classes,
) {
  final chain = <ClassDeclaration>[decl];
  var cur = decl;
  while (true) {
    final sup = cur.extendsClause?.superclass;
    if (sup == null) return null;
    final name = sup.name.lexeme;
    if (name == 'LintRuleTest') return chain;
    final next = classes[name];
    if (next == null) return null;
    if (chain.contains(next)) return null;
    chain.add(next);
    cur = next;
  }
}

ClassInfo? _classInfo(
  List<ClassDeclaration> chain,
  Map<String, int> skips,
  String fileName,
) {
  var rule = '';
  var meta = false;
  var v219 = false;
  final experiments = <String>[];
  final classCompanions = <String, String>{};
  for (final decl in chain) {
    for (final m in decl.withClause?.mixinTypes ?? const <NamedType>[]) {
      if (m.name.lexeme == 'LanguageVersion219Mixin') {
        v219 = true;
      } else {
        skips['setup_mixin_${m.name.lexeme}'] =
            (skips['setup_mixin_${m.name.lexeme}'] ?? 0) + 1;
        return null;
      }
    }
    for (final member in decl.body.members) {
      if (member is MethodDeclaration && member.name.lexeme == 'lintRule') {
        final body = member.body;
        if (body is ExpressionFunctionBody) {
          final e = body.expression;
          if (e is SimpleStringLiteral || e is AdjacentStrings) {
            rule = (e as StringLiteral).stringValue ?? '';
          } else if (e is PrefixedIdentifier && e.prefix.name == 'LintNames') {
            rule = e.identifier.name;
          }
        }
      } else if (member is MethodDeclaration &&
          member.name.lexeme == 'addMetaPackageDep') {
        meta = true;
      } else if (member is MethodDeclaration &&
          member.name.lexeme == 'addFlutterPackageDep') {
        skips['setup_flutter'] = (skips['setup_flutter'] ?? 0) + 1;
        return null;
      } else if (member is MethodDeclaration &&
          (member.name.lexeme == 'addFixnumPackageDep' ||
              member.name.lexeme == 'addTestReflectiveLoaderPackageDep')) {
        skips['setup_extra_packages'] =
            (skips['setup_extra_packages'] ?? 0) + 1;
        return null;
      } else if (member is MethodDeclaration &&
          member.name.lexeme == 'experimentalFeatures') {
        final body = member.body;
        if (body is ExpressionFunctionBody && body.expression is ListLiteral) {
          for (final el in (body.expression as ListLiteral).elements) {
            if (el is SpreadElement) {
              // `...super.experimentalFeatures` and similar — the default
              // (empty) set; nothing to record.
              continue;
            }
            if (el is! Expression) {
              skips['setup_experiments'] =
                  (skips['setup_experiments'] ?? 0) + 1;
              return null;
            }
            final e = el;
            if (e is PrefixedIdentifier && e.prefix.name == 'Feature') {
              // Feature constant names are snake_case; experiment flags are
              // the kebab-case form (verified against the Feature registry).
              final flag = e.identifier.name.toLowerCase().replaceAll('_', '-');
              if (!experiments.contains(flag)) experiments.add(flag);
            } else {
              skips['setup_experiments'] =
                  (skips['setup_experiments'] ?? 0) + 1;
              return null;
            }
          }
        } else {
          skips['setup_experiments'] = (skips['setup_experiments'] ?? 0) + 1;
          return null;
        }
      } else if (member is MethodDeclaration &&
          member.name.lexeme == 'setUp' &&
          !member.isStatic) {
        final r = _setUpInfo(member, skips);
        if (r == null) return null;
        classCompanions.addAll(r);
      }
    }
  }
  if (rule.isEmpty) {
    skips['no_rule_name'] = (skips['no_rule_name'] ?? 0) + 1;
    return null;
  }
  return ClassInfo(
    chain.first.namePart.typeName.lexeme,
    rule,
    meta,
    v219,
    experiments,
    classCompanions,
  );
}

Map<String, String>? _setUpInfo(
  MethodDeclaration setUp,
  Map<String, int> skips,
) {
  final companions = <String, String>{};
  final visitor = _InvocationCollector();
  setUp.body.visitChildren(visitor);
  for (final node in visitor.nodes) {
    if (node is MethodInvocation) {
      final n = node.methodName.name;
      if (n == 'setUp' && node.target is SuperExpression) continue;
      if (_denyMethods.containsKey(n)) {
        skips[_denyMethods[n]!] = (skips[_denyMethods[n]!] ?? 0) + 1;
        return null;
      }
      if (n == 'newFile' || n == 'newFile2') {
        if (node.argumentList.arguments.isEmpty) {
          skips['setup_setup'] = (skips['setup_setup'] ?? 0) + 1;
          return null;
        }
        final loc = resolveNewFilePath(
          node.argumentList.arguments[0].argumentExpression,
        );
        final content = node.argumentList.arguments.length > 1
            ? stringLitValue(
                node.argumentList.arguments[1].argumentExpression,
                'interpolated_companion',
                skips,
                'non_literal_companion',
              )
            : null;
        if (loc == null || content == null) {
          skips['setup_setup'] = (skips['setup_setup'] ?? 0) + 1;
          return null;
        }
        final (kind, rel) = loc;
        if (kind == 'pubspec') {
          skips['setup_pubspec'] = (skips['setup_pubspec'] ?? 0) + 1;
          return null;
        }
        if (kind != 'lib') {
          skips['setup_setup'] = (skips['setup_setup'] ?? 0) + 1;
          return null;
        }
        companions[rel] = content;
        continue;
      }
      skips['setup_setup:$n'] = (skips['setup_setup:$n'] ?? 0) + 1;
      return null;
    } else if (node is InstanceCreationExpression) {
      final n = node.constructorName.type.name.lexeme;
      if (_denyMethods.containsKey(n)) {
        skips[_denyMethods[n]!] = (skips[_denyMethods[n]!] ?? 0) + 1;
        return null;
      }
    }
  }
  return companions;
}

class _InvocationCollector extends RecursiveAstVisitor<void> {
  final List<AstNode> nodes = [];
  @override
  void visitMethodInvocation(MethodInvocation node) {
    nodes.add(node);
    super.visitMethodInvocation(node);
  }

  @override
  void visitInstanceCreationExpression(InstanceCreationExpression node) {
    nodes.add(node);
    super.visitInstanceCreationExpression(node);
  }
}

PortCase? _extractCase(
  ClassInfo info,
  String method,
  MethodDeclaration member,
  Map<String, int> skips,
) {
  final skip = (String key) => skips[key] = (skips[key] ?? 0) + 1;
  final visitor = _InvocationCollector();
  member.body.visitChildren(visitor);

  // Denylist scan over the whole method body.
  for (final node in visitor.nodes) {
    String? n;
    if (node is MethodInvocation) n = node.methodName.name;
    if (node is InstanceCreationExpression) {
      n = node.constructorName.type.name.lexeme;
    }
    if (n != null && _denyMethods.containsKey(n)) {
      skip(_denyMethods[n]!);
      return null;
    }
  }

  // var bindings: `var x = newFile(path, content);`
  final varPath = <String, (String, String)>{};
  final companions = <String, String>{};
  final assertOffset = _firstAssertOffset(visitor.nodes);
  for (final node in visitor.nodes) {
    if (node is! MethodInvocation) continue;
    final n = node.methodName.name;
    if (n != 'newFile' && n != 'newFile2') continue;
    final args = node.argumentList.arguments;
    if (args.length < 2) continue;
    final loc = resolveNewFilePath(args[0].argumentExpression);
    final content = stringLitValue(
      args[1].argumentExpression,
      'interpolated_companion',
      skips,
      'non_literal_companion',
    );
    if (loc == null) {
      skip('unresolved_companion');
      continue;
    }
    if (content == null) {
      skip('non_literal_companion');
      continue;
    }
    if (loc.$1 == 'pubspec') {
      skip('pubspec_rewrite');
      return null;
    }
    // Bind the path to the variable name if inside a variable declaration.
    AstNode? p = node.parent;
    while (p != null && p is! VariableDeclaration && p is! Statement) {
      p = p.parent;
    }
    if (p is VariableDeclaration) {
      varPath[p.name.lexeme] = loc;
    }
    if (assertOffset != null && node.offset < assertOffset) {
      if (loc.$1 == 'lib') {
        companions[loc.$2] = content;
      } else if (loc.$1 == 'root') {
        companions['\u0000root\u0000${loc.$2}'] = content;
      }
    }
  }

  // First supported assertion in source order.
  final asserts =
      visitor.nodes
          .whereType<MethodInvocation>()
          .where((m) => _assertKind(m.methodName.name) != null)
          .toList()
        ..sort((a, b) => a.offset.compareTo(b.offset));
  if (asserts.isEmpty) {
    skip('no_assert');
    return null;
  }
  final call = asserts.first;
  final kind = _assertKind(call.methodName.name)!;
  final args = call.argumentList.arguments;

  String? code;
  List<ExpRow> rows = const [];
  CaseTarget target = const CaseTarget(TargetKind.main, 'test.dart');

  switch (kind) {
    case 'assertDiagnostics':
    case 'assertNoDiagnostics':
      code = stringLitValue(
        args[0].argumentExpression,
        'interpolated',
        skips,
        'non_literal',
      );
      if (code == null) return null;
      if (kind == 'assertDiagnostics' && args.length > 1) {
        final r = _expectList(args[1].argumentExpression, info.rule, skips);
        if (r == null) return null;
        rows = r;
      }
    case 'assertDiagnosticsFromMarkup':
      code = stringLitValue(
        args[0].argumentExpression,
        'interpolated',
        skips,
        'non_literal',
      );
      if (code == null) return null;
      final parsed = parseMarkup(code);
      if (parsed == null) {
        skip('markup_parse');
        return null;
      }
      code = parsed.code;
      rows = [
        for (final r in parsed.ranges) ExpRow(info.rule, r.offset, r.length),
      ];
    case 'assertDiagnosticsInFileNameFromMarkup':
    case 'assertNoDiagnosticsInFileName':
      if (args.length < 2) {
        skip('unsupported_assert');
        return null;
      }
      final fileName = stringLitValue(
        args[0].argumentExpression,
        'x',
        skips,
        'x',
      );
      code = stringLitValue(
        args[1].argumentExpression,
        'interpolated',
        skips,
        'non_literal',
      );
      if (fileName == null || code == null) {
        skip('unsupported_assert');
        return null;
      }
      if (kind == 'assertDiagnosticsInFileNameFromMarkup') {
        final parsed = parseMarkup(code);
        if (parsed == null) {
          skip('markup_parse');
          return null;
        }
        code = parsed.code;
        rows = [
          for (final r in parsed.ranges) ExpRow(info.rule, r.offset, r.length),
        ];
      }
      target = CaseTarget(TargetKind.libFile, fileName);
    case 'assertDiagnosticsInTestDirFromMarkup':
    case 'assertNoDiagnosticsInTestDir':
      final src = stringLitValue(
        args[0].argumentExpression,
        'interpolated',
        skips,
        'non_literal',
      );
      if (src == null) return null;
      if (kind == 'assertDiagnosticsInTestDirFromMarkup') {
        final parsed = parseMarkup(src);
        if (parsed == null) {
          skip('markup_parse');
          return null;
        }
        code = parsed.code;
        rows = [
          for (final r in parsed.ranges) ExpRow(info.rule, r.offset, r.length),
        ];
      } else {
        code = src;
      }
      target = const CaseTarget(TargetKind.testDir, 'test.dart');
    case 'assertDiagnosticsInHookFromMarkup':
    case 'assertNoDiagnosticsInHook':
      if (args.length < 2) {
        skip('unsupported_assert');
        return null;
      }
      final fileName = stringLitValue(
        args[0].argumentExpression,
        'x',
        skips,
        'x',
      );
      final src = stringLitValue(
        args[1].argumentExpression,
        'interpolated',
        skips,
        'non_literal',
      );
      if (fileName == null || src == null) {
        skip('unsupported_assert');
        return null;
      }
      if (kind == 'assertDiagnosticsInHookFromMarkup') {
        final parsed = parseMarkup(src);
        if (parsed == null) {
          skip('markup_parse');
          return null;
        }
        code = parsed.code;
        rows = [
          for (final r in parsed.ranges) ExpRow(info.rule, r.offset, r.length),
        ];
      } else {
        code = src;
      }
      target = CaseTarget(TargetKind.hook, fileName);
    case 'assertDiagnosticsInBinFromMarkup':
      final src = stringLitValue(
        args[0].argumentExpression,
        'interpolated',
        skips,
        'non_literal',
      );
      if (src == null) return null;
      final parsed = parseMarkup(src);
      if (parsed == null) {
        skip('markup_parse');
        return null;
      }
      code = parsed.code;
      rows = [
        for (final r in parsed.ranges) ExpRow(info.rule, r.offset, r.length),
      ];
      target = const CaseTarget(TargetKind.bin, 'bin.dart');
    case 'assertNoDiagnosticsInFile':
    case 'assertDiagnosticsInFile':
      if (args.isEmpty) {
        skip('unsupported_assert');
        return null;
      }
      final arg = args[0].argumentExpression;
      String? varName;
      if (arg is PrefixedIdentifier && arg.identifier.name == 'path') {
        varName = arg.prefix.name;
      } else if (arg is SimpleIdentifier) {
        varName = arg.name;
      } else if (arg is MethodInvocation && arg.methodName.name == 'path') {
        final t = arg.target;
        if (t is SimpleIdentifier) varName = t.name;
      }
      final bound = varName == null ? null : varPath[varName];
      if (bound == null) {
        skip('unresolved_in_file');
        return null;
      }
      final (bkind, brel) = bound;
      if (bkind == 'main') {
        target = const CaseTarget(TargetKind.main, 'test.dart');
      } else if (bkind == 'lib') {
        target = CaseTarget(TargetKind.libFile, brel);
      } else if (bkind == 'root') {
        target = CaseTarget(TargetKind.rootFile, brel);
      } else {
        skip('unresolved_in_file');
        return null;
      }
      if (kind == 'assertDiagnosticsInFile') {
        if (args.length < 2) {
          skip('unsupported_assert');
          return null;
        }
        final r = _expectList(args[1].argumentExpression, info.rule, skips);
        if (r == null) return null;
        rows = r;
      } else {
        rows = const [];
      }
      // The bound file's content was captured as a companion above.
      code = companions[brel] ?? companions['\u0000root\u0000$brel'];
      if (code == null) {
        skip('unresolved_in_file');
        return null;
      }
      // Remove from companions: it becomes the case target.
      companions.remove(brel);
      companions.remove('\u0000root\u0000$brel');
      if (bkind == 'root') {
        // Root-based targets stay at the package root (shared); collisions
        // across cases are detected during materialization.
        target = CaseTarget(TargetKind.rootFile, brel);
      }
  }

  // Drop companions equal to the target file content (same file).
  return PortCase('${info.className}.$method', target, code!, rows, companions);
}

final Map<String, String> _assertKinds = {
  'assertDiagnostics': 'assertDiagnostics',
  'assertNoDiagnostics': 'assertNoDiagnostics',
  'assertDiagnosticsFromMarkup': 'assertDiagnosticsFromMarkup',
  'assertDiagnosticsInFileNameFromMarkup':
      'assertDiagnosticsInFileNameFromMarkup',
  'assertDiagnosticsInTestDirFromMarkup':
      'assertDiagnosticsInTestDirFromMarkup',
  'assertDiagnosticsInHookFromMarkup': 'assertDiagnosticsInHookFromMarkup',
  'assertDiagnosticsInBinFromMarkup': 'assertDiagnosticsInBinFromMarkup',
  'assertNoDiagnosticsInFileName': 'assertNoDiagnosticsInFileName',
  'assertNoDiagnosticsInTestDir': 'assertNoDiagnosticsInTestDir',
  'assertNoDiagnosticsInHook': 'assertNoDiagnosticsInHook',
  'assertNoDiagnosticsInFile': 'assertNoDiagnosticsInFile',
  'assertDiagnosticsInFile': 'assertDiagnosticsInFile',
};

String? _assertKind(String name) => _assertKinds[name];

int? _firstAssertOffset(List<AstNode> nodes) {
  final asserts =
      nodes
          .whereType<MethodInvocation>()
          .where((m) => _assertKind(m.methodName.name) != null)
          .toList()
        ..sort((a, b) => a.offset.compareTo(b.offset));
  return asserts.isEmpty ? null : asserts.first.offset;
}

List<ExpRow>? _expectList(Expression arg, String rule, Map<String, int> skips) {
  if (arg is ListLiteral) {
    final rows = <ExpRow>[];
    for (final el in arg.elements) {
      if (el is! MethodInvocation) {
        skips['unsupported_expect'] = (skips['unsupported_expect'] ?? 0) + 1;
        return null;
      }
      final call = el;
      final args = call.argumentList.arguments;
      if (call.methodName.name == 'lint' && args.length >= 2) {
        final a0 = args[0].argumentExpression;
        final a1 = args[1].argumentExpression;
        final o = a0 is IntegerLiteral ? a0.value : null;
        final l = a1 is IntegerLiteral ? a1.value : null;
        if (o == null || l == null) {
          skips['unsupported_expect'] = (skips['unsupported_expect'] ?? 0) + 1;
          return null;
        }
        var name = rule;
        for (final a in args.skip(2)) {
          if (a is NamedArgument && a.name.lexeme == 'name') {
            final v = a.argumentExpression;
            if (v is SimpleStringLiteral) name = v.value;
          }
        }
        rows.add(ExpRow(name, o, l));
      } else if (call.methodName.name == 'error' && args.length >= 3) {
        final c = args[0].argumentExpression;
        String codeName;
        if (c is PrefixedIdentifier) {
          codeName = camelToSnake(c.identifier.name);
        } else if (c is SimpleIdentifier) {
          codeName = camelToSnake(c.name);
        } else {
          skips['unsupported_expect'] = (skips['unsupported_expect'] ?? 0) + 1;
          return null;
        }
        final a1 = args[1].argumentExpression;
        final a2 = args[2].argumentExpression;
        final o = a1 is IntegerLiteral ? a1.value : null;
        final l = a2 is IntegerLiteral ? a2.value : null;
        if (o == null || l == null) {
          skips['unsupported_expect'] = (skips['unsupported_expect'] ?? 0) + 1;
          return null;
        }
        rows.add(ExpRow(codeName, o, l));
      } else {
        skips['unsupported_expect'] = (skips['unsupported_expect'] ?? 0) + 1;
        return null;
      }
    }
    return rows;
  }
  skips['unsupported_expect'] = (skips['unsupported_expect'] ?? 0) + 1;
  return null;
}

// ---------------------------------------------------------------------------
// Materialization
// ---------------------------------------------------------------------------

class _Rewrite {
  final String text;
  final List<(int, int, int)> edits; // (position, oldLen, newLen)
  const _Rewrite(this.text, this.edits);
}

/// Rewrites `package:test/<rel>` references for case-local companion files so
/// they resolve inside the case directory `c<i>/`. A reference only counts
/// when directly followed by a quote (end of a URI string literal).
_Rewrite _applyRewrites(String code, List<(String, String)> rules) {
  final edits = <(int, int, int)>[];
  final sb = StringBuffer();
  var i = 0;
  outer:
  while (i < code.length) {
    for (var r = 0; r < rules.length; r++) {
      final (find, replace) = rules[r];
      if (code.startsWith(find, i)) {
        final after = i + find.length;
        if (after >= code.length || code[after] == "'" || code[after] == '"') {
          edits.add((i, find.length, replace.length));
          sb.write(replace);
          i = after;
          continue outer;
        }
      }
    }
    sb.writeCharCode(code.codeUnitAt(i));
    i++;
  }
  return _Rewrite(sb.toString(), edits);
}

int _adjustOffset(int offset, List<(int, int, int)> edits) {
  var d = 0;
  for (final (pos, oldL, newL) in edits) {
    if (pos + oldL <= offset) d += newL - oldL;
  }
  return offset + d;
}

String _targetPath(TargetKind kind, String rel, int i) => switch (kind) {
  TargetKind.main => 'lib/c$i/test.dart',
  TargetKind.libFile => 'lib/c$i/$rel',
  TargetKind.testDir => 'test/c$i/test.dart',
  TargetKind.hook => 'hook/c$i/$rel',
  TargetKind.bin => 'bin/c$i/bin.dart',
  TargetKind.rootFile => rel,
};

void port(String rulesDir, String corpusDir) async {
  final res = extractRules(rulesDir);
  final portStats = {
    'files': res.files,
    'rules': res.rules.length,
    'cases': res.cases.length,
    'caseSkips': res.caseSkips,
    'classSkips': res.classSkips,
    'parseErrorFiles': res.parseErrorFiles,
  };

  // Group cases by (rule, setup group).
  final order = <String>[];
  final groups = <String, List<(ClassInfo, PortCase)>>{};
  for (final (info, pc) in res.cases) {
    final key =
        info.rule + (info.meta ? '__meta' : '') + (info.v219 ? '__v219' : '');
    if (!groups.containsKey(key)) order.add(key);
    groups.putIfAbsent(key, () => []).add((info, pc));
  }
  order.sort();

  Directory(corpusDir).createSync(recursive: true);
  final packages = <Map<String, Object?>>[];
  for (final key in order) {
    final entries = groups[key]!;
    final info = entries.first.$1;
    final pkgDir = Directory('${corpusDir == '' ? '.' : corpusDir}/$key');
    pkgDir.createSync(recursive: true);

    // The harness writes no sdk constraint (language version = latest).
    // pub requires a lower bound, so pin the running SDK's language version;
    // 2.19-mixin groups keep their explicit language.
    final sdkLine = info.v219
        ? "environment:\n  sdk: '>=2.19.0 <4.0.0'\n"
        : "environment:\n  sdk: '^3.13.0'\n";
    final pubspec = StringBuffer('name: test\n$sdkLine');
    if (info.meta) {
      pubspec.write('dependencies:\n  meta: any\n');
    }
    File('${pkgDir.path}/pubspec.yaml').writeAsStringSync(pubspec.toString());

    final options = StringBuffer('analyzer:\n');
    if (info.experiments.isNotEmpty) {
      options.write('  enable-experiment:\n');
      for (final e in info.experiments) {
        options.write('    - $e\n');
      }
    }
    options.write('  optional-checks:\n');
    options.write('    propagate-linter-exceptions: true\n');
    options.write('linter:\n  rules:\n    - ${info.rule}\n');
    File(
      '${pkgDir.path}/analysis_options.yaml',
    ).writeAsStringSync(options.toString());

    final rootContent = <String, String>{};
    final caseJson = <Map<String, Object?>>[];
    for (var i = 0; i < entries.length; i++) {
      final (ci, pc) = entries[i];

      // Root-level companion collision detection.
      var skip = false;
      final rootRels = <String>{};
      for (final rel in pc.companions.keys) {
        if (rel.startsWith('\u0000root\u0000')) rootRels.add(rel);
      }
      if (pc.target.kind == TargetKind.rootFile) {
        rootRels.add('\u0000root\u0000${pc.target.rel}');
      }
      for (final rel in rootRels) {
        final plain = rel.substring('\u0000root\u0000'.length);
        final content = rel == '\u0000root\u0000${pc.target.rel}'
            ? pc.code
            : pc.companions[rel]!;
        final existing = rootContent[plain];
        if (existing == null) {
          rootContent[plain] = content;
        } else if (existing != content) {
          res.caseSkips['companion_collision'] =
              (res.caseSkips['companion_collision'] ?? 0) + 1;
          skip = true;
        }
      }
      if (skip) continue;

      // Rewrite rules: case-local files referenced via package:test/.
      final rewriteRules = <(String, String)>[
        ('package:test/test.dart', 'package:test/c$i/test.dart'),
        for (final rel in pc.companions.keys.where(
          (r) => !r.startsWith('\u0000root\u0000'),
        ))
          ('package:test/$rel', 'package:test/c$i/$rel'),
        ...ci.classCompanions.keys.map(
          (rel) => ('package:test/$rel', 'package:test/c$i/$rel'),
        ),
      ];
      // Keep longest-first so prefix paths cannot shadow each other.
      rewriteRules.sort((a, b) => b.$1.length.compareTo(a.$1.length));

      final rewritten = _applyRewrites(pc.code, rewriteRules);
      final rows = [
        for (final r in pc.rows)
          [r.code, _adjustOffset(r.offset, rewritten.edits), r.length],
      ];

      final targetRel = _targetPath(pc.target.kind, pc.target.rel, i);
      final files = <String>[targetRel];
      void writeCaseFile(String relInCase, String content) {
        final f = File('${pkgDir.path}/lib/c$i/$relInCase');
        f.parent.createSync(recursive: true);
        f.writeAsStringSync(content);
        final p = 'lib/c$i/$relInCase';
        if (!files.contains(p)) files.add(p);
      }

      if (pc.target.kind == TargetKind.rootFile) {
        final f = File('${pkgDir.path}/${pc.target.rel}');
        f.parent.createSync(recursive: true);
        f.writeAsStringSync(rewritten.text);
      } else {
        final f = File('${pkgDir.path}/$targetRel');
        f.parent.createSync(recursive: true);
        f.writeAsStringSync(rewritten.text);
      }

      for (final entry in pc.companions.entries) {
        if (entry.key.startsWith('\u0000root\u0000')) {
          final plain = entry.key.substring('\u0000root\u0000'.length);
          final f = File('${pkgDir.path}/$plain');
          f.parent.createSync(recursive: true);
          f.writeAsStringSync(_applyRewrites(entry.value, rewriteRules).text);
          if (!files.contains(plain)) files.add(plain);
        } else {
          writeCaseFile(
            entry.key,
            _applyRewrites(entry.value, rewriteRules).text,
          );
        }
      }
      for (final entry in ci.classCompanions.entries) {
        if (!pc.companions.containsKey(entry.key)) {
          writeCaseFile(
            entry.key,
            _applyRewrites(entry.value, rewriteRules).text,
          );
        }
      }

      caseJson.add({
        'id': '$key#c$i',
        'source': pc.source,
        'target': targetRel,
        'files': files,
        'rows': rows,
      });
    }

    if (caseJson.isEmpty) {
      // Nothing materialized (all cases skipped); drop the package skeleton.
      pkgDir.deleteSync(recursive: true);
      continue;
    }
    packages.add({
      'name': key,
      'rule': info.rule,
      'meta': info.meta,
      'v219': info.v219,
      'experiments': info.experiments,
      'cases': caseJson,
    });
  }

  // Resolve dependencies for every package (offline first).
  final pubgetStatus = <String, String>{};
  for (final pkg in packages) {
    final name = pkg['name'] as String;
    final dir = '$corpusDir/$name';
    var r = await Process.run(dartBinPath(), [
      'pub',
      'get',
      '--offline',
    ], workingDirectory: dir);
    if (r.exitCode != 0) {
      r = await Process.run(dartBinPath(), [
        'pub',
        'get',
      ], workingDirectory: dir);
      pubgetStatus[name] = r.exitCode == 0 ? 'online' : 'failed';
    } else {
      pubgetStatus[name] = 'offline';
    }
  }

  final manifest = {
    'portStats': portStats,
    'pubgetStatus': pubgetStatus,
    'packages': packages,
  };
  File(
    '$corpusDir/manifest.json',
  ).writeAsStringSync(const JsonEncoder.withIndent('  ').convert(manifest));
  final skips = res.caseSkips.values.fold<int>(0, (a, b) => a + b);
  stderr.writeln(
    'port: ${res.files} files, ${res.rules.length} rules, '
    '${packages.length} packages, ${res.cases.length} cases materialized, '
    '$skips skip-counter hits (some counters annotate without dropping the '
    'case), ${res.caseSkips} ',
  );
}

// ---------------------------------------------------------------------------
// Run + comparison
// ---------------------------------------------------------------------------

class ProcResult {
  final int rc;
  final String stdout;
  final String stderr;
  final int ms;
  final String killed; // '', 'timeout', 'rss:<kb>'
  const ProcResult(this.rc, this.stdout, this.stderr, this.ms, this.killed);
}

class DiagRow {
  final String file;
  final String severity;
  final String code;
  final int line;
  final int col;
  final int offset;
  final int length;
  const DiagRow(
    this.file,
    this.severity,
    this.code,
    this.line,
    this.col,
    this.offset,
    this.length,
  );

  String get key => '$severity|$code|$line:$col|$offset:$length';
}

List<DiagRow> parseDiagnostics(String out, String pkgReal) {
  Object? doc;
  try {
    doc = jsonDecode(out);
  } catch (_) {
    // Fall back to JSONL.
    for (final line in out.split('\n')) {
      final t = line.trim();
      if (t.isEmpty || !t.startsWith('{')) continue;
      try {
        doc = jsonDecode(t);
        break;
      } catch (_) {}
    }
  }
  List diags = const [];
  if (doc is Map && doc['diagnostics'] is List) diags = doc['diagnostics'];
  final rows = <DiagRow>[];
  for (final d in diags) {
    if (d is! Map) continue;
    final loc = d['location'];
    if (loc is! Map) continue;
    final range = loc['range'];
    if (range is! Map) continue;
    final start = range['start'];
    final end = range['end'];
    if (start is! Map || end is! Map) continue;
    var file = (loc['file'] ?? '').toString();
    if (file.startsWith('$pkgReal/')) file = file.substring(pkgReal.length + 1);
    rows.add(
      DiagRow(
        file,
        (d['severity'] ?? '').toString(),
        (d['code'] ?? '').toString(),
        (start['line'] as num?)?.toInt() ?? 0,
        (start['column'] as num?)?.toInt() ?? 0,
        (start['offset'] as num?)?.toInt() ?? 0,
        ((end['offset'] as num?)?.toInt() ?? 0) -
            ((start['offset'] as num?)?.toInt() ?? 0),
      ),
    );
  }
  return rows;
}

const _ignoredCodes = {
  'unused_element',
  'unused_field',
  'unused_local_variable',
};

Future<int?> _rssKb(int pid) async {
  try {
    final r = await Process.run('ps', ['-o', 'rss=', '-p', '$pid']);
    final s = r.stdout.toString().trim();
    return s.isEmpty ? null : int.tryParse(s);
  } catch (_) {
    return null;
  }
}

Future<ProcResult> runProcess(
  String exe,
  List<String> args, {
  Duration timeout = const Duration(minutes: 30),
  int rssLimitKb = 8 * 1024 * 1024,
}) async {
  final p = await Process.start(exe, args);
  final sw = Stopwatch()..start();
  var killed = '';
  var probing = false;
  Timer? timer;
  timer = Timer.periodic(const Duration(milliseconds: 250), (_) async {
    if (killed.isNotEmpty || probing) return;
    probing = true;
    try {
      if (sw.elapsed > timeout) {
        killed = 'timeout';
        p.kill(ProcessSignal.sigkill);
        return;
      }
      final rss = await _rssKb(p.pid);
      if (rss != null && rss > rssLimitKb) {
        killed = 'rss:$rss';
        p.kill(ProcessSignal.sigkill);
      }
    } finally {
      probing = false;
    }
  });
  final out = await p.stdout.transform(utf8.decoder).join();
  final err = await p.stderr.transform(utf8.decoder).join();
  final rc = await p.exitCode;
  timer.cancel();
  return ProcResult(rc, out, err, sw.elapsedMilliseconds, killed);
}

Future<void> runCmd(List<String> args, Stopwatch watch) async {
  String? corpusDir;
  String? outDir;
  String? dartBin;
  String? dartqBin;
  String? sdkDir;
  String? only;
  for (var i = 0; i < args.length; i++) {
    switch (args[i]) {
      case '--dart':
        dartBin = args[++i];
      case '--dartq':
        dartqBin = args[++i];
      case '--sdk':
        sdkDir = args[++i];
      case '--only':
        only = args[++i];
      default:
        if (corpusDir == null) {
          corpusDir = args[i];
        } else if (outDir == null) {
          outDir = args[i];
        } else {
          _fail(_usage);
        }
    }
  }
  if (corpusDir == null || outDir == null || dartBin == null) _fail(_usage);

  final manifest =
      jsonDecode(File('$corpusDir/manifest.json').readAsStringSync())
          as Map<String, Object?>;
  final packages = (manifest['packages'] as List).cast<Map<String, Object?>>();
  final rawDir = Directory('$outDir/raw');
  rawDir.createSync(recursive: true);

  final results = <String, Object?>{
    'dart': dartBin,
    'dartq': dartqBin ?? '',
    'sdk': sdkDir ?? '',
    'corpusDir': corpusDir,
    'startedAt': DateTime.now().toUtc().toIso8601String(),
    'packages': <Object?>[],
  };
  final resultsPackages = results['packages'] as List;

  for (final pkg in packages) {
    final name = pkg['name'] as String;
    if (only != null && name != only) continue;
    final pkgReal = Directory('$corpusDir/$name').resolveSymbolicLinksSync();
    final sdkReal = (sdkDir == null)
        ? ''
        : Directory(sdkDir).resolveSymbolicLinksSync();

    final oracle = await runProcess(dartBin, [
      'analyze',
      '--format=json',
      pkgReal,
    ]);
    File('${rawDir.path}/$name.oracle.json').writeAsStringSync(oracle.stdout);
    File('${rawDir.path}/$name.oracle.err').writeAsStringSync(oracle.stderr);

    ProcResult? dq;
    if (dartqBin != null) {
      dq = await runProcess(dartqBin, [
        'analyze',
        '--cold',
        '--format=json',
        '--sdk',
        sdkReal,
        pkgReal,
      ]);
      File('${rawDir.path}/$name.dq.json').writeAsStringSync(dq.stdout);
      File('${rawDir.path}/$name.dq.err').writeAsStringSync(dq.stderr);
    }

    final oracleRows = parseDiagnostics(oracle.stdout, pkgReal);
    final dqRows = dq == null ? null : parseDiagnostics(dq.stdout, pkgReal);
    final cases = (pkg['cases'] as List).cast<Map<String, Object?>>();

    final caseResults = <Object?>[];
    var parityFails = 0;
    var fidelityFails = 0;
    var oracleRowsTotal = 0;
    var dqRowsTotal = 0;
    for (final c in cases) {
      final target = c['target'] as String;
      final files = (c['files'] as List).cast<String>();
      final expected = (c['rows'] as List)
          .map((r) => (r as List))
          .map(
            (r) =>
                (r[0] as String, (r[1] as num).toInt(), (r[2] as num).toInt()),
          )
          .toList();

      // Fidelity: expected vs oracle on the target file, minus the codes the
      // harness ignores.
      final oracleTarget = oracleRows
          .where((r) => r.file == target && !_ignoredCodes.contains(r.code))
          .toList();
      final oracleKeys = {
        for (final r in oracleTarget) '${r.code}|${r.offset}|${r.length}',
      };
      final expectedKeys = {
        for (final r in expected) '${r.$1}|${r.$2}|${r.$3}',
      };
      final expectedOnly = expectedKeys.difference(oracleKeys).toList()..sort();
      final oracleOnly = oracleKeys.difference(expectedKeys).toList()..sort();
      final fidelityOk = expectedOnly.isEmpty && oracleOnly.isEmpty;
      if (!fidelityOk) fidelityFails++;

      // Parity: dartq rows == oracle rows per case file, no filtering.
      var parityOk = dq != null;
      final oOnlyAll = <String>[];
      final dqOnlyAll = <String>[];
      if (dq == null) {
        parityOk = false;
      } else {
        for (final f in files) {
          final oSet = {
            for (final r in oracleRows.where((r) => r.file == f)) r.key,
          };
          final dSet = {
            for (final r in dqRows!.where((r) => r.file == f)) r.key,
          };
          final oOnly = oSet.difference(dSet).toList()..sort();
          final dqOnly = dSet.difference(oSet).toList()..sort();
          oOnlyAll.addAll(oOnly);
          dqOnlyAll.addAll(dqOnly);
        }
        parityOk = oOnlyAll.isEmpty && dqOnlyAll.isEmpty;
      }
      if (!parityOk) parityFails++;
      oracleRowsTotal += oracleRows.where((r) => files.contains(r.file)).length;
      if (dqRows != null) {
        dqRowsTotal += dqRows.where((r) => files.contains(r.file)).length;
      }

      caseResults.add({
        'id': c['id'],
        'source': c['source'],
        'target': target,
        'fidelity': {
          'ok': fidelityOk,
          'expected': expectedKeys.length,
          'oracle': oracleKeys.length,
          'expectedOnly': expectedOnly,
          'oracleOnly': oracleOnly,
        },
        'parity': {'ok': parityOk, 'oOnly': oOnlyAll, 'dqOnly': dqOnlyAll},
      });
    }

    // Package-level extras: rows outside any case file.
    final caseFiles = {
      for (final c in cases) ...(c['files'] as List).cast<String>(),
    };
    final extraO = oracleRows.where((r) => !caseFiles.contains(r.file)).length;
    final extraDq =
        dqRows?.where((r) => !caseFiles.contains(r.file)).length ?? 0;

    resultsPackages.add({
      'name': name,
      'rule': pkg['rule'],
      'oracle': {'rc': oracle.rc, 'ms': oracle.ms, 'killed': oracle.killed},
      'dq': dq == null ? null : {'rc': dq.rc, 'ms': dq.ms, 'killed': dq.killed},
      'oracleRows': oracleRowsTotal,
      'dqRows': dqRowsTotal,
      'extraRows': {'oracle': extraO, 'dq': extraDq},
      'parityFails': parityFails,
      'fidelityFails': fidelityFails,
      'cases': caseResults,
    });
    stderr.writeln(
      'run: $name oracle=${oracleRowsTotal} dq=$dqRowsTotal '
      'parityFails=$parityFails fidelityFails=$fidelityFails',
    );
  }

  results['wallSeconds'] = watch.elapsedMilliseconds / 1000.0;
  File(
    '$outDir/results.json',
  ).writeAsStringSync(const JsonEncoder.withIndent('  ').convert(results));

  // failing_cases.tsv
  final tsv = StringBuffer('case_id\tpackage\to_only\tdq_only\n');
  for (final p in resultsPackages.cast<Map<String, Object?>>()) {
    for (final c in (p['cases'] as List).cast<Map<String, Object?>>()) {
      final parity = c['parity'] as Map<String, Object?>;
      if (parity['ok'] == true) continue;
      tsv.write(
        '${c['id']}\t${p['name']}\t'
        '${(parity['oOnly'] as List).join(';')}\t'
        '${(parity['dqOnly'] as List).join(';')}\n',
      );
    }
  }
  File('$outDir/failing_cases.tsv').writeAsStringSync(tsv.toString());
  stderr.writeln('run: done in ${watch.elapsedMilliseconds / 1000.0}s');
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

void report(String resultsJson, String outMd) {
  final r =
      jsonDecode(File(resultsJson).readAsStringSync()) as Map<String, Object?>;
  final corpusDir =
      r['corpusDir'] as String? ?? File(resultsJson).parent.parent.path;
  final manifest =
      jsonDecode(File('$corpusDir/manifest.json').readAsStringSync())
          as Map<String, Object?>;
  final portStats = manifest['portStats'] as Map<String, Object?>;
  final packages = (r['packages'] as List).cast<Map<String, Object?>>();

  var cases = 0, parityOk = 0, fidelityOk = 0;
  var oracleRows = 0, dqRows = 0;
  final perRule = <String, Map<String, Object?>>{};
  final crashes = <String>[];
  for (final p in packages) {
    final name = p['name'] as String;
    final rule = p['rule'] as String;
    final pcases = (p['cases'] as List).cast<Map<String, Object?>>();
    cases += pcases.length;
    oracleRows += p['oracleRows'] as int;
    dqRows += p['dqRows'] as int;
    final oracle = p['oracle'] as Map<String, Object?>;
    final dq = p['dq'] as Map<String, Object?>?;
    if ((oracle['killed'] as String?)?.isNotEmpty == true) {
      crashes.add('$name (oracle ${oracle['killed']})');
    }
    if (dq != null && (dq['killed'] as String?)?.isNotEmpty == true) {
      crashes.add('$name (dartq ${dq['killed']})');
    }
    final agg = perRule.putIfAbsent(
      rule,
      () => {
        'package': name,
        'cases': 0,
        'parityFails': 0,
        'fidelityFails': 0,
        'oracleRows': 0,
        'dqRows': 0,
      },
    );
    agg['cases'] = (agg['cases'] as int) + pcases.length;
    agg['parityFails'] =
        (agg['parityFails'] as int) + (p['parityFails'] as int);
    agg['fidelityFails'] =
        (agg['fidelityFails'] as int) + (p['fidelityFails'] as int);
    agg['oracleRows'] = (agg['oracleRows'] as int) + (p['oracleRows'] as int);
    agg['dqRows'] = (agg['dqRows'] as int) + (p['dqRows'] as int);
    for (final c in pcases) {
      if ((c['parity'] as Map)['ok'] == true) parityOk++;
      if ((c['fidelity'] as Map)['ok'] == true) fidelityOk++;
    }
  }

  final ruleList = perRule.entries.toList()
    ..sort((a, b) {
      final byFail = (b.value['parityFails'] as int).compareTo(
        a.value['parityFails'] as int,
      );
      if (byFail != 0) return byFail;
      return a.key.compareTo(b.key);
    });

  final unimplemented =
      ruleList
          .where(
            (e) =>
                (e.value['dqRows'] as int) == 0 &&
                (e.value['oracleRows'] as int) > 0,
          )
          .map((e) => e.key)
          .toList()
        ..sort();

  final md = StringBuffer();
  md.writeln('# Official linter test suite: dartq baseline (2026-10-28)');
  md.writeln();
  md.writeln(
    'Oracle: `dart analyze` (${r['dart']}), SDK ${r['sdk']}. '
    'dartq: `${r['dartq']}`.',
  );
  md.writeln('Wall time (run phase): ${r['wallSeconds']}s.');
  md.writeln();
  md.writeln('## Corpus');
  md.writeln();
  md.writeln('| metric | value |');
  md.writeln('| --- | --- |');
  md.writeln('| upstream test files parsed | ${portStats['files']} |');
  md.writeln('| rules referenced | ${portStats['rules']} |');
  md.writeln('| packages materialized | ${packages.length} |');
  md.writeln('| cases extracted | ${portStats['cases']} |');
  md.writeln(
    '| cases skipped (extraction) | '
    '${(portStats['caseSkips'] as Map).values.fold<int>(0, (a, b) => a + (b as int))} |',
  );
  md.writeln(
    '| classes skipped (setup) | '
    '${(portStats['classSkips'] as Map).values.fold<int>(0, (a, b) => a + (b as int))} |',
  );
  md.writeln(
    '| parse-error files | ${(portStats['parseErrorFiles'] as List).length} |',
  );
  md.writeln();
  md.writeln('Skip reasons (cases): `${portStats['caseSkips']}`');
  md.writeln();
  md.writeln('Skip reasons (classes): `${portStats['classSkips']}`');
  md.writeln();
  md.writeln(
    'Files with parse errors: '
    '`${portStats['parseErrorFiles']}`',
  );
  md.writeln();
  md.writeln('## Results');
  md.writeln();
  md.writeln('| metric | value |');
  md.writeln('| --- | --- |');
  md.writeln('| cases compared | $cases |');
  md.writeln(
    '| parity pass (dartq == oracle, unfiltered) | '
    '$parityOk (${(parityOk / (cases == 0 ? 1 : cases) * 100).toStringAsFixed(2)}%) |',
  );
  md.writeln(
    '| fidelity pass (expectations == oracle, filtered) | '
    '$fidelityOk (${(fidelityOk / (cases == 0 ? 1 : cases) * 100).toStringAsFixed(2)}%) |',
  );
  md.writeln('| oracle rows (case files) | $oracleRows |');
  md.writeln('| dartq rows (case files) | $dqRows |');
  md.writeln();
  md.writeln('## Per-rule parity (sorted by failing cases)');
  md.writeln();
  md.writeln(
    '| rule | cases | parity fail | fidelity fail | oracle rows | dartq rows |',
  );
  md.writeln('| --- | --- | --- | --- | --- | --- |');
  for (final e in ruleList) {
    final v = e.value;
    if ((v['parityFails'] as int) == 0 && (v['fidelityFails'] as int) == 0) {
      continue;
    }
    md.writeln(
      '| ${e.key} | ${v['cases']} | ${v['parityFails']} | '
      '${v['fidelityFails']} | ${v['oracleRows']} | ${v['dqRows']} |',
    );
  }
  final clean = ruleList
      .where(
        (e) =>
            (e.value['parityFails'] as int) == 0 &&
            (e.value['fidelityFails'] as int) == 0,
      )
      .length;
  md.writeln('| (${clean} further rules fully green) | | | | | |');
  md.writeln();
  if (unimplemented.isNotEmpty) {
    md.writeln('## Rules with zero dartq rows (oracle has rows)');
    md.writeln();
    for (final u in unimplemented) {
      md.writeln('- $u');
    }
    md.writeln();
  }
  if (crashes.isNotEmpty) {
    md.writeln('## Crashes / timeouts / RSS kills');
    md.writeln();
    for (final c in crashes) {
      md.writeln('- $c');
    }
    md.writeln();
  }
  md.writeln('## Method notes');
  md.writeln();
  md.writeln(
    '- One package per (rule, setup group) under the corpus dir; '
    'each case lives in `lib/c<i>/` (or `test|hook|bin/c<i>/`) with its '
    'companion files, `package:test/...` references rewritten accordingly '
    'and expected offsets shifted past rewrite points.',
  );
  md.writeln(
    '- The harness pubspec has no sdk constraint (language version '
    'defaults to latest); the corpus mirrors that instead of pinning a '
    'constraint. `LanguageVersion219Mixin` classes get '
    "`sdk: '>=2.19.0 <4.0.0'` (language 2.19).",
  );
  md.writeln(
    '- Fidelity applies the harness ignore list '
    '(`unused_element`, `unused_field`, `unused_local_variable`) to the '
    'oracle; parity applies no filter.',
  );
  md.writeln(
    '- The official harness runs against a mock SDK and mock '
    'packages; fidelity deltas can be artifacts of the real SDK, not '
    'engine defects. Parity (dartq vs oracle on the same inputs) is the '
    'deliverable.',
  );
  md.writeln(
    '- Raw outputs and `failing_cases.tsv` live next to '
    '`results.json`.',
  );
  md.writeln();

  File(outMd).writeAsStringSync(md.toString());
  stderr.writeln('report: wrote $outMd');
}
