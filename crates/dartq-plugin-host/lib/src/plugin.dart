/// The plugin lifecycle every dartq plugin implements.
///
/// Mirrors the observable contract of
/// `package:analysis_server_plugin/plugin.dart` (upstream Dart SDK source,
/// `lib/plugin.dart`): a name, one registration callback, start-up, and
/// shutdown — so an official plugin reads naturally as a dartq plugin.
library;

import 'dart:async';

import 'registry.dart';

/// The base class third-party plugins extend.
abstract class Plugin {
  /// Allows const subclasses.
  const Plugin();

  /// A user-visible name, used for error-reporting purposes.
  String get name;

  /// Registers analysis rules, quick fixes, and assists. Called once by the
  /// host at start-up.
  FutureOr<void> register(PluginRegistry registry);

  /// Initializes any necessary start-up state; called once per analysis
  /// context, after [register].
  FutureOr<void> start() {}

  /// Releases resources; called before the host process exits.
  FutureOr<void> shutDown() {}
}
