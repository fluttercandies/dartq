// Intentional diagnostics: the undefined extension family. Explicit
// extension applications resolve against the extension's own members only,
// so the missing method and setter are attributed here.
extension Priced on String {
  String get priceTag => '$this: 1.00';

  void stamp(String note) {}
}

void price(String item) {
  // expect: undefined_extension_method
  Priced(item).stampExtra('x');

  // expect: undefined_extension_setter
  Priced(item).priceTag = '2.00';

  print(Priced(item).priceTag);
}
