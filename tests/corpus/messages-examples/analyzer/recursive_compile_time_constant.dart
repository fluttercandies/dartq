// messages-example: analyzer/messages.yaml CompileTimeErrorCode.recursiveCompileTimeConstant
const secondsPerHour = minutesPerHour * 60;
const minutesPerHour = secondsPerHour / 60;
