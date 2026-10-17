// messages-example: linter/messages.yaml LinterLintCode.sizedBoxForWhitespace
import 'package:flutter/material.dart';

Widget buildRow() {
  return Row(
    children: <Widget>[
      const Text('...'),
      Container(width: 4, child: Text('...')),
      const Expanded(child: Text('...')),
    ],
  );
}
