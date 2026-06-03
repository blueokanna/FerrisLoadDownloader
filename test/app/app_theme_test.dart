import 'package:flutter_test/flutter_test.dart';
import 'package:m3u8_downloader/src/app/app_theme.dart';

void main() {
  test('keeps monet and amoled theme profiles available', () {
    final ids = appThemeProfiles.map((profile) => profile.id).toSet();

    expect(ids, contains('monet_flow'));
    expect(ids, contains('amoled_monet'));
    expect(ids, contains('amoled_pantone'));
    expect(
      appThemeProfiles
          .firstWhere((profile) => profile.id == 'monet_flow')
          .supportsDynamicColor,
      isTrue,
    );
    expect(
      appThemeProfiles
          .firstWhere((profile) => profile.id == 'amoled_monet')
          .useAmoledSurface,
      isTrue,
    );
    expect(
      appThemeProfiles
          .firstWhere((profile) => profile.id == 'amoled_pantone')
          .useAmoledSurface,
      isTrue,
    );
  });
}
