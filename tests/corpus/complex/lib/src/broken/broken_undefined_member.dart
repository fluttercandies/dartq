// Intentional diagnostics: undefined_method, undefined_getter,
// undefined_setter. One known receiver, three missing members.
class Ledger {
  int balance = 0;
}

void reconcile(Ledger ledger) {
  // expect: undefined_method
  ledger.auditAll();

  // expect: undefined_getter
  final closing = ledger.balanceInCents;

  // expect: undefined_setter
  ledger.balanceInEuros = 4;

  print(closing);
}
