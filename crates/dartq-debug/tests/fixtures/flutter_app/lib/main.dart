// A minimal Flutter app for dartq-debug's real-VM e2e tests
// (tests/real_flutter.rs). The harness copies this file over the
// template's lib/main.dart inside a freshly generated project.
//
// Every behavior the tests observe is a marker line on stdout:
//   probe:tag=<dart-define> reload=<reloadTag>  startup + define passthrough
//   build:title=<title>                          every ProbeApp.build
//   home-build:<label>                           every ProbeHome.build
//   tick:<n> echo:<2n>                           the stateful ticker
//
// The ticker rebuilds the home widget every second, so a breakpoint in
// build code keeps getting hit without any RPC trigger, and `probeEcho`
// gives hot-reload tests a breakpoint target outside the build path.

import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';

const probeTag = String.fromEnvironment('PROBE_TAG', defaultValue: 'unset');
const String reloadTag = 'v1';

void main() {
  debugPrint('probe:tag=$probeTag reload=$reloadTag');
  runApp(const ProbeApp());
}

/// Doubles `value` — a hot-reload-stable breakpoint target.
int probeEcho(int value) => value * 2;

/// Test hook: exits the app from a `Timer` so the evaluation that armed
/// the timer can return before the process dies.
void scheduleExit(int code, [int millis = 300]) {
  Timer(Duration(milliseconds: millis), () => exit(code));
}

class ProbeApp extends StatelessWidget {
  const ProbeApp({super.key});

  @override
  Widget build(BuildContext context) {
    final title = 'probe-$reloadTag';
    debugPrint('build:title=$title');
    return MaterialApp(
      title: title,
      home: const ProbeHome(),
    );
  }
}

class ProbeHome extends StatefulWidget {
  const ProbeHome({super.key});

  @override
  State<ProbeHome> createState() => _ProbeHomeState();
}

class _ProbeHomeState extends State<ProbeHome> {
  int ticks = 0;

  @override
  void initState() {
    super.initState();
    Timer.periodic(const Duration(seconds: 1), (timer) {
      ticks += 1;
      debugPrint('tick:$ticks echo:${probeEcho(ticks)}');
    });
  }

  @override
  Widget build(BuildContext context) {
    final label = 'ticks $ticks ($reloadTag)';
    debugPrint('home-build:$label');
    return Scaffold(
      body: Center(child: Text(label)),
    );
  }
}
