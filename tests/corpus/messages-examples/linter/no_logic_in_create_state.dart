// messages-example: linter/messages.yaml LinterLintCode.noLogicInCreateState
import 'package:flutter/material.dart';

class const MyWidget({super.key}) extends StatefulWidget {
  @override
  MyState createState() => MyState(0);
}

class MyState extends State {
  int x;

  MyState(this.x);

  @override
  Widget build(BuildContext context) => MyWidget();
}
