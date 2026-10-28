
import 'package:flutter/foundation.dart';

import 'framework.dart';

typedef ValueWidgetBuilder<T> =
    Widget Function(BuildContext context, T value, Widget? child);

class ValueListenableBuilder<T> extends StatefulWidget {
  const ValueListenableBuilder({
    super.key,
    required this.valueListenable,
    required this.builder,
    this.child,
  });

  final ValueListenable<T> valueListenable;

  final Widget Function(BuildContext, T, Widget?) builder;

  final Widget? child;
}
