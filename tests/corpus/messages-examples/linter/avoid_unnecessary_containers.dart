// messages-example: linter/messages.yaml LinterLintCode.avoidUnnecessaryContainers
import 'package:flutter/material.dart';

Widget buildRow() {
  return Container(child: Row(children: [Text('a'), Text('b')]));
}
