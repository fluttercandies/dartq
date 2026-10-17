// messages-example: analyzer/messages.yaml WarningCode.unusedField
class C {
  final String _originalValue;
  final String _currentValue;

  C(this._originalValue) : _currentValue = _originalValue;

  String get value => _currentValue;
}
