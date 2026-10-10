# compat: the compatibility sample plugin

A real dartq plugin, shaped like an official `analysis_server_plugin`
package, running on the dartq plugin host (`dartq_plugin_host`). This
directory is the seed of the porting guide: it shows, on a working sample,
how every concept of the official plugin API maps onto the dartq plugin
surface — read it next to
`.third/dart-sdk/pkg/analysis_server_plugin/doc/writing_a_plugin.md`.

## Layout

- `dartq_plugin.json` — the plugin manifest the dartq core discovers
  (identity, entrypoint, capabilities, resource budgets).
- `lib/main.dart` — the plugin sources, in the official convention: a
  top-level `plugin` variable, a `Plugin` subclass, one analysis rule, one
  `CorrectionProducer`-shaped fix.
- `bin/compat_plugin.dart` — the launchable entrypoint (the manifest's
  `entrypoint`); plays the integrating-host role the official analysis
  server plays: it imports `lib/main.dart` and runs `plugin` over one wire
  session.

The sample lives inside the `dartq_plugin_host` package (like the
`echo`/`live`/`slow` fixtures in `bin/`), so `package:dartq_plugin_host`
imports resolve without an extra `pub get`. A plugin in a real workspace is
its own Dart package with a path/git dependency on `dartq_plugin_host`; only
the package boundary differs, not the code.

## What the plugin does

Deterministic by design, so tests can pin every face of the protocol:

- **Diagnostics**: for each engine diagnostics batch the core pushes
  (`notification/engine_diagnostics`), the `CompanionRule` reports one
  `compat/companion` finding per engine row on a `.dart` file, at the same
  UTF-16 range — the diagnostics flow, over a file set, in arrival order.
- **Fixes**: `request/fixes` returns the registered range fix (the
  requested UTF-16 range replaced with `/* compat: fixed */`) followed by a
  message-only suggestion — prioritized changes with concrete edits.
- **UTF-16 positions**: both faces carry offsets through untouched; a range
  after an emoji (`😀`, a surrogate pair) round-trips in UTF-16 code units
  end to end.
- **Crash**: launched with the `crash` argument, the plugin throws on the
  first post-handshake interaction — the unhandled error terminates the
  process, and the core must report `dartq/plugin_crashed` without
  following it down.

## Porting guide: official API → dartq

| `analysis_server_plugin` concept | dartq equivalent | Notes |
| --- | --- | --- |
| `Plugin` (`lib/plugin.dart`): `name`, `register`, `start`, `shutDown` | `Plugin` (`lib/src/plugin.dart`): identical lifecycle | `register` may be sync or async on both sides. |
| Top-level `plugin` variable in `lib/main.dart` | Same convention (`samples/compat/lib/main.dart`) | The entrypoint plays the integrating-host role. |
| `PluginRegistry.registerAssist(ProducerGenerator)` | `PluginRegistry.registerAssist` | Same signature shape. |
| `PluginRegistry.registerFixForRule(DiagnosticCode, generator)` | `PluginRegistry.registerFixForRule(String code, generator)` | The rule identity is its stable string code; no analyzer dependency. |
| `AnalysisRule`: `name`/`description`/`diagnosticCode`, `reportAtOffset` | A rule class computing `PluginDiagnostic` rows over the pushed engine batch (`onEngineDiagnostics` → `notification/diagnostic`) | Protocol v1 has no AST/element context (the Query API is a later milestone, PRD section 14.2); the engine diagnostics batch is the rule's context. Rows carry UTF-16 `offset`/`length`. |
| `CorrectionProducer`: `priority`, `message`, `compute(FixContext)` | `Producer`: `priority`, `message`, `compute(ProducerContext)` | The context carries `(file, offset, length)` in UTF-16 units; AST/type access lands with the Query API. |
| Fix kinds ordering (`DartFixKindPriority`) | `PrioritizedChange.priority` (lower sorts first) | Message-only changes (empty `edits`) are suggestions. |
| `PluginServer` version handshake (`versionCheck`) | `initialize` request/reply: `protocol_version` + `capabilities` | The effective capability set is the intersection; unknown names mean "not enabled", never an error. |
| Plugin registration via the analysis server's plugin list | `dartq_plugin.json` manifest + the `<workspace>/plugins/` convention, or explicit override paths | The manifest adds resource budgets (`timeouts`, `memory_budget_mb`) the core enforces (PRD section 14.3). |
| Plugin isolate crash surfaces in the analyzer diagnostics pages | Crash isolation: the core reports one `dartq/plugin_crashed` (or `dartq/plugin_timeout`) diagnostic and keeps running | Reproduce with the sample's `crash` argument. |
| Server→plugin analysis roots / priority files | `notification/engine_diagnostics` (one batch per analyze cycle) | The v1 wire is dartq's own (the official `analyzer_plugin` protocol is pre-1.0 and adapted at this layer). |

## Running it

The core spawns the entrypoint itself; nothing to run by hand. Discovery
follows the `<workspace>/plugins/*/dartq_plugin.json` convention, so a
workspace picks the sample up by linking the directory into its `plugins/`:

```
ln -s crates/dartq-plugin-host/samples/compat <workspace>/plugins/compat
```

Suites that exercise the sample:

- Dart (in-process, `dart test` in `crates/dartq-plugin-host`):
  `test/compat_sample_test.dart`.
- Rust (live process, `#[ignore]`d for the serial phase):
  `crates/dartq-plugin/tests/compat_live.rs`.
- Overhead benchmark axis: `harness bench --metric plugin-overhead`.
