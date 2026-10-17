// The dartq launch face for this vendored plugin (the manifest's
// `entrypoint`). The dartq core spawns `dart bin/dartq_serve.dart` inside
// this package directory and serves wire v1 over stdio; `startPlugin`
// reads `dartq_plugin.json` from the working directory. Recipe per the
// in-repo analysis_server_plugin import shim's README: the plugin's own
// `lib/main.dart` top-level `plugin` variable, served unchanged.
import 'package:analysis_server_plugin/plugin.dart';

import 'package:leancode_lint/main.dart';

Future<void> main(List<String> args) async {
  await startPlugin(plugin);
}
