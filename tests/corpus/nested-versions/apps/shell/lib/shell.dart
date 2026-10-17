// Hub of the 3.7 shell member. Imports across every dependency family
// of the corpus in one file: two workspace siblings (core, deep_ui),
// the modern vendored major (ext_lib, 3.0) and the legacy vendored
// package (legacy_lib, 2.19). Four packages at four settled language
// versions live in one root resolution; this file is the positive
// cross-version edge of the import graph.
//
// Expectation: no errors at 3.7. `visits` draws the shell-override-set
// prefer_final_locals lint (var that is never reassigned) — the pin that
// apps/shell/analysis_options.yaml really applies to this subtree and
// only here.

import 'package:core/core.dart';
import 'package:deep_ui/deep_ui.dart';
import 'package:ext_lib/ext_lib.dart';
import 'package:legacy_lib/legacy_lib.dart';

const String shellBanner = 'shell/3.7';

String dashboard() {
  var visits = 3;
  final panel = Panel(title: 'main', badge: kCoreLimit);
  final greeting = ExtGreeter.greet('shell');
  final legacy = LegacyGreeter.greetLegacy('shell');
  return '$shellBanner $panel $greeting $legacy visits=$visits';
}
