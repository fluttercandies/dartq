
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';

import 'framework.dart';

abstract class SliverWithKeepAliveWidget extends RenderObjectWidget {}

abstract class SliverMultiBoxAdaptorWidget extends SliverWithKeepAliveWidget {}

class SliverList extends SliverMultiBoxAdaptorWidget {
  SliverList.list({
    super.key,
    required List<Widget> children,
    bool addAutomaticKeepAlives = true,
    bool addRepaintBoundaries = true,
    bool addSemanticIndexes = true,
  });
}

class SliverMainAxisGroup extends Widget {
  final List<Widget> slivers;
  const SliverMainAxisGroup({required this.slivers});
}

class SliverCrossAxisGroup extends Widget {
  final List<Widget> slivers;
  const SliverCrossAxisGroup({required this.slivers});
}
