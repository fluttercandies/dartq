/// Full-text search helpers over SKU catalogs: constant-pattern switch
/// statements, string extensions and one bounded regex use.
library;

/// Search outcome tags used by the index facade.
enum QueryKind { exact, prefix, fuzzy, malformed }

/// Classifies a raw query; constant string patterns in a switch statement.
QueryKind classifyQuery(String raw) {
  switch (raw.trim()) {
    case '':
      return QueryKind.malformed;
    case 'help' || 'h':
      return QueryKind.exact;
    case String s when s.endsWith('*'):
      return QueryKind.prefix;
    case String s when s.contains('?'):
      return QueryKind.fuzzy;
    default:
      return QueryKind.exact;
  }
}

/// Tokenizer extension over the catalog's text fields.
extension QueryText on String {
  Iterable<String> get tokens =>
      toLowerCase().split(RegExp(r'[^a-z0-9]+')).where((t) => t.isNotEmpty);

  bool matchesAll(Set<String> required) {
    final mine = tokens.toSet();
    return required.every(mine.contains);
  }
}

/// Bounded scorer: at most [limit] haystacks are scanned; the abort pattern
/// keeps the worst case linear in the bound instead of the catalog size.
List<String> topMatches({
  required Iterable<String> haystacks,
  required String needle,
  int limit = 5,
}) {
  final needleLower = needle.toLowerCase();
  final hits = <String>[];
  for (final haystack in haystacks) {
    if (hits.length >= limit) {
      break;
    }
    if (haystack.toLowerCase().contains(needleLower)) {
      hits.add(haystack);
    }
  }
  return hits;
}
