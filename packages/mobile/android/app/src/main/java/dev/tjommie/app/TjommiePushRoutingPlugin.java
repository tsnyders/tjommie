package dev.tjommie.app;

import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.content.Context;
import android.os.Build;
import com.getcapacitor.JSObject;
import com.getcapacitor.Plugin;
import com.getcapacitor.PluginCall;
import com.getcapacitor.PluginMethod;
import com.getcapacitor.annotation.CapacitorPlugin;

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
}
