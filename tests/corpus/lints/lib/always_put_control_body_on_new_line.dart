// always_put_control_body_on_new_line — official rule faces
// (pkg/linter always_put_control_body_on_new_line.dart:30-78): the
// first statement of every control-structure body must start on its own
// line; empty blocks are exempt.
// Expected rows: 6 — ALWAYS_PUT_CONTROL_BODY_ON_NEW_LINE keyed
// always_put_control_body_on_new_line.dart, one per same-line body:
//   `if (c) x = 1;`                  -> then body on the control line
//   `while (c) { x = 2; }`           -> block body on the control line
//   `for (...) { x += i; }`          -> block body on the control line
//   `do { x -= 1; } while (c);`      -> block body on the do line
//   `if (c) { x = 1; } else ...`     -> then block on the control line
//   `... else { y = 2; }`            -> else block on the else line
void f(bool c) {
  int x = 0;
  int y = 0;
  if (c) x = 1;
  while (c) {
    x = 2;
  }
  for (var i = 0; i < 1; i++) {
    x += i;
  }
  do {
    x -= 1;
  } while (c);
  if (c) {
    x = 1;
  } else {
    y = 2;
  }
}

// GOOD: every body starts on its own line.
void g(bool c) {
  int x = 0;
  int y = 0;
  if (c) {
    x = 1;
  }
  while (c) {
    x = 2;
  }
  for (var i = 0; i < 1; i++) {
    x += i;
  }
  do {
    x -= 1;
  } while (c);
  if (c) {
    x = 1;
  } else {
    y = 2;
  }
  if (c) {}
  while (c) {}
}
