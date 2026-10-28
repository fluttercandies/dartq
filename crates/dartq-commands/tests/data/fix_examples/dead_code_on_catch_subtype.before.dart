void f() {
  try {
    throw 1;
  } on num catch (e) {
    print(e);
  } on int catch (e) {
    print(e);
  }
}
