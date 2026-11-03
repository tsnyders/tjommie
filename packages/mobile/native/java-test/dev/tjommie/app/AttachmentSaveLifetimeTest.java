package dev.tjommie.app;
import static org.junit.Assert.*;
import org.junit.Test;

public class AttachmentSaveLifetimeTest {
  @Test public void detachedProviderCleanupWarningWaitsForForegroundAndCoalescesUntilDelivered() {
    AttachmentSaveLifetime.CleanupNotice notice = new AttachmentSaveLifetime.CleanupNotice();
    notice.retain(); assertFalse(notice.take(false)); notice.retain(); assertFalse(notice.take(false));
    assertTrue(notice.take(true)); assertFalse(notice.take(true)); notice.retain(); assertTrue(notice.take(true));
  }
  private AttachmentSaveLifetime intent() { return new AttachmentSaveLifetime("save", 7, 100); }
  @Test public void aTenMinutePickerHasNoActiveTransferAndNeedsExplicitActivation() {
    AttachmentSaveLifetime save = intent(); assertTrue(save.pending(7, 600_100)); assertFalse(save.transferring()); assertFalse(save.valid(7, 1, 600_100));
    assertTrue(save.choose(7, 600_101)); assertTrue(save.selected(7, 600_102)); assertFalse(save.copy(7, 1, 600_102));
    assertTrue(save.activate(7, 12, 600_103)); assertTrue(save.valid(7, 12, 600_104)); assertFalse(save.activate(7, 13, 600_104));
    assertTrue(save.copy(7, 12, 600_105)); assertFalse(save.copy(7, 12, 600_106));
  }
  @Test public void transportOnlyInvalidationPreservesOnlyByteFreePickerAndSelectedIntents() {
    AttachmentSaveLifetime picker = intent(); assertFalse(picker.invalidate(true)); assertTrue(picker.choose(7, 200));
    assertFalse(picker.invalidate(true)); assertTrue(picker.selected(7, 300)); assertTrue(picker.activate(7, 25, 400));
    assertTrue(picker.invalidate(true)); assertFalse(picker.valid(7, 25, 401)); assertFalse(picker.activate(7, 26, 402));
    AttachmentSaveLifetime copy = intent(); assertTrue(copy.choose(7, 101)); assertTrue(copy.activate(7, 1, 102)); assertTrue(copy.copy(7, 1, 103)); assertTrue(copy.invalidate(true)); assertFalse(copy.valid(7, 1, 104));
  }
  @Test public void anActiveTransferCannotRebindAfterGenerationLoss() {
    AttachmentSaveLifetime save = intent(); assertTrue(save.choose(7, 101)); assertTrue(save.activate(7, 2, 102));
    assertFalse(save.valid(7, 3, 103)); assertFalse(save.valid(7, 2, 104)); assertFalse(save.activate(7, 3, 105)); assertEquals("disconnect", save.reason());
  }
  @Test public void navigationAndSwitchAwayThenBackNeverReactivateAnOldPicker() {
    AttachmentSaveLifetime save = intent(); assertFalse(save.choose(8, 101)); assertFalse(save.choose(7, 102)); assertFalse(save.activate(7, 1, 103));
    AttachmentSaveLifetime selected = intent(); assertTrue(selected.choose(7, 101)); assertTrue(selected.invalidate(false)); assertFalse(selected.activate(7, 1, 102));
  }
  @Test public void hardCancellationIsStickyInEveryPhaseAndLateResultsCannotPublish() {
    for (String reason : new String[] { "cancelled", "host_changed", "refused", "revoked", "navigation", "destroyed" }) {
      AttachmentSaveLifetime save = intent(); save.cancel(reason); assertFalse(save.choose(7, 101)); assertFalse(save.activate(7, 1, 102)); assertFalse(save.valid(7, 1, 103)); save.cancel("other"); assertEquals(reason, save.reason());
    }
  }
  @Test public void absolutePickerExpiryIsCheckedOnReturnWithoutABackgroundTimer() {
    AttachmentSaveLifetime save = intent(); assertFalse(save.choose(7, 100 + AttachmentSaveLifetime.MAX_MILLIS)); assertEquals("timeout", save.reason());
    AttachmentSaveLifetime selected = intent(); assertTrue(selected.choose(7, 101)); assertFalse(selected.activate(7, 1, 100 + AttachmentSaveLifetime.MAX_MILLIS));
  }
}
