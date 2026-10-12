// Valid Dart 3.15 acceptance corpus — private named parameters (primary-constructor declaring params with private names, private initializing formals (`this._x`), inferred types, optional private named params, public-name invocation).
//
// Provenance: shapes modeled on the official language tests
// upstream Dart SDK source, tests/language/private_named_parameters/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
class Record({required final String _name, required final int _size});

extension RecordAccess on Record {
  String get name => _name;

  int get size => _size;
}

class PairFields {
  String _left;

  int _right;

  PairFields({required this._left, required this._right});

  String get left => _left;

  int get right => _right;
}

class Inferred {
  int _count;

  Inferred({required this._count});

  int get count => _count;
}

class OptionalPrivate {
  String? _label;

  OptionalPrivate({this._label});

  String? get label => _label;
}

void main() {
  final record = Record(name: 'r', size: 2);
  final pair = PairFields(left: 'l', right: 3);
  final inferred = Inferred(count: 9);
  final optional = OptionalPrivate(label: 'opt');
  final empty = OptionalPrivate();
  final items = <Object>[
    record.name,
    record.size,
    pair.left,
    pair.right,
    inferred,
    optional,
    empty,
  ];
  if (items.length != 7) {
    throw StateError('unreachable');
  }
}
