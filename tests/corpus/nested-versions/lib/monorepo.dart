// Root package hub file at language version 3.7 (package default from
// `sdk: ^3.7.0`). Pulls one symbol from each workspace member through
// the single root package_config resolution: the root's own lib/, a
// depth-2 member (shell) and its dependency (core) in one import graph.
//
// Expectation: analyzes clean at 3.7 (the root options set has
// always_declare_return_types on; every declaration here is typed).

import 'package:core/core.dart';
import 'package:shell/shell.dart';

void main() {
  print(describeCore(kCoreLimit));
  print(dashboard());
  print(shellBanner);
}
