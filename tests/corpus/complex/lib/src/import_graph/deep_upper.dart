/// Deep import chain, level 2: combines a shown import and a prefixed
/// import of the same underlying library — resolution must keep both
/// namespaces straight.
library;

import 'deep_core.dart' as bare;

import 'deep_mid.dart';

/// A report over a chain: counts nodes via the shown type, edges via the
/// prefixed one.
class DeepChainReport {
  final List<DeepEdge> edges;

  DeepChainReport(this.edges);

  int get nodeCount {
    final seen = <String>{};
    for (final edge in edges) {
      seen.add(edge.from.id);
      seen.add(edge.to.id);
    }
    return seen.length;
  }

  /// Builds a fresh node with the prefixed type for the tail of a chain.
  bare.DeepNode extendChain(bare.DeepNode tail, String nextId) {
    final next = bare.DeepNode(nextId);
    edges.add(DeepEdge(tail, next));
    return next;
  }

  String headline() => 'chain of $nodeCount nodes, ${edges.length} edges';
}
