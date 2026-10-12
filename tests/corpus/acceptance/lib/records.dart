// Valid Dart 3.15 acceptance corpus — records (literals, type annotations,
// positional/named/mixed shapes, positional and named field access, nested
// records, the empty record, equality-bearing usage).
//
// Provenance: shapes modeled on the official language tests
// upstream Dart SDK source, tests/language/records/simple/literals_and_field_access_test.dart
// and upstream Dart SDK source, tests/language/record_type_test.dart (SDK main @
// e6f4207a). Covered by scripts/acceptance-gate.sh (zero-diagnostics bar,
// `dartq check` machine face).

(int, int) swap((int, int) pair) => (pair.$2, pair.$1);

({String name, int age}) person() => (name: 'ada', age: 36);

(int, {String tag}) mixed(int n) => (n, tag: 'n$n');

void main() {
  final positional = (1, 'one');
  final named = (x: 2, y: 3);
  final both = (4, label: 'four');
  final nested = (positional, named);
  final swapped = swap((7, 8));
  final p = person();
  final m = mixed(5);
  final empty = ();
  final records = <Object>[
    positional,
    named,
    both,
    nested,
    swapped,
    p,
    m,
    empty,
  ];

  if (positional.$1 +
          named.x +
          both.$1 +
          both.label.length +
          nested.$2.y +
          swapped.$1 +
          p.age +
          p.name.length +
          m.$1 +
          m.tag.length +
          records.length !=
      21) {
    throw StateError('unreachable');
  }
}
