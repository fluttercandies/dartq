// Targets super_in_invalid_context: a 'super' invocation outside a class
// member context. Oracle message (nail-pinned): "Invalid context for
// 'super' invocation."
void f() {
  super.toString();
}
