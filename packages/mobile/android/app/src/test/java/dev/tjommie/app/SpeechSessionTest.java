package dev.tjommie.app;

import org.junit.Test;
import static org.junit.Assert.*;

public class SpeechSessionTest {
  @Test public void downloadQueryDiagnosticsNeverTreatUncheckableAsMissing() {
    assertEquals("support_uncheckable", SpeechSession.supportProblem(14));
    assertEquals("language_unsupported", SpeechSession.supportProblem(12));
    assertEquals("language_unavailable", SpeechSession.supportProblem(13));
    assertEquals("support_failed", SpeechSession.supportProblem(7));
    assertEquals("download_unobservable", SpeechSession.downloadProblem(15));
  }
  @Test public void onlySuccessfulCurrentDownloadAllowsFreshCaptureConfirmation() {
    SpeechSession.Preparation preparation = new SpeechSession.Preparation();
    SpeechSession.Preparation.Request first = preparation.begin("request_one");
    assertNull(preparation.confirmed());
    assertTrue(preparation.finish(first, false));
    assertNull(preparation.confirmed()); // scheduled, progress, timeout and error are not success
    SpeechSession.Preparation.Request current = preparation.begin("request_two");
    assertFalse(preparation.finish(first, true));
    assertTrue(preparation.finish(current, true));
    SpeechSession.Preparation.Confirmation proof = preparation.confirmed();
    assertTrue(preparation.owns(proof));
    assertFalse(preparation.release("request_one"));
    assertTrue(preparation.owns(proof));
    assertTrue(preparation.release("request_two"));
    assertFalse(preparation.owns(proof));
  }
  @Test public void pauseNavigationAndPermissionLossInvalidateDownloadAndCaptureProofs() {
    SpeechSession.Preparation preparation = new SpeechSession.Preparation();
    SpeechSession.Preparation.Request request = preparation.begin("request");
    preparation.clear();
    assertFalse(preparation.finish(request, true)); assertNull(preparation.confirmed());
    request = preparation.begin("new_request"); preparation.finish(request, true);
    SpeechSession.Preparation.Confirmation proof = preparation.confirmed();
    preparation.clear(); assertFalse(preparation.owns(proof));
    SpeechSession session = new SpeechSession();
    SpeechSession.Ticket capture = session.begin("capture", "en-US");
    session.cancel(capture.id);
    assertFalse(session.listening(capture)); // a late download success cannot start audio
  }
  @Test public void releasingSettingsCannotCancelAnIndependentOrAlreadyListeningCapture() {
    SpeechSession.Preparation preparation = new SpeechSession.Preparation();
    preparation.finish(preparation.begin("settings"), true);
    SpeechSession.Preparation.Confirmation proof = preparation.confirmed();
    SpeechSession sessions = new SpeechSession();
    SpeechSession.Ticket ticket = sessions.begin("chat", "en-US");
    assertFalse(preparation.awaitedBy(sessions, ticket, null, "settings"));
    assertFalse(preparation.awaitedBy(sessions, ticket, proof, "old_settings"));
    assertTrue(preparation.awaitedBy(sessions, ticket, proof, "settings"));
    sessions.listening(ticket);
    assertFalse(preparation.awaitedBy(sessions, ticket, proof, "settings"));
    preparation.release("settings");
    assertTrue(sessions.owns(ticket));
  }
  @Test public void permissionStatusNeverQueriesSupportBeforeGrant() {
    assertEquals("permission_required", SpeechSession.permissionProblem(false, false));
    assertEquals("permission_denied", SpeechSession.permissionProblem(false, true));
    assertNull(SpeechSession.permissionProblem(true, false));
    assertFalse(SpeechSession.canQuerySupport(true, true, false, true));
    assertFalse(SpeechSession.canQuerySupport(false, true, true, true));
    assertFalse(SpeechSession.canQuerySupport(true, false, true, true));
    assertFalse(SpeechSession.canQuerySupport(true, true, true, false));
    assertTrue(SpeechSession.canQuerySupport(true, true, true, true));
  }
  @Test public void cancellationFencesPermissionAndRecognitionCallbacks() {
    SpeechSession session = new SpeechSession();
    SpeechSession.Ticket old = session.begin("old", "en-US");
    assertNotNull(session.cancel("old"));
    SpeechSession.Ticket current = session.begin("new", "en-US");
    assertFalse(session.owns(old)); assertFalse(session.listening(old)); assertFalse(session.finish(old)); assertTrue(session.owns(current));
  }
  @Test public void onlyOneSessionAndOneFinalAreAccepted() {
    SpeechSession session = new SpeechSession();
    SpeechSession.Ticket ticket = session.begin("one", "en-US");
    try { session.begin("two", "en-US"); fail(); } catch (IllegalStateException expected) {}
    assertTrue(session.awaitingConfirmation(ticket));
    assertTrue(session.listening(ticket)); assertFalse(session.awaitingConfirmation(ticket)); // duplicate SDK completion cannot replace the capture deadline
    assertTrue(session.transcribing(ticket)); assertFalse(session.awaitingConfirmation(ticket)); assertFalse(session.transcribing(ticket));
    assertTrue(session.finish(ticket)); assertFalse(session.finish(ticket));
  }
  @Test public void wrongCancellationCannotAffectAnotherSession() {
    SpeechSession session = new SpeechSession();
    SpeechSession.Ticket ticket = session.begin("one", "en-US");
    assertNull(session.cancel("other")); assertTrue(session.owns(ticket));
  }
  @Test public void rejectsInvalidSessionAndUntestedLanguageFamily() {
    for (String id : new String[] { "", "../one", "a".repeat(81) }) try { new SpeechSession().begin(id, "en-US"); fail(); } catch (IllegalArgumentException expected) {}
    try { new SpeechSession().begin("one", "af-ZA"); fail(); } catch (IllegalArgumentException expected) {}
  }
  @Test public void trustsOnlyTheConfiguredLocalAppOrigin() {
    assertTrue(SpeechSession.trustedOrigin("https://localhost/chat/"));
    assertTrue(SpeechSession.trustedOrigin("https://localhost:443/"));
    for (String url : new String[] { "http://localhost/", "https://localhost.evil/", "https://user@localhost/", "https://localhost:444/", "file:///tmp/test", "bad" }) assertFalse(SpeechSession.trustedOrigin(url));
  }
}
