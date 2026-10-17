// Trigger for dart_code_linter `correct-game-instantiating`.
// Shape: a Flame `Game` subclass instantiated via a non-`GameWidget` path —
// the rule's flagged form.
void f() {
  final game = MyGame();
  runApp(game);
}

class MyGame extends Game {}

class Game {}

void runApp(Object app) {}
