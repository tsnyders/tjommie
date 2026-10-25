package dev.tjommie.app;

import android.content.Context;
import android.content.SharedPreferences;
import android.content.res.Configuration;
import android.graphics.Color;
import android.graphics.drawable.ColorDrawable;
import android.os.Build;
import android.view.Window;
import android.view.ViewTreeObserver;
import android.webkit.WebView;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.core.view.WindowInsetsControllerCompat;
import com.getcapacitor.JSObject;
import com.getcapacitor.Plugin;
import com.getcapacitor.PluginCall;
import com.getcapacitor.PluginMethod;
import com.getcapacitor.WebViewListener;
import com.getcapacitor.annotation.CapacitorPlugin;

/** Canvas and icon contrast follow the app's choice, independently of the Android theme. */
@CapacitorPlugin(name = "TjommieAppearance")
public class TjommieAppearancePlugin extends Plugin {
  private static final String PREFS = "tjommie_appearance";
  private ViewTreeObserver.OnGlobalLayoutListener layoutListener;
  private WebViewListener pageListener;
  private boolean keyboardVisible;
  private int imeBottom;

  static void applySaved(com.getcapacitor.Bridge bridge) {
    if (bridge == null || bridge.getActivity() == null) return;
    SharedPreferences prefs = bridge.getActivity().getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    String color = prefs.getString("canvas", "#070707");
    if (color == null || !color.matches("#[0-9a-fA-F]{6}")) color = "#070707";
    boolean dark = prefs.getBoolean("dark", true);
    Window window = bridge.getActivity().getWindow();
    int canvas = Color.parseColor(color);
    window.setBackgroundDrawable(new ColorDrawable(canvas));
    // SystemBars can paint this separately and pad it on older WebViews.
    // The padded system-bar regions must follow the app, not the OS theme.
    window.getDecorView().setBackgroundColor(canvas);
    if (bridge.getWebView() != null) bridge.getWebView().setBackgroundColor(canvas);
    WindowInsetsControllerCompat controller = WindowCompat.getInsetsController(window, window.getDecorView());
    controller.setAppearanceLightStatusBars(!dark);
    controller.setAppearanceLightNavigationBars(!dark);
    if (Build.VERSION.SDK_INT >= 29) {
      window.setStatusBarContrastEnforced(false);
      window.setNavigationBarContrastEnforced(false);
    }
  }

  @Override
  public void load() {
    getActivity().runOnUiThread(() -> {
      WebView web = getBridge().getWebView();
      applySaved(getBridge());
      // Capacitor SystemBars owns decor padding and IME resizing. Observe only:
      // adding keyboard padding here or in CSS would shrink the page twice.
      layoutListener = () -> publishInsets(false);
      web.getViewTreeObserver().addOnGlobalLayoutListener(layoutListener);
      pageListener = new WebViewListener() {
        @Override
        public void onPageCommitVisible(WebView view, String url) {
          view.post(() -> { applySaved(getBridge()); publishInsets(true); });
        }
      };
      web.post(() -> publishInsets(true));
    });
  }

  private JSObject currentInsets() {
    WebView web = getBridge().getWebView();
    WindowInsetsCompat insets = ViewCompat.getRootWindowInsets(web);
    keyboardVisible = insets != null && insets.isVisible(WindowInsetsCompat.Type.ime());
    imeBottom = insets == null ? 0 : insets.getInsets(WindowInsetsCompat.Type.ime()).bottom;
    JSObject value = new JSObject();
    value.put("keyboardVisible", keyboardVisible);
    value.put("imeBottom", imeBottom / getContext().getResources().getDisplayMetrics().density);
    return value;
  }

  private void publishInsets(boolean force) {
    boolean oldVisible = keyboardVisible;
    int oldBottom = imeBottom;
    JSObject value = currentInsets();
    if (!force && oldVisible == keyboardVisible && oldBottom == imeBottom) return;
    getBridge().getWebView().evaluateJavascript(
      "(function(){var v=" + value.toString() + ";window.tjommieInsets=v;window.dispatchEvent(new CustomEvent('tjommie:insets',{detail:v}));})()", null
    );
  }

  @PluginMethod
  public void setCanvas(PluginCall call) {
    String color = call.getString("color");
    Boolean dark = call.getBoolean("dark");
    if (color == null || !color.matches("#[0-9a-fA-F]{6}") || dark == null) {
      call.reject("Invalid appearance", "invalid");
      return;
    }
    getActivity().runOnUiThread(() -> {
      getContext().getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString("canvas", color).putBoolean("dark", dark).apply();
      applySaved(getBridge());
      call.resolve();
    });
  }

  @PluginMethod
  public void getInsets(PluginCall call) {
    getActivity().runOnUiThread(() -> { publishInsets(true); call.resolve(currentInsets()); });
  }

  @Override
  protected void handleOnStart() {
    // Plugin load runs before Bridge.Builder installs its final listener list.
    getBridge().removeWebViewListener(pageListener);
    getBridge().addWebViewListener(pageListener);
    getBridge().getWebView().post(() -> { applySaved(getBridge()); publishInsets(true); });
  }

  @Override
  protected void handleOnConfigurationChanged(Configuration config) {
    // Post after all plugins have handled the OS theme/rotation change, so the
    // SystemBars default cannot replace the app-specific icon contrast.
    getBridge().getWebView().post(() -> { applySaved(getBridge()); publishInsets(true); });
  }

  @Override
  protected void handleOnDestroy() {
    WebView web = getBridge().getWebView();
    if (layoutListener != null && web.getViewTreeObserver().isAlive()) web.getViewTreeObserver().removeOnGlobalLayoutListener(layoutListener);
    if (pageListener != null) getBridge().removeWebViewListener(pageListener);
  }
}
