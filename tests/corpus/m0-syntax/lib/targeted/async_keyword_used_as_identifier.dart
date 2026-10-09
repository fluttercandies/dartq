// Targets async_keyword_used_as_identifier: the keywords 'await' and 'yield'
// can't be used as identifiers in an asynchronous or generator function.
void f() async {
  var await = 1;
}
