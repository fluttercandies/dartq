
library flutter_localizations;

class LocalizationsDelegate<T> {
  const LocalizationsDelegate();
}

abstract class WidgetsLocalizations {}

abstract class GlobalWidgetsLocalizations implements WidgetsLocalizations {
  static const LocalizationsDelegate<WidgetsLocalizations> delegate =
      LocalizationsDelegate<WidgetsLocalizations>();
}

abstract class GlobalMaterialLocalizations implements WidgetsLocalizations {
  static const LocalizationsDelegate<WidgetsLocalizations> delegate =
      LocalizationsDelegate<WidgetsLocalizations>();
}

abstract class GlobalCupertinoLocalizations implements WidgetsLocalizations {
  static const LocalizationsDelegate<WidgetsLocalizations> delegate =
      LocalizationsDelegate<WidgetsLocalizations>();
}
