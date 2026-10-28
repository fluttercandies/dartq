
@immutable
class IconData {
  final int codePoint;

  final String? fontFamily;

  const IconData(
    this.codePoint, {
    this.fontFamily,
    String? fontPackage,
    bool matchTextDirection = false,
    List<String>? fontFamilyFallback,
  });
}
