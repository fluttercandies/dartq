// Trigger for dart_code_linter `prefer-single-widget-per-file`.
// Shape: two public widget classes declared in one file — the rule's
// flagged form.
class FirstWidget extends Widget {}

class SecondWidget extends Widget {}

class Widget {
  const Widget();
}
