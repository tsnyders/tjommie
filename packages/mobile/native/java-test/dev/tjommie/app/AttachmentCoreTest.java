package dev.tjommie.app;

import static org.junit.Assert.*;
import java.io.File;
import java.io.IOException;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.List;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public class AttachmentCoreTest {
  @Rule public TemporaryFolder temporary = new TemporaryFolder();
  private static String digest(byte[] bytes) { return AttachmentFileStore.hex(AttachmentFileStore.sha256().digest(bytes)); }
  @Test public void stagesAndVerifiesBeforeExposingAFile() throws Exception {
    File root = temporary.newFolder(); AttachmentFileStore store = new AttachmentFileStore(root); byte[] bytes = "verified bytes".getBytes(java.nio.charset.StandardCharsets.UTF_8);
    String id = store.start(bytes.length, digest(bytes)); assertThrows(IOException.class, () -> store.append(id, 1, bytes));
    store.append(id, 0, bytes); assertArrayEquals(bytes, Files.readAllBytes(store.verify(id).toPath())); store.cancel(id); assertEquals(0, root.list().length);
  }
  @Test public void cancellationCorruptionAndBoundsNeverLeaveAPublishedPartial() throws Exception {
    File root = temporary.newFolder(); AttachmentFileStore store = new AttachmentFileStore(root); byte[] bytes = new byte[] { 1, 2, 3 };
    String cancelled = store.start(3, digest(bytes)); store.append(cancelled, 0, new byte[] { 1 }); store.cancel(cancelled);
    String corrupt = store.start(3, digest(new byte[] { 9, 9, 9 })); store.append(corrupt, 0, bytes); assertThrows(IOException.class, () -> store.verify(corrupt));
    assertThrows(IOException.class, () -> store.start(AttachmentFileStore.MAX_BYTES + 1, digest(bytes))); assertEquals(0, root.list().length);
    String first = store.start(0, digest(new byte[0])), second = store.start(0, digest(new byte[0])); assertThrows(IOException.class, () -> store.start(0, digest(new byte[0]))); store.verify(first); store.cancel(second); store.clear(); assertEquals(0, root.list().length);
  }
  @Test public void nativeRangeStreamsUseOneBoundedBufferAndStopAfterClose() throws Exception {
    List<Integer> reads = new ArrayList<>(); long size = AttachmentInputStream.CHUNK * 2L + 7;
    AttachmentInputStream stream = new AttachmentInputStream(0, size - 1, (offset, length) -> { reads.add(length); return new byte[length]; });
    byte[] buffer = new byte[4096]; long received = 0; for (int count; (count = stream.read(buffer)) >= 0;) received += count;
    assertEquals(size, received); assertEquals(java.util.Arrays.asList(AttachmentInputStream.CHUNK, AttachmentInputStream.CHUNK, 7), reads);
    stream.close(); assertThrows(IOException.class, stream::read);
    AttachmentInputStream bad = new AttachmentInputStream(0, 9, (offset, length) -> new byte[9]); assertThrows(IOException.class, bad::read);
  }
  @Test public void previewInvalidationDiscardsBufferedBytesBeforeAnotherRead() throws Exception {
    java.util.concurrent.atomic.AtomicBoolean valid = new java.util.concurrent.atomic.AtomicBoolean(true);
    java.util.concurrent.atomic.AtomicInteger cancelled = new java.util.concurrent.atomic.AtomicInteger();
    byte[] bytes = new byte[] { 1, 2, 3, 4 };
    AttachmentInputStream stream = new AttachmentInputStream(0, bytes.length - 1, new AttachmentInputStream.Reader() {
      public byte[] read(long offset, int length) { return bytes; }
      public boolean valid() { return valid.get(); }
      public void cancel() { cancelled.incrementAndGet(); }
    });
    assertEquals(1, stream.read()); valid.set(false);
    assertThrows(IOException.class, stream::read);
    assertArrayEquals(new byte[bytes.length], bytes);
    assertThrows(IOException.class, () -> stream.read(new byte[4], 0, 0)); stream.close();
    assertEquals(1, cancelled.get());
  }
  @Test public void webViewSeesTheSourceExtentThenReadsOnlyTheSelectedRangeWithoutFetchingItsPrefix() throws Exception {
    long size = AttachmentFileStore.MAX_BYTES;
    for (String header : new String[] { "bytes=0-215", "bytes=64-127", "bytes=" + (size - 32768) + "-", "bytes=-9", "bytes=" + (size - 7) + "-999999999" }) {
      long[] selected = AttachmentRanges.parse(header, size); List<Long> offsets = new ArrayList<>(); List<Integer> lengths = new ArrayList<>();
      AttachmentInputStream stream = new AttachmentInputStream(size, selected[0], selected[1], (offset, length) -> {
        offsets.add(offset); lengths.add(length); byte[] bytes = new byte[length];
        for (int index = 0; index < length; index++) bytes[index] = (byte) ((offset + index) * 31 + 7); return bytes;
      });
      // Chromium derives its one Content-Length from available(), then independently applies Range.
      assertEquals(size, stream.available()); assertTrue(offsets.isEmpty());
      long[] negotiated = AttachmentRanges.parse(header, stream.available()); assertArrayEquals(selected, negotiated);
      assertEquals(selected[0], stream.skip(selected[0])); assertTrue(offsets.isEmpty());
      int expectedLength = (int) (selected[1] - selected[0] + 1); byte[] received = stream.readAllBytes();
      assertEquals(expectedLength, received.length); assertEquals(java.util.Collections.singletonList(selected[0]), offsets);
      assertEquals(java.util.Collections.singletonList(expectedLength), lengths);
      for (int index = 0; index < received.length; index++) assertEquals((byte) ((selected[0] + index) * 31 + 7), received[index]);
      assertEquals(0, stream.available()); assertEquals(-1, stream.read()); stream.close(); assertThrows(IOException.class, stream::available);
    }
    AttachmentInputStream small = new AttachmentInputStream(0, 215, (offset, length) -> new byte[length]);
    assertEquals(216, small.available()); assertEquals(0, small.read()); assertEquals(215, small.available()); small.close();
    AttachmentInputStream empty = new AttachmentInputStream(0, -1, (offset, length) -> { throw new AssertionError("Empty previews must not fetch"); });
    assertEquals(0, empty.available()); assertEquals(-1, empty.read()); empty.close();
    java.io.InputStream head = AttachmentInputStream.headerOnly(216); assertEquals(216, head.available()); assertEquals(-1, head.read()); head.close();
  }
  @Test public void webViewRangeMetadataCannotBypassRevocationOrReadAnUnselectedPrefix() throws Exception {
    java.util.concurrent.atomic.AtomicBoolean valid = new java.util.concurrent.atomic.AtomicBoolean(true);
    java.util.concurrent.atomic.AtomicInteger reads = new java.util.concurrent.atomic.AtomicInteger();
    AttachmentInputStream stream = new AttachmentInputStream(216, 64, 127, new AttachmentInputStream.Reader() {
      public byte[] read(long offset, int length) { reads.incrementAndGet(); return new byte[length]; }
      public boolean valid() { return valid.get(); }
    });
    assertEquals(216, stream.available()); assertEquals(32, stream.skip(32)); valid.set(false);
    assertThrows(IOException.class, () -> stream.skip(32)); assertThrows(IOException.class, stream::available); assertThrows(IOException.class, stream::read); assertEquals(0, reads.get());
    AttachmentInputStream unselected = new AttachmentInputStream(216, 64, 127, (offset, length) -> { throw new AssertionError("Range must be selected first"); });
    assertThrows(IOException.class, unselected::read);
  }
  @Test public void failedTemporaryDeletionRemainsPendingForTheNextClear() throws Exception {
    File root = temporary.newFolder(); AttachmentFileStore store = new AttachmentFileStore(root);
    File staged = store.verify(store.start(0, digest(new byte[0])));
    Files.delete(staged.toPath()); Files.createDirectory(staged.toPath());
    File child = new File(staged, "synthetic-blocker"); Files.write(child.toPath(), new byte[] { 1 });
    assertThrows(IOException.class, store::clear); assertThrows(IOException.class, store::clear);
    Files.delete(child.toPath()); store.clear(); assertEquals(0, root.list().length);
  }
  @Test public void rangesAgreeOnSuffixOpenEndedClampAndMalformedInputs() {
    assertArrayEquals(new long[] { 1, 9 }, AttachmentRanges.parse("bytes=-9", 10)); assertArrayEquals(new long[] { 8, 9 }, AttachmentRanges.parse("bytes=8-99", 10)); assertArrayEquals(new long[] { 0, 9 }, AttachmentRanges.parse("bytes=0-", 10));
    for (String header : new String[] { "bytes=10-", "bytes=-0", "bytes=4-1", "bytes=0-1,3-4", "bytes=0.5-3", "bytes=9007199254740992-" }) assertThrows(IllegalArgumentException.class, () -> AttachmentRanges.parse(header, 10));
  }
  @Test public void cancellationInterruptsAnActiveCopyBeforeQueuedCleanup() throws Exception {
    java.util.concurrent.atomic.AtomicBoolean cancelled = new java.util.concurrent.atomic.AtomicBoolean();
    java.io.ByteArrayOutputStream output = new java.io.ByteArrayOutputStream() {
      @Override public synchronized void write(byte[] bytes, int offset, int count) { super.write(bytes, offset, count); cancelled.set(true); }
    };
    byte[] bytes = new byte[128];
    assertThrows(IOException.class, () -> AttachmentFileStore.copy(new java.io.ByteArrayInputStream(bytes), output, new byte[16], cancelled::get));
    assertEquals(16, output.size());
  }
}
