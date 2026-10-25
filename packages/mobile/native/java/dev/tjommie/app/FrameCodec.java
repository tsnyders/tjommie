package dev.tjommie.app;

import java.io.ByteArrayOutputStream;
import java.io.EOFException;
import java.io.IOException;
import java.io.InputStream;
import java.net.ProtocolException;

/** Frames on the wire: a 4-byte big-endian length, then at most 65,535 Noise bytes. Never empty. */
final class FrameCodec {
  static final int MAX_FRAME = 65535;

  private FrameCodec() {}

  /** The bytes to write for one payload. */
  static byte[] encode(byte[] payload) throws ProtocolException {
    if (payload.length == 0 || payload.length > MAX_FRAME) throw new ProtocolException("Frame size out of range");
    byte[] out = new byte[4 + payload.length];
    out[0] = (byte) (payload.length >>> 24);
    out[1] = (byte) (payload.length >>> 16);
    out[2] = (byte) (payload.length >>> 8);
    out[3] = (byte) payload.length;
    System.arraycopy(payload, 0, out, 4, payload.length);
    return out;
  }

  /** The next payload, or null if the stream ended cleanly between frames. A bad length is a ProtocolException; a stream that ends inside a frame is an EOFException. */
  static byte[] read(InputStream in) throws IOException {
    int first = in.read();
    if (first < 0) return null;
    int b1 = in.read();
    int b2 = in.read();
    int b3 = in.read();
    if ((b1 | b2 | b3) < 0) throw new EOFException("Ended inside a frame's length");
    int length = (first << 24) | (b1 << 16) | (b2 << 8) | b3;
    if (length <= 0 || length > MAX_FRAME) throw new ProtocolException("Frame size out of range");
    // Grow only as bytes arrive; even a stalled peer cannot reserve a large message buffer.
    ByteArrayOutputStream bytes = new ByteArrayOutputStream(Math.min(length, 64 * 1024));
    byte[] chunk = new byte[Math.min(length, 64 * 1024)];
    int left = length;
    while (left > 0) {
      int got = in.read(chunk, 0, Math.min(chunk.length, left));
      if (got < 0) throw new EOFException("Ended inside a frame");
      bytes.write(chunk, 0, got);
      left -= got;
    }
    return bytes.toByteArray();
  }
}
