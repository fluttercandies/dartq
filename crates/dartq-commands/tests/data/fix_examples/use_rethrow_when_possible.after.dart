void f() {
  try {
    g();
  } catch (e) {
    rethrow;
  }
}
