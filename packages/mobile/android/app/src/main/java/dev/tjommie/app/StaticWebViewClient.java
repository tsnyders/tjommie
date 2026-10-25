package dev.tjommie.app;

import android.net.Uri;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebView;
import com.getcapacitor.Bridge;
import com.getcapacitor.BridgeWebViewClient;
import java.util.Map;

// Next exports a document per route. Capacitor's SPA fallback would serve the home
// document on reload. Keep Capacitor's bridge injection and all other handling.
final class StaticWebViewClient extends BridgeWebViewClient {
  StaticWebViewClient(Bridge bridge) { super(bridge); }

  @Override
  public WebResourceResponse shouldInterceptRequest(WebView view, WebResourceRequest request) {
    Uri url = request.getUrl();
    String path = StaticRoutes.document(url.getPath());
    if (!request.isForMainFrame() || !"GET".equals(request.getMethod()) ||
        !"https".equals(url.getScheme()) || !"localhost".equals(url.getHost()) || path == null) {
      return super.shouldInterceptRequest(view, request);
    }
    Uri document = url.buildUpon().path(path).build();
    return super.shouldInterceptRequest(view, new WebResourceRequest() {
      public Uri getUrl() { return document; }
      public boolean isForMainFrame() { return request.isForMainFrame(); }
      public boolean isRedirect() { return request.isRedirect(); }
      public boolean hasGesture() { return request.hasGesture(); }
      public String getMethod() { return request.getMethod(); }
      public Map<String, String> getRequestHeaders() { return request.getRequestHeaders(); }
    });
  }
}
