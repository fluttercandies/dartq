// messages-example: analyzer/messages.yaml CompileTimeErrorCode.equalKeysInConstMap
const map = <int, String>{1: 'a', 2: 'b', 1: 'c', 4: 'd'};
