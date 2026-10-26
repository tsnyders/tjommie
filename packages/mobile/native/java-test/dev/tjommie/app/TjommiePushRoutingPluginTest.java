package dev.tjommie.app;

import com.google.android.gms.tasks.TaskCompletionSource;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;
import org.junit.Test;
import static org.junit.Assert.*;

public class TjommiePushRoutingPluginTest {
  @Test public void resolvesWithoutFirebaseWhenPushIsDisabled() {
    AtomicBoolean resolved = new AtomicBoolean();
    TjommiePushRoutingPlugin.awaitDeletion(false, () -> { throw new AssertionError("Firebase was touched"); }, Runnable::run,
      () -> resolved.set(true), message -> fail(message));
    assertTrue(resolved.get());
  }

  @Test public void waitsForFirebaseDeletion() {
    TaskCompletionSource<Void> deletion = new TaskCompletionSource<>();
    AtomicBoolean resolved = new AtomicBoolean();
    TjommiePushRoutingPlugin.awaitDeletion(true, deletion::getTask, Runnable::run,
      () -> resolved.set(true), message -> fail(message));
    assertFalse(resolved.get());
    deletion.setResult(null);
    assertTrue(resolved.get());
  }

  @Test public void rejectsFailedDeletionWithoutTheToken() {
    TaskCompletionSource<Void> deletion = new TaskCompletionSource<>();
    AtomicReference<String> rejected = new AtomicReference<>();
    TjommiePushRoutingPlugin.awaitDeletion(true, deletion::getTask, Runnable::run,
      () -> fail("Deletion failed"), rejected::set);
    deletion.setException(new IllegalStateException("secret-token"));
    assertEquals("Could not unregister push notifications.", rejected.get());
  }

  @Test public void rejectsWhenFirebaseThrowsBeforeReturningATask() {
    AtomicReference<String> rejected = new AtomicReference<>();
    TjommiePushRoutingPlugin.awaitDeletion(true, () -> { throw new IllegalStateException("secret-token"); }, Runnable::run,
      () -> fail("Deletion unexpectedly succeeded"), rejected::set);
    assertEquals("Could not unregister push notifications.", rejected.get());
  }
}
