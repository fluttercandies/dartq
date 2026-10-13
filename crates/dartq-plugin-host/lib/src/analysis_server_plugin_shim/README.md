# analysis_server_plugin: the dartq import shim

A stand-in for the official `package:analysis_server_plugin` package. A
plugin written verbatim against the official API — imports it, a top-level
`plugin` variable, a `Plugin` subclass, `registerAssist` /
`registerFixForRule`, `CorrectionProducer`-shaped fixes and assists, real
analyzer `AnalysisRule` lint/warning rules — compiles and runs against this
package unchanged, on the dartq plugin host's wire v1. The Rust core and the
wire format are untouched.

The shim depends on `package:analyzer` (the same dependency the official
package carries): the rule registration faces are typed against the
analyzer's `AbstractAnalysisRule`, and the host executes registered rules
over the analyzed files it parses itself (parse-only — the wire never
carries an AST).

## Launch-path injection

The dartq core spawns each plugin as `dart <entrypoint>` inside the plugin's
package directory (`crates/dartq-plugin`, `SpawnSpec::dart`), so the plugin
package's own package resolution decides what `package:analysis_server_plugin`
means. Mapping it to this shim is one package_config entry — either
reproducibly via the plugin package's `pubspec.yaml`:

```yaml
dependency_overrides:
  analysis_server_plugin:
    path: <path-to-dartq>/crates/dartq-plugin-host/lib/src/analysis_server_plugin_shim
```

or directly in the plugin's `.dart_tool/package_config.json`:

```json
{
  "name": "analysis_server_plugin",
  "rootUri": "<path-to-dartq>/crates/dartq-plugin-host/lib/src/analysis_server_plugin_shim",
  "packageUri": "lib/",
  "languageVersion": "3.10"
}
```

The shim delegates onto `package:dartq_plugin_host`, so the plugin's package
config also needs the established `dartq_plugin_host` self-resolution entry
(the same one every sample under `samples/` already carries):

```json
{
  "name": "dartq_plugin_host",
  "rootUri": "<path-to-dartq>/crates/dartq-plugin-host",
  "packageUri": "lib/",
  "languageVersion": "3.10"
}
```

The `dartq_plugin_host` package itself consumes the shim the same way (its
`pubspec.yaml` carries the `dependency_overrides` line above), which is what
makes the in-package fixture (`test/shim_official_plugin.dart`) resolve.

The plugin's manifest entrypoint is then just the launch face:

```dart
import 'package:analysis_server_plugin/plugin.dart';

import 'package:my_plugin/main.dart';

Future<void> main(List<String> args) async {
  await startPlugin(plugin);
}
```

`startPlugin` stands in for the analysis server's generated bootstrap (the
official package has no process entrypoint; the server imports the plugin's
`lib/main.dart` and runs the top-level `plugin` over an `analyzer_plugin`
channel). It reads `dartq_plugin.json` from the working directory and serves
wire v1 over stdio; every failure — manifest, wire, producer, a throwing
rule — exits the process, which the core reports as one
`dartq/plugin_crashed` diagnostic (PRD section 14.3).

## API alignment

| Official face (upstream Dart SDK source, `pkg/analysis_server_plugin`) | Shim | Delegation |
| --- | --- | --- |
| `lib/plugin.dart` `Plugin` (`name`, `register`, `start`, `shutDown`) | Same, verbatim | `_PluginAdapter` → mirror `Plugin` → `servePlugin` lifecycle |
| `lib/registry.dart` `PluginRegistry.registerAssist(ProducerGenerator)` | Same name, shim-local `ProducerGenerator` | Kind validated (official `ArgumentError` message), generator adapted onto `dartq.PluginRegistryImpl.registerAssist` |
| `lib/registry.dart` `PluginRegistry.registerFixForRule(DiagnosticCode, ProducerGenerator)` | Same name, the analyzer's real `DiagnosticCode` | Keyed by `code.lowerCaseName` (what the official registry stores), adapted onto `dartq.PluginRegistryImpl.registerFixForRule` |
| `lib/src/registry.dart` `registerLintRule` / `registerWarningRule` (`RegistryBase`, `AbstractAnalysisRule`) | Same names, the analyzer's real rule type | Registered verbatim onto `dartq.PluginRegistryImpl.lintRules` / `warningRules`; the host runs them (T5) and reports the census in the `initialize` reply |
| `lib/src/registry.dart` `PluginRegistryImpl` kind validation + `StubCorrectionProducerContext` | Same behavior | One stub instantiation per registration; null kind is an `ArgumentError` with the official text |
| `lib/edit/dart/dart_fix_kind_priority.dart` `DartFixKindPriority` | Verbatim | Constants |
| `lib/edit/dart/correction_producer.dart` `CorrectionApplicability` | Verbatim | Data only (wire v1 reads none of it yet) |
| `lib/edit/dart/correction_producer.dart` `ProducerGenerator`, `CorrectionProducerContext`, `CorrectionProducer` (kind getters, selection members, per-request construction) | Same names, wire-v1 compute | `compute()` returns the wire's `SourceEdit` list; edits and priorities flow through `request/fixes` |
| `analyzer_plugin` `FixKind` / `AssistKind` (re-exported by official `edit/fix/fix.dart` / `edit/assist/assist.dart`) | Verbatim data shapes | `priority`/`message` become the wire change's fields; official ordering is higher-first (`Fix.compareFixes`), the wire is lower-first, so the adapter negates |
| Official `PluginServer` version handshake | Wire v1 `initialize` | `protocol_version` mismatch answers an error frame; the core converts it to `dartq/plugin_crashed` (`SessionError::RemoteError`) |

## Not mirrored (structural, pending the engine Query API, PRD section 14.2)

These faces need resolution results or `package:analyzer_plugin`'s
change-building stack, which wire v1 does not carry. A plugin touching them
fails at compile time against the shim:

- `CorrectionProducer<T extends ParsedUnitResult>`, `ResolvedCorrectionProducer`,
  `ParsedCorrectionProducer`, `MultiCorrectionProducer`,
  `CorrectionProducerWithDiagnostic`, and the AST/element members (`node`,
  `unit`, `utils`, `coveringNode`, `typeSystem`, ...).
- `compute(ChangeBuilder)` and `edit/change_builder/`; `Fix`, `Assist`
  (they wrap `SourceChange`); `edit/range_factory.dart`;
  `edit/correction_utils.dart`; the fix/assist contexts.

Rule execution is parse-only (the T4/T5 boundary): a rule declaring
`canUseParsedResult == false` is skipped by the host's runner instead of
being resolved.
