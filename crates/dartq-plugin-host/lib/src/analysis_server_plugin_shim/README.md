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
`dartq/plugin_crashed` diagnostic.

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
| `lib/edit/dart/correction_producer.dart` `ProducerGenerator`, `CorrectionProducerContext`, `CorrectionProducer` (official member set: kinds, selection, applicability, AST/resolution faces, `compute(ChangeBuilder)`) | Same names, official signatures | The producer face computes into the real `package:analyzer_plugin` `ChangeBuilder`; the wire acquisition is the adapter's — it drives `compute(builder)` per fix request and harvests the builder's `sourceChange` (official `fix_processor.dart` shape), keeping the target file's edits as the wire `SourceEdit` list |
| `lib/edit/dart/correction_producer.dart` `ParsedCorrectionProducer`, `ResolvedCorrectionProducer`, `CorrectionProducerWithDiagnostic` | Same names, official bases (`ParsedCorrectionProducer extends CorrectionProducer<ParsedUnitResult>`, `ResolvedCorrectionProducer extends CorrectionProducer<ResolvedUnitResult>`) | The wire-backed members answer real values; the AST/resolution-owned members throw a typed boundary error when reached, and `addDartFileEdit` over the wire workspace fails with the builder's own `AnalysisException` (no analysis session — the census and startup paths never call `compute`) |
| `lib/edit/change_builder/change_builder.dart` | Verbatim re-exports | The real `package:analyzer_plugin` change-builder types |
| `lib/edit/correction_utils.dart` `CorrectionUtils` | Line/text core (official bodies) | `endOfLine`, line offsets/prefixes, `getRangeText`, `getText` — the content-backed members a producer reads through `utils` |
| `analyzer_plugin` `FixKind` / `AssistKind` (re-exported by official `edit/fix/fix.dart` / `edit/assist/assist.dart`) | Verbatim re-exports (the same `package:analyzer_plugin` declarations, so both import URIs name one type) | `priority`/`message` become the wire change's fields; official ordering is higher-first (`Fix.compareFixes`), the wire is lower-first, so the adapter negates |
| Official `PluginServer` version handshake | Wire v1 `initialize` | `protocol_version` mismatch answers an error frame; the core converts it to `dartq/plugin_crashed` (`SessionError::RemoteError`) |

The official `ProducerGenerator` returns `CorrectionProducer<ParsedUnitResult>`;
the shim's `ProducerGenerator` is that exact shape, and the shim mirrors the
official single hierarchy one-to-one (`ParsedCorrectionProducer extends
CorrectionProducer<ParsedUnitResult>`; `ResolvedCorrectionProducer extends
CorrectionProducer<ResolvedUnitResult>`, a subtype through `ResolvedUnitResult
implements ParsedUnitResult`), so the constructor tearoffs the vendored
plugins register are assignable exactly as upstream. The one structural delta
from upstream: the shim's `CorrectionProducer` is not `sealed` (the
in-package fixture producers extend it directly), and the wire edit
acquisition lives in the registry adapter instead of the analysis server.

## Not mirrored (structural, pending the engine Query API)

These faces need resolution results or `package:analyzer_plugin`'s
change-building stack, which wire v1 does not carry. A plugin touching them
at registration or handshake time still works — the registration path reads
only the const kind getters — but the members themselves fail at the access
site with a typed error, and a `request/fixes` producer whose compute needs
them fails into the `dartq/plugin_crashed` containment:

- The AST/resolution-owned producer members that throw at the access site:
  `node`, `unit`, `unitResult`, `token`, `utils`, `sessionHelper`,
  `libraryResult`, `libraryElement2`, `typeProvider`, `typeSystem`,
  `inStaticContext`, and the session-coupled helpers
  (`analysisOptions`, `isEnabled`, `getCodeStyleOptions`,
  `getEnclosingFunctionBody`, `isOperatorSelected`, `getTopLevelDeclarations`,
  `librariesWithExtensions`, `inferUndefinedExpressionType`, the
  `get*Declaration` family).
- `MultiCorrectionProducer`; the fix/assist contexts
  (`dart_fix_context.dart`, `fix_context.dart`, `dart_assist_context.dart`);
  `edit/range_factory.dart` (plugins read the real `package:analyzer_plugin`
  one directly, as the official package does).

Rule execution follows the official per-library dispatch: a library runs
parsed when every enabled rule answers `canUseParsedResult`, and resolves
through a per-cycle `AnalysisContextCollection` otherwise — a rule
declaring `canUseParsedResult == false` receives its `ResolvedUnitResult`
like the official host.
