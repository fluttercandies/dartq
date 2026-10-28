void f(List<int> list, void Function(int) fn) {
  for (var e in list) {
    fn(e);
  }
}
