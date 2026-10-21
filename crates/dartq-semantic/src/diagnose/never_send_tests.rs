//! The flow pins behind the `args` corpus rows: a `Never`-returning
//! method send ends its branch (`usageException(...)` as a bare send and
//! through a `runner!` receiver), so the null-tested variable stays
//! promoted after the `if` and a `try`/`catch` body whose catch tail is
//! such a send cannot complete normally; an assignment whose right side
//! is a `!` on an index expression promotes the nullable local; a `!` on
//! a property access chain yields the non-null face for the next selector.
//!
//! Oracle provenance (`dart analyze` 3.13.1): every shape is silent.

use super::test_rig::{assert_silent, report_ops};

#[test]
fn bare_never_send_ends_the_null_branch() {
    let report = report_ops(
        "class Cmd { void go() {} }\n\
         class Runner {\n\
           Never usageException(String message) => throw message;\n\
           void run(Cmd? command) {\n\
             if (command == null) {\n\
               usageException('x');\n\
             }\n\
             command.go();\n\
           }\n\
         }\n",
    );
    assert_silent(&report);
}

#[test]
fn never_send_through_bang_receiver_ends_the_null_branch() {
    let report = report_ops(
        "class Runner { Never usageException(String message) => throw message; }\n\
         class Cmd {\n\
           Runner? runner;\n\
           void go() {}\n\
           void h(Cmd? command) {\n\
             if (command == null) {\n\
               runner!.usageException('y');\n\
             }\n\
             command.go();\n\
           }\n\
         }\n",
    );
    assert_silent(&report);
}

#[test]
fn catch_tail_never_send_completes_the_body() {
    let report = report_ops(
        "class P {\n\
           Never usageException(String m) => throw m;\n\
           int parse(int x) {\n\
             try {\n\
               return x;\n\
             } catch (e) {\n\
               usageException('a');\n\
             }\n\
           }\n\
         }\n",
    );
    assert_silent(&report);
}

#[test]
fn bang_on_index_assignment_promotes_the_local() {
    let report = report_ops(
        "class Cmd { void go() {} }\n\
         class M<K, V> { V? operator [](K k) => null; }\n\
         void f(M<String, Cmd> commands, String name) {\n\
           Cmd? command;\n\
           command = commands[name]!;\n\
           command.go();\n\
         }\n",
    );
    assert_silent(&report);
}

#[test]
fn bang_on_property_chain_reads_non_null() {
    let report = report_ops(
        "class Keys { int get length => 0; }\n\
         class Help { Keys get keys => Keys(); }\n\
         class Opt { Help? allowedHelp; }\n\
         int k(Opt option) {\n\
           if (option.allowedHelp != null) {\n\
             return option.allowedHelp!.keys.length;\n\
           }\n\
           return 0;\n\
         }\n",
    );
    assert_silent(&report);
}

#[test]
fn never_send_through_field_receiver_ends_the_null_branch() {
    let report = report_ops(
        "class Runner { Never usageException(String message) => throw message; }\n\
         class Cmd {\n\
           Runner runner = Runner();\n\
           void go() {}\n\
           void h(Cmd? command) {\n\
             if (command == null) {\n\
               runner.usageException('y');\n\
             }\n\
             command.go();\n\
           }\n\
         }\n",
    );
    assert_silent(&report);
}

#[test]
fn never_send_through_bang_local_ends_the_null_branch() {
    let report = report_ops(
        "class Runner { Never usageException(String message) => throw message; }\n\
         class Cmd {\n\
           void go() {}\n\
           void h(Cmd? command, Runner? runner) {\n\
             if (command == null) {\n\
               runner!.usageException('y');\n\
             }\n\
             command.go();\n\
           }\n\
         }\n",
    );
    assert_silent(&report);
}
