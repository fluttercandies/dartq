use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

/// The corpus `test_failInTry`: a trailing `fail(...)` under a single
/// catch clause reports at the whole statement through its `;`.
#[test]
fn fail_in_try_reports() {
    let f = Fixture::parse(
        "import 'package:test/test.dart';\n\nvoid f() {\n  try {\n    f();\n    fail('fail');\n  } catch (e) {}\n}\n",
    );
    let lints = f.run(&one_rule("use_test_throws_matchers"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Use the 'throwsA' matcher instead of using 'fail' when there is no \
             exception thrown."
    );
    let start = utf16_at(&f, tok(&f.source, &f.tokens, "fail", 0));
    assert_eq!(lints[0].range, start..start + 13);
}

/// The corpus `test_failWithExpectInCatch` and
/// `test_failWithExpectInOnCatch`: a `finally` block and a second catch
/// clause each silence the face.
#[test]
fn finally_and_second_catch_stay_silent() {
    let f = Fixture::parse(
        "import 'package:test/test.dart';\n\nvoid f() {\n  try {\n    f();\n    fail('fail');\n  } catch (e) {\n    expect(e, null);\n  } finally {\n    print('hello');\n  }\n}\n",
    );
    assert!(f.run(&one_rule("use_test_throws_matchers")).is_empty());

    let f = Fixture::parse(
        "import 'package:test/test.dart';\n\nvoid f() {\n  try {\n    f();\n    fail('fail');\n  } on Exception catch (e) {\n    expect(e, null);\n  } catch (e) {\n    expect(e, null);\n  }\n}\n",
    );
    assert!(f.run(&one_rule("use_test_throws_matchers")).is_empty());
}

/// The corpus `test_noFail` and the empty-body gate: a try whose body
/// never calls `fail` — or has no statements at all — stays silent.
#[test]
fn no_fail_stays_silent() {
    let f = Fixture::parse(
        "import 'package:test/test.dart';\n\nvoid f() {\n  try {\n    f();\n  } catch (e) {}\n}\n",
    );
    assert!(f.run(&one_rule("use_test_throws_matchers")).is_empty());

    let f = Fixture::parse(
        "import 'package:test/test.dart';\n\nvoid f() {\n  try {}\n  catch (e) {\n    fail('fail');\n  }\n}\n",
    );
    assert!(f.run(&one_rule("use_test_throws_matchers")).is_empty());
}

/// The URI identity stand-in: without an import of a package that
/// exports the test `fail`, the bare name never reports.
#[test]
fn missing_test_import_stays_silent() {
    let f = Fixture::parse(
        "void f() {\n  try {\n    f();\n    fail('fail');\n  } catch (e) {}\n}\n",
    );
    assert!(f.run(&one_rule("use_test_throws_matchers")).is_empty());
}

/// `fail` is not the last body statement, or the send is a member call:
/// neither reports (the `x.fail` send carries the trailing access
/// marker the bare top-level name face excludes).
#[test]
fn member_send_and_non_last_statement_stay_silent() {
    let f = Fixture::parse(
        "import 'package:test/test.dart';\n\nvoid f() {\n  try {\n    fail('fail');\n    f();\n  } catch (e) {}\n}\n",
    );
    assert!(f.run(&one_rule("use_test_throws_matchers")).is_empty());

    let f = Fixture::parse(
        "import 'package:test/test.dart';\n\nclass C {\n  void fail(String m) {}\n}\n\nvoid f(C c) {\n  try {\n    c.fail('fail');\n  } catch (e) {}\n}\n",
    );
    assert!(f.run(&one_rule("use_test_throws_matchers")).is_empty());
}
