// string-sequence family — fixture wave 2 kitchen file. The two
// concatenation rules partition cleanly: prefer_adjacent_string_
// concatenation needs BOTH operands string literals; prefer_interpolation_
// to_compose_strings needs a (literal, value) pair — the both-literal pair
// passes the interpolation rule verbatim.
// Expected rows: 5, all keyed sequences_family.dart —
// PREFER_ADJACENT_STRING_CONCATENATION (`'alpha' + 'beta'`),
// PREFER_INTERPOLATION_TO_COMPOSE_STRINGS (`'level ' + level.toString()`
// — the toString() call carries no arguments),
// UNNECESSARY_BRACE_IN_STRING_INTERPS (`'${mode}.'` — bare identifier,
// closing brace not followed by an identifier character),
// UNNECESSARY_STRING_ESCAPES (`'a\:b'` — the escaped colon needs no
// escape), UNNECESSARY_STRING_INTERPOLATIONS (`'${'raw'}'` — the sole
// interpolation expression is itself a complete string literal).
final joined = 'alpha' + 'beta';

final plain = 'a\:b';

final tagged = '${'raw'}';

void bannerUser(int level) {
  final banner = 'level ' + level.toString();
  print(banner);
}

void tagUser(String mode) {
  final tag = '${mode}.';
  print(tag);
}
