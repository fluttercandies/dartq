// messages-example: analyzer/messages.yaml WarningCode.invalidWidgetPreviewApplication (variant 4)
import 'package:flutter/widgets.dart';
import 'package:flutter/widget_previews.dart';

class PublicWidget extends StatelessWidget {
  const new();

  // Invalid application to a instance member.
  @Preview()
  Widget instancePreview() => PublicWidget();

  @override
  Widget build(BuildContext context) => Text('Foo');
}
