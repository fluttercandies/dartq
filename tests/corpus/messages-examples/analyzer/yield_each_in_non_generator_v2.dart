// messages-example: analyzer/messages.yaml CompileTimeErrorCode.yieldEachInNonGenerator (variant 2)
Stream<int> get digits async {
  yield* [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
}
