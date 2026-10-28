void f() {
  try {
    throw 1;
  } on num catch (e) {
    print(e);
  }
}
