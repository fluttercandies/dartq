/// The rule runner: executes a plugin's enabled analyzer lint rules over the
/// cycle's analyzed files, parsing with the real `package:analyzer` (parse
/// stack only).
///
/// Mirrors the official execution in `pkg/analysis_server_plugin/lib/src/
/// plugin_server.dart` (`_computeDiagnosticsFromPlugin`), minus resolution:
/// every enabled rule's node processors register once per file (each file is
/// its own library at dartq's parsed level), the unit is visited with
/// `AnalysisRuleVisitor(shouldPropagateExceptions: true)` — one bad rule
/// kills the session, exactly the official posture, and PRD section 14.3
/// absorbs it — then the rules' `afterLibrary` hooks run. The core never
/// ships an AST across the wire; the host parses the files it is told about,
/// overlays first (the core owns the open-document text).
library;

import 'dart:io' as io show File, FileSystemException;

import 'package:analyzer/analysis_rule/analysis_rule.dart'
    show AbstractAnalysisRule;
import 'package:analyzer/analysis_rule/rule_context.dart' show RuleContextUnit;
import 'package:analyzer/dart/analysis/utilities.dart' show parseString;
import 'package:analyzer/diagnostic/diagnostic.dart' show Diagnostic;
import 'package:analyzer/error/listener.dart'
    show DiagnosticReporter, RecordingDiagnosticListener;
import 'package:analyzer/file_system/file_system.dart' as resource show File;
import 'package:analyzer/file_system/memory_file_system.dart'
    show MemoryResourceProvider;
import 'package:analyzer/source/file_source.dart' show FileSource;
import 'package:analyzer/src/analysis_rule/rule_context.dart'
    show RuleContextWithParsedResults;
import 'package:analyzer/src/lint/linter_visitor.dart'
    show AnalysisRuleVisitor, RuleVisitorRegistryImpl;

import 'cycle_config.dart';
import 'diagnostic.dart';
import 'plugin_session.dart' show EngineBatch;
import 'registry.dart';

/// Runs the registry's enabled rules over [batch]'s files with [config]'s
/// enablement, overlays, and severity overrides, and returns the findings in
/// per-file, in-file emission order. The failure posture is the official
/// one: a rule that throws propagates out of this call (the visitor rethrows
/// under `shouldPropagateExceptions: true`), tearing the session down; a
/// file that cannot be read is skipped (the analyzed set races deletion, the
/// same posture the daemon's merge takes for unknown files).
List<PluginDiagnostic> runRules({
  required PluginRegistryImpl registry,
  required PluginCycleConfig config,
  required EngineBatch batch,
}) {
  final rules = registry.enabled(config.enabledRules);
  if (rules.isEmpty || batch.files.isEmpty) {
    return const <PluginDiagnostic>[];
  }
  // One provider per cycle holds every analyzed file's text (overlay or
  // disk), so the rules' `File` handles answer `path` without touching the
  // disk mid-visit; the cycle's garbage collects with the call. The
  // provider demands absolute paths (its contract), so each wire path is
  // mapped through the cwd — the wire path itself stays the row identity
  // (the core keyed the analyzed set by it).
  final provider = MemoryResourceProvider();
  final seen = <String>{};
  final findings = <PluginDiagnostic>[];
  for (final path in batch.files) {
    if (!seen.add(path)) {
      continue;
    }
    final content = config.overlayFor(path) ?? _readDisk(path);
    if (content == null) {
      continue;
    }
    final absolute = _absolute(path);
    provider.newFile(absolute, content);
    findings.addAll([
      for (final diagnostic in _runRulesOnFile(
        rules,
        path: absolute,
        content: content,
        file: provider.getFile(absolute),
        config: config,
      ))
        _toWire(diagnostic, config, path),
    ]);
  }
  return findings;
}

/// Registers [rules] on one file and visits it: the official per-library
/// shape (`registerNodeProcessors` once, one visitor over the unit,
/// `afterLibrary` at the end), with the per-unit reporter the official
/// server assigns before each visit. Returns the file's diagnostics in
/// emission order.
List<Diagnostic> _runRulesOnFile(
  List<AbstractAnalysisRule> rules, {
  required String path,
  required String content,
  required resource.File file,
  required PluginCycleConfig config,
}) {
  final source = FileSource(file);
  final listener = RecordingDiagnosticListener();
  final reporter = DiagnosticReporter(listener, source);
  final unit = parseString(
    content: content,
    path: path,
    throwIfDiagnostics: false,
  ).unit;
  final unitContext = RuleContextUnit(
    file: file,
    content: content,
    diagnosticReporter: reporter,
    unit: unit,
  );
  final context = RuleContextWithParsedResults([unitContext], unitContext);
  final visitRegistry = RuleVisitorRegistryImpl(enableTiming: false);
  for (final rule in rules) {
    rule.reporter = reporter;
    rule.registerNodeProcessors(visitRegistry, context);
  }
  context.currentUnit = unitContext;
  unit.accept(
    AnalysisRuleVisitor(visitRegistry, shouldPropagateExceptions: true),
  );
  AnalysisRuleVisitor(visitRegistry).afterLibrary();
  return listener.diagnostics;
}

/// The absolute form of a wire path (the analyzer's memory provider
/// demands absolute paths); a no-op for already-absolute paths. Wire paths
/// are the engine's workspace-relative or absolute forms without `..`
/// segments, so no normalization past the cwd join is needed.
String _absolute(String path) => io.File(path).absolute.path;

/// Converts one analyzer diagnostic to the wire row under [wirePath] (the
/// analyzed-set identity the finding belongs to — the provider works in
/// absolute paths, the wire does not): lower-case code name, severity
/// override first (the config's per-code re-leveling, official
/// `_configuredSeverity`), declared severity otherwise, UTF-16 offsets.
/// Offsets are clamped non-negative here — the rows are untrusted plugin
/// output (the §14.3 posture), and a negative row would fail the wire's
/// unsigned parse and tear the whole session for one weird rule.
PluginDiagnostic _toWire(
  Diagnostic diagnostic,
  PluginCycleConfig config,
  String wirePath,
) {
  // `name.toLowerCase()` instead of `lowerCaseName`: the sample overlays
  // compile this file against _fe_analyzer_shared 91.0.0, which predates
  // that getter; its definition (93+) is exactly `_name.toLowerCase()`, so
  // the emitted identity is unchanged.
  // ignore: deprecated_member_use
  final code = diagnostic.diagnosticCode.name.toLowerCase();
  return PluginDiagnostic(
    file: wirePath,
    severity:
        config.severityOverrides[code] ??
        Severity.values.byName(diagnostic.severity.name),
    code: code,
    message: diagnostic.message,
    offset: diagnostic.offset < 0 ? 0 : diagnostic.offset,
    length: diagnostic.length < 0 ? 0 : diagnostic.length,
  );
}

/// The disk text of [path], null when it cannot be read.
String? _readDisk(String path) {
  try {
    return io.File(path).readAsStringSync();
  } on io.FileSystemException {
    return null;
  }
}
