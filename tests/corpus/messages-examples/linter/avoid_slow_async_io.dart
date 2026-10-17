// messages-example: linter/messages.yaml LinterLintCode.avoidSlowAsyncIo
import 'dart:io';

Future<void> g(File f) async {
  await f.exists();
}
