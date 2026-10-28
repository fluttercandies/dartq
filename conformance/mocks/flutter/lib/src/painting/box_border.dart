
import 'basic_types.dart';
import 'border_radius.dart';
import 'borders.dart';

class Border extends BoxBorder {
  @override
  final BorderSide top;

  final BorderSide right;

  @override
  final BorderSide bottom;

  final BorderSide left;

  const Border({
    this.top = BorderSide.none,
    this.right = BorderSide.none,
    this.bottom = BorderSide.none,
    this.left = BorderSide.none,
  });

  factory Border.all({
    Color color = const Color(0xFF000000),
    double width = 1.0,
    BorderStyle style = BorderStyle.solid,
    double strokeAlign = BorderSide.strokeAlignInside,
  }) => throw 0;

  const Border.fromBorderSide(BorderSide side)
    : top = side,
      right = side,
      bottom = side,
      left = side;
}

class BorderDirectional extends BoxBorder {
  @override
  final BorderSide top;

  final BorderSide start;

  final BorderSide end;

  @override
  final BorderSide bottom;

  const BorderDirectional({
    this.top = BorderSide.none,
    this.start = BorderSide.none,
    this.end = BorderSide.none,
    this.bottom = BorderSide.none,
  });
}

abstract class BoxBorder extends ShapeBorder {
  const BoxBorder();

  BorderSide get bottom;

  BorderSide get top;
}

enum BoxShape { rectangle, circle }
