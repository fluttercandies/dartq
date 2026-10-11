//! Matrix scenes: class modifiers, generics, mixins, extension types, enums,
//! records, operators, constructors, and external declarations.
//!
//! Every scene renders one self-contained, syntactically valid Dart block.
//! `id` suffixes keep top-level names collision-free within a file.

use crate::rng::SplitMix64;
use std::fmt::Write as _;

pub(super) fn class_modifiers(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(
        w,
        "\n/// Cell: class modifiers (abstract/base/final/interface/sealed/mixin class).\n"
    );
    dw!(
        w,
        "abstract class Shape{id} {{\n  double area();\n\n  String describe() => 'shape{id}';\n}}\n\n"
    );
    dw!(
        w,
        "final class Report{id} {{\n  final String title;\n\n  const Report{id}(this.title);\n}}\n\n"
    );
    dw!(w, "base class Engine{id} {{\n  void start() {{}}\n}}\n\n");
    dw!(
        w,
        "interface class Cache{id} {{\n  final Map<String, int> slots = <String, int>{{}};\n}}\n\n"
    );
    dw!(
        w,
        "mixin class Tracker{id} {{\n  final List<String> events = <String>[];\n\n  void track(String event) => events.add(event);\n}}\n\n"
    );
    dw!(
        w,
        "sealed class Vehicle{id} {{\n  const Vehicle{id}();\n}}\n\n"
    );
    dw!(
        w,
        "final class Car{id} extends Vehicle{id} {{\n  final int doors;\n\n  const Car{id}(this.doors);\n}}\n"
    );
}

pub(super) fn generic_class(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(w, "\n/// Cell: generic classes with bounds.\n");
    dw!(
        w,
        "class Box{id}<T extends Object?> {{\n  final T value;\n\n  const Box{id}(this.value);\n\n  Box{id}<U> map<U extends Object?>(U Function(T) transform) =>\n      Box{id}<U>(transform(value));\n}}\n\n"
    );
    dw!(
        w,
        "class Registry{id}<K extends Comparable<K>, V> {{\n  final Map<K, V> _entries = <K, V>{{}};\n\n  void put(K key, V value) {{\n    _entries[key] = value;\n  }}\n\n  V? lookup(K key) => _entries[key];\n\n  List<K> get keys => _entries.keys.toList();\n}}\n"
    );
}

pub(super) fn mixin_apply(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(w, "\n/// Cell: mixin declaration and application.\n");
    dw!(
        w,
        "mixin Loggable{id} {{\n  final List<String> entries = <String>[];\n\n  void log(String message) {{\n    entries.add('log: $message');\n  }}\n}}\n\n"
    );
    dw!(
        w,
        "mixin Counted{id} {{\n  int _count = 0;\n\n  int get count => _count;\n\n  void bump() => _count++;\n}}\n\n"
    );
    dw!(
        w,
        "class Session{id} with Loggable{id}, Counted{id} {{\n  final String user;\n\n  Session{id}(this.user);\n\n  void act(String action) {{\n    bump();\n    log('$user did $action (#$count)');\n  }}\n}}\n"
    );
}

pub(super) fn mixin_constraint(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(w, "\n/// Cell: mixin with an `on` constraint.\n");
    dw!(
        w,
        "abstract class DataStore{id} {{\n  void flush();\n\n  int get pending;\n}}\n\n"
    );
    dw!(
        w,
        "mixin Audited{id} on DataStore{id} {{\n  void audit() {{\n    for (var i = pending; i > 0; i--) {{\n      flush();\n    }}\n  }}\n}}\n\n"
    );
    dw!(
        w,
        "class SqlStore{id} extends DataStore{id} with Audited{id} {{\n  @override\n  void flush() {{}}\n\n  @override\n  int get pending => 0;\n}}\n"
    );
}

pub(super) fn extension_type(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(w, "\n/// Cell: extension types.\n");
    dw!(
        w,
        "extension type Meters{id}(double value) implements double {{\n  Meters{id} operator +(Meters{id} other) => Meters{id}(value + other.value);\n\n  String describe() => '${{value}}m';\n}}\n\n"
    );
    dw!(
        w,
        "extension type OrderId{id}(int raw) {{\n  bool get isValid => raw > 0;\n}}\n"
    );
}

pub(super) fn extension_declare(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(w, "\n/// Cell: classic extension.\n");
    dw!(
        w,
        "extension Slugify{id} on String {{\n  String get slug => trim().toLowerCase().replaceAll(' ', '-');\n}}\n\n"
    );
    dw!(
        w,
        "extension NumberTools{id} on int {{\n  int get doubled => this * 2;\n}}\n"
    );
}

pub(super) fn enum_plain(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(w, "\n/// Cell: plain enums.\n");
    dw!(w, "enum Status{id} {{ idle, running, done }}\n\n");
    dw!(w, "enum Color{id} {{ red, green, blue }}\n");
}

pub(super) fn enum_enhanced(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(w, "\n/// Cell: enhanced enum with members.\n");
    dw!(
        w,
        "enum Priority{id} implements Comparable<Priority{id}> {{\n  low(1),\n  normal(2),\n  high(3);\n\n  const Priority{id}(this.weight);\n\n  final int weight;\n\n  @override\n  int compareTo(Priority{id} other) => weight.compareTo(other.weight);\n\n  bool get isHigh => this == high;\n}}\n"
    );
}

pub(super) fn record_values(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(w, "\n/// Cell: records (positional and named).\n");
    dw!(w, "(int, String) pair{id}() => (7, 'seven');\n\n");
    dw!(
        w,
        "({{int width, int height}}) size{id}() => (width: 3, height: 4);\n\n"
    );
    dw!(
        w,
        "(int x, int y, {{String tag}}) point{id}() => (1, 2, tag: 'p');\n\n"
    );
    dw!(
        w,
        "int sum{id}() {{\n  final (a, b) = pair{id}();\n  final rect = size{id}();\n  return a + b + rect.width * rect.height;\n}}\n"
    );
}

pub(super) fn operators(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(w, "\n/// Cell: operator overloading.\n");
    dw!(
        w,
        "class Money{id} {{\n  final int cents;\n\n  const Money{id}(this.cents);\n\n  Money{id} operator +(Money{id} other) => Money{id}(cents + other.cents);\n\n  Money{id} operator -(Money{id} other) => Money{id}(cents - other.cents);\n\n  bool operator <(Money{id} other) => cents < other.cents;\n\n  bool operator >(Money{id} other) => cents > other.cents;\n\n  @override\n  bool operator ==(Object other) => other is Money{id} && other.cents == cents;\n\n  @override\n  int get hashCode => cents;\n\n  @override\n  String toString() => 'Money{id}($cents)';\n}}\n"
    );
}

pub(super) fn factory_ctor(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(
        w,
        "\n/// Cell: factory constructors (computed and redirecting).\n"
    );
    dw!(
        w,
        "class Connection{id} {{\n  final String url;\n\n  const Connection{id}._(this.url);\n\n  factory Connection{id}.https(String host) => Connection{id}._('https://$host');\n\n  factory Connection{id}.parse(String raw) {{\n    final trimmed = raw.trim();\n    return Connection{id}._(trimmed.isEmpty ? 'about:blank' : trimmed);\n  }}\n\n  const factory Connection{id}.constant(String url) = Connection{id}._;\n}}\n"
    );
}

pub(super) fn const_ctor(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(w, "\n/// Cell: constant constructors.\n");
    dw!(
        w,
        "class Config{id} {{\n  final String name;\n\n  final int retries;\n\n  const Config{id}({{this.name = 'default', this.retries = 3}});\n}}\n\n"
    );
    dw!(w, "const config{id} = Config();\n\n");
    dw!(
        w,
        "final List<Config{id}> presets{id} = <Config{id}>[\n  Config{id}(),\n  const Config{id}(retries: 1),\n];\n"
    );
}

pub(super) fn external_decl(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(w, "\n/// Cell: external declarations.\n");
    dw!(
        w,
        "abstract class Bridge{id} {{\n  external int measure(String input);\n\n  external static int lookup(int key);\n\n  external String get label;\n}}\n\n"
    );
    dw!(w, "external int topLevelHook{id}(int value);\n");
}
