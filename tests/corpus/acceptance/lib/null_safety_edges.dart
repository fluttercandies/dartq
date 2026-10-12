// Valid Dart 3.15 acceptance corpus — null-safety edges (local and private-final field promotion, ?? / ??= / ?. / ! operators, null-aware chains with fallbacks, is/is! narrowing, definite assignment of late locals).
//
// Provenance: shapes modeled on the official language tests
// .third/dart-sdk/tests/language/null/, tests/language/null_aware/, tests/language/type_promotion/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
class User {
  final String? nickname;

  User(this.nickname);
}

class Account {
  final String? _email;

  Account(this._email);

  bool get hasPromotedEmail => _email != null && _email.isNotEmpty;

  String get emailLocal {
    final email = _email;
    if (email == null) {
      return 'unknown';
    }
    return email.split('@').first;
  }
}

String viaLocal(String? text) {
  if (text == null || text.isEmpty) {
    return '<empty>';
  }
  return text.substring(0, 1);
}

String viaWhile(Object? value) {
  while (value is String) {
    return value.toUpperCase();
  }
  return '<none>';
}

int chainedNullAware(User? user) =>
    user?.nickname?.length ?? user?.hashCode ?? 0;

int bangOperator(int? maybe) => maybe! + 1;

int coalesceAssign(int? seed) {
  var value = seed;
  value ??= 7;
  return value;
}

void definiteAssignment() {
  late int assigned;
  final seed = 3;
  if (seed > 0) {
    assigned = seed;
  } else {
    assigned = 0;
  }
  if (assigned + seed == 6) {}
}

void main() {
  final user = User('nick');
  final anon = User(null);
  final account = Account('a@b.c');
  final empty = Account(null);
  final values = <Object?>[
    viaLocal(null),
    viaLocal('ada'),
    viaWhile('str'),
    viaWhile(9),
    chainedNullAware(user),
    chainedNullAware(anon),
    chainedNullAware(null),
    bangOperator(null),
    coalesceAssign(null),
    coalesceAssign(2),
    account.hasPromotedEmail,
    account.emailLocal,
    empty.hasPromotedEmail,
    empty.emailLocal,
  ];
  definiteAssignment();
  if (values.length != 14) {
    throw StateError('unreachable');
  }
}
