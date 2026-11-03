package dev.tjommie.app;

import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.net.Uri;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.SystemClock;
import androidx.core.app.NotificationCompat;
import androidx.core.app.ServiceCompat;
import androidx.core.content.ContextCompat;
import java.io.IOException;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;

/** A single, non-restarting foreground component for an explicit current-device Save. */
public final class AttachmentSaveService extends Service {
  private static final String CHANNEL = "attachment-save", START = "attachment.save.START", CANCEL = "attachment.save.CANCEL";
  private static final int NOTIFICATION = 4873;
  private static final AtomicReference<Run> ACTIVE = new AtomicReference<>();
  interface Listener { void heartbeat(String challenge); void stopped(); }
  private static final class Run {
    final Context context; final AttachmentSaveLifetime lifetime; final Listener listener;
    final CompletableFuture<Void> ready = new CompletableFuture<>(), stopped = new CompletableFuture<>();
    final AtomicBoolean closed = new AtomicBoolean();
    AttachmentSaveService service;
    Run(Context context, AttachmentSaveLifetime lifetime, Listener listener) { this.context = context.getApplicationContext(); this.lifetime = lifetime; this.listener = listener; }
  }
  private Run running;
  private final Handler handler = new Handler(Looper.getMainLooper());
  private final Runnable check = new Runnable() { @Override public void run() {
    Run run = running; if (run == null || run.closed.get()) return;
    long now = SystemClock.elapsedRealtime();
    if (!run.lifetime.active(now)) { finish(run, run.lifetime.reason()); return; }
    String challenge = run.lifetime.heartbeat(now);
    try { if (challenge != null) run.listener.heartbeat(challenge); }
    catch (RuntimeException error) { finish(run, "connection_closed"); return; }
    handler.postDelayed(this, 1_000);
  } };

  static CompletableFuture<Void> start(Context context, AttachmentSaveLifetime lifetime, Listener listener) {
    Run run = new Run(context, lifetime, listener);
    if (!ACTIVE.compareAndSet(null, run)) { run.ready.completeExceptionally(new IOException("Another save is active")); return run.ready; }
    try { ContextCompat.startForegroundService(context, intent(context, START, lifetime.id)); }
    catch (RuntimeException error) { finish(run, "promotion_failed"); }
    new Handler(Looper.getMainLooper()).postDelayed(() -> { if (!run.ready.isDone()) finish(run, "promotion_failed"); }, AttachmentSaveLifetime.PROMOTION_MILLIS);
    return run.ready;
  }
  static CompletableFuture<Void> stop(AttachmentSaveLifetime lifetime, String reason) {
    lifetime.cancel(reason); Run run = ACTIVE.get();
    if (run == null || run.lifetime != lifetime) return CompletableFuture.completedFuture(null);
    finish(run, reason); return run.stopped;
  }
  private static Intent intent(Context context, String action, String id) {
    return new Intent(context, AttachmentSaveService.class).setAction(action).setData(Uri.parse("tjommie-save:" + id));
  }
  private static void finish(Run run, String reason) {
    run.lifetime.cancel(reason);
    if (!run.closed.compareAndSet(false, true)) return;
    Runnable stop = () -> {
      AttachmentSaveService service = run.service;
      if (service != null) {
        service.handler.removeCallbacks(service.check); service.running = null;
        ServiceCompat.stopForeground(service, ServiceCompat.STOP_FOREGROUND_REMOVE); service.stopSelf();
      } else run.context.stopService(intent(run.context, START, run.lifetime.id));
      ACTIVE.compareAndSet(run, null);
      run.ready.completeExceptionally(new IOException("The save could not stay connected"));
      // Retire the foreground component before file cleanup can wait on provider IO.
      run.stopped.complete(null); try { run.listener.stopped(); } catch (RuntimeException ignored) { /* The component is already retired if its page was destroyed. */ }
    };
    if (Looper.myLooper() == Looper.getMainLooper()) stop.run(); else new Handler(Looper.getMainLooper()).post(stop);
  }
  @Override public int onStartCommand(Intent intent, int flags, int startId) {
    Run run = ACTIVE.get();
    if (run == null || run.closed.get()) { stopSelf(startId); return START_NOT_STICKY; }
    String id = intent != null && Uri.parse("tjommie-save:" + run.lifetime.id).equals(intent.getData()) ? run.lifetime.id : null;
    AttachmentSaveLifetime.Command command = run.lifetime.command(id, intent != null && START.equals(intent.getAction()), intent != null && CANCEL.equals(intent.getAction()));
    if (command == AttachmentSaveLifetime.Command.IGNORE) return START_NOT_STICKY;
    if (command == AttachmentSaveLifetime.Command.CANCEL) { finish(run, "cancelled"); return START_NOT_STICKY; }
    running = run; run.service = this;
    try {
      if (Build.VERSION.SDK_INT >= 26) {
        NotificationChannel channel = new NotificationChannel(CHANNEL, "File saves", NotificationManager.IMPORTANCE_LOW);
        getSystemService(NotificationManager.class).createNotificationChannel(channel);
      }
      PendingIntent cancel = PendingIntent.getService(this, 0, intent(this, CANCEL, run.lifetime.id), PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
      android.app.Notification notification = new NotificationCompat.Builder(this, CHANNEL)
        .setSmallIcon(android.R.drawable.stat_sys_download).setContentTitle("Saving a file")
        .setContentText("Keeping the transfer connected while you choose where to save.")
        .setOngoing(true).setOnlyAlertOnce(true).setPriority(NotificationCompat.PRIORITY_LOW)
        .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
        .addAction(android.R.drawable.ic_menu_close_clear_cancel, "Cancel", cancel).build();
      ServiceCompat.startForeground(this, NOTIFICATION, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC);
      if (!run.lifetime.promote(SystemClock.elapsedRealtime())) { finish(run, "promotion_failed"); return START_NOT_STICKY; }
      run.ready.complete(null); handler.post(check);
    } catch (RuntimeException error) { finish(run, "promotion_failed"); }
    return START_NOT_STICKY;
  }
  @Override public void onTimeout(int startId, int fgsType) { if (running != null) finish(running, "timeout"); else stopSelf(); }
  @Override public void onTaskRemoved(Intent rootIntent) { if (running != null) finish(running, "cancelled"); else stopSelf(); }
  @Override public void onDestroy() { if (running != null) finish(running, "destroyed"); super.onDestroy(); }
  @Override public IBinder onBind(Intent intent) { return null; }
}
