// Targets non_void_return_for_setter: a setter with a non-void declared
// return type. Oracle message (nail-pinned): "The return type of the
// setter must be 'void' or absent."
class C {
  int set s(int v) => 1;
}
