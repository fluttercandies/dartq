/// The official registration surface, mirrored verbatim, delegating onto the
/// dartq mirror registry.
///
/// Mirrors `package:analysis_server_plugin/registry.dart`
/// (`.third/dart-sdk`, `lib/registry.dart` and `lib/src/registry.dart`):
/// `registerAssist` and `registerFixForRule` with the official names, and the
/// official registration-time behavior — each generator is instantiated once
/// with a stub context to validate its kind, and a null kind is an
/// `ArgumentError` with the official message.
///
/// Deliberate deltas, each anchored:
/// - The official `PluginRegistry extends RegistryBase`
///   (`package:analyzer/src/lint/registry.dart`). The shim cannot extend the
///   analyzer's rule registry (no analyzer dependency by design), so the two
///   rule-registration names are explicit `UnsupportedError` faces: dartq
///   wire v1 has no rule-registry registration and carries no AST, so rule
///   semantics are inexpressible until the engine Query API (PRD section
///   14.2). A plugin calling them dies into the core's `dartq/plugin_crashed`
///   containment — never a hang.
/// - The official `registerFixForRule` takes the analyzer's `DiagnosticCode`.
///   The shim declares its own [DiagnosticCode] with the official const
///   constructor and name members; the registration path consumes only
///   `lowerCaseName` (the wire's rule-code key), exactly what the official
///   `PluginRegistryImpl` stores (`lib/src/registry.dart`).
library;

import 'dart:async';

import 'package:dartq_plugin_host/dartq_plugin_host.dart' as dartq;

import 'edit/dart/correction_producer.dart';
import 'plugin.dart';

/// A diagnostic code, mirroring the official `DiagnosticCode`
/// (`.third/dart-sdk`, `pkg/_fe_analyzer_shared/lib/src/base/errors.dart`):
/// the same const constructor and the name members the registration and
/// suppression paths consume. The official base is abstract (plugins
/// construct `LintCode`, an analyzer type); this shim's variant is concrete
/// so plugin code can declare its codes without the analyzer.
///
/// Faces that need the analyzer's severity/type vocabulary (`severity`,
/// `type`, and the `DiagnosticSeverity`/`DiagnosticType` enums) are omitted
/// rather than stubbed: a plugin reading them fails to compile against the
/// shim instead of failing at runtime.
final class DiagnosticCode {
  /// Initializes a newly created error code; the parameter set is the
  /// official constructor's.
  const DiagnosticCode({
    String? correctionMessage,
    this.hasPublishedDocs = false,
    this.isUnresolvedIdentifier = false,
    required String name,
    required String problemMessage,
    required String uniqueName,
  }) : _name = name,
       _uniqueName = uniqueName,
       _correctionMessage = correctionMessage,
       _problemMessage = problemMessage;

  final String _name;
  final String _uniqueName;
  final String? _correctionMessage;
  final String _problemMessage;

  /// Whether diagnostics with this code have published documentation.
  final bool hasPublishedDocs;

  /// Whether this diagnostic is caused by an unresolved identifier.
  final bool isUnresolvedIdentifier;

  /// The template used to create the correction to be displayed, or null.
  String? get correctionMessage => _correctionMessage;

  /// The name of the error code, converted to all lower case.
  String get lowerCaseName => _name.toLowerCase();

  /// The unique name of this error code, converted to all lower case.
  String get lowerCaseUniqueName => _uniqueName.toLowerCase();

  /// The name of the error code.
  String get name => _name;

  /// The template used to create the problem message to be displayed.
  String get problemMessage => _problemMessage;

  /// The unique name of this error code.
  String get uniqueName => _uniqueName;

  /// The URL of the published documentation, or null.
  String? get url =>
      hasPublishedDocs ? 'https://dart.dev/diagnostics/$lowerCaseName' : null;

  @override
  String toString() => lowerCaseUniqueName;
}

/// The registry a plugin registers its extensions with. The two mirrored
/// kinds delegate onto the dartq mirror registry; the two rule kinds are
/// explicit `UnsupportedError` faces (wire v1 carries no rule registry, PRD
/// section 14.2).
abstract class PluginRegistry {
  /// Registers this assist [generator] with the host.
  void registerAssist(ProducerGenerator generator);

  /// Registers this fix [generator] for the given diagnostic [code].
  void registerFixForRule(DiagnosticCode code, ProducerGenerator generator);

  /// Not expressible on the dartq wire v1: there is no rule-registry
  /// registration and no AST channel (PRD section 14.2 Query API). Throws,
  /// tearing the plugin down into the core's `dartq/plugin_crashed`
  /// containment.
  void registerLintRule(Object rule) => throw UnsupportedError(
    'analysis_server_plugin shim: registerLintRule has no dartq wire v1 '
    'counterpart (official anchor: package:analysis_server_plugin/registry '
    'extends RegistryBase, package:analyzer/src/lint/registry.dart; dartq '
    'PRD section 14.2)',
  );

  /// Not expressible on the dartq wire v1; see [registerLintRule].
  void registerWarningRule(Object rule) => throw UnsupportedError(
    'analysis_server_plugin shim: registerWarningRule has no dartq wire v1 '
    'counterpart (official anchor: package:analysis_server_plugin/registry '
    'extends RegistryBase, package:analyzer/src/lint/registry.dart; dartq '
    'PRD section 14.2)',
  );
}

/// The registry the shim hands to [Plugin.register]: delegates the mirrored
/// kinds onto [host], the dartq mirror registry the wire session serves from.
///
/// Mirrors the official `PluginRegistryImpl` behavior (`lib/src/registry.dart`):
/// each generator is instantiated once with
/// [StubCorrectionProducerContext.instance] and its kind is validated — a
/// null kind is an `ArgumentError` carrying the official message.
/// `extends` (the official shape) rather than `implements`: the two
/// rule-registration faces are inherited as the interface's concrete
/// throwing bodies — `implements` would take only their signatures and
/// demand fresh bodies here.
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
        file: context.file,
        offset: context.offset,
        length: context.length,
      ),
    );
    return _AdaptedProducer(producer, _face);
  }
}

final class _AdaptedProducer implements dartq.Producer {
  _AdaptedProducer(this._producer, this._face);

  final CorrectionProducer _producer;
  final _ProducerFace _face;

  @override
  int get priority => -_requiredKind().priority;

  @override
  String get message => _requiredKind().message;

  @override
  Future<List<dartq.SourceEdit>> compute(dartq.ProducerContext context) =>
      // The producer already holds its per-request context (the adapted
      // generator built it); the wire context it was reached through is the
      // same one.
      _producer.compute();

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
/// the core reports `dartq/plugin_crashed` (PRD section 14.3).
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
