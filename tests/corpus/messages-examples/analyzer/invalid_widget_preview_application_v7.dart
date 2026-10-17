// messages-example: analyzer/messages.yaml WarningCode.invalidWidgetPreviewApplication (variant 7)
import 'package:flutter/widgets.dart';
import 'package:flutter/widget_previews.dart';

abstract class AbstractWidget extends StatelessWidget {
  // Invalid application to a constructor of an abstract class.
  @Preview()
  AbstractWidget();

  @override
  Widget build(BuildContext context) => Text('Foo');
}
