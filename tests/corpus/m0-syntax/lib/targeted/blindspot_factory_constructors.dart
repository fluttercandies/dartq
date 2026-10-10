// Targets the factory/initializer family (analyzer ParserErrorCode, pinned
// snapshot syntax-codes.tsv).
// Targets factory_with_initializers (TSV:131): a factory constructor can't
// have an initializer list.
class FactoryWithInitializers {
  int x = 0;
  factory FactoryWithInitializers() : x = 1 {
    return FactoryWithInitializers._();
  }
  FactoryWithInitializers._();
}

// Targets factory_without_body (TSV:132): a non-redirecting factory must
// have a body.
class FactoryWithoutBody {
  factory FactoryWithoutBody();
}

// Targets non_constructor_factory (TSV:245): 'factory' on a member whose
// name isn't the enclosing class's constructor name.
class NonConstructorFactory {
  factory other() => NonConstructorFactory._();
  NonConstructorFactory._();
}

// Targets constructor_with_type_arguments (TSV:72): type arguments after
// the constructor name in an initializer invocation. Fasta emit site:
// pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart:8895
// ("A constructor invocation can't have type arguments after the
// constructor name.", pkg/_fe_analyzer_shared/lib/src/messages/
// diagnostic.g.dart, codeConstructorWithTypeArguments).
class ConstructorWithTypeArguments extends Base {
  ConstructorWithTypeArguments() : super<int>();
}

class Base {
  Base();
}

// Targets missing_expression_in_initializer (TSV:202).
class MissingExpressionInInitializer {
  int x;
  MissingExpressionInInitializer() : x = ;
}

// Targets missing_closing_parenthesis (TSV:198): the '(' group is never
// closed before the member's semicolon.
class MissingClosingParenthesis {
  int y = (1;
}
