// messages-example: linter/messages.yaml LinterLintCode.avoidInitToNull
class C {
  int? f = null;

  void m() {
    if (f != null) {
      print(f);
    }
  }
}
