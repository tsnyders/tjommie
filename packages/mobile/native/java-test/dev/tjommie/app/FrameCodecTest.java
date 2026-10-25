package dev.tjommie.app;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertThrows;

import java.io.ByteArrayInputStream;
import java.io.EOFException;
import java.net.ProtocolException;
import org.junit.Test;

public class FrameCodecTest {
  @Test
  public void encodesALengthInFrontOfThePayload() throws Exception {
    assertArrayEquals(new byte[] { 0, 0, 0, 3, 7, 8, 9 }, FrameCodec.encode(new byte[] { 7, 8, 9 }));
  }

  @Test
  public void readsFramesBackToBackAndEndsCleanly() throws Exception {
    byte[] wire = new byte[] { 0, 0, 0, 2, 1, 2, 0, 0, 0, 1, 9 };
    ByteArrayInputStream in = new ByteArrayInputStream(wire);
    assertArrayEquals(new byte[] { 1, 2 }, FrameCodec.read(in));
    assertArrayEquals(new byte[] { 9 }, FrameCodec.read(in));
    assertNull(FrameCodec.read(in));
  }

  @Test
  public void refusesEmptyAndOversizeFrames() {
    assertThrows(ProtocolException.class, () -> FrameCodec.encode(new byte[0]));
    assertThrows(ProtocolException.class, () -> FrameCodec.encode(new byte[FrameCodec.MAX_FRAME + 1]));
    assertThrows(ProtocolException.class, () -> FrameCodec.read(new ByteArrayInputStream(new byte[] { 0, 0, 0, 0 })));
    assertThrows(ProtocolException.class, () -> FrameCodec.read(new ByteArrayInputStream(new byte[] { 0x01, 0, 0, 1 })));
    assertThrows(ProtocolException.class, () -> FrameCodec.read(new ByteArrayInputStream(new byte[] { (byte) 0xFF, (byte) 0xFF, (byte) 0xFF, (byte) 0xFF })));
  }

  @Test
  public void acceptsTheLargestFrameLength() throws Exception {
    byte[] payload = new byte[FrameCodec.MAX_FRAME];
    byte[] wire = FrameCodec.encode(payload);
    assertEquals(4 + FrameCodec.MAX_FRAME, wire.length);
    assertEquals(FrameCodec.MAX_FRAME, FrameCodec.read(new ByteArrayInputStream(wire)).length);
  }

  @Test
  public void aStreamThatEndsInsideAFrameIsAnError() {
    assertThrows(EOFException.class, () -> FrameCodec.read(new ByteArrayInputStream(new byte[] { 0, 0 })));
    assertThrows(EOFException.class, () -> FrameCodec.read(new ByteArrayInputStream(new byte[] { 0, 0, 0, 5, 1, 2 })));
  }

  @Test
  public void aClaimedOversizeFrameIsRejectedBeforeReadingItsBody() {
    assertThrows(ProtocolException.class, () -> FrameCodec.read(new ByteArrayInputStream(new byte[] { 0x00, (byte) 0xFF, (byte) 0xFF, (byte) 0xFF })));
  }
}
