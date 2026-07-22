import 'package:flutter_test/flutter_test.dart';
import 'package:m3u8_downloader/src/home/source_input.dart';

void main() {
  test('extracts Bilibili short links from shared text', () {
    expect(
      extractSourceUrl('【分享】这个视频很好看 https://b23.tv/AbC123 复制打开'),
      'https://b23.tv/AbC123',
    );
  });

  test('preserves signed media query parameters', () {
    expect(
      extractSourceUrl('https://cdn.example/video.m3u8?token=a%2Fb&expires=99'),
      'https://cdn.example/video.m3u8?token=a%2Fb&expires=99',
    );
  });

  test('rejects text without an HTTP source', () {
    expect(extractSourceUrl('not a media link'), isNull);
  });
}
