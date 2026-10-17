// messages-example: analyzer/messages.yaml WarningCode.unnecessaryCastPattern
void f(int x) {
  if (x case var z as num) {
    print(z);
  }
}
