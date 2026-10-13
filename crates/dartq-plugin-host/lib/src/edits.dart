/// Edit-generation vocabulary: the types a plugin's producers compute and
/// the context they receive.
///
/// The wire shapes mirror `SourceEdit` / `PrioritizedChange` in the Rust
/// crate `dartq-plugin`; the producer surface mirrors
/// `package:analysis_server_plugin` (upstream Dart SDK source,
/// `pkg/analysis_server_plugin`). `ProducerContext`
/// grows with the engine Query API (PRD section 14.2).
library;

import 'dart:async';

import 'query_api.dart';

/// One contiguous text replacement (wire shape `SourceEdit`).
class SourceEdit {
  /// Builds an edit; [offset] and [length] are UTF-16 units.
  const SourceEdit({
    required this.offset,
    required this.length,
    this.replacement = '',
  });

  /// UTF-16 offset the replacement starts at.
  final int offset;

  /// UTF-16 length being replaced.
  final int length;

  /// The replacement text.
  final String replacement;

  /// The edit as the wire's `edits` entry.
  Map<String, Object?> toJson() => {
    'offset': offset,
    'length': length,
    'replacement': replacement,
  };
}

/// One proposed source change with a priority ordering (lower sorts first).
class PrioritizedChange {
  /// Builds a change; [edits] may be empty for a message-only suggestion.
  const PrioritizedChange({
    required this.priority,
    required this.message,
    this.edits = const [],
  });

  /// Lower priority numbers sort first (analyzer convention).
  final int priority;

  /// Human-readable title of the change.
  final String message;

  /// The concrete edits.
  final List<SourceEdit> edits;

  /// The change as the wire's `fixes` entry.
  Map<String, Object?> toJson() => {
    'priority': priority,
    'message': message,
    'edits': [for (final edit in edits) edit.toJson()],
  };
}

/// Everything a producer sees for one request. [queries] is the T4 Query
/// API facade — null when the host runs no query transport (then a
/// resolution-owned fact is simply unavailable to the producer, the
/// documented parsed-level limitation).
class ProducerContext {
  /// Builds a context for the requested UTF-16 location.
  const ProducerContext({
    required this.file,
    required this.offset,
    required this.length,
    this.queries,
  });

  /// Path of the file the request targets.
  final String file;

  /// UTF-16 offset the request targets.
  final int offset;

  /// UTF-16 length the request targets.
  final int length;

  /// The engine Query API (the reverse `request/query` face, T4): the
  /// daemon's index query kinds plus `element_at`, typed. Throws
  /// [QueryError] when the `query` capability was not negotiated.
  final QueryApi? queries;
}

/// A computed, prioritized change returned by a [ProducerGenerator].
abstract interface class Producer {
  /// Lower priority numbers sort first.
  int get priority;

  /// Human-readable title of the change.
  String get message;

  /// Computes the concrete edits.
  Future<List<SourceEdit>> compute(ProducerContext context);
}

/// Signature a plugin registers for assists and rule fixes.
typedef ProducerGenerator =
    FutureOr<Producer> Function(ProducerContext context);
