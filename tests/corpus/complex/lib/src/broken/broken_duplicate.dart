// Intentional diagnostics: duplicate_definition. The same top-level name
// declared twice in one library.
class ReceivingDock {
  int capacity = 2;
}

// expect: duplicate_definition
class ReceivingDock {
  int capacity = 4;
}

void dock() {
  print(ReceivingDock().capacity);
}
