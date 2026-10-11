// Targets unused_field: a library-private field whose value is written
// (via the initializing formal) but never read anywhere. The class and
// constructor are public, isolating the code to the field.
class C {
  int _x;

  C(this._x);
}
