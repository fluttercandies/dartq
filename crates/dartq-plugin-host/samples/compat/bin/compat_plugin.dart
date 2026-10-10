import 'dart:io';

import '../lib/main.dart' as compat;

/// The launchable entrypoint the dartq core spawns for this plugin (the
/// manifest's `entrypoint`). Plays the integrating-host role the official
/// guide's analysis server plays: it imports the plugin's `lib/main.dart`
/// and runs its top-level `plugin` over one wire session.
///
/// Reads frames on stdin, writes replies and emissions on stdout; never run
/// interactively. Pass `crash` to enable the intentional-crash mode.
Future<void> main(List<String> args) async {
  await compat.run(
    args,
    input: stdin,
    send: (line) async {
      stdout.writeln(line);
      await stdout.flush();
    },
  );
}
