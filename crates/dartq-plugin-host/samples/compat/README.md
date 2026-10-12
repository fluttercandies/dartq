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
  top-level `plugin` variable, a `Plugin` subclass, one real analyzer
  `AnalysisRule`, one `CorrectionProducer`-shaped fix.
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

- **Rule execution (T5)**: `boolean_literal_review` is a real analyzer lint
  rule (an `AnalysisRule` subclass reporting the `boolean_literal` code at
  every boolean literal). It is
  disabled by default: the core enables it per cycle via
  `notification/plugin_config` (`enabled_rules`), and the host then executes
  it over the cycle's analyzed files, parsing them itself (overlays first —
  the core owns the open-document text). The findings ride back as plugin
  diagnostics; a per-code severity override re-levels them.
- **Fixes**: `request/fixes` returns the registered rule fix on the plugin's
  own finding (the finding's range replaced with `/* compat: fixed */`)
  followed by the message-only assist — prioritized changes with concrete
  edits, best priority first.
- **UTF-16 positions**: both faces carry offsets through untouched — a
  range after an emoji (`😀`, a surrogate pair) round-trips in UTF-16 code
  units end to end.
- **Crash**: launched with the `crash` argument, the plugin throws on the
  first producer interaction — the unhandled error terminates the process,
  and the core must report `dartq/plugin_crashed` without following it down.

## Porting guide: official API → dartq

| `analysis_server_plugin` concept | dartq equivalent | Notes |
| --- | --- | --- |
| `Plugin` (`lib/plugin.dart`): `name`, `register`, `start`, `shutDown` | `Plugin` (`lib/src/plugin.dart`): identical lifecycle | `register` may be sync or async on both sides. |
| Top-level `plugin` variable in `lib/main.dart` | Same convention (`samples/compat/lib/main.dart`) | The entrypoint plays the integrating-host role. |
| `PluginRegistry.registerAssist(ProducerGenerator)` | `PluginRegistry.registerAssist` | Same signature shape. |
| `PluginRegistry.registerFixForRule(DiagnosticCode, generator)` | `PluginRegistry.registerFixForRule(String code, generator)` | The dartq face keys by the code's lower-case name (what the official registry stores); pass `myCode.lowerCaseName`. |
| `AnalysisRule` (analyzer type): `name`/`description`/`diagnosticCode`, `registerNodeProcessors`, `reportAtNode` | The same real analyzer rule, registered via `PluginRegistry.registerLintRule` / `registerWarningRule` | The host executes the rule over the cycle's analyzed files, parsing them itself (parse-only; `canUseParsedResult` rules). Enablement flows from the analysis options through the core's `notification/plugin_config`; the census rides the `initialize` reply. |
| `CorrectionProducer`: `priority`, `message`, `compute(FixContext)` | `Producer`: `priority`, `message`, `compute(ProducerContext)` | The context carries `(file, offset, length)` in UTF-16 units; AST/type access lands with the Query API. |
| Fix kinds ordering (`DartFixKindPriority`) | `PrioritizedChange.priority` (lower sorts first) | Message-only changes (empty `edits`) are suggestions. |
| Rule enablement via `analysis_options.yaml` (`plugins: <name>.diagnostics:`) | Handled by the core: it parses the section, resolves it against the plugin's census (lint rules disabled unless enabled, warning rules the reverse), and pushes `notification/plugin_config` | Severity overrides (`true`/`false`/severity string) re-level the rule's rows. |
| `PluginServer` version handshake (`versionCheck`) | `initialize` request/reply: `protocol_version` + `capabilities` + `rules` census | The effective capability set is the intersection; unknown names mean "not enabled", never an error. |
| Plugin registration via the analysis server's plugin list | `dartq_plugin.json` manifest + the `<workspace>/plugins/` convention, or explicit override paths | The manifest adds resource budgets (`timeouts`, `memory_budget_mb`) the core enforces (PRD section 14.3). |
| Plugin isolate crash surfaces in the analyzer diagnostics pages | Crash isolation: the core reports one `dartq/plugin_crashed` (or `dartq/plugin_timeout`) diagnostic and keeps running | Reproduce with the sample's `crash` argument. A throwing rule propagates exactly like the official server's `shouldPropagateExceptions: true`. |
| Server→plugin analysis roots / priority files | `notification/engine_diagnostics` (one batch per analyze cycle, carrying the analyzed file set) | The v1 wire is dartq's own (the official `analyzer_plugin` protocol is pre-1.0 and adapted at this layer). |

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
