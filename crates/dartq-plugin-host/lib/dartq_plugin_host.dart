/// Host-side Dart package of the dartiq plugin protocol (PRD section 14).
///
/// Mirrors the observable interface shape of `analysis_server_plugin`
/// ([Plugin], [PluginRegistry]) and implements the server side of the wire
/// contract specified by the Rust crate `dartq-plugin`. The core spawns one
/// process running [runPluginHost] per plugin.
library;

export 'src/capabilities.dart' show Capability, CapabilitySet;
export 'src/diagnostic.dart' show PluginDiagnostic, Severity;
export 'src/edits.dart'
    show Producer, ProducerContext, ProducerGenerator, PrioritizedChange, SourceEdit;
export 'src/frame.dart'
    show
        PluginFrame,
        PluginProtocolException,
        decodeFrame,
        maxFrameBytes,
        methodDiagnosticNotification,
        methodEngineDiagnostics,
        methodInitialize,
        methodRequestFixes,
        methodShutdown,
        protocolVersion;
export 'src/host.dart' show runPluginHost;
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
    show EngineDiagnosticsHandler, FixesHandler, PluginSession, SessionState;
export 'src/registry.dart' show PluginRegistry, PluginRegistryImpl;
