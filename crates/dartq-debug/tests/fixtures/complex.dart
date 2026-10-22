// Comprehensive fixture for the live end-to-end session test.
// Exercises: conditional breakpoints (false + true legs), exception
// pauses, nested object graphs, awaiter stacks, step Into/Over/Out,
// multiple breakpoints, stdout capture, and source reload.
// Line numbers are load-bearing — keep them stable.

class Node {
  Node(this.value, this.next);
  int value;
  Node? next;
  @override
  String toString() => 'Node($value)';
}

Node buildList(int depth) {
  Node? head;
  for (var i = depth; i >= 1; i--) {
    head = Node(i, head);
  }
  return head!;
}

Future<int> fetchValue(int n) async {
  await Future<void>.delayed(const Duration(milliseconds: 10));
  return n * 2;
}

Future<int> compute() async {
  final v = await fetchValue(21);        // L29: awaiter target
  return v + 1;                          // L30
}

int iterate(List<int> xs) {
  var total = 0;
  for (var i = 0; i < xs.length; i++) {
    total += xs[i];                      // L36: conditional bp (i==2)
  }
  return total;                          // L38
}

int mayThrow(bool go) {
  if (go) {
    throw StateError('deliberate');      // L43: exception pause
  }
  return 0;                              // L45
}

int add(int a, int b) => a + b;          // L48: step Into target

int caller(int x) {
  final y = add(x, 1);                   // L51: step Over/Into boundary
  return y * 10;                         // L52
}

Future<void> main() async {
  final list = buildList(4);             // L56
  final sum = iterate([10, 20, 30, 40]); // L57: bp 1 hit (line)
  final deep = await compute();          // L58: awaiter pause point
  var threw = 0;
  try {
    mayThrow(true);                      // L61
  } on StateError {
    threw = 1;                           // L63
  }
  final viaAdd = caller(5);              // L65
  print('list=${list.value} sum=$sum deep=$deep threw=$threw viaAdd=$viaAdd');
}
