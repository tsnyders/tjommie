package dev.tjommie.app;

/** A byte-free local picker intent; only explicit activation binds a current transfer generation. */
final class AttachmentSaveLifetime {
  static final class CleanupNotice {
    private boolean pending;
    synchronized void retain() { pending = true; }
    synchronized boolean take(boolean resumed) { if (!resumed || !pending) return false; pending = false; return true; }
  }
  static final long MAX_MILLIS = 30 * 60_000L;
  enum Phase { PICKER, SELECTED, ACTIVE, COPY, CLOSED }
  final String id;
  private final long page, started;
  private long transfer;
  private Phase phase = Phase.PICKER;
  private String reason;
  AttachmentSaveLifetime(String id, long page, long now) { this.id = id; this.page = page; started = now; }
  private boolean current(long page, long now) {
    if (phase == Phase.CLOSED) return false;
    if (this.page != page) { cancel("navigation"); return false; }
    if (now - started >= MAX_MILLIS) { cancel("timeout"); return false; }
    return true;
  }
  synchronized boolean pending(long page, long now) { return current(page, now) && phase == Phase.PICKER; }
  synchronized boolean choose(long page, long now) { if (!pending(page, now)) return false; phase = Phase.SELECTED; return true; }
  synchronized boolean selected(long page, long now) { return current(page, now) && phase == Phase.SELECTED; }
  synchronized boolean activate(long page, long transfer, long now) {
    if (!selected(page, now)) return false; this.transfer = transfer; phase = Phase.ACTIVE; return true;
  }
  synchronized boolean valid(long page, long transfer, long now) {
    if (!current(page, now) || !transferring()) return false;
    if (this.transfer != transfer) { cancel("disconnect"); return false; }
    return true;
  }
  synchronized boolean invalidate(boolean transfersOnly) { if (transfersOnly && !transferring()) return false; cancel("invalidated"); return true; }
  synchronized boolean copy(long page, long transfer, long now) { if (!valid(page, transfer, now) || phase != Phase.ACTIVE) return false; phase = Phase.COPY; return true; }
  synchronized boolean transferring() { return phase == Phase.ACTIVE || phase == Phase.COPY; }
  synchronized void cancel(String reason) { if (phase != Phase.CLOSED) { phase = Phase.CLOSED; this.reason = reason; } }
  synchronized String reason() { return reason; }
  synchronized Phase phase() { return phase; }
}
