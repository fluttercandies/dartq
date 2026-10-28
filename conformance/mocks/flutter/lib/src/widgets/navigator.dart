
import 'framework.dart';

class Navigator extends StatefulWidget {
  static NavigatorState of(
    BuildContext context, {
    bool rootNavigator = false,
  }) => throw 0;
}

class NavigatorState extends State<Navigator>
    with TickerProviderStateMixin, RestorationMixin {
  @optionalTypeArgs
  void pop<T extends Object?>([T? result]) {}
}
