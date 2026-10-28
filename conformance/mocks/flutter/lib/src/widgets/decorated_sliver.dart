
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';

import 'framework.dart';

class DecoratedSliver extends SingleChildRenderObjectWidget {
  const DecoratedSliver({
    super.key,
    required Decoration decoration,
    DecorationPosition position = DecorationPosition.background,
    Widget? sliver,
  }) : super(child: sliver);
}
