/// The rule runner: executes a plugin's enabled analyzer lint rules over the
/// cycle's analyzed files, mirroring the official plugin server's dispatch
/// (`pkg/analysis_server_plugin/lib/src/plugin_server.dart`).
///
/// Dispatch is per library, upstream `_canUseParsedResult` semantics: when
/// every enabled rule answers `canUseParsedResult`, the cycle keeps the
/// parse-only path — each file parses with `parseString` and is its own
/// library. When any enabled rule needs resolution, the libraries of the
/// analyzed set resolve through a per-cycle `AnalysisContextCollection` and
/// all enabled rules run over the `ResolvedUnitResult`s — the resolved
/// context is the official shape (`RuleContextWithResolvedResults` over
/// every unit of the resolved library), so a rule that reads the element
/// model (riverpod_lint's `element.element`/`declaredFragment` style)
/// produces its findings instead of silently reporting nothing.
///
/// The failure posture is the official one: a rule that throws propagates
/// out of this call (the visitor rethrows under
/// `shouldPropagateExceptions: true`), tearing the session down; a file
/// that cannot be read is skipped; a library whose resolution fails
/// degrades to its parsed-capable rules for that cycle — upstream swallows
/// the same `InconsistentAnalysisException` (a later pass retries); here
/// the degradation is the equivalent non-fatal answer.
library;

import 'dart:io' as io show File, FileSystemException;

import 'package:analyzer/analysis_rule/analysis_rule.dart'
    show AbstractAnalysisRule;
import 'package:analyzer/analysis_rule/rule_context.dart' show RuleContextUnit;
import 'package:analyzer/dart/analysis/results.dart'
    show ResolvedLibraryResult, SomeResolvedLibraryResult;
import 'package:analyzer/dart/analysis/session.dart'
    show InconsistentAnalysisException;
import 'package:analyzer/dart/analysis/utilities.dart' show parseString;
import 'package:analyzer/diagnostic/diagnostic.dart' show Diagnostic;
import 'package:analyzer/error/listener.dart'
    show DiagnosticReporter, RecordingDiagnosticListener;
import 'package:analyzer/file_system/file_system.dart' as resource show File;
import 'package:analyzer/file_system/memory_file_system.dart'
    show MemoryResourceProvider;
import 'package:analyzer/file_system/overlay_file_system.dart'
    show OverlayResourceProvider;
import 'package:analyzer/file_system/physical_file_system.dart'
    show PhysicalResourceProvider;
import 'package:analyzer/source/file_source.dart' show FileSource;
import 'package:analyzer/src/analysis_rule/rule_context.dart'
    show RuleContextWithParsedResults, RuleContextWithResolvedResults;
import 'package:analyzer/src/dart/analysis/analysis_context_collection.dart'
    show AnalysisContextCollectionImpl;
import 'package:analyzer/src/dart/analysis/driver_based_analysis_context.dart'
    show DriverBasedAnalysisContext;
import 'package:analyzer/src/dart/analysis/file_state.dart'
    show LibraryFileKind, PartFileKind;
import 'package:analyzer/src/lint/linter_visitor.dart'
    show AnalysisRuleVisitor, RuleVisitorRegistryImpl;

import 'cycle_config.dart';
import 'diagnostic.dart';
import 'plugin_session.dart' show EngineBatch;
import 'registry.dart';

/// Runs the registry's enabled rules over [batch]'s files with [config]'s
/// enablement, overlays, and severity overrides, and returns the findings in
/// per-file, in-file emission order.
///
/// The collection that resolves libraries is per-cycle state: it is built
/// once (over the analyzed set's absolute paths), mirrors every overlay into
/// its resource provider before the first resolve, serves every library of
/// the cycle, and is disposed with the call — a fresh cycle never reads a
/// stale resolution, and the host's footprint stays bounded.
Future<List<PluginDiagnostic>> runRules({
  required PluginRegistryImpl registry,
  required PluginCycleConfig config,
  required EngineBatch batch,
}) async {
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
  final files = <({String wirePath, String absolute, String content})>[];
  final seen = <String>{};
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
    files.add((wirePath: path, absolute: absolute, content: content));
  }
  if (files.isEmpty) {
    return const <PluginDiagnostic>[];
  }

  // Per-library dispatch, upstream `_canUseParsedResult`: resolution is
  // cycle-global here because the wire carries one enablement set (upstream's
  // per-directory plugin configurations have no counterpart). When no rule
  // needs resolution nothing below the parse path runs — zero behavior
  // change for parsed-capable plugins.
  if (rules.every((rule) => rule.canUseParsedResult)) {
    return [
      for (final file in files)
        ..._runRulesOnFile(
          rules,
          path: file.absolute,
          content: file.content,
          file: provider.getFile(file.absolute),
        ).map((d) => _toWire(d, config, file.wirePath)),
    ];
  }
  return _runRulesResolved(rules, files, config, provider);
}

/// The resolved path: builds one [AnalysisContextCollectionImpl] over the
/// analyzed files' absolute paths (the locator lifts each to its context
/// root — options file or package_config — exactly as upstream's
/// `includedPaths` do), mirrors the cycle's overlays into the collection's
/// resource provider, groups the analyzed files into libraries through the
/// driver's file-state graph (a `part of` file runs with its library, never
/// alone), resolves each library once, and runs every enabled rule over the
/// resolved units in upstream order (all units of the library, defining
/// unit as `RuleContext.definingUnit`).
Future<List<PluginDiagnostic>> _runRulesResolved(
  List<AbstractAnalysisRule> rules,
  List<({String wirePath, String absolute, String content})> files,
  PluginCycleConfig config,
  MemoryResourceProvider provider,
) async {
  // The overlay provider wraps the physical disk: files without overlays
  // read through, overlay files read the pushed text — the same source of
  // truth the parse path reads. `drainStreams: false` keeps the scheduler's
  // event stream open; this runner drives resolution synchronously instead
  // of subscribing to it, and a drained stream would still complete the
  // futures but with nothing observing them. Warning/lint computation stays
  // off (upstream `configureAnalysisOptionsBuilder` semantics): the plugin
  // isolate never re-runs the analyzer's own diagnostics.
  final resourceProvider = OverlayResourceProvider(
    PhysicalResourceProvider.INSTANCE,
  );
  var overlayStamp = 0;
  for (final file in files) {
    if (config.overlayFor(file.wirePath) != null) {
      resourceProvider.setOverlay(
        file.absolute,
        content: file.content,
        modificationStamp: overlayStamp++,
      );
    }
  }
  final collection = _buildCollection(
    resourceProvider: resourceProvider,
    includedPaths: [for (final file in files) file.absolute],
  );
  try {
    // Group the analyzed set into libraries: a part file resolves with its
    // containing library (upstream `_handleResolvedUnit` ignores non-defining
    // units and the parsed path groups via `fileKinds`). Two files of one
    // library share one resolution.
    final libraries = <String, _Library>{};
    for (final file in files) {
      final context = _contextFor(collection, file.absolute);
      if (context == null) {
        continue;
      }
      final fileState = context.driver.fsState.getFileForPath(file.absolute);
      final kind = fileState.kind;
      final library = switch (kind) {
        PartFileKind(:var library?) => library,
        _ => kind.asLibrary,
      };
      libraries
          .putIfAbsent(
            library.file.path,
            () => _Library(context: context, kind: library),
          )
          .analyzedFiles
          .add(file);
    }
    // Findings map back to wire paths through the analyzed set's absolutes:
    // a diagnostic on a unit outside the analyzed set drops (upstream's
    // `isAnalyzed` gate answers the same way).
    final wireByAbsolute = <String, String>{
      for (final file in files) file.absolute: file.wirePath,
    };
    final parsedRules = [
      for (final rule in rules)
        if (rule.canUseParsedResult) rule,
    ];
    final findings = <PluginDiagnostic>[];
    for (final library in libraries.values) {
      final resolved = await _runRulesOnLibrary(rules, library: library);
      if (resolved == null) {
        // The library failed to resolve (bad SDK, missing deps, an
        // `InconsistentAnalysisException` upstream would also swallow): the
        // cycle survives by degrading this library to its parsed-capable
        // rules — the resolution-requiring rules simply have no element
        // model to report from.
        for (final file in library.analyzedFiles) {
          for (final diagnostic in _runRulesOnFile(
            parsedRules,
            path: file.absolute,
            content: file.content,
            file: provider.getFile(file.absolute),
          )) {
            findings.add(_toWire(diagnostic, config, file.wirePath));
          }
        }
        continue;
      }
      for (final MapEntry(key: path, value: diagnostics) in resolved.entries) {
        final wirePath = wireByAbsolute[path];
        if (wirePath == null) {
          continue;
        }
        for (final diagnostic in diagnostics) {
          findings.add(_toWire(diagnostic, config, wirePath));
        }
      }
    }
    return findings;
  } finally {
    await collection.dispose();
  }
}

/// Builds the resolved-path collection with warning/lint computation off
/// (upstream `plugin_server.dart`'s options callback: warnings and lints
/// are the main analysis isolate's job, never the plugin isolate's).
///
/// The callback's named parameter is analyzer-era sensitive and cannot be
/// spelled statically for both eras this file compiles against: analyzer 14
/// renamed it to `configureAnalysisOptionsBuilder`, receiving the mutable
/// `AnalysisOptionsBuilder` (the built `AnalysisOptionsImpl`'s fields became
/// final), while analyzer 13 and earlier spell it `updateAnalysisOptions4`,
/// receiving the built `AnalysisOptionsImpl`. The constructor tearoff +
/// `Function.apply` dispatch at runtime: the 14-era spelling is tried
/// first, and the missing parameter falls back to the 13-era one — a
/// sidecar compiling against the older era pays one failed dynamic
/// invocation per process, on its first resolved-path cycle, nothing after.
AnalysisContextCollectionImpl _buildCollection({
  required OverlayResourceProvider resourceProvider,
  required List<String> includedPaths,
}) {
  final shared = <Symbol, Object?>{
    #resourceProvider: resourceProvider,
    #includedPaths: includedPaths,
    #drainStreams: false,
  };
  try {
    return Function.apply(AnalysisContextCollectionImpl.new, [], {
          ...shared,
          #configureAnalysisOptionsBuilder:
              ({required analysisOptionsBuilder}) => analysisOptionsBuilder
                ..warning = false
                ..lint = false,
        })
        as AnalysisContextCollectionImpl;
  } on NoSuchMethodError {
    return Function.apply(AnalysisContextCollectionImpl.new, [], {
          ...shared,
          #updateAnalysisOptions4: ({required analysisOptions}) =>
              analysisOptions
                ..warning = false
                ..lint = false,
        })
        as AnalysisContextCollectionImpl;
  }
}

/// One library of the analyzed set: the owning analysis context, the
/// driver's [LibraryFileKind] (its `fileKinds` enumerate the units, parts
/// included), and the analyzed files that belong to it (the wire rows they
/// map back to).
final class _Library {
  _Library({required this.context, required this.kind});

  final DriverBasedAnalysisContext context;
  final LibraryFileKind kind;
  final List<({String wirePath, String absolute, String content})>
  analyzedFiles = [];
}

/// Runs [rules] over [library]'s resolved units, upstream
/// `_computeDiagnosticsFromPlugin` shape: the resolved library's units are
/// the context's `allUnits`, each unit gets its own recording reporter, the
/// rules' node processors register once per library, every analyzed unit is
/// visited, then `afterLibrary` fires.
///
/// Returns the diagnostics keyed by the absolute path of the unit they were
/// reported on, or null when the library could not resolve — the caller
/// degrades it to its parsed-capable rules. The catch is the resolution
/// boundary only: a rule throwing mid-visit still propagates (the official
/// `shouldPropagateExceptions: true` posture, which the core surfaces as
/// `dartq/plugin_crashed`), and so does a malformed-context crash — only a
/// failed *resolve* degrades.
Future<Map<String, List<Diagnostic>>?> _runRulesOnLibrary(
  List<AbstractAnalysisRule> rules, {
  required _Library library,
}) async {
  final session = library.context.currentSession;
  final SomeResolvedLibraryResult result;
  try {
    result = await session.getResolvedLibrary(library.kind.file.path);
  } on InconsistentAnalysisException {
    // State raced mid-resolution; upstream's `_handleResolvedUnit` swallows
    // it the same way — there is no subsequent pass here, so the caller
    // degrades the library to parsed.
    return null;
  }
  if (result is! ResolvedLibraryResult) {
    return null;
  }
  // One reporter per unit (upstream's diagnosticListeners map): findings
  // report against the unit's own source so offsets land on the right file.
  final allUnits = <RuleContextUnit>[];
  final listeners = <RuleContextUnit, RecordingDiagnosticListener>{};
  RuleContextUnit? defining;
  final definingFragment = result.element.firstFragment;
  for (final unit in result.units) {
    final listener = RecordingDiagnosticListener();
    final unitContext = RuleContextUnit(
      file: unit.file,
      content: unit.content,
      diagnosticReporter: DiagnosticReporter(
        listener,
        unit.libraryElement.firstFragment.source,
      ),
      unit: unit.unit,
    );
    allUnits.add(unitContext);
    listeners[unitContext] = listener;
    if (unit.unit.declaredFragment == definingFragment) {
      defining = unitContext;
    }
  }
  defining ??= allUnits.firstOrNull;
  if (defining == null) {
    return const <String, List<Diagnostic>>{};
  }
  final package = library.context.contextRoot.workspace.findPackageFor(
    library.kind.file.path,
  );
  final context = RuleContextWithResolvedResults(
    allUnits,
    defining,
    result.element.typeProvider,
    result.element.typeSystem,
    package,
  );
  final visitRegistry = RuleVisitorRegistryImpl(enableTiming: false);
  for (final rule in rules) {
    rule.registerNodeProcessors(visitRegistry, context);
  }
  for (final unit in allUnits) {
    // Upstream skips units outside the analyzed set (`isAnalyzed`): a
    // library's parts can sit outside it (options-excluded), and visiting
    // them is skipped the same way — their findings would drop at the
    // wire-path mapping anyway.
    if (!library.context.contextRoot.isAnalyzed(unit.file.path)) {
      continue;
    }
    for (final rule in rules) {
      rule.reporter = unit.diagnosticReporter;
    }
    context.currentUnit = unit;
    unit.unit.accept(
      AnalysisRuleVisitor(visitRegistry, shouldPropagateExceptions: true),
    );
  }
  context.currentUnit = defining;
  AnalysisRuleVisitor(visitRegistry).afterLibrary();

  return {
    for (final unit in allUnits) unit.file.path: listeners[unit]!.diagnostics,
  };
}

/// The analysis context of [collection] that owns [path], or null when no
/// context claims it (the locator roots every included path, so a miss
/// means the file sat outside every root the set produced — upstream's
/// `isAnalyzed` gate answers the same way by dropping the finding).
DriverBasedAnalysisContext? _contextFor(
  AnalysisContextCollectionImpl collection,
  String path,
) {
  for (final context in collection.contexts) {
    if (context.contextRoot.isAnalyzed(path)) {
      return context;
    }
  }
  return null;
}

/// Registers [rules] on one file and visits it: the official per-library
/// shape (`registerNodeProcessors` once, one visitor over the unit,
/// `afterLibrary` at the end), with the per-unit reporter the official
/// server assigns before each visit. Returns the file's diagnostics in
/// emission order. This is the parse-only path: the file is its own
/// library, exactly what it was before resolution entered the host.
List<Diagnostic> _runRulesOnFile(
  List<AbstractAnalysisRule> rules, {
  required String path,
  required String content,
  required resource.File file,
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

/// The absolute form of a wire path (the analyzer's memory provider and the
/// context collection both demand absolute paths); a no-op for
/// already-absolute paths. Wire paths are the engine's workspace-relative
/// or absolute forms without `..` segments, so no normalization past the
/// cwd join is needed.
String _absolute(String path) => io.File(path).absolute.path;

/// Converts one analyzer diagnostic to the wire row under [wirePath] (the
/// analyzed-set identity the finding belongs to — the provider works in
/// absolute paths, the wire does not): lower-case code name, severity
/// override first (the config's per-code re-leveling, official
/// `_configuredSeverity`), declared severity otherwise, UTF-16 offsets.
/// Offsets are clamped non-negative here — the rows are untrusted plugin
/// output, and a negative row would fail the wire's
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
