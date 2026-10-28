
// Mock of pkg/expect/lib/config.dart for standalone analysis: the real
// file parses the test-runner configuration through package:smith, which
// is outside the analysis surface. Public names and types are the real
// ones; values are inert.
library expect_config;

final String configAsString = '';

bool get isDart2jsConfiguration => false;

bool get isDart2WasmConfiguration => false;

bool get isDdcConfiguration => false;

bool get isVmJitConfiguration => false;

bool get isVmAotConfiguration => false;

bool get isVmConfiguration => isVmJitConfiguration || isVmAotConfiguration;

bool get isBrowserConfiguration => false;

bool get isWebConfiguration =>
    isDart2jsConfiguration || isDart2WasmConfiguration || isDdcConfiguration;
