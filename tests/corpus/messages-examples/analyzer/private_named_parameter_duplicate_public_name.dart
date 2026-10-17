// messages-example: analyzer/messages.yaml CompileTimeErrorCode.privateNamedParameterDuplicatePublicName
class C {
  final int? _x;
  C({int? x, this._x});
  int? get x => _x;
}
