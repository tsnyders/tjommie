package dev.tjommie.app;

import java.net.URI;

/** Main-thread session fence; permission and recognizer callbacks retain their original ticket. */
final class SpeechSession {
  static final class Ticket {
    final String id;
    final String language;
    final long generation;
    String phase = "permission";
    Ticket(String id, String language, long generation) { this.id = id; this.language = language; this.generation = generation; }
  }
  private long generation;
  private Ticket active;

  Ticket begin(String id, String language) {
    if (active != null) throw new IllegalStateException("Another dictation is still active.");
    if (id == null || !id.matches("[A-Za-z0-9_-]{1,80}") || language == null || !language.matches("en(?:-[A-Za-z0-9]{2,8})*")) throw new IllegalArgumentException("Invalid speech session or language.");
    active = new Ticket(id, language, ++generation);
    return active;
  }
  Ticket current() { return active; }
  boolean owns(Ticket ticket) { return ticket != null && active == ticket && ticket.generation == generation; }
  boolean listening(Ticket ticket) { if (!owns(ticket) || !"permission".equals(ticket.phase)) return false; ticket.phase = "listening"; return true; }
  boolean transcribing(Ticket ticket) { if (!owns(ticket) || !"listening".equals(ticket.phase)) return false; ticket.phase = "transcribing"; return true; }
  boolean finish(Ticket ticket) { if (!owns(ticket)) return false; active = null; generation++; return true; }
  Ticket cancel(String id) { Ticket ticket = active; if (ticket == null || !ticket.id.equals(id)) return null; finish(ticket); return ticket; }

  static boolean trustedOrigin(String value) {
    try {
      URI uri = new URI(value);
      return "https".equals(uri.getScheme()) && "localhost".equals(uri.getHost()) && uri.getUserInfo() == null && (uri.getPort() == -1 || uri.getPort() == 443);
    } catch (Exception ignored) { return false; }
  }
}
