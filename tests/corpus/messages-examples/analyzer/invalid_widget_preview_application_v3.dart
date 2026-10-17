// messages-example: analyzer/messages.yaml WarningCode.invalidWidgetPreviewApplication (variant 3)
import 'package:flutter/widgets.dart';
import 'package:flutter/widget_previews.dart';

class PublicWidget extends StatelessWidget {
  // Invalid application to a private constructor.
  @Preview()
  const PublicWidget._();

  @override
  Widget build(BuildContext context) => Text('Foo');
}
