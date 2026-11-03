package dev.tjommie.app;

import java.util.regex.Matcher;
import java.util.regex.Pattern;

final class AttachmentRanges {
  private static final Pattern RANGE = Pattern.compile("bytes=(\\d*)-(\\d*)");
  static long[] parse(String header, long size) {
    if (header == null) return null;
    Matcher match = RANGE.matcher(header);
    if (!match.matches() || size == 0 || match.group(1).isEmpty() && match.group(2).isEmpty()) throw new IllegalArgumentException("Invalid range");
    try {
      Long first = match.group(1).isEmpty() ? null : Long.valueOf(match.group(1));
      Long last = match.group(2).isEmpty() ? null : Long.valueOf(match.group(2));
      if (first != null && first > 9007199254740991L || last != null && last > 9007199254740991L) throw new IllegalArgumentException("Invalid range");
      long start = first == null ? Math.max(0, size - last) : first;
      long end = first == null || last == null ? size - 1 : Math.min(last, size - 1);
      if (start < 0 || start >= size || end < start || first == null && last == 0) throw new IllegalArgumentException("Invalid range");
      return new long[] { start, end };
    } catch (NumberFormatException error) { throw new IllegalArgumentException("Invalid range"); }
  }
}
