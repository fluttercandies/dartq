// Trigger for solid_lints `feature_envy`.
// Shape: a method that accesses another object's members more than its
// own — the rule's flagged form.
class Wallet {
  double balance = 0;
  String currency = 'usd';
}

class Report {
  String describe(Wallet wallet) =>
      '${wallet.currency}: ${wallet.balance}';
}
