package dev.tjommie.app;

import static org.junit.Assert.*;
import com.getcapacitor.PluginCall;
import java.io.File;
import java.nio.file.Files;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public class TjommieAttachmentPluginTest {
  @Rule public TemporaryFolder temporary = new TemporaryFolder();
  private static final class CleanupCall extends PluginCall {
    boolean resolved; String error;
    CleanupCall() { super(null, "TjommieAttachments", "cleanup-test", "invalidate", null); }
    @Override public void resolve() { resolved = true; }
    @Override public void reject(String message, String code) { error = code; }
  }
  @Test public void webViewOwnsLengthWhileThePluginSuppliesOneRangeAndAdvertisesRangeSupport() {
    java.util.Map<String, String> headers = TjommieAttachmentPlugin.previewHeaders(216, 0, 215, true);
    assertEquals("bytes", headers.get("Accept-Ranges")); assertEquals("bytes 0-215/216", headers.get("Content-Range")); assertEquals(2, headers.size());
    for (String name : headers.keySet()) assertFalse("Content-Length".equalsIgnoreCase(name));
    java.util.Map<String, String> complete = TjommieAttachmentPlugin.previewHeaders(216, 0, 215, false);
    assertEquals(java.util.Collections.singletonMap("Accept-Ranges", "bytes"), complete);
    java.util.Map<String, String> late = TjommieAttachmentPlugin.previewHeaders(209715200, 209682432, 209715199, true);
    assertEquals("bytes 209682432-209715199/209715200", late.get("Content-Range")); assertEquals(2, late.size());
  }
  @Test public void explicitCleanupRejectsFailureAndOnlyResolvesAfterDeletion() throws Exception {
    File root = temporary.newFolder(); AttachmentFileStore store = new AttachmentFileStore(root);
    String digest = AttachmentFileStore.hex(AttachmentFileStore.sha256().digest(new byte[0]));
    File staged = store.verify(store.start(0, digest)); Files.delete(staged.toPath()); Files.createDirectory(staged.toPath());
    File child = new File(staged, "synthetic-blocker"); Files.write(child.toPath(), new byte[] { 1 });
    CleanupCall failed = new CleanupCall(); TjommieAttachmentPlugin.clearTemporary(store, failed);
    assertFalse(failed.resolved); assertEquals("cleanup_failed", failed.error);
    Files.delete(child.toPath()); CleanupCall succeeded = new CleanupCall(); TjommieAttachmentPlugin.clearTemporary(store, succeeded);
    assertTrue(succeeded.resolved); assertNull(succeeded.error); assertEquals(0, root.list().length);
    CleanupCall missing = new CleanupCall(); TjommieAttachmentPlugin.clearTemporary(null, missing);
    assertFalse(missing.resolved); assertEquals("cleanup_failed", missing.error);
  }
}
