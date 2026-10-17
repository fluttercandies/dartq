// messages-example: linter/messages.yaml LinterLintCode.diagnosticDescribeAllProperties
import 'package:flutter/material.dart';

class const C({super.key}) extends Widget {
  bool get p1 => true;

  bool get p2 => false;

  @override
  void debugFillProperties(DiagnosticPropertiesBuilder properties) {
    super.debugFillProperties(properties);
    properties.add(DiagnosticsProperty<bool>('p1', p1));
  }
}
