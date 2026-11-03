package dev.tjommie.app;

import static org.junit.Assert.*;
import org.junit.Test;

public class AttachmentSaveLifetimeTest {
  @org.junit.Rule public org.junit.rules.TemporaryFolder temporary = new org.junit.rules.TemporaryFolder();
  private AttachmentSaveLifetime lease(long now) { return new AttachmentSaveLifetime("save", "heartbeat", 7, now); }
  @Test public void promotionMustCompleteBeforeStagingOrThePickerCanProceed() {
    AttachmentSaveLifetime pending = lease(100);
    assertFalse(pending.valid(7, 101)); assertFalse(pending.picker(7, 101)); assertNull(pending.heartbeat(101));
    assertFalse(pending.promote(100 + AttachmentSaveLifetime.PROMOTION_MILLIS)); assertEquals("promotion_failed", pending.reason());
    AttachmentSaveLifetime ready = lease(100); assertTrue(ready.promote(101)); assertFalse(ready.promote(102));
    assertTrue(ready.valid(7, 102)); assertTrue(ready.picker(7, 102)); assertTrue(ready.copy(7, 103)); assertFalse(ready.picker(7, 103));
  }
  @Test public void onlyTheOutstandingChallengeFromThisSaveAndPageCanRenewTheLease() {
    AttachmentSaveLifetime save = lease(0); assertTrue(save.promote(1)); String first = save.heartbeat(1);
    assertNotNull(first); assertNull(save.heartbeat(2));
    assertFalse(save.acknowledge("other", first, 7, true, 2)); assertFalse(save.acknowledge("heartbeat", first, 8, true, 2));
    assertFalse(save.acknowledge("heartbeat", "invented", 7, true, 2));
    assertTrue(save.acknowledge("heartbeat", first, 7, true, 3));
    assertNull(save.heartbeat(30_002)); String second = save.heartbeat(30_003); assertNotNull(second); assertNotEquals(first, second);
    assertFalse(save.acknowledge("heartbeat", first, 7, true, 40_000));
    assertFalse(save.active(45_003)); assertEquals("connection_timeout", save.reason());
    assertFalse(save.acknowledge("heartbeat", second, 7, true, 45_004));
  }
  @Test public void authenticatedTicksKeepANineMinutePickerAndItsCopyAliveWithoutExtendingTheAbsoluteLimit() {
    AttachmentSaveLifetime save = lease(0); assertTrue(save.promote(0)); assertTrue(save.picker(7, 1));
    for (long now = 0; now < AttachmentSaveLifetime.MAX_MILLIS; now += AttachmentSaveLifetime.TICK_MILLIS) {
      String challenge = save.heartbeat(now); assertNotNull(challenge); assertTrue(save.acknowledge("heartbeat", challenge, 7, true, now));
      if (now == 540_000) assertTrue(save.copy(7, now));
      assertTrue(save.valid(7, now + 1));
    }
    assertFalse(save.active(AttachmentSaveLifetime.MAX_MILLIS)); assertEquals("timeout", save.reason());
  }
  @Test public void failedAuthenticationOrExplicitCancellationCannotBeUndoneByALateReply() {
    AttachmentSaveLifetime failed = lease(0); assertTrue(failed.promote(0)); String challenge = failed.heartbeat(0);
    assertFalse(failed.acknowledge("heartbeat", challenge, 7, false, 1)); assertEquals("connection_closed", failed.reason());
    assertFalse(failed.acknowledge("heartbeat", challenge, 7, true, 2));
    for (String reason : new String[] { "cancelled", "disconnect", "revoked", "host_changed", "navigation", "destroyed", "timeout" }) {
      AttachmentSaveLifetime cancelled = lease(0); assertTrue(cancelled.promote(0)); String late = cancelled.heartbeat(0);
      cancelled.cancel(reason); cancelled.cancel("other"); assertEquals(reason, cancelled.reason());
      assertFalse(cancelled.valid(7, 1)); assertFalse(cancelled.acknowledge("heartbeat", late, 7, true, 1)); assertNull(cancelled.heartbeat(1));
    }
  }
  @Test public void oldInvalidationReservesCleanupBeforeANewGenerationCanStage() throws Exception {
    java.io.File root = temporary.newFolder(); AttachmentFileStore store = new AttachmentFileStore(root);
    String digest = AttachmentFileStore.hex(AttachmentFileStore.sha256().digest(new byte[0]));
    String old = store.start(0, digest); java.util.List<Runnable> queue = new java.util.ArrayList<>();
    java.util.concurrent.CompletableFuture<Void> stopped = new java.util.concurrent.CompletableFuture<>();
    AttachmentSaveLifetime.cleanupAfterRetirement(queue::add, stopped, () -> { try { store.clear(); } catch (Exception error) { throw new AssertionError(error); } }, () -> fail("Cleanup must follow retirement"));
    String[] next = new String[1]; queue.add(() -> { try { next[0] = store.start(0, digest); } catch (Exception error) { throw new AssertionError(error); } });
    assertEquals(2, queue.size()); assertNull(next[0]); assertTrue(new java.io.File(root, old + ".part").isFile());
    stopped.complete(null); for (Runnable job : queue) job.run();
    assertFalse(new java.io.File(root, old + ".part").exists()); assertTrue(store.verify(next[0]).isFile()); store.clear();
  }
  @Test public void destroyingThePluginDrainsReservedCleanupEvenWhenRetirementCompletesAfterShutdown() throws Exception {
    java.util.concurrent.ExecutorService io = java.util.concurrent.Executors.newSingleThreadExecutor();
    java.util.concurrent.CompletableFuture<Void> stopped = new java.util.concurrent.CompletableFuture<>();
    java.util.concurrent.atomic.AtomicBoolean cleared = new java.util.concurrent.atomic.AtomicBoolean(), failed = new java.util.concurrent.atomic.AtomicBoolean();
    try {
      AttachmentSaveLifetime.cleanupAfterRetirement(io, stopped, () -> cleared.set(true), () -> failed.set(true));
      io.shutdown(); assertFalse(cleared.get()); stopped.complete(null);
      assertTrue(io.awaitTermination(2, java.util.concurrent.TimeUnit.SECONDS)); assertTrue(cleared.get()); assertFalse(failed.get());
    } finally { io.shutdownNow(); }
  }
  @Test public void unconfirmedRetirementDoesNotRunCleanupOrClaimDeletion() {
    java.util.concurrent.CompletableFuture<Void> stopped = new java.util.concurrent.CompletableFuture<>(); stopped.completeExceptionally(new IllegalStateException("not retired"));
    java.util.concurrent.atomic.AtomicBoolean cleared = new java.util.concurrent.atomic.AtomicBoolean(), failed = new java.util.concurrent.atomic.AtomicBoolean();
    AttachmentSaveLifetime.cleanupAfterRetirement(Runnable::run, stopped, () -> cleared.set(true), () -> failed.set(true));
    assertFalse(cleared.get()); assertTrue(failed.get());
  }
  @Test public void anOldNotificationCancelCannotRetireANewerSaveOrRepromoteItsService() {
    AttachmentSaveLifetime current = new AttachmentSaveLifetime("new-save", "new-heartbeat", 8, 0);
    assertEquals(AttachmentSaveLifetime.Command.START, current.command("new-save", true, false)); assertTrue(current.promote(0));
    assertEquals(AttachmentSaveLifetime.Command.IGNORE, current.command("old-save", false, true));
    assertEquals(AttachmentSaveLifetime.Command.IGNORE, current.command(null, false, true));
    assertEquals(AttachmentSaveLifetime.Command.IGNORE, current.command("new-save", true, false));
    assertTrue(current.valid(8, 1)); assertEquals(AttachmentSaveLifetime.Phase.STAGING, current.phase());
    assertEquals(AttachmentSaveLifetime.Command.CANCEL, current.command("new-save", false, true)); current.cancel("cancelled");
    assertEquals(AttachmentSaveLifetime.Command.IGNORE, current.command("new-save", true, false));
  }
}
