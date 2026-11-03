package dev.tjommie.app;

import org.junit.Test;
import static org.junit.Assert.*;

public class SpeechSessionTest {
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
    assertTrue(session.listening(ticket)); assertTrue(session.transcribing(ticket)); assertFalse(session.transcribing(ticket));
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
