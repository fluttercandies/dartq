// messages-example: linter/messages.yaml LinterLintCode.useBuildContextSynchronouslyAsyncUse
import 'package:flutter/material.dart';

class const MyWidget({super.key}) extends Widget {
  void onButtonTapped(BuildContext context) async {
    await Future.delayed(const Duration(seconds: 1));
    Navigator.of(context).pop();
  }
}
