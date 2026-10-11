/// Deep import chain, level 0: no imports, pure domain-free node type.
library;

/// A node in the deep traversal fixture; recursion gives the graph a real
/// shape for resolution depth checks.
class DeepNode {
  final String id;
  final List<DeepNode> children;

  const DeepNode(this.id, {this.children = const []});

  int get depth => children.isEmpty ? 1 : 1 + longestChildDepth();

  int longestChildDepth() {
    var best = 0;
    for (final child in children) {
      if (child.depth > best) {
        best = child.depth;
      }
    }
    return best;
  }

  Iterable<String> walk() sync* {
    yield id;
    for (final child in children) {
      yield* child.walk();
    }
  }
}
