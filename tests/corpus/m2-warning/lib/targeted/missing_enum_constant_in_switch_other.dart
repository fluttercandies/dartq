// Companion of missing_enum_constant_in_switch.dart: the exhaustive face —
// the switch covers every constant of the enum, so both sides stay silent
// (upstream test_all_enhanced expects zero diagnostics for this shape at
// the patterns-era language version). Fires no scored code of its own.
enum Status { idle, running, done }

void g(Status s) {
  switch (s) {
    case Status.idle:
      break;
    case Status.running:
      break;
    case Status.done:
      break;
  }
}
