
import 'package:meta/meta.dart';

@immutable
abstract class Key {
  const factory Key(String value) = ValueKey<String>;

  @protected
  const Key.empty();
}

abstract class LocalKey extends Key {
  const LocalKey() : super.empty();
}

class UniqueKey extends LocalKey {}

class ValueKey<T> extends LocalKey {
  final T value;

  const ValueKey(this.value);
}
