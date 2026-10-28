// Ports analyzer package inline-diagnostic tests into standalone Dart
// packages for differential testing.
//
// Usage:
//   dart run bin/port_analyzer_tests.dart port --test-dir <DIR> \
//       --corpus-dir <DIR> [--chunk-size 2000]
//   dart run bin/port_analyzer_tests.dart run <corpusDir> <outDir> \
//       --dartq <BIN> --sdk <DIR> [--dart <BIN>]
//   dart run bin/port_analyzer_tests.dart report <corpusDir> <outDir> \
//       <outMd> [--test-dir <DIR>]
//
// `port` materializes one Dart package per (setup group, chunk of cases)
// under the corpus directory, plus `manifest.jsonl` (one row per materialized
// case) and `skipped.jsonl` (one row per skipped case). `run` executes the
// oracle (`dart analyze`) and dartq once per package with an RSS watchdog
// and compares three sets per case: expectations vs oracle (fidelity) and
// dartq vs oracle (parity). `report` renders the markdown baseline.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:official_corpus_port/extract.dart';

const _usage =
    'usage: port_analyzer_tests.dart port --test-dir <DIR> --corpus-dir <DIR> '
    '[--chunk-size N] | run <corpusDir> <outDir> --dartq <BIN> --sdk <DIR> '
    '[--dart <BIN>] | report <corpusDir> <outDir> <outMd> [--test-dir <DIR>]';

const _rewriteFrom = 'package:test/';

Future<void> main(List<String> args) async {
  if (args.isEmpty) _fail(_usage);
  switch (args.first) {
    case 'port':
      portCommand(args.sublist(1));
    case 'run':
      await runCommand(args.sublist(1));
    case 'report':
      reportCommand(args.sublist(1));
    default:
      _fail(_usage);
  }
}

void portCommand(List<String> args) {
  String? testDir;
  String? corpusDir;
  var chunkSize = 2000;
  for (var i = 0; i < args.length; i++) {
    switch (args[i]) {
      case '--test-dir':
        testDir = args[++i];
      case '--corpus-dir':
        corpusDir = args[++i];
      case '--chunk-size':
        chunkSize = int.parse(args[++i]);
      default:
        _fail('Unknown argument: ${args[i]}');
    }
  }
  if (testDir == null || corpusDir == null) {
    _fail('Required: --test-dir <DIR> --corpus-dir <DIR>');
  }
  if (chunkSize < 1) {
    _fail('--chunk-size must be positive');
  }

  var corpus = Directory(corpusDir);
  if (corpus.existsSync() && corpus.listSync().isNotEmpty) {
    _fail('Corpus directory exists and is not empty: $corpusDir');
  }
  corpus.createSync(recursive: true);

  var testRoot = Directory(testDir);
  if (!testRoot.existsSync()) {
    _fail('Test directory does not exist: $testDir');
  }

  var files = <String>[];
  _collectTestFiles(testRoot, testRoot.path, files);
  files.sort();
  stderr.writeln('Found ${files.length} test files.');

  var cases = <ExtractedCase>[];
  var skips = <SkippedCase>[];
  var fileErrors = 0;
  for (var rel in files) {
    var content = File('${testRoot.path}/$rel').readAsStringSync();
    try {
      var extraction = extractFile(rel, content);
      cases.addAll(extraction.cases);
      skips.addAll(extraction.skips);
    } catch (e) {
      fileErrors++;
      stderr.writeln('ERROR extracting $rel: $e');
    }
  }
  stderr.writeln(
    'Extracted ${cases.length} cases, ${skips.length} skips, '
    '$fileErrors file errors.',
  );

  var skipCounts = <String, int>{};
  for (var skip in skips) {
    skipCounts[skip.reason] = (skipCounts[skip.reason] ?? 0) + 1;
  }
  for (var reason in skipCounts.keys.toList()..sort()) {
    stderr.writeln('  skip $reason: ${skipCounts[reason]}');
  }

  var manifest = File('$corpusDir/manifest.jsonl').openSync(mode: FileMode.write);
  var skippedFile = File('$corpusDir/skipped.jsonl').openSync(mode: FileMode.write);
  var jsonEncoder = JsonEncoder();

  void writeLine(RandomAccessFile file, Object value) {
    file.writeStringSync('${jsonEncoder.convert(value)}\n');
  }

  for (var skip in skips) {
    writeLine(skippedFile, {'id': skip.id, 'reason': skip.reason});
  }

  // Group by setup key, then chunk into packages.
  var groups = <String, List<ExtractedCase>>{};
  for (var c in cases) {
    var key = [
      if (c.needsMeta) 'meta',
      if (c.needsStrictCasts) 'strict_casts',
    ].join('+');
    groups.putIfAbsent(key.isEmpty ? 'default' : key, () => []).add(c);
  }

  var materialized = 0;
  for (var group in groups.keys.toList()..sort()) {
    var groupCases = groups[group]!;
    for (var k = 0; k * chunkSize < groupCases.length; k++) {
      var packageName = '${group}_$k';
      var packageDir = '$corpusDir/$packageName';
      _writePackage(packageDir, group.contains('meta'), group.contains('strict_casts'));
      var chunk = groupCases.sublist(
        k * chunkSize,
        (k + 1) * chunkSize < groupCases.length
            ? (k + 1) * chunkSize
            : groupCases.length,
      );
      for (var i = 0; i < chunk.length; i++) {
        var row = _materializeCase(
          packageDir,
          packageName,
          chunk[i],
          i,
        );
        writeLine(manifest, row);
        materialized++;
      }
      stderr.writeln(
        'Wrote package $packageName (${chunk.length} cases); running pub get...',
      );
      _pubGet(packageDir);
    }
  }

  manifest.closeSync();
  skippedFile.closeSync();
  stderr.writeln('Done: $materialized materialized, ${skips.length} skipped.');
}

void _collectTestFiles(Directory dir, String rootPath, List<String> out) {
  for (var entry in dir.listSync()) {
    if (entry is Directory) {
      if (entry.path.endsWith('.dart_tool')) continue;
      _collectTestFiles(entry, rootPath, out);
    } else if (entry is File &&
        entry.path.endsWith('_test.dart')) {
      out.add(entry.path.substring(rootPath.length + 1));
    }
  }
}

void _writePackage(String dir, bool meta, bool strictCasts) {
  Directory('$dir/lib').createSync(recursive: true);
  // The official harness analyzes the test package at the analyzer's current
  // (latest) language version; a real package's language version is the
  // lower bound of its sdk constraint, so pin the current one.
  var pubspec = StringBuffer('name: test\nenvironment:\n  sdk: ^3.13.0\n');
  if (meta) {
    pubspec.write('dependencies:\n  meta: any\n');
  }
  File('$dir/pubspec.yaml').writeAsStringSync(pubspec.toString());
  File('$dir/analysis_options.yaml').writeAsStringSync(
    strictCasts ? 'analyzer:\n  language:\n    strict-casts: true\n' : '',
  );
}

/// Materializes one case at `lib/c<i>/`; returns its manifest row.
Map<String, Object> _materializeCase(
  String packageDir,
  String packageName,
  ExtractedCase c,
  int i,
) {
  var caseDir = 'lib/c$i';
  Directory('$packageDir/$caseDir').createSync(recursive: true);

  var rewrites = <Map<String, Object>>[];
  var expected = c.expected.map((e) => e.toJson()).toList();

  var testRel = '$caseDir/test.dart';
  var (testText, testPoints) = _rewritePackageTestUris(c.code, 'c$i/');
  if (testPoints.isNotEmpty) {
    _shiftColumns(testPoints, expected, testRel, rewrites);
  }
  File('$packageDir/$testRel').writeAsStringSync(testText);

  var companionRels = <String>[];
  for (var companion in c.companions) {
    var rel = '$caseDir/${companion.relativePath}';
    File('$packageDir/$rel').parent.createSync(recursive: true);
    var (text, points) = _rewritePackageTestUris(companion.content, 'c$i/');
    File('$packageDir/$rel').writeAsStringSync(text);
    companionRels.add(rel);
    if (points.isNotEmpty) {
      rewrites.add({
        'file': rel,
        'lines': points.map((p) => p.line).toList(),
        'shift': points.first.shift,
      });
    }
  }

  return {
    'id': c.id,
    'package': packageName,
    'path': testRel,
    'expected': expected,
    if (rewrites.isNotEmpty) 'rewrites': rewrites,
  };
}

/// Rewrites `package:test/<x>` to `package:test/<idxDir><x>` so case-local
/// companions resolve. Returns the new text plus per-occurrence insertion
/// points (line, insertion offset in the original line, inserted length).
(String, List<_Insertion>) _rewritePackageTestUris(String text, String idxDir) {
  if (!text.contains(_rewriteFrom)) {
    return (text, const []);
  }
  var insert = idxDir;
  var out = StringBuffer();
  var points = <_Insertion>[];
  var line = 0;
  var lineStart = 0;
  var i = 0;
  while (i < text.length) {
    if (text.codeUnitAt(i) == 0x0A) {
      line++;
      lineStart = i + 1;
    }
    if (text.startsWith(_rewriteFrom, i)) {
      out.write(_rewriteFrom);
      out.write(insert);
      points.add(_Insertion(line, i - lineStart, insert.length));
      i += _rewriteFrom.length;
    } else {
      out.write(text[i]);
      i++;
    }
  }
  return (out.toString(), points);
}

/// Adjusts expected columns for rewrites on marked lines and records them.
void _shiftColumns(
  List<_Insertion> points,
  List<Map<String, Object>> expected,
  String fileRel,
  List<Map<String, Object>> rewrites,
) {
  var byLine = <int, List<_Insertion>>{};
  for (var p in points) {
    byLine.putIfAbsent(p.line, () => []).add(p);
  }
  var lines = <int>[];
  for (var e in expected) {
    var line = e['line'] as int;
    var col = e['column'] as int;
    var pointsOnLine = byLine[line];
    if (pointsOnLine == null) continue;
    var delta = 0;
    for (var p in pointsOnLine) {
      if (p.offset <= col) delta += p.shift;
    }
    if (delta > 0) {
      e['column'] = col + delta;
    }
  }
  for (var line in byLine.keys.toList()..sort()) {
    lines.add(line);
  }
  rewrites.add({
    'file': fileRel,
    'lines': lines,
    'shift': points.first.shift,
  });
}

void _pubGet(String packageDir) {
  var result = Process.runSync(
    'dart',
    ['pub', 'get', '--offline'],
    workingDirectory: packageDir,
  );
  if (result.exitCode != 0) {
    stderr.writeln(
      'Offline pub get failed for $packageDir; retrying online...',
    );
    result = Process.runSync(
      'dart',
      ['pub', 'get'],
      workingDirectory: packageDir,
    );
  }
  if (result.exitCode != 0) {
    stderr.writeln(
      'pub get failed for $packageDir:\n${result.stdout}\n${result.stderr}',
    );
  }
}

Never _fail(String message) {
  stderr.writeln(message);
  exit(2);
}

class _Insertion {
  final int line;
  final int offset;
  final int shift;
  _Insertion(this.line, this.offset, this.shift);
}

// ---------------------------------------------------------------------------
// run: execute oracle + dartq per package and compare three sets per case
// ---------------------------------------------------------------------------

const _rssLimitKb = 8 * 1024 * 1024;
const _processTimeout = Duration(minutes: 30);

class _ToolRun {
  final String state; // 'ok', 'crashed', 'timeout', 'rss:<kb>', 'reused'
  final int ms;
  const _ToolRun(this.state, this.ms);
}

Future<void> runCommand(List<String> args) async {
  var dartBin = 'dart';
  String? dartqBin;
  String? sdkDir;
  var rest = <String>[];
  for (var i = 0; i < args.length; i++) {
    switch (args[i]) {
      case '--dart':
        dartBin = args[++i];
      case '--dartq':
        dartqBin = args[++i];
      case '--sdk':
        sdkDir = args[++i];
      default:
        rest.add(args[i]);
    }
  }
  if (rest.length != 2 || dartqBin == null || sdkDir == null) {
    _fail(_usage);
  }
  var corpusDir = rest[0];
  var outDir = rest[1];
  Directory(outDir).createSync(recursive: true);
  var watch = Stopwatch()..start();

  var rows = _readManifest('$corpusDir/manifest.jsonl');
  var byPackage = <String, List<Map<String, dynamic>>>{};
  for (var row in rows) {
    byPackage.putIfAbsent(row['package'] as String, () => []).add(row);
  }
  var packages =
      Directory(corpusDir)
          .listSync()
          .whereType<Directory>()
          .where((d) => Directory('${d.path}/lib').existsSync())
          .map((d) => d.uri.pathSegments.where((s) => s.isNotEmpty).last)
          .toList()
        ..sort();

  var sdkReal = Directory(sdkDir).resolveSymbolicLinksSync();
  var results = <String, Object?>{
    'dart': dartBin,
    'dartq': dartqBin,
    'sdk': sdkDir,
    'corpusDir': corpusDir,
    'packages': <Object?>[],
  };
  var resultsPackages = results['packages'] as List;
  var totals = <String, int>{};
  var fidelityCounts = <String, int>{};
  var jaccardSrc = <String, Set<String>>{}, jaccardDq = <String, Set<String>>{};
  var tsv = StringBuffer('case_id\tpackage_path\to_only\tdq_only\n');

  for (var pkg in packages) {
    var pkgReal = Directory('$corpusDir/$pkg').resolveSymbolicLinksSync();
    var oracle = await _runTool(
      dartBin,
      ['analyze', '--format=json', pkgReal],
      '$outDir/$pkg.oracle.json',
      '$outDir/$pkg.oracle.log',
      checkLog: true,
    );
    var dq = await _runTool(
      dartqBin,
      ['analyze', '--cold', '--format=json', '--sdk', sdkReal, pkgReal],
      '$outDir/$pkg.dq.json',
      '$outDir/$pkg.dq.log',
      checkLog: false,
    );
    var pkgJson = {
      'name': pkg,
      'oracle': {'state': oracle.state, 'ms': oracle.ms},
      'dq': {'state': dq.state, 'ms': dq.ms},
      'cases': <Object?>[],
    };
    var casesJson = pkgJson['cases'] as List;
    var compared = 0;
    var parityOk = 0;

    if (const ['ok', 'reused'].contains(oracle.state) &&
        const ['ok', 'reused'].contains(dq.state)) {
      var oracleRows = _parseDiagnostics(
        File('$outDir/$pkg.oracle.json').readAsStringSync(),
        pkgReal,
      );
      var dqRows = _parseDiagnostics(
        File('$outDir/$pkg.dq.json').readAsStringSync(),
        pkgReal,
      );
      var oracleByCase = _byCase(oracleRows);
      var dqByCase = _byCase(dqRows);

      for (var row in byPackage[pkg] ?? const <Map<String, dynamic>>[]) {
        var caseIndex = _caseIndex(row['path'] as String);
        var path = row['path'] as String;
        var expected = {
          for (var e in (row['expected'] as List).cast<Map<String, dynamic>>())
            '${e['code']}|${e['line']}|${e['column']}|${e['length']}',
        };
        var oracleCase = oracleByCase[caseIndex] ?? const [];
        var dqCase = dqByCase[caseIndex] ?? const [];

        // Fidelity: expectations vs oracle on test.dart, positions only.
        var oracleTest = <String, String>{};
        for (var d in oracleCase) {
          if (d.file == path) {
            oracleTest['${d.code}|${d.line}|${d.column}|${d.length}'] =
                d.severity;
          }
        }
        var missing = expected.difference(oracleTest.keys.toSet()).toList()
          ..sort();
        var extra = oracleTest.keys.toSet().difference(expected).toList()
          ..sort();
        String fidelity;
        if (missing.isEmpty && extra.isEmpty) {
          fidelity = 'match';
        } else if (missing
            .map((k) => k.split('|').first)
            .toSet()
            .intersection(extra.map((k) => k.split('|').first).toSet())
            .isNotEmpty) {
          fidelity = 'extraction_bug';
        } else if (missing.isEmpty &&
            extra.every((k) => oracleTest[k] == 'INFO')) {
          fidelity = 'lint_or_hint_extra';
        } else {
          fidelity = 'mock_sdk_difference';
        }
        fidelityCounts[fidelity] = (fidelityCounts[fidelity] ?? 0) + 1;

        // Parity: dartq vs oracle over every file of the case, severity
        // included, keys relative to the case directory.
        var oSet = {
          for (var d in oracleCase) '${d.severity}|${d.code}|${d.caseRel}|${d.line}:${d.column}',
        };
        var dqSet = {
          for (var d in dqCase) '${d.severity}|${d.code}|${d.caseRel}|${d.line}:${d.column}',
        };
        var oOnly = oSet.difference(dqSet).toList()..sort();
        var dqOnly = dqSet.difference(oSet).toList()..sort();
        for (var d in oracleCase) {
          jaccardSrc.putIfAbsent(d.severity, () => {}).add(
            '$pkg|$caseIndex|${d.caseRel}|${d.code}|${d.line}:${d.column}',
          );
        }
        for (var d in dqCase) {
          jaccardDq.putIfAbsent(d.severity, () => {}).add(
            '$pkg|$caseIndex|${d.caseRel}|${d.code}|${d.line}:${d.column}',
          );
        }
        compared++;
        if (oOnly.isEmpty && dqOnly.isEmpty) {
          parityOk++;
        } else {
          tsv.write(
            '${row['id']}\t$pkg/$path\t${oOnly.join(';')}\t${dqOnly.join(';')}\n',
          );
        }
        casesJson.add({
          'id': row['id'],
          'path': path,
          'fidelity': fidelity,
          'missing': missing,
          'extra': extra,
          'parity': oOnly.isEmpty && dqOnly.isEmpty,
          'oOnly': oOnly,
          'dqOnly': dqOnly,
        });
      }
    } else {
      totals['unavailable'] = (totals['unavailable'] ?? 0) +
          (byPackage[pkg]?.length ?? 0);
    }
    totals['cases'] = (totals['cases'] ?? 0) + (byPackage[pkg]?.length ?? 0);
    totals['compared'] = (totals['compared'] ?? 0) + compared;
    totals['parityOk'] = (totals['parityOk'] ?? 0) + parityOk;
    resultsPackages.add(pkgJson);
    stderr.writeln(
      'run: $pkg oracle=${oracle.state} dq=${dq.state} compared=$compared '
      'parityOk=$parityOk',
    );
  }

  results['wallSeconds'] = watch.elapsedMilliseconds / 1000;
  results['totals'] = totals;
  results['fidelity'] = fidelityCounts;
  results['jaccard'] = {
    for (var sev in {...jaccardSrc.keys, ...jaccardDq.keys})
      sev: {
        'oracle': jaccardSrc[sev]?.length ?? 0,
        'dartq': jaccardDq[sev]?.length ?? 0,
        'both':
            jaccardSrc[sev]?.toSet().intersection(jaccardDq[sev] ?? {}).length ??
            0,
      },
  };
  File(
    '$outDir/results.json',
  ).writeAsStringSync(const JsonEncoder.withIndent('  ').convert(results));
  File('$outDir/failing_cases.tsv').writeAsStringSync(tsv.toString());
  stderr.writeln('run: done in ${watch.elapsedMilliseconds / 1000}s');
}

List<Map<String, dynamic>> _readManifest(String path) {
  return File(path)
      .readAsLinesSync()
      .where((l) => l.isNotEmpty)
      .map((l) => jsonDecode(l) as Map<String, dynamic>)
      .toList();
}

/// Runs [exe], streaming stdout to [outPath] and stderr to [logPath], with an
/// RSS and wall-clock watchdog. Reuses a previous valid output when present.
Future<_ToolRun> _runTool(
  String exe,
  List<String> args,
  String outPath,
  String logPath, {
  required bool checkLog,
}) async {
  if (_outputValid(outPath, checkLog: checkLog)) {
    return const _ToolRun('reused', 0);
  }
  var proc = await Process.start(exe, args);
  var watch = Stopwatch()..start();
  var killed = '';
  var busy = false;
  Timer? timer;
  timer = Timer.periodic(const Duration(seconds: 2), (_) async {
    if (killed.isNotEmpty || busy) return;
    busy = true;
    try {
      if (watch.elapsed > _processTimeout) {
        killed = 'timeout';
        proc.kill(ProcessSignal.sigkill);
        return;
      }
      var rss = await _rssKb(proc.pid);
      if (rss != null && rss > _rssLimitKb) {
        killed = 'rss:$rss';
        proc.kill(ProcessSignal.sigkill);
      }
    } finally {
      busy = false;
    }
  });
  var stdoutFuture = proc.stdout.transform(utf8.decoder).join();
  var stderrFuture = proc.stderr.transform(utf8.decoder).join();
  await proc.exitCode;
  timer.cancel();
  var out = await stdoutFuture;
  var err = await stderrFuture;
  File(outPath).writeAsStringSync(out);
  File(logPath).writeAsStringSync(err);
  var state = killed.isNotEmpty ? killed : 'ok';
  if (state == 'ok' && !_outputValid(outPath, checkLog: checkLog)) {
    state = 'crashed';
    File(outPath).renameSync('$outPath.bad.$state');
  }
  return _ToolRun(state, watch.elapsedMilliseconds);
}

bool _outputValid(String path, {required bool checkLog}) {
  Object? doc;
  try {
    doc = jsonDecode(File(path).readAsStringSync());
  } catch (_) {
    return false;
  }
  if (doc is! Map || doc['diagnostics'] is! List) return false;
  if (checkLog) {
    var log = File('${path.substring(0, path.length - 5)}.log');
    if (log.existsSync()) {
      var text = log.readAsStringSync();
      var head = text.length <= 4096 ? text : text.substring(0, 4096);
      if (head.contains('Internal error') ||
          head.contains('unexpected error')) {
        return false;
      }
    }
  }
  return true;
}

Future<int?> _rssKb(int pid) async {
  try {
    var r = await Process.run('ps', ['-o', 'rss=', '-p', '$pid']);
    var s = r.stdout.toString().trim();
    return s.isEmpty ? null : int.tryParse(s);
  } catch (_) {
    return null;
  }
}

class _Diag {
  final String file; // package-relative, e.g. lib/c3/test.dart
  final String caseRel; // case-relative, e.g. test.dart
  final String severity;
  final String code;
  final int line; // 0-based
  final int column; // 0-based
  final int length;
  _Diag(
    this.file,
    this.caseRel,
    this.severity,
    this.code,
    this.line,
    this.column,
    this.length,
  );
}

List<_Diag> _parseDiagnostics(String out, String pkgReal) {
  Object? doc;
  try {
    doc = jsonDecode(out);
  } catch (_) {
    for (var line in out.split('\n')) {
      var t = line.trim();
      if (t.isEmpty || !t.startsWith('{')) continue;
      try {
        doc = jsonDecode(t);
        break;
      } catch (_) {}
    }
  }
  var diags = doc is Map && doc['diagnostics'] is List
      ? doc['diagnostics'] as List
      : const [];
  var result = <_Diag>[];
  for (var d in diags) {
    if (d is! Map) continue;
    var loc = d['location'];
    if (loc is! Map) continue;
    var range = loc['range'];
    if (range is! Map) continue;
    var start = range['start'];
    var end = range['end'];
    if (start is! Map || end is! Map) continue;
    var file = (loc['file'] ?? '').toString();
    if (file.startsWith('$pkgReal/')) file = file.substring(pkgReal.length + 1);
    var segments = file.split('/');
    if (segments.length < 3 ||
        segments[0] != 'lib' ||
        !segments[1].startsWith('c') ||
        int.tryParse(segments[1].substring(1)) == null) {
      continue;
    }
    result.add(
      _Diag(
        file,
        segments.skip(2).join('/'),
        (d['severity'] ?? '').toString(),
        (d['code'] ?? '').toString(),
        ((start['line'] as num?) ?? 1).toInt() - 1,
        ((start['column'] as num?) ?? 1).toInt() - 1,
        ((end['offset'] as num?) ?? 0).toInt() -
            ((start['offset'] as num?) ?? 0).toInt(),
      ),
    );
  }
  return result;
}

Map<int, List<_Diag>> _byCase(List<_Diag> rows) {
  var byCase = <int, List<_Diag>>{};
  for (var d in rows) {
    byCase.putIfAbsent(_caseIndex(d.file), () => []).add(d);
  }
  return byCase;
}

int _caseIndex(String path) =>
    int.parse(path.split('/')[1].substring(1));

// ---------------------------------------------------------------------------
// report: render the markdown baseline
// ---------------------------------------------------------------------------

void reportCommand(List<String> args) {
  String? testDir;
  var rest = <String>[];
  for (var i = 0; i < args.length; i++) {
    if (args[i] == '--test-dir') {
      testDir = args[++i];
    } else {
      rest.add(args[i]);
    }
  }
  if (rest.length != 3) _fail(_usage);
  var corpusDir = rest[0];
  var outDir = rest[1];
  var outMd = rest[2];
  var r = jsonDecode(File('$outDir/results.json').readAsStringSync())
      as Map<String, Object?>;
  var totals = (r['totals'] as Map).cast<String, int>();
  var fidelity = (r['fidelity'] as Map).cast<String, int>();
  var jaccard = (r['jaccard'] as Map).cast<String, Map>();
  var packages = (r['packages'] as List).cast<Map<String, Object?>>();
  var skipped = <String, int>{};
  for (var line in File('$corpusDir/skipped.jsonl').readAsLinesSync()) {
    if (line.isEmpty) continue;
    var row = jsonDecode(line) as Map;
    var reason = row['reason'] as String;
    skipped[reason] = (skipped[reason] ?? 0) + 1;
  }

  var compared = totals['compared'] ?? 0;
  var crashes = <String>[];
  var failingCodes = <String, Set<String>>{};
  var failingCases = 0;
  for (var p in packages) {
    for (final kind in const ['oracle', 'dq']) {
      var state = ((p[kind] as Map)['state'] as String);
      if (state != 'ok' && state != 'reused') {
        crashes.add('${p['name']} ($kind $state)');
      }
    }
    for (var c in (p['cases'] as List).cast<Map<String, Object?>>()) {
      if (c['parity'] == true) continue;
      failingCases++;
      for (var key in [...(c['oOnly'] as List), ...(c['dqOnly'] as List)]) {
        var code = (key as String).split('|')[1];
        failingCodes.putIfAbsent(code, () => {}).add(c['id'] as String);
      }
    }
  }

  var topCodes = failingCodes.entries.toList()
    ..sort((a, b) {
      var byCases = b.value.length.compareTo(a.value.length);
      return byCases != 0 ? byCases : a.key.compareTo(b.key);
    });

  var md = StringBuffer();
  md.writeln('# Official analyzer test suite: dartq baseline (2026-10-28)');
  md.writeln();
  md.writeln(
    'Oracle: `dart analyze` (${r['dart']}). dartq: `${r['dartq']}` with '
    '`--sdk ${r['sdk']}`.',
  );
  md.writeln('Run phase wall time: ${r['wallSeconds']}s.');
  md.writeln();
  md.writeln('## Corpus');
  md.writeln();
  md.writeln('| metric | value |');
  md.writeln('| --- | --- |');
  md.writeln('| cases materialized | ${totals['cases']} |');
  md.writeln('| cases skipped | ${skipped.values.fold(0, (a, b) => a + b)} |');
  md.writeln('| cases compared | $compared |');
  md.writeln(
    '| cases unavailable (tool crash/kill) | ${totals['unavailable'] ?? 0} |',
  );
  md.writeln();
  md.writeln('Skip reasons: `${skipped}`');
  md.writeln();
  md.writeln('## Fidelity (expectations vs oracle)');
  md.writeln();
  for (var cls in fidelity.keys.toList()..sort()) {
    md.writeln(
      '- $cls: ${fidelity[cls]} '
      '(${(100.0 * fidelity[cls]! / (compared == 0 ? 1 : compared)).toStringAsFixed(2)}%)',
    );
  }
  md.writeln();
  md.writeln('## Parity (dartq vs oracle, severity included)');
  md.writeln();
  md.writeln('| metric | value |');
  md.writeln('| --- | --- |');
  md.writeln(
    '| cases passing (identical sets) | ${totals['parityOk']} of $compared '
    '(${(100.0 * (totals['parityOk'] ?? 0) / (compared == 0 ? 1 : compared)).toStringAsFixed(2)}%) |',
  );
  md.writeln('| cases failing | $failingCases |');
  for (var sev in jaccard.keys.toList()..sort()) {
    var v = jaccard[sev]!.cast<String, int>();
    var union = v['oracle']! + v['dartq']! - v['both']!;
    md.writeln(
      '| row Jaccard $sev | '
      '${union == 0 ? 1.0 : v['both']! / union} '
      '(oracle=${v['oracle']}, dartq=${v['dartq']}, both=${v['both']}) |',
    );
  }
  md.writeln();
  md.writeln('## Top failing codes (top 60)');
  md.writeln();
  md.writeln('| code | failing cases | diagnostics test file |');
  md.writeln('| --- | --- | --- |');
  for (var e in topCodes.take(60)) {
    var rel = 'src/diagnostics/${e.key.toLowerCase()}_test.dart';
    var exists = testDir != null && File('$testDir/$rel').existsSync();
    md.writeln('| ${e.key} | ${e.value.length} | ${exists ? rel : '-'} |');
  }
  md.writeln();
  if (crashes.isNotEmpty) {
    md.writeln('## Crashes / timeouts / RSS kills');
    md.writeln();
    for (var c in crashes) {
      md.writeln('- $c');
    }
    md.writeln();
  }
  md.writeln('## Method notes');
  md.writeln();
  md.writeln(
    '- One package per (setup group, chunk); each case lives in `lib/c<i>/` '
    'with its companions; `package:test/<x>` imports rewritten to '
    '`package:test/c<i>/<x>` with expected columns shifted.',
  );
  md.writeln(
    '- Corpus packages pin the oracle SDK\'s current language version '
    '(sdk `^3.13.0`), matching the official harness which analyzes the test '
    'package at the analyzer\'s latest language version.',
  );
  md.writeln(
    '- Fidelity compares codes+positions on `test.dart` only (expectations '
    'carry no severity); classification: `extraction_bug` = same code at '
    'different position on both sides; `lint_or_hint_extra` = oracle-only '
    'INFO rows; `mock_sdk_difference` = the rest (the official harness runs '
    'against a reduced mock SDK, `dart analyze` against the real one).',
  );
  md.writeln(
    '- Parity keys are `SEV|CODE|file-relative-to-case|line:col` over every '
    'file of the case; pass = identical key sets; row Jaccard is computed '
    'over the same keys per severity.',
  );
  md.writeln(
    '- Raw outputs, `failing_cases.tsv` and `results.json` live in the run '
    'output directory.',
  );
  File(outMd).writeAsStringSync(md.toString());
  stderr.writeln('report: wrote $outMd');
}
