// Valid Dart 3.15 acceptance corpus — string literals (single/double quotes, raw strings, adjacent concatenation (plain and raw-tailed), multiline/raw-multiline, simple/braced/nested interpolation, escapes, const-evaluable interpolation).
//
// Provenance: shapes modeled on the official language tests
// .third/dart-sdk/tests/language/string/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
const simple = 'hello';

const doubleQuoted = "world";

const rawSingle = r'C:\path\no\escapes';

const rawDouble = r"also $raw";

const adjacent = 'con' 'cat' 'enated';

const adjacentRaw = 'mixed' r'-raw-tail';

const multiline = '''
line one
line "two" with 'quotes'
''';

const rawMultiline = r'''
raw \n stays
''';

const interpolated = 'value: $simple';

const expression = 'len: ${simple.length}';

final nested = 'outer ${simple.isEmpty ? 'empty' : 'full: $simple'}';

String interpolateTwice(String who, int n) => '$who said $n times';

String braces(Object o) => '{${o.toString()}}';

const escapes = 'tab\t newline\n quote\' double\" backslash\\ dollar\$ end';

const empty = '';
const justQuote = "'";

void main() {
  final dynamic1 = interpolateTwice('ada', 2);
  final dynamic2 = braces([1, 2]);
  final parts = <String>[
    simple,
    doubleQuoted,
    rawSingle,
    rawDouble,
    adjacent,
    adjacentRaw,
    multiline,
    rawMultiline,
    interpolated,
    expression,
    nested,
    dynamic1,
    dynamic2,
    escapes,
    empty,
    justQuote,
  ];
  if (parts.length != 16) {
    throw StateError('unreachable');
  }
}
