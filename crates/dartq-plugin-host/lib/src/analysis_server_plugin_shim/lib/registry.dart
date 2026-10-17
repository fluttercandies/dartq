/// The official registration surface, mirrored verbatim, delegating onto the
/// dartq mirror registry.
///
/// Mirrors `package:analysis_server_plugin/registry.dart`
/// (upstream Dart SDK source, `lib/registry.dart` and `lib/src/registry.dart`):
/// `registerAssist`, `registerFixForRule`, `registerLintRule`, and
/// `registerWarningRule` with the official names, and the official
/// registration-time behavior — each generator is instantiated once with a
/// stub context to validate its kind, and a null kind is an `ArgumentError`
/// with the official message.
///
/// Deliberate deltas, each anchored:
/// - The rule kinds register the real analyzer `AbstractAnalysisRule`
///   objects onto the host registry (T5: the host executes them over
///   analyzer ASTs it parses itself — the same dependency the official
///   package carries).
/// - The edit kinds delegate the mirrored generator shape onto the host
///   registry (the official `CorrectionProducer` kinds validated here, then
///   adapted to the wire session's producer face).
library;

import 'dart:async';

import 'package:analyzer/analysis_rule/analysis_rule.dart'
    show AbstractAnalysisRule;
import 'package:analyzer/dart/analysis/results.dart' show ParsedUnitResult;
import 'package:analyzer/dart/analysis/session.dart' show AnalysisSession;
import 'package:analyzer/error/error.dart' show DiagnosticCode;
import 'package:analyzer/file_system/file_system.dart' show ResourceProvider;
import 'package:analyzer/file_system/physical_file_system.dart'
    show PhysicalResourceProvider;
import 'package:analyzer_plugin/utilities/change_builder/change_builder_core.dart'
    show ChangeBuilder;
import 'package:analyzer_plugin/utilities/change_builder/change_workspace.dart'
    show ChangeWorkspace;
import 'package:dartq_plugin_host/dartq_plugin_host.dart' as dartq;

import 'edit/dart/correction_producer.dart';
import 'plugin.dart';

/// The registry a plugin registers its extensions with. The mirrored kinds
/// delegate onto the dartq mirror registry; the two rule kinds register the
/// real analyzer rules onto the host registry's rule surface (executed by
/// the host's rule runner over analyzer ASTs it parses itself).
abstract class PluginRegistry {
  /// Registers this assist [generator] with the host.
  void registerAssist(ProducerGenerator generator);

  /// Registers this fix [generator] for the given diagnostic [code].
  void registerFixForRule(DiagnosticCode code, ProducerGenerator generator);

  /// Registers this lint [rule] with the host: disabled by default, enabled
  /// via the analysis options' rule configs (official `RegistryBase`
  /// semantics, resolved by the core against the registration census).
  void registerLintRule(AbstractAnalysisRule rule);

  /// Registers this warning [rule] with the host: enabled by default,
  /// disabled via the analysis options' rule configs.
  void registerWarningRule(AbstractAnalysisRule rule);
}

/// The registry the shim hands to [Plugin.register]: delegates the mirrored
/// kinds onto [host], the dartq mirror registry the wire session serves from.
///
/// Mirrors the official `PluginRegistryImpl` behavior (`lib/src/registry.dart`):
/// each generator is instantiated once with
/// [StubCorrectionProducerContext.instance] and its kind is validated — a
/// null kind is an `ArgumentError` carrying the official message. `extends`
/// (the official shape) rather than `implements`.
final class PluginRegistryImpl extends PluginRegistry {
  /// Builds a registry delegating onto [host].
  PluginRegistryImpl(this.host);

  /// The dartq registry the fix and assist delegations land in (the source
  /// of truth the wire session serves). Held as the abstract face
  /// `dartq.Plugin.register` hands out — the delegations only need
  /// `registerAssist`/`registerFixForRule`; the host passes its concrete
  /// `dartq.PluginRegistryImpl`, the same interface/impl shape as the
  /// official `PluginRegistryImpl extends PluginRegistry`.
  final dartq.PluginRegistry host;

  @override
  void registerAssist(ProducerGenerator generator) {
    final producer = generator(context: StubCorrectionProducerContext.instance);
    final assistKind = producer.assistKind;
    if (assistKind == null) {
      throw ArgumentError.value(
        generator,
        'generator',
        "Assist producer '${producer.runtimeType}' must declare a non-null "
            "'assistKind'.",
      );
    }
    host.registerAssist(_AdaptedGenerator(generator, _ProducerFace.assist));
  }

  @override
  void registerFixForRule(DiagnosticCode code, ProducerGenerator generator) {
    final producer = generator(context: StubCorrectionProducerContext.instance);
    final fixKind = producer.fixKind;
    if (fixKind == null) {
      throw ArgumentError.value(
        generator,
        'generator',
        "Fix producer '${producer.runtimeType}' must declare a non-null "
            "'fixKind'.",
      );
    }
    // The official registry keys fixes by `code.lowerCaseName`
    // (`lib/src/registry.dart`, `fixKinds.putIfAbsent(...)`); the wire's
    // engine diagnostics carry their codes in the same lower-case form.
    host.registerFixForRule(
      code.lowerCaseName,
      _AdaptedGenerator(generator, _ProducerFace.fix),
    );
  }

  @override
  void registerLintRule(AbstractAnalysisRule rule) =>
      host.registerLintRule(rule);

  @override
  void registerWarningRule(AbstractAnalysisRule rule) =>
      host.registerWarningRule(rule);
}

/// Which kind face an adapted generator reads off each per-request producer.
enum _ProducerFace { assist, fix }

/// Bridges one official-style generator onto the dartq mirror generator
/// signature: builds the shim context from the wire request and instantiates
/// a fresh producer per request (official semantics — producers carry
/// per-diagnostic state, so the constructor runs per fix request).
final class _AdaptedGenerator {
  _AdaptedGenerator(this._generator, this._face);

  final ProducerGenerator _generator;
  final _ProducerFace _face;

  FutureOr<dartq.Producer> call(dartq.ProducerContext context) {
    final producer = _generator(
      context: CorrectionProducerContext(
        path: context.file,
        offset: context.offset,
        length: context.length,
      ),
    );
    return _AdaptedProducer(producer, _face);
  }
}

final class _AdaptedProducer implements dartq.Producer {
  _AdaptedProducer(this._producer, this._face);

  final CorrectionProducer<ParsedUnitResult> _producer;
  final _ProducerFace _face;

  @override
  int get priority => -_requiredKind().priority;

  @override
  String get message => _requiredKind().message;

  @override
  Future<List<dartq.SourceEdit>> compute(dartq.ProducerContext context) async {
    // The official acquisition path
    // (`pkg/analysis_server_plugin/lib/src/correction/fix_processor.dart`):
    // construct one builder per producer run, drive the producer's compute,
    // then read the builder's `sourceChange` and keep the target file's
    // edits. Generic file edits run fully over this workspace; a producer
    // editing a Dart file through `addDartFileEdit` needs the analysis
    // session the wire does not carry and fails contained (the engine Query
    // API lands that channel).
    final producer = _producer;
    final builder = ChangeBuilder(
      workspace: _WireWorkspace(),
      defaultEol: producer.defaultEol,
    );
    await producer.compute(builder);
    final change = builder.sourceChange;
    return [
      for (final fileEdit in change.edits)
        if (fileEdit.file == context.file)
          for (final edit in fileEdit.edits)
            dartq.SourceEdit(
              offset: edit.offset,
              length: edit.length,
              replacement: edit.replacement,
            ),
    ];
  }

  /// The producer's kind for this face. A producer whose kind was non-null at
  /// registration but null here is misbehaving; the error tears the session
  /// down into the `dartq/plugin_crashed` containment.
  ({int priority, String message}) _requiredKind() {
    switch (_face) {
      case _ProducerFace.assist:
        final kind = _producer.assistKind;
        if (kind == null) {
          throw ArgumentError.value(
            _producer,
            'producer',
            "Producer '${_producer.runtimeType}' declared a non-null kind at "
                "registration but returned null at request time.",
          );
        }
        return (priority: kind.priority, message: kind.message);
      case _ProducerFace.fix:
        final kind = _producer.fixKind;
        if (kind == null) {
          throw ArgumentError.value(
            _producer,
            'producer',
            "Producer '${_producer.runtimeType}' declared a non-null kind at "
                "registration but returned null at request time.",
          );
        }
        return (priority: kind.priority, message: kind.message);
    }
  }
}

/// Serves one wire session for a [Plugin] written against this shim: the
/// integration [startPlugin] plays. Wraps the plugin and its registry into
/// the dartq mirror types and hands them to the mirror's [dartq.servePlugin].
///
/// Every failure path — registration (`UnsupportedError`, `ArgumentError`),
/// the wire loop, a producer that throws — propagates: the process exits and
/// the core reports `dartq/plugin_crashed`.
Future<void> serveShimPlugin(
  Plugin plugin, {
  required dartq.PluginConfig config,
  required Stream<List<int>> input,
  required Future<void> Function(String line) send,
}) async {
  await dartq.servePlugin(
    _PluginAdapter(plugin),
    config: config,
    input: input,
    send: send,
  );
}

final class _PluginAdapter extends dartq.Plugin {
  _PluginAdapter(this._plugin);

  final Plugin _plugin;

  @override
  String get name => _plugin.name;

  @override
  FutureOr<void> register(dartq.PluginRegistry registry) =>
      _plugin.register(PluginRegistryImpl(registry));

  @override
  FutureOr<void> start() => _plugin.start();

  @override
  FutureOr<void> shutDown() => _plugin.shutDown();
}

/// The workspace the adapter hands the real `package:analyzer_plugin` change
/// builder: the sidecar's physical file system (the files the host analyzes)
/// and no analysis session. Generic file edits run fully; `getSession`
/// answers null, so a producer editing a Dart file through
/// `addDartFileEdit` gets the builder's own `AnalysisException` instead of a
/// fabricated fix — the engine Query API lands the session channel.
///
/// One fresh instance per producer run, matching the official processor's
/// per-run builder construction; the workspace itself is stateless.
final class _WireWorkspace extends ChangeWorkspace {
  @override
  ResourceProvider get resourceProvider => PhysicalResourceProvider.INSTANCE;

  @override
  bool containsFile(String path) => true;

  @override
  AnalysisSession? getSession(String path) => null;
}
