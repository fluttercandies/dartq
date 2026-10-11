/// Customer domain: constructor matrix (named, factory, redirecting factory,
/// private implementation), private-final field promotion and accessors.
library;

import '../core/identifiers.dart';

enum CustomerTier { bronze, silver, gold, platinum }

/// A customer aggregate. [_nickname] is a private final nullable field, so
/// null-aware checks promote it in place (Dart field promotion).
class Customer {
  final CustomerRef id;
  final String name;
  final CustomerTier tier;

  /// Nullable because it is filled lazily by the CRM sync.
  final String? _nickname;

  /// Set once by the first audit run; later attempts are ignored.
  late final String auditCode = 'AUD-${id.value}';

  int _complaintCount = 0;

  Customer._(this.id, this.name, this.tier, this._nickname);

  /// Standard signup path.
  factory Customer.signup({
    required CustomerRef id,
    required String name,
    CustomerTier tier = CustomerTier.bronze,
    String? nickname,
  }) {
    return Customer._(id, name, tier, nickname);
  }

  /// Anonymous checkout: no CRM record is created.
  factory Customer.guest() => Customer._(
      const CustomerRef.anonymous(), 'guest', CustomerTier.bronze, null);

  /// Verified accounts always start at silver; redirection keeps the
  /// construction path single-sourced.
  factory Customer.verified({
    required CustomerRef id,
    required String name,
    String? nickname,
  }) = _VerifiedCustomer;

  String? get nickname => _nickname;

  int get complaintCount => _complaintCount;

  set complaintCount(int value) {
    if (value >= 0) {
      _complaintCount = value;
    }
  }

  /// Display name with the promoted private field; the null check promotes
  /// [_nickname] so the interpolation reads it as non-nullable.
  String displayName() {
    if (_nickname != null) {
      return '${_nickname.trim()} ($name)';
    }
    return name;
  }

  /// Tier shorthand used by pricing; the switch expression is exhaustive
  /// because the enum is closed.
  int get tierWeight => switch (tier) {
        CustomerTier.bronze => 1,
        CustomerTier.silver => 2,
        CustomerTier.gold => 3,
        CustomerTier.platinum => 4,
      };

  @override
  String toString() => 'Customer($id, $name, $tier)';
}

/// Implementation behind [Customer.verified]; same-library privacy keeps it
/// unreachable from outside the file.
final class _VerifiedCustomer extends Customer {
  _VerifiedCustomer({
    required CustomerRef id,
    required String name,
    String? nickname,
  }) : super._(id, name, CustomerTier.silver, nickname);
}

/// Construction helper over a CRM record: exercises a factory-style function
/// taking a record parameter, the modern replacement for parameter bags.
typedef CrmRecord = ({
  CustomerRef id,
  String name,
  CustomerTier tier,
  String? nick,
});

Customer customerFromCrm(CrmRecord record) => Customer.signup(
      id: record.id,
      name: record.name,
      tier: record.tier,
      nickname: record.nick,
    );
