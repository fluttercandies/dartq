/// Label and report text formatting: multi-level interpolation, adjacent
/// string literals, raw and multiline strings, and a fluent builder.
library;

/// String extension surface used by the label printer.
extension LabelText on String {
  String get initials {
    final words = split(RegExp(r'\s+')).where((w) => w.isNotEmpty);
    return words.map((w) => w[0].toUpperCase()).take(3).join();
  }

  String truncate(int max) =>
      length <= max ? this : '${substring(0, max - 1)}…';

  String get squashed => replaceAll(RegExp(r'\s+'), ' ').trim();
}

/// Numeric label helpers kept off String to avoid extension conflicts.
extension CountText on int {
  String get padded3 => toString().padLeft(3, '0');
}

/// Fluent builder producing multi-line labels; every stage returns this so
/// call sites chain cascades.
class LabelBuilder {
  final StringBuffer _buffer = StringBuffer();

  LabelBuilder title(String text) {
    _buffer.writeln(text.toUpperCase());
    return this;
  }

  LabelBuilder row(String key, Object value) {
    _buffer
      ..write(key.padRight(12))
      ..write(': ')
      ..writeln('$value');
    return this;
  }

  LabelBuilder rule() {
    _buffer.writeln('-' * 24);
    return this;
  }

  String build() => _buffer.toString();
}

/// Renders a shelf label; exercises interpolation nesting, adjacent
/// literals and raw strings.
String shelfTag({
  required String product,
  required String zone,
  required int aisle,
  required int shelf,
}) {
  final prefix = 'WH-SH';
  return '$prefix '
      'zone=${zone.toUpperCase()} '
      'pos=A${aisle.padded3}-S${shelf.padded3} '
      'item=${product.squashed.truncate(18)}';
}

/// The hazard placard template; a multiline adjacent-literal block.
const String hazardPlacardTemplate = 'HAZMAT STORAGE\n'
    'keep aisle clear\n'
    'max stack height: 4\n'
    r'notify safety@warehouse.internal';
