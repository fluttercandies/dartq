
import 'dart:ui';

import 'package:flutter/rendering.dart';

import 'framework.dart';

class Container extends StatelessWidget {
  final Widget? child;

  final AlignmentGeometry? alignment;

  final EdgeInsetsGeometry? padding;

  final Decoration? decoration;

  final Decoration? foregroundDecoration;

  final EdgeInsetsGeometry? margin;

  Container({
    super.key,
    this.alignment,
    this.padding,
    Color? color,
    this.decoration,
    this.foregroundDecoration,
    double? width,
    double? height,
    BoxConstraints? constraints,
    this.margin,
    Matrix4? transform,
    AlignmentGeometry? transformAlignment,
    this.child,
    Clip clipBehavior = Clip.none,
  });

  @override
  Widget build(BuildContext context) => throw 0;
}

class DecoratedBox extends SingleChildRenderObjectWidget {
  const DecoratedBox({
    super.key,
    required Decoration decoration,
    DecorationPosition position = DecorationPosition.background,
    super.child,
  });
}
