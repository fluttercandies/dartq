// messages-example: analyzer/messages.yaml WarningCode.nonNullableEqualsParameter
class C {
  @override
  bool operator ==(Object? other) => false;
}
