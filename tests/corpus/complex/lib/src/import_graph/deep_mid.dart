/// Deep import chain, level 1: re-exports the core surface and builds the
/// first derived shape over it.
library;

import 'deep_core.dart';

export 'deep_core.dart' show DeepNode;

/// A labeled edge between two nodes; kept tiny on purpose.
class DeepEdge {
  final DeepNode from;
  final DeepNode to;
  final String label;

  const DeepEdge(this.from, this.to, {this.label = 'flows'});

  @override
  String toString() => '${from.id} --$label--> ${to.id}';
}

/// Chains [root] into a single-edge graph rooted at it.
DeepEdge chainPair(DeepNode root, DeepNode next) => DeepEdge(root, next);
