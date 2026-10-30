package dev.tjommie.app;

import android.os.Bundle;
import android.webkit.WebView;
import androidx.activity.OnBackPressedCallback;
import com.getcapacitor.BridgeActivity;

public class MainActivity extends BridgeActivity {
  @Override
  public void onCreate(Bundle savedInstanceState) {
    registerPlugin(TjommieSecureStorePlugin.class);
    registerPlugin(TjommieSocketPlugin.class);
    registerPlugin(TjommiePairingScannerPlugin.class);
    registerPlugin(TjommieAppearancePlugin.class);
    registerPlugin(TjommiePushRoutingPlugin.class);
    super.onCreate(savedInstanceState);
    getBridge().setWebViewClient(new StaticWebViewClient(getBridge()));
    TjommieAppearancePlugin.applySaved(getBridge());
    // Back goes back through the page's own history (the UI is one page that moves with the history API); with nowhere left to go it does what Back
    // always does. An open keyboard takes Back first, before this is asked. External links are not handled here: Capacitor already opens an
    // address outside the app's own origin in the default browser.
    getOnBackPressedDispatcher().addCallback(
      this,
      new OnBackPressedCallback(true) {
        @Override
        public void handleOnBackPressed() {
          WebView web = getBridge() == null ? null : getBridge().getWebView();
          if (web != null) {
            web.evaluateJavascript(
              "(function(){var selection=window.getSelection();if(selection&&!selection.isCollapsed){selection.removeAllRanges();return true;}var overlays=Array.from(document.querySelectorAll('[role=dialog],[role=alertdialog],[role=menu],[role=listbox]')).filter(function(e){return e.getClientRects().length>0;});if(overlays.length){overlays[overlays.length-1].dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',code:'Escape',bubbles:true,cancelable:true}));return true;}var e=new CustomEvent('tjommie:back',{cancelable:true});window.dispatchEvent(e);return e.defaultPrevented;})()",
              value -> {
                if ("true".equals(value)) return;
                if (web.canGoBack()) web.goBack();
                else {
                  setEnabled(false);
                  getOnBackPressedDispatcher().onBackPressed();
                  setEnabled(true);
                }
              }
            );
            return;
          }
          setEnabled(false);
          getOnBackPressedDispatcher().onBackPressed();
          setEnabled(true);
        }
      }
    );
  }
}
