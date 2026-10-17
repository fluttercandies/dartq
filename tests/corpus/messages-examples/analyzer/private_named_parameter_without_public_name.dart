// messages-example: analyzer/messages.yaml CompileTimeErrorCode.privateNamedParameterWithoutPublicName
class C {
  final int? _2legit;
  C({this._2legit = 0});
  int? get twoLegit => _2legit;
}
