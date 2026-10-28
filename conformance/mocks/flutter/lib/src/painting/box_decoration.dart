
import 'basic_types.dart';
import 'border_radius.dart';
import 'box_border.dart';
import 'decoration.dart';

class BoxDecoration extends Decoration {
  final Color? color;

  final BoxBorder? border;

  final BorderRadiusGeometry? borderRadius;

  final BlendMode? backgroundBlendMode;

  final BoxShape shape;

  const BoxDecoration({
    this.color,
    DecorationImage? image,
    this.border,
    this.borderRadius,
    List<BoxShadow>? boxShadow,
    Gradient? gradient,
    this.backgroundBlendMode,
    this.shape = BoxShape.rectangle,
  }) : assert(
         backgroundBlendMode == null || color != null || gradient != null,
         "backgroundBlendMode applies to BoxDecoration's background color or "
         'gradient, but no color or gradient was provided.',
       );
}
