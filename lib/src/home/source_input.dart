String? extractSourceUrl(String input) {
  final value = input.trim();
  if (value.isEmpty) {
    return null;
  }

  final direct = Uri.tryParse(value);
  if (direct != null && (direct.scheme == 'http' || direct.scheme == 'https')) {
    return direct.toString();
  }

  final match = RegExp(r'''https?://[^\s<>"']+''').firstMatch(value);
  if (match == null) {
    return null;
  }
  final candidate = match
      .group(0)!
      .replaceFirst(RegExp(r'''[,.;:!?\)\]\}，。；：！？）》】]+$'''), '');
  final parsed = Uri.tryParse(candidate);
  if (parsed == null || (parsed.scheme != 'http' && parsed.scheme != 'https')) {
    return null;
  }
  return parsed.toString();
}
