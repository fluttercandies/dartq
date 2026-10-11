import 'dart:io';

import '../lib/main.dart' as jaspr_compat;

/// The launchable entrypoint the dartq core spawns for this plugin (the
/// manifest's `entrypoint`). Plays the integrating-host role: it runs the
/// real `jaspr_lints` plugin — imported from the pinned `.third` clone via
/// the path dependency — over one wire session.
///
/// Reads frames on stdin, writes replies and emissions on stdout; never run
/// interactively.
Future<void> main(List<String> args) async {
  await jaspr_compat.run(
    input: stdin,
    send: (line) async {
      stdout.writeln(line);
      await stdout.flush();
    },
  );
}
