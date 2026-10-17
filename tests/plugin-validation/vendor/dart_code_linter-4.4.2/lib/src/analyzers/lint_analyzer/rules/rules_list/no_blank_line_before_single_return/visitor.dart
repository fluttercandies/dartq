part of 'no_blank_line_before_single_return_rule.dart';

class _Visitor extends RecursiveAstVisitor<void> {
  final _statements = <ReturnStatement>[];

  Iterable<ReturnStatement> get statements => _statements;

  @override
  void visitReturnStatement(ReturnStatement node) {
    super.visitReturnStatement(node);
    _statements.add(node);
  }
}
