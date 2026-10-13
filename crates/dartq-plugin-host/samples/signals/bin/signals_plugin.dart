import 'dart:io';

import '../lib/main.dart' as signals_compat;

/// The launchable entrypoint the dartq core spawns for this plugin (the
/// manifest's `entrypoint`). Plays the integrating-host role: it runs the
/// real `signals_lint` plugin — imported from the vendored snapshot via/// the path dependency — over one wire session.
///
/// Reads frames on stdin, writes replies and emissions on stdout; never run
/// interactively.
Future<void> main(List<String> args) async {
  await signals_compat.run(
    input: stdin,
    send: (line) async {
      stdout.writeln(line);
      await stdout.flush();
    },
  );
}
