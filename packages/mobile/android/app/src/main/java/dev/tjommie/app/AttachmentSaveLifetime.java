package dev.tjommie.app;

import java.util.UUID;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.Executor;
import java.util.concurrent.RejectedExecutionException;
import java.util.concurrent.TimeUnit;

/** One explicit Save, measured with a monotonic clock and renewed only by its current connection. */
final class AttachmentSaveLifetime {
  static final long MAX_MILLIS = 30 * 60_000L, ACK_MILLIS = 45_000L, TICK_MILLIS = 30_000L, PROMOTION_MILLIS = 5_000L;
  enum Phase { STARTING, STAGING, PICKER, COPY, CLOSED }
  enum Command { START, CANCEL, IGNORE }
  final String id, heartbeatId;
  final long generation;
  private final long startedAt;
  private long deadline, nextTick;
  private String pending, reason;
  private Phase phase = Phase.STARTING;

  AttachmentSaveLifetime(String id, String heartbeatId, long generation, long now) {
    this.id = id; this.heartbeatId = heartbeatId; this.generation = generation; startedAt = now; deadline = now + PROMOTION_MILLIS;
  }
  synchronized boolean promote(long now) {
    if (!active(now) || phase != Phase.STARTING) return false;
    phase = Phase.STAGING; deadline = now + ACK_MILLIS; nextTick = now; return true;
  }
  synchronized boolean active(long now) {
    if (phase == Phase.CLOSED) return false;
    if (now - startedAt >= MAX_MILLIS) { cancel("timeout"); return false; }
    if (now >= deadline) { cancel(phase == Phase.STARTING ? "promotion_failed" : "connection_timeout"); return false; }
    return true;
  }
  synchronized boolean valid(long page, long now) { return generation == page && active(now) && phase != Phase.STARTING; }
  synchronized String heartbeat(long now) {
    if (!active(now) || phase == Phase.STARTING || pending != null || now < nextTick) return null;
    pending = UUID.randomUUID().toString(); return pending;
  }
  synchronized boolean acknowledge(String heartbeatId, String challenge, long page, boolean alive, long now) {
    if (!valid(page, now) || !this.heartbeatId.equals(heartbeatId) || pending == null || !pending.equals(challenge)) return false;
    if (!alive) { cancel("connection_closed"); return false; }
    pending = null; deadline = now + ACK_MILLIS; nextTick = now + TICK_MILLIS; return true;
  }
  synchronized boolean picker(long page, long now) { if (!valid(page, now) || phase != Phase.STAGING) return false; phase = Phase.PICKER; return true; }
  synchronized boolean copy(long page, long now) { if (!valid(page, now) || phase != Phase.PICKER) return false; phase = Phase.COPY; return true; }
  synchronized void cancel(String reason) { if (phase != Phase.CLOSED) { phase = Phase.CLOSED; this.reason = reason; pending = null; } }
  synchronized String reason() { return reason; }
  synchronized Phase phase() { return phase; }
  synchronized Command command(String incomingId, boolean start, boolean cancel) {
    if (phase == Phase.CLOSED || !id.equals(incomingId)) return Command.IGNORE;
    if (cancel) return Command.CANCEL;
    return start && phase == Phase.STARTING ? Command.START : Command.IGNORE;
  }

  // Reserve IO ordering now, including before executor shutdown, while retiring the service off that queue.
  static void cleanupAfterRetirement(Executor io, CompletableFuture<Void> stopped, Runnable cleanup, Runnable failed) {
    try { io.execute(() -> {
      try { stopped.get(PROMOTION_MILLIS, TimeUnit.MILLISECONDS); }
      catch (InterruptedException error) { Thread.currentThread().interrupt(); failed.run(); return; }
      catch (Exception error) { failed.run(); return; }
      cleanup.run();
    }); } catch (RejectedExecutionException error) { failed.run(); }
  }
}
