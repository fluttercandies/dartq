void g(List<int> xs) {
  for (var e in xs) {
    if (e) {
      continue;
    }
    (() {
      return;
    })();
    continue;
  }
}
