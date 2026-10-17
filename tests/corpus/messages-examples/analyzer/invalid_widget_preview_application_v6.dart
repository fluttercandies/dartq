// messages-example: analyzer/messages.yaml WarningCode.invalidWidgetPreviewApplication (variant 6)
import 'package:flutter/widgets.dart';
import 'package:flutter/widget_previews.dart';

// ignore: unused_element
class _PrivateWidget extends StatelessWidget {
  const new();

  // Invalid application to a static method of a private class.
  @Preview()
  Widget privateStatic() => _PrivateWidget();

  @override
  Widget build(BuildContext context) => Text('Foo');
}
