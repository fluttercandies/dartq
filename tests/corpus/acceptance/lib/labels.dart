// Valid Dart 3.15 acceptance corpus — labels (labels on for/while/do-while loops and blocks, labeled break/continue, labels across switch statements, continue-to-label from switch case).
//
// Provenance: shapes modeled on the official language tests
// upstream Dart SDK source, tests/language/label/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
String describeLoops() {
  final out = StringBuffer();
  outer:
  for (var i = 0; i < 3; i++) {
    inner:
    for (var j = 0; j < 3; j++) {
      if (i == j) {
        continue outer;
      }
      if (i + j > 3) {
        break inner;
      }
      out.write('$i$j ');
    }
  }
  return out.toString();
}

int labeledWhile(int limit) {
  var ticks = 0;
  loop:
  while (ticks < limit) {
    var inner = 0;
    do {
      inner++;
      ticks++;
      if (inner == 2) {
        continue loop;
      }
    } while (inner < 4);
    break loop;
  }
  return ticks;
}

int labeledBlock(bool bail) {
  var steps = 0;
  section:
  {
    steps++;
    if (bail) {
      break section;
    }
    steps += 10;
  }
  return steps;
}

int labeledSwitch(int v) {
  switch (v) {
    case 1:
      break;
    default:
      break;
  }
  var seen = 0;
  scan:
  for (final n in [v, v, v]) {
    switch (n) {
      case 0:
        continue scan;
      default:
        seen += n;
    }
  }
  return seen;
}

void main() {
  final a = describeLoops();
  final b = labeledWhile(5);
  final c = labeledBlock(true);
  final d = labeledBlock(false);
  final e = labeledSwitch(2);
  final f = labeledSwitch(0);
  if (a.length + b + c + d + e + f != 24) {
    throw StateError('unreachable');
  }
}
