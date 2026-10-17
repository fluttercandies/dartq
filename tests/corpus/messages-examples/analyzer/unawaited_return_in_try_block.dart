// messages-example: analyzer/messages.yaml ScannerErrorCode.unawaitedReturnInTryBlock
Future<String> futureString(Future<String> value) async {
  try {
    return value;
  } catch (e) {
    print(e);
    return 'default';
  }
}
