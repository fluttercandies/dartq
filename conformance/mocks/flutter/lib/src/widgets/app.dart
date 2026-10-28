
export 'dart:ui' show Locale;

typedef LocaleListResolutionCallback =
    Locale? Function(List<Locale>? locales, Iterable<Locale> supportedLocales);

typedef LocaleResolutionCallback =
    Locale? Function(Locale? locale, Iterable<Locale> supportedLocales);
