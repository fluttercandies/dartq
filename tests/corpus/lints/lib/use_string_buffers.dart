// use_string_buffers — official rule faces
// (pkg/linter use_string_buffers.dart:93-113, test/rules/
// use_string_buffers_test.dart:20-156,191-201): a `+=` into a String
// target inside a do/for/for-in/while body reports at the assignment,
// and an `=` whose right side composes the target as the leftmost
// `+`-chain operand (or an interpolation with an empty leading segment)
// reports at the target identifier; a target declared inside the loop
// body is not an accumulation.
//
// EXPECTED-AFTER-#1135-FIX POSTURE: adjudication #1135 (option A2)
// replaces dartq's RHS string-literal gate with the declaration-side
// gate (a written `String` annotation or a string-literal initializer).
// The expectations below are written to the post-fix target behavior:
// until the fix lands, dartq under-reports exactly the two marked A2
// rows (non-literal RHS compositions) while the oracle reports them, so
// a first gate run over the judged universe is expected red on those
// rows and turns green exactly when the fix lands. Every other row is
// parity today. The oracle has no dynamic-target face upstream (no
// upstream test exercises one), so the A2 over-report removal carries no
// corpus shape here.
//
// Expected rows: 6 — USE_STRING_BUFFERS keyed use_string_buffers.dart,
// message "Use a string buffer rather than '+' to compose strings.":
//   the `buffer += ''.toLowerCase()` write (annotated String field, A2 target)
//   the `buffer += 'a'` write (annotated String field)
//   the `buffer` target of `buffer = buffer + 'a'`
//   the `buffer` target of `buffer = '${buffer}a'`
//   the `buffer += s` write (literal-initializer local, A2 target)
//   the `buffer += 'a'` write in the do body
class A {
  String buffer = '';

  // Upstream test test_field_plusEquals_nonStringLiteral: the write-type
  // gate reads the declared field type, not the right side (A2 target).
  void foo(int n) {
    int aux = n;
    while (aux-- > 0) {
      buffer += ''.toLowerCase();
    }
  }

  // Upstream test test_field_plusEquals_stringLiteral.
  void bar(int n) {
    int aux = n;
    while (aux-- > 0) {
      buffer += 'a';
    }
  }

  // GOOD: a field write outside any loop body is not an accumulation
  // (upstream test test_field_plus_bufferReference).
  void outsideLoop() {
    buffer = buffer + buffer;
  }
}

// Upstream test test_localVariable_plus_stringLiteral: local declarators
// join through the string-literal initializer face; the `=` face reports
// the target identifier.
void baz() {
  var buffer = '';
  for (int i = 0; i < 10; i++) {
    buffer = buffer + 'a';
  }
}

// Upstream test
// test_localVariable_assignment_interpolatedStringLiteralAsPrefix: an
// interpolation with an empty leading segment whose first expression is
// the target is a prefix face.
void qux() {
  var buffer = '';
  for (int i = 0; i < 10; i++) {
    buffer = '${buffer}a';
  }
}

// Upstream test test_localVariable_plusEquals_nonStringLiteral: a
// non-literal right side does not silence the write on a
// declared-String target (A2 target).
void nonLiteralRhs() {
  var buffer = '';
  for (final s in ['a']) {
    buffer += s;
  }
}

// Upstream test test_localVariable_doLoop_plusEquals_stringLiteral: do
// bodies are registered loops.
void doLoop() {
  var buffer = '';
  do {
    buffer += 'a';
  } while (buffer.length < 10);
}

// GOOD: a non-String target with a user-defined `operator +` stays
// silent on both faces (upstream tests test_field_nonString_plus and
// test_field_nonString_plusEquals): the declaration gate must not read
// `B b = B()` as a String binder.
class B {
  // The explicit return type keeps the type_annotate_public_apis face
  // out of this file (the upstream test omits it under a single-rule
  // run; this corpus runs the full enabled universe).
  B operator +(B other) => this;

  void m() {
    B b = B();
    for (var i = 0; i < 10; i++) {
      b = b + this;
      b += this;
    }
  }
}

// GOOD: a target declared inside the loop body is not an accumulation
// (upstream test test_loopVariable_plusEquals_nonStringLiteral).
void loopLocal() {
  for (final s in ['a', 'b']) {
    var buffer = '';
    buffer += s;
  }
}

// GOOD: an interpolation whose leading segment is non-empty is not a
// prefix face (upstream test
// test_localVariable_assignment_interpolatedStringLiteralNotAsPrefix).
void notAsPrefix() {
  var buffer = '';
  for (int i = 0; i < 10; i++) {
    buffer = 'a$buffer';
  }
}
