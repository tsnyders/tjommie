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
  boolean awaitingConfirmation(Ticket ticket) { return owns(ticket) && "permission".equals(ticket.phase); }
  boolean listening(Ticket ticket) { if (!owns(ticket) || !"permission".equals(ticket.phase)) return false; ticket.phase = "listening"; return true; }
  boolean transcribing(Ticket ticket) { if (!owns(ticket) || !"listening".equals(ticket.phase)) return false; ticket.phase = "transcribing"; return true; }
  boolean finish(Ticket ticket) { if (!owns(ticket)) return false; active = null; generation++; return true; }
  Ticket cancel(String id) { Ticket ticket = active; if (ticket == null || !ticket.id.equals(id)) return null; finish(ticket); return ticket; }

  /** Download success is preparation history, never authorization to capture on another recognizer. */
  static final class Preparation {
    static final class Request {
      final String id;
      final long generation;
      Request(String id, long generation) { this.id = id; this.generation = generation; }
    }
    static final class Confirmation {
      final String owner;
      final long generation;
      Confirmation(String owner, long generation) { this.owner = owner; this.generation = generation; }
    }
    private long generation;
    private Request active;
    private Confirmation confirmed;
    Request begin(String id) {
      if (active != null) throw new IllegalStateException("Another model request is active.");
      if (id == null || !id.matches("[A-Za-z0-9_-]{1,80}")) throw new IllegalArgumentException("Invalid model request.");
      confirmed = null;
      active = new Request(id, ++generation);
      return active;
    }
    boolean owns(Request request) { return request != null && active == request && request.generation == generation; }
    boolean finish(Request request, boolean success) {
      if (!owns(request)) return false;
      active = null;
      ++generation;
      confirmed = success ? new Confirmation(request.id, generation) : null;
      return true;
    }
    Confirmation confirmed() { return confirmed; }
    boolean owns(Confirmation proof) { return proof != null && confirmed == proof && proof.generation == generation; }
    boolean awaitedBy(SpeechSession sessions, Ticket ticket, Confirmation proof, String owner) {
      return sessions.awaitingConfirmation(ticket) && owns(proof) && proof.owner.equals(owner);
    }
    boolean release(String id) {
      if (id == null || (active == null || !active.id.equals(id)) && (confirmed == null || !confirmed.owner.equals(id))) return false;
      clear(); return true;
    }
    void clear() { active = null; confirmed = null; ++generation; }
  }

  static String supportProblem(int error) { return error == 14 ? "support_uncheckable" : error == 12 ? "language_unsupported" : error == 13 ? "language_unavailable" : "support_failed"; }
  static String downloadProblem(int error) { return error == 15 ? "download_unobservable" : error == 14 ? "support_uncheckable" : error == 12 ? "language_unsupported" : error == 13 ? "language_unavailable" : "download_failed"; }

  static String permissionProblem(boolean granted, boolean denied) { return granted ? null : denied ? "permission_denied" : "permission_required"; }
  static boolean canQuerySupport(boolean trusted, boolean onDevice, boolean granted, boolean idle) { return trusted && onDevice && granted && idle; }

  static boolean trustedOrigin(String value) {
    try {
      URI uri = new URI(value);
      return "https".equals(uri.getScheme()) && "localhost".equals(uri.getHost()) && uri.getUserInfo() == null && (uri.getPort() == -1 || uri.getPort() == 443);
    } catch (Exception ignored) { return false; }
  }
}
