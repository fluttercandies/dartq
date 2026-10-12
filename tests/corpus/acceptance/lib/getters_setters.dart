// Valid Dart 3.15 acceptance corpus — getters/setters (top-level/static/instance getter-setter pairs, abstract accessors, implementing abstract accessors, covariant setter parameters, cascaded accessor use).
//
// Provenance: shapes modeled on the official language tests
// .third/dart-sdk/tests/language/getter/, tests/language/setter/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
double topGetter = 0.5;

int get topSquared => topGetter.toInt() * 2;

set topScaled(int v) => topGetter = v / 2;

class Thermometer {
  static String unit = 'C';

  static int _offset = 0;

  static int get offset => _offset;

  static set offset(int v) => _offset = v;

  double _celsius;

  Thermometer(this._celsius);

  double get celsius => _celsius;

  set celsius(double v) => _celsius = v;

  double get fahrenheit => _celsius * 1.8 + 32;

  set fahrenheit(double v) => _celsius = (v - 32) / 1.8;
}

abstract class Sensor {
  double get reading;

  set calibration(double factor);

  String get id => 'unset';
}

class FixedSensor implements Sensor {
  @override
  final double reading;

  @override
  set calibration(double factor) {
    if (factor < 0) {}
  }

  @override
  final String id;

  FixedSensor(this.reading, this.id);
}

class Base {
  int _v = 0;

  int get v => _v;

  set v(int value) => _v = value;
}

class Covariant extends Base {
  @override
  set v(covariant num value) => _v = value.toInt();
}

void main() {
  topScaled = topSquared + 1;
  Thermometer.offset = Thermometer.offset + 1;
  final t = Thermometer(20.0)
    ..celsius = 21.5
    ..fahrenheit = 70.0;
  final sensor = FixedSensor(1.5, 's-1');
  sensor.calibration = 2;
  final cov = Covariant()..v = 3.5;
  final readings = <Object>[
    topGetter,
    topSquared,
    Thermometer.unit,
    Thermometer.offset,
    t.celsius,
    t.fahrenheit,
    sensor.reading,
    sensor.id,
    cov.v,
  ];
  if (readings.length != 9) {
    throw StateError('unreachable');
  }
}
