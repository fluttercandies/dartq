// Targets non_exhaustive_switch_expression: a switch expression over an
// enum that does not cover every constant. Oracle message (nail-pinned):
// "The type 'E' isn't exhaustively matched by the switch cases since it
// doesn't match the pattern 'E.b'."
enum E { a, b }

int f(E e) => switch (e) {
      E.a => 1,
    };
