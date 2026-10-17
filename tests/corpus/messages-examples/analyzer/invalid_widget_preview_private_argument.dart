// messages-example: analyzer/messages.yaml WarningCode.invalidWidgetPreviewPrivateArgument
import 'package:flutter/widgets.dart';
import 'package:flutter/widget_previews.dart';

const String _name = 'My Foo Preview';

@Preview(name: _name)
Widget myPreview() => Text('Foo');
