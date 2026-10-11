// Boundary closure-capture-and-labels: a local read only inside a closure
// body is spared (the walk covers nested function bodies), and a label
// referenced only from a nested loop is spared (the reference falls through
// to the outer label-scope tracker). The whole file must stay silent.
void f() {
  var captured = 0;
  var bump = () { print(captured); };
  bump();
  outer:
  for (var i = 0; i < 2; i++) {
    for (var j = 0; j < 2; j++) {
      if (j == 1) { continue outer; }
    }
  }
}
