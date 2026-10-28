void g() {}

void f(int x) {
  switch (x) {
    case 1:
      g();
      break /* 🙂 keep */;
  }
}
