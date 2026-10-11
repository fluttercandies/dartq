// Targets body_might_complete_normally_nullable: a block body that can
// complete normally under a nullable return type reports at the body's `{`
// (checkForBodyMayCompleteNormally; a non-nullable return type is the
// COMPILE_TIME_ERROR twin's business).
int? f() {}

void main() {
  f();
}

// Negative: a generator body always completes normally by the generator
// contract (body.isGenerator early return).
Iterable<int>? g() sync* {}

// Negative: an async body under an illegal async return type spares the
// completion warning — the Future<Never> <: T lower-bound check defers the
// face to illegal_async_return_type instead (no second error).
int? h() async {}
