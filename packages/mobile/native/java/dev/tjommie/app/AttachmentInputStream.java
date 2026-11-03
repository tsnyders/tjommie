package dev.tjommie.app;

import java.io.IOException;
import java.io.ByteArrayInputStream;
import java.io.InputStream;
import java.util.Arrays;

/** One bounded native buffer; a cancelled response never keeps a complete media file. */
final class AttachmentInputStream extends InputStream {
  interface Reader { byte[] read(long offset, int length) throws IOException; default boolean valid() { return true; } default void cancel() {} }
  static final int CHUNK = 256 * 1024;
  private final Reader reader;
  private final long size, start, end;
  private long next;
  private long prefix;
  private boolean reading;
  private byte[] buffer = new byte[0];
  private int position;
  private volatile boolean closed;
  AttachmentInputStream(long start, long end, Reader reader) { this(end + 1, start, end, reader); }
  AttachmentInputStream(long size, long start, long end, Reader reader) {
    if (size < 0 || start < 0 || (size == 0 ? start != 0 || end != -1 : start >= size || end < start || end >= size)) throw new IllegalArgumentException("Invalid preview range");
    this.size = size; this.start = start; this.next = start; this.end = end; this.prefix = start; this.reader = reader;
  }
  // WebView computes its Content-Length and applies the original Range before reading our body.
  // Advertise the complete source for that negotiation, then report only remaining selected bytes.
  @Override public synchronized int available() throws IOException {
    checkOpen(); long remaining = reading ? Math.max(0, end - next + 1) + buffer.length - position : size - start + prefix;
    return (int) Math.min(Integer.MAX_VALUE, remaining);
  }
  @Override public synchronized long skip(long count) throws IOException {
    checkOpen(); if (count <= 0) return 0;
    if (prefix > 0) { long skipped = Math.min(count, prefix); prefix -= skipped; if (prefix == 0) reading = true; return skipped; }
    reading = true; long cursor = next - (buffer.length - position), skipped = Math.min(count, Math.max(0, end - cursor + 1));
    Arrays.fill(buffer, (byte) 0); buffer = new byte[0]; position = 0; next = cursor + skipped; return skipped;
  }
  static InputStream headerOnly(long size) {
    if (size < 0) throw new IllegalArgumentException("Invalid preview size");
    return new ByteArrayInputStream(new byte[0]) { @Override public int available() { return (int) Math.min(Integer.MAX_VALUE, size); } };
  }
  @Override public int read() throws IOException { byte[] one = new byte[1]; return read(one, 0, 1) < 0 ? -1 : one[0] & 255; }
  @Override public int read(byte[] target, int offset, int length) throws IOException {
    if (target == null) throw new NullPointerException();
    if (offset < 0 || length < 0 || offset > target.length - length) throw new IndexOutOfBoundsException();
    synchronized (this) { checkOpen(); if (length == 0) return 0; if (prefix != 0) { close(); throw new IOException("Preview range was not selected"); } reading = true; }
    if (position == buffer.length) {
      Arrays.fill(buffer, (byte) 0); if (next > end) return -1;
      int count = (int) Math.min(CHUNK, end - next + 1); byte[] bytes = reader.read(next, count);
      synchronized (this) {
        if (closed || !reader.valid() || bytes == null || bytes.length != count) { if (bytes != null) Arrays.fill(bytes, (byte) 0); close(); throw new IOException("Invalid preview bytes"); }
        buffer = bytes; position = 0; next += count;
      }
    }
    synchronized (this) {
      checkOpen(); int count = Math.min(length, buffer.length - position);
      System.arraycopy(buffer, position, target, offset, count); position += count; checkOpen(); return count;
    }
  }
  private void checkOpen() throws IOException { if (closed || !reader.valid()) { close(); throw new IOException("Preview closed"); } }
  @Override public synchronized void close() { if (closed) return; closed = true; Arrays.fill(buffer, (byte) 0); buffer = new byte[0]; position = 0; reader.cancel(); }
}
