
import 'package:flutter/foundation.dart';

import 'basic.dart';
import 'framework.dart';

class Placeholder extends StatelessWidget {
  const Placeholder({
    super.key,
    this.color = const Color(0xFF455A64),
    this.strokeWidth = 2.0,
    this.fallbackWidth = 400.0,
    this.fallbackHeight = 400.0,
    this.child,
  });

  final Color color;

  final double strokeWidth;

  final double fallbackWidth;

  final double fallbackHeight;

  final Widget? child;

  @override
  Widget build(BuildContext context) => throw 0;
}
