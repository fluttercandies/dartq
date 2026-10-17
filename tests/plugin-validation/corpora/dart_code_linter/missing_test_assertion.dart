// Trigger for dart_code_linter `missing-test-assertion`.
// Shape: a `test` body that performs no assertion call — the rule's
// flagged form.
void main() {
  test('adds numbers', () {
    add(1, 2);
  });
}

int add(int a, int b) => a + b;

void test(String name, void Function() body) {}
