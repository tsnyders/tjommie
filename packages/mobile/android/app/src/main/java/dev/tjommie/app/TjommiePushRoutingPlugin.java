package dev.tjommie.app;

import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.content.Context;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import com.getcapacitor.JSObject;
import com.getcapacitor.Plugin;
import com.getcapacitor.PluginCall;
import com.getcapacitor.PluginMethod;
import com.getcapacitor.annotation.CapacitorPlugin;
import com.google.android.gms.tasks.Task;
import com.google.firebase.messaging.FirebaseMessaging;
import java.util.concurrent.Executor;
import java.util.function.Consumer;
import java.util.function.Supplier;

// Capacitor PushNotifications delivers notification taps to JS, including cold starts. This plugin supplies its channel and build gate.
@CapacitorPlugin(name = "TjommiePushRouting")
public class TjommiePushRoutingPlugin extends Plugin {
  @Override public void load() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return;
    NotificationManager manager = (NotificationManager) getContext().getSystemService(Context.NOTIFICATION_SERVICE);
    manager.createNotificationChannel(new NotificationChannel("bots", "Bots", NotificationManager.IMPORTANCE_HIGH));
  }

  @PluginMethod public void available(PluginCall call) {
    JSObject result = new JSObject();
    result.put("value", BuildConfig.PUSH);
    call.resolve(result);
  }

  @PluginMethod public void unregister(PluginCall call) {
    awaitDeletion(BuildConfig.PUSH, () -> {
      FirebaseMessaging messaging = FirebaseMessaging.getInstance();
      messaging.setAutoInitEnabled(false);
      return messaging.deleteToken();
    }, new Handler(Looper.getMainLooper())::post, call::resolve, call::reject);
  }

  static void awaitDeletion(boolean enabled, Supplier<Task<Void>> deleteToken, Executor callbacks, Runnable resolve, Consumer<String> reject) {
    if (!enabled) {
      resolve.run();
      return;
    }
    Task<Void> deletion;
    try {
      deletion = deleteToken.get();
    } catch (RuntimeException error) {
      reject.accept("Could not unregister push notifications.");
      return;
    }
    deletion.addOnCompleteListener(callbacks, task -> {
      if (task.isSuccessful()) resolve.run();
      else reject.accept("Could not unregister push notifications.");
    });
  }
}
