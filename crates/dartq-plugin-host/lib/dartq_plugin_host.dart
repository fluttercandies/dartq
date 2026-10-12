/// Host-side Dart package of the dartq plugin protocol (PRD section 14).
///
/// Mirrors the observable interface shape of `analysis_server_plugin`
/// ([Plugin], [PluginRegistry]) and implements the server side of the wire
/// contract specified by the Rust crate `dartq-plugin`. The core spawns one
/// process per plugin; a plugin package's entry point hands its [Plugin]
/// instance to [servePlugin] — the integration that registers, starts, and
/// serves the plugin's producers on the wire — while [runPluginHost] is the
/// bare hook-less loop underneath.
library;

export 'src/capabilities.dart' show Capability, CapabilitySet;
export 'src/census.dart' show RuleCodeInfo, RuleCensus, RuleKind;
export 'src/cycle_config.dart'
    show OverlayEntry, PluginCycleConfig, severityByName;
export 'src/diagnostic.dart' show PluginDiagnostic, Severity;
export 'src/edits.dart'
    show
        Producer,
        ProducerContext,
        ProducerGenerator,
        PrioritizedChange,
        SourceEdit;
export 'src/frame.dart'
    show
        PluginFrame,
        PluginProtocolException,
        decodeFrame,
        maxFrameBytes,
        methodDiagnosticNotification,
        methodEngineDiagnostics,
        methodInitialize,
        methodPluginConfig,
        methodRequestFixes,
        methodShutdown,
        protocolVersion;
export 'src/host.dart' show runPluginHost, servePlugin;
export 'src/plugin.dart' show Plugin;
export 'src/plugin_config.dart'
    show
        PluginConfig,
        PluginConfigException,
        PluginTimeouts,
        defaultCallbackTimeoutMs,
        defaultMemoryBudgetMb,
        defaultRequestRetries,
        defaultRequestTimeoutMs;
export 'src/plugin_session.dart'
    show
        EngineBatch,
        EngineDiagnosticsHandler,
        FixesHandler,
        PluginConfigHandler,
        PluginSession,
        SessionState;
export 'src/registry.dart' show PluginRegistry, PluginRegistryImpl;
export 'src/rule_runner.dart' show runRules;
