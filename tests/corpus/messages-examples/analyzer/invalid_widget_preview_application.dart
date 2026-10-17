// messages-example: analyzer/messages.yaml WarningCode.invalidWidgetPreviewApplication
import 'package:flutter/widgets.dart';
import 'package:flutter/widget_previews.dart';

// Invalid application to private top-level function.
@Preview()
// ignore: unused_element
Widget _myPrivatePreview() => Text('Foo');
