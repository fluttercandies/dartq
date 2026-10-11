/// Deferred loading: the heavy report surface is imported `deferred as` and
/// touched only inside a runtime path, never in a const context.
library;

import 'deep_edge.dart' deferred as lazy;

/// Synchronously available summary; safe before the deferred library loads.
String stubSummary() => 'edge report not loaded';

/// Loads the deferred namespace on demand and formats through it. The
/// explicit load first is the deployment pattern; the member access below
/// is the analysis surface under test.
Future<String> snapshot({required int nodeCount, required int edgeCount}) async {
  await lazy.loadLibrary();
  return lazy.stubbedHeadline(nodeCount, edgeCount);
}
