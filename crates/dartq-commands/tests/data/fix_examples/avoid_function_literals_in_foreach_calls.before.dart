void g(List<int> xs) {
  xs.forEach((e) {
    if (e) {
      return;
    }
    (() {
      return;
    })();
    return;
  });
}
