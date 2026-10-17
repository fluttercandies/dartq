// messages-example: analyzer/messages.yaml WarningCode.unusedElementParameter
class C {
  void _m(int x, [int? y]) {}

  void n() => _m(0);
}
