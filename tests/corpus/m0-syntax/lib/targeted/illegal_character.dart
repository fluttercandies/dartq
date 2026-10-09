// Targets illegal_character: an astral character in code position is scanned
// as two unpaired surrogates — illegal_character emitted once per surrogate.
void f([int? a]) {}
void main() {
  f(😀);
}
