
class MediaQuery extends InheritedWidget {
  final MediaQueryData data;

  const MediaQuery({
    super.key,
    required this.data,
    required super.child,
  });

  static MediaQueryData of(BuildContext context) =>
      throw UnimplementedError();
}

class MediaQueryData {
  const MediaQueryData();
}
