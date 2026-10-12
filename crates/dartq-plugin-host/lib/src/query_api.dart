/// The Query API (T4): the plugin-facing face of the wire's reverse
/// direction — the plugin pulls resolution-owned engine facts (the daemon's
/// index query kinds plus `element_at`) over `request/query` round trips.
///
/// The reply rows are the frozen query schema's own rows, schema-identical
/// with the daemon's `dartq/query` answers; there is no pagination in v1
/// (the 16 MiB frame bound is the response ceiling). Parsed-level AST
/// context stays host-local — this face is only for what the host cannot
/// compute itself.
///
/// Mirrors the request/reply shapes of `session.rs` in the Rust crate
/// `dartq-plugin`; both sides must change together.
library;

import 'frame.dart';

/// Which engine fact a query asks for. The first five are the daemon's
/// index query kinds; `elementAt` is the offset-keyed face. The lowercase
/// spellings are the wire names.
enum QueryKind {
  /// Declaration rows plus every resolved reference to them.
  symbol('symbol'),

  /// The reference rows of the symbol face.
  references('references'),

  /// The invocation-shaped subset of the reference rows.
  callers('callers'),

  /// The queried type's subtypes / the queried member's overriding rows.
  implementations('implementations'),

  /// The declaring library's importers.
  dependencies('dependencies'),

  /// Offset-keyed element lookup: `{file, offset}` → the element rows the
  /// position denotes (its own declaration, or the declaration its use
  /// resolves to).
  elementAt('element_at');

  const QueryKind(this.wireName);

  /// The lowercase name used on the wire.
  final String wireName;
}

/// One reverse query the plugin sends.
class QueryRequest {
  /// Builds a name-keyed query ([QueryKind.symbol] family).
  const QueryRequest.named(this.kind, this.name)
    : file = null,
      offset = null;

  /// Builds the offset-keyed query ([QueryKind.elementAt]).
  const QueryRequest.at(this.file, this.offset)
    : kind = QueryKind.elementAt,
      name = null;

  /// Which fact is requested.
  final QueryKind kind;

  /// The queried symbol name (name-keyed kinds).
  final String? name;

  /// The queried file, in the analyzed set's wire spelling
  /// ([QueryKind.elementAt]).
  final String? file;

  /// UTF-16 offset into [file] ([QueryKind.elementAt]).
  final int? offset;

  /// The request as the wire frame's `params`. Extra fields the core
  /// ignores (additive evolution, the same posture the rest of the wire
  /// takes).
  Map<String, Object?> toParams() => {
    'kind': kind.wireName,
    if (name != null) 'name': name,
    if (file != null) 'file': file,
    if (offset != null) 'offset': offset,
  };
}

/// A declined or failed query: the wire's error channel
/// (`{"error": message}`) surfaced as an exception. The session stays up —
/// a declined query is not a contract break; the producer decides how to
/// degrade.
class QueryError implements Exception {
  /// Builds the error with the core's message.
  const QueryError(this.message);

  /// The core's failure description.
  final String message;

  @override
  String toString() => 'QueryError: $message';
}

/// One declaration row of a reply — the frozen schema's
/// `declarations`/`implementations` shape (`element_at` returns the same
/// rows under `elements`). Line and column are 1-based, the query face's
/// convention; `length` is the UTF-16 length of the name span.
final class DeclarationRow {
  /// Parses one row object. A malformed row is a wire-contract break —
  /// [PluginProtocolException], fatal, matching the codec's posture.
  DeclarationRow.fromJson(Map<String, Object?> json)
    : file = json['file'] as String,
      container = json['container'] as String?,
      kind = json['kind'] as String,
      name = json['name'] as String,
      line = json['line'] as int,
      column = json['column'] as int,
      length = json['length'] as int;

  /// The declaring file's path.
  final String file;

  /// The enclosing container's name; null for top-level rows.
  final String? container;

  /// The declaration family (the query face's kind grammar).
  final String kind;

  /// The declared name.
  final String name;

  /// 1-based line of the name span's start.
  final int line;

  /// 1-based UTF-16 column of the name span's start.
  final int column;

  /// UTF-16 length of the name span.
  final int length;
}

/// One resolved reference row of a reply — the frozen schema's
/// `references` shape. The use site's `offset` is UTF-16.
final class ReferenceRow {
  /// Parses one row object (see [DeclarationRow.fromJson] for the failure
  /// posture).
  ReferenceRow.fromJson(Map<String, Object?> json)
    : targetFile = json['target_file'] as String,
      kind = json['kind'] as String,
      file = json['file'] as String,
      offset = json['offset'] as int,
      length = json['length'] as int,
      qualified = json['qualified'] as String?;

  /// The referenced declaration's file.
  final String targetFile;

  /// The relation kind (read/invoked/written/…).
  final String kind;

  /// The use site's file.
  final String file;

  /// UTF-16 offset of the use site's span.
  final int offset;

  /// UTF-16 length of the use site's span.
  final int length;

  /// The qualified-name spelling (`p.fa`) when the use was resolved
  /// through an armed import prefix; null for bare uses.
  final String? qualified;
}

/// The symbol face's full answer: the declaration rows plus the resolved
/// reference rows.
final class SymbolAnswer {
  /// Builds the answer from its row lists.
  const SymbolAnswer({required this.declarations, required this.references});

  /// Every declaration of the queried name, path-then-offset order.
  final List<DeclarationRow> declarations;

  /// Every resolved reference to them.
  final List<ReferenceRow> references;
}

/// Signature of the transport a [QueryApi] rides: sends the request frame
/// and completes with the reply params (the error channel surfaces as a
/// [QueryError] from the send itself). [PluginSession.query] is the
/// canonical implementation.
typedef QueryTransport =
    Future<Map<String, Object?>> Function(QueryRequest request);

/// The session-like query facade (the T4 sub-task: producers ask the
/// engine through typed methods; each method is one `request/query` round
/// trip under the plugin's own correlation id).
final class QueryApi {
  /// Builds the facade over [transport].
  const QueryApi(this.transport);

  final QueryTransport transport;

  /// The queried name's declarations plus every resolved reference.
  Future<SymbolAnswer> symbol(String name) async {
    final params = await transport(QueryRequest.named(QueryKind.symbol, name));
    return SymbolAnswer(
      declarations: _declarationRows(params, 'declarations'),
      references: _referenceRows(params, 'references'),
    );
  }

  /// The reference rows of the symbol face.
  Future<List<ReferenceRow>> references(String name) async {
    final params = await transport(
      QueryRequest.named(QueryKind.references, name),
    );
    return _referenceRows(params, 'references');
  }

  /// The invocation-shaped subset of the reference rows.
  Future<List<ReferenceRow>> callers(String name) async {
    final params = await transport(QueryRequest.named(QueryKind.callers, name));
    return _referenceRows(params, 'references');
  }

  /// The queried type's subtypes / the queried member's overriding rows.
  Future<List<DeclarationRow>> implementations(String name) async {
    final params = await transport(
      QueryRequest.named(QueryKind.implementations, name),
    );
    return _declarationRows(params, 'implementations');
  }

  /// The declaring library's importers, path-ascending, deduplicated.
  Future<List<String>> dependencies(String name) async {
    final params = await transport(
      QueryRequest.named(QueryKind.dependencies, name),
    );
    final rows = params['dependencies'];
    if (rows is! List) {
      throw const PluginProtocolException(
        'query reply `dependencies` must be an array',
      );
    }
    return [
      for (final row in rows)
        if (row is Map) (row as Map<String, Object?>)['path'] as String,
    ];
  }

  /// The element rows the `{file, offset}` position denotes: its own
  /// declaration, or the declarations its use resolves to. A miss is an
  /// empty list.
  Future<List<DeclarationRow>> elementAt(String file, int offset) async {
    final params = await transport(QueryRequest.at(file, offset));
    return _declarationRows(params, 'elements');
  }

  List<DeclarationRow> _declarationRows(Map<String, Object?> params, String key) =>
      _rows(params, key, DeclarationRow.fromJson);

  List<ReferenceRow> _referenceRows(Map<String, Object?> params, String key) =>
      _rows(params, key, ReferenceRow.fromJson);

  static List<T> _rows<T>(
    Map<String, Object?> params,
    String key,
    T Function(Map<String, Object?>) parse,
  ) {
    final array = params[key];
    if (array is! List) {
      throw PluginProtocolException('query reply `$key` must be an array');
    }
    return [
      for (final row in array)
        parse((row as Map).cast<String, Object?>()),
    ];
  }
}
