// messages-example: linter/messages.yaml LinterLintCode.useDecoratedBox
import 'package:flutter/material.dart';

Widget buildArea() {
  return Container(
    decoration: const BoxDecoration(
      color: Colors.red,
      borderRadius: BorderRadius.all(Radius.circular(5)),
    ),
    child: const Text('...'),
  );
}
