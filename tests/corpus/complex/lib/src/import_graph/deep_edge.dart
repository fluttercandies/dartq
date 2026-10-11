/// Deep import chain, level 3: consumes the report through a package: URI
/// import (the canonical in-package form) plus the relative chain.
library;

import 'package:corpus_complex/src/import_graph/deep_core.dart';

import 'deep_upper.dart';

/// Facade for the reporting endpoint; the package import proves that
/// self-package URIs resolve against the materialized package config.
class EdgeReportFacade {
  final DeepChainReport report;

  EdgeReportFacade(this.report);

  String get headline => report.headline();

  /// Emits one line per edge, sorted for deterministic snapshots.
  List<String> emitLines() {
    final lines = <String>[];
    for (final edge in report.edges) {
      lines.add(edge.toString());
    }
    lines.sort();
    return lines;
  }

  /// Depth of the tallest node the report has seen; falls back to a fresh
  /// empty node when the report is empty.
  int maxDepth() {
    if (report.edges.isEmpty) {
      return DeepNode('empty').depth;
    }
    var best = 0;
    for (final edge in report.edges) {
      best = edge.from.depth > best ? edge.from.depth : best;
      best = edge.to.depth > best ? edge.to.depth : best;
    }
    return best;
  }
}

/// Formats a chain headline from raw counts; exists so the deferred import
/// in deferred_report.dart has a plain function to touch.
String stubbedHeadline(int nodeCount, int edgeCount) =>
    'chain of $nodeCount nodes, $edgeCount edges';
