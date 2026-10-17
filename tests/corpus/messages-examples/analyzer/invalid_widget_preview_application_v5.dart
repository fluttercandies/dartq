// messages-example: analyzer/messages.yaml WarningCode.invalidWidgetPreviewApplication (variant 5)
import 'package:flutter/widgets.dart';
import 'package:flutter/widget_previews.dart';

// ignore: unused_element
class _PrivateWidget extends StatelessWidget {
  // Invalid application to a constructor of a private class.
  @Preview()
  const _PrivateWidget();

  @override
  Widget build(BuildContext context) => Text('Foo');
}
