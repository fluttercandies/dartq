// Targets expected_token (head-pop law): a closer with no matching opener
// pops the stack head immediately — expected_token pinned at the '}'.
void f([int? a]) {}

void main() {
  f(}
}
