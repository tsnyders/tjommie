package dev.tjommie.app;

import java.io.IOException;
import java.io.InputStream;
import java.util.Arrays;

/** One bounded native buffer; a cancelled response never keeps a complete media file. */
final class AttachmentInputStream extends InputStream {
  interface Reader { byte[] read(long offset, int length) throws IOException; default boolean valid() { return true; } default void cancel() {} }
  static final int CHUNK = 256 * 1024;
  private final Reader reader;
  private final long end;
  private long next;
  private byte[] buffer = new byte[0];
  private int position;
  private volatile boolean closed;
  AttachmentInputStream(long start, long end, Reader reader) { this.next = start; this.end = end; this.reader = reader; }
  @Override public int read() throws IOException { byte[] one = new byte[1]; return read(one, 0, 1) < 0 ? -1 : one[0] & 255; }
  @Override public int read(byte[] target, int offset, int length) throws IOException {
    if (target == null) throw new NullPointerException();
    if (offset < 0 || length < 0 || offset > target.length - length) throw new IndexOutOfBoundsException();
    checkOpen(); if (length == 0) return 0;
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
