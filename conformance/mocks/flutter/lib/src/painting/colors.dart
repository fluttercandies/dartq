
import 'package:flutter/foundation.dart';
import 'package:flutter/painting.dart';

@immutable
class ColorSwatch<T> extends Color {
  const ColorSwatch(super.primary, this._swatch);

  @protected
  final Map<T, InvalidType> _swatch;

  Color operator [](T key) => throw 0;
}
