package dev.tjommie.app;

import android.app.Activity;
import android.content.Intent;
import android.net.Uri;
import android.provider.DocumentsContract;
import android.util.Base64;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebView;
import androidx.activity.result.ActivityResult;
import com.getcapacitor.Bridge;
import com.getcapacitor.JSObject;
import com.getcapacitor.Plugin;
import com.getcapacitor.PluginCall;
import com.getcapacitor.PluginMethod;
import com.getcapacitor.WebViewListener;
import com.getcapacitor.annotation.ActivityCallback;
import com.getcapacitor.annotation.CapacitorPlugin;
import java.io.ByteArrayInputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.security.MessageDigest;
import java.util.Arrays;
import java.util.HashMap;
import java.util.Map;
import java.util.Set;
import java.util.UUID;
import java.util.WeakHashMap;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.RejectedExecutionException;
import java.util.concurrent.Semaphore;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;

/** App-private staging and opaque WebView handles; no arbitrary filesystem reader or HTTP listener. */
@CapacitorPlugin(name = "TjommieAttachments")
public class TjommieAttachmentPlugin extends Plugin {
  private static final Map<Bridge, TjommieAttachmentPlugin> INSTANCES = new WeakHashMap<>();
  private static final class Meta {
    final String name, mime; final long size, generation; final Semaphore serial = new Semaphore(1); final AtomicInteger queued = new AtomicInteger(); final AtomicBoolean cancelled = new AtomicBoolean();
    final Set<AttachmentInputStream> streams = ConcurrentHashMap.newKeySet();
    Meta(JSObject info, long generation) throws IOException {
      name = info.optString("name", "file").replace('/', '_').replace('\\', '_').replace('\0', '_');
      Object bytes = info.opt("size");
      if (!(bytes instanceof Number) || !Double.isFinite(((Number) bytes).doubleValue()) || ((Number) bytes).doubleValue() != Math.floor(((Number) bytes).doubleValue())) throw new IOException("Invalid file metadata");
      size = ((Number) bytes).longValue(); if (size < 0 || size > AttachmentFileStore.MAX_BYTES || name.isEmpty() || name.length() > 255) throw new IOException("Invalid file metadata");
      String type = info.optString("contentType", "application/octet-stream");
      mime = type.length() <= 127 && type.matches("image/(png|jpeg|gif|webp)|audio/(wav|mpeg|mp4|ogg)|video/(mp4|webm)|application/pdf|text/markdown") ? type : "application/octet-stream";
      this.generation = generation;
    }
  }
  private static final class Pending {
    final String preview; final int length; final long generation; final CompletableFuture<byte[]> answer = new CompletableFuture<>();
    Pending(String preview, int length, long generation) { this.preview = preview; this.length = length; this.generation = generation; }
  }
  private final ExecutorService io = Executors.newSingleThreadExecutor();
  private final AtomicLong generation = new AtomicLong();
  private final AtomicBoolean picking = new AtomicBoolean();
  private final Map<String, Meta> saves = new ConcurrentHashMap<>(), previews = new ConcurrentHashMap<>();
  private final Map<String, Pending> pending = new ConcurrentHashMap<>();
  private AttachmentFileStore store;
  private WebViewListener pageListener;

  @Override public void load() {
    try { store = new AttachmentFileStore(new File(getContext().getCacheDir(), "attachment-stage")); } catch (IOException error) { store = null; }
    synchronized (INSTANCES) { INSTANCES.put(getBridge(), this); }
    pageListener = new WebViewListener() { @Override public void onPageStarted(WebView web) { invalidatePage(); } };
  }
  @Override protected void handleOnStart() { getBridge().removeWebViewListener(pageListener); getBridge().addWebViewListener(pageListener); }
  @Override protected void handleOnStop() { closePreviews(); if (!picking.get()) invalidatePage(); }
  @Override protected void handleOnDestroy() {
    invalidatePage(); getBridge().removeWebViewListener(pageListener); synchronized (INSTANCES) { INSTANCES.remove(getBridge()); } io.shutdown();
  }
  private void invalidatePage() { invalidatePage(null); }
  private void invalidatePage(PluginCall call) {
    generation.incrementAndGet(); closePreviews(); saves.clear();
    try { io.execute(() -> clearTemporary(store, call)); }
    catch (RejectedExecutionException error) { if (call != null) call.reject("Temporary file cleanup is unavailable", "cleanup_failed"); }
  }
  static void clearTemporary(AttachmentFileStore store, PluginCall call) {
    try {
      if (store == null) throw new IOException("Download staging unavailable");
      store.clear(); if (call != null) call.resolve();
    } catch (IOException error) { if (call != null) call.reject("Temporary file cleanup failed. Try again or reopen the app.", "cleanup_failed"); }
  }
  private void closePreview(String key) {
    Meta meta = previews.remove(key);
    if (meta != null) { meta.cancelled.set(true); for (AttachmentInputStream stream : meta.streams) stream.close(); meta.streams.clear(); }
    for (Map.Entry<String, Pending> entry : pending.entrySet()) if (entry.getValue().preview.equals(key)) { entry.getValue().answer.completeExceptionally(new IOException("Preview closed")); pending.remove(entry.getKey(), entry.getValue()); }
  }
  private void closePreviews() { for (String key : previews.keySet()) closePreview(key); for (Pending request : pending.values()) request.answer.completeExceptionally(new IOException("Preview closed")); pending.clear(); }
  @PluginMethod public void invalidate(PluginCall call) { invalidatePage(call); }
  private String id(PluginCall call, String key) throws IOException { String value = call.getString(key); if (value == null || !value.matches("[0-9a-f-]{36}")) throw new IOException("Invalid file handle"); return value; }
  private static byte[] decode(String encoded) throws IOException {
    if (encoded == null || encoded.isEmpty() || encoded.length() > 349528 || encoded.length() % 4 != 0 || !encoded.matches("[A-Za-z0-9+/]*={0,2}")) throw new IOException("Invalid file chunk");
    byte[] bytes;
    try { bytes = Base64.decode(encoded, Base64.NO_WRAP); } catch (IllegalArgumentException error) { throw new IOException("Invalid file chunk"); }
    if (bytes.length > AttachmentInputStream.CHUNK || !Base64.encodeToString(bytes, Base64.NO_WRAP).equals(encoded)) throw new IOException("Invalid file chunk"); return bytes;
  }
  @PluginMethod public void startSave(PluginCall call) {
    long page = generation.get(); io.execute(() -> {
      try {
        if (store == null || page != generation.get()) throw new IOException("Download unavailable");
        JSObject info = call.getObject("info"); if (info == null) throw new IOException("Invalid file metadata");
        Meta meta = new Meta(info, page); String key = store.start(meta.size, info.optString("sha256", ""));
        if (page != generation.get()) { store.cancel(key); throw new IOException("Download closed"); }
        saves.put(key, meta); JSObject result = new JSObject(); result.put("saveId", key); call.resolve(result);
      } catch (IOException error) { call.reject(error.getMessage(), "unavailable"); }
    });
  }
  @PluginMethod public void appendSave(PluginCall call) { io.execute(() -> {
    try {
      String key = id(call, "saveId"); Meta meta = saves.get(key); Object value = call.getData().opt("offset");
      if (meta == null || meta.generation != generation.get() || !(value instanceof Number) || ((Number) value).doubleValue() != Math.floor(((Number) value).doubleValue())) throw new IOException("Download closed or invalid offset");
      store.append(key, ((Number) value).longValue(), decode(call.getString("bytes"))); call.resolve();
    } catch (IOException error) { call.reject(error.getMessage(), "corrupt"); }
  }); }
  @PluginMethod public void cancelSave(PluginCall call) {
    try {
      String key = id(call, "saveId"); Meta meta = saves.get(key);
      // The copy owns the IO executor; cancellation must reach it before queued cleanup.
      if (meta != null) meta.cancelled.set(true);
      io.execute(() -> { try { saves.remove(key); if (store != null) store.cancel(key); call.resolve(); } catch (IOException error) { call.reject("Download cleanup failed", "cleanup_failed"); } });
    } catch (IOException error) { call.reject("Invalid file handle", "bad_params"); }
  }
  @PluginMethod public void finishSave(PluginCall call) { io.execute(() -> {
    try {
      String key = id(call, "saveId"); Meta meta = saves.get(key);
      if (meta == null || meta.cancelled.get() || meta.generation != generation.get()) throw new IOException("Download closed");
      store.verify(key); if (!picking.compareAndSet(false, true)) throw new IOException("Another save picker is open");
      getActivity().runOnUiThread(() -> {
        try { Intent intent = new Intent(Intent.ACTION_CREATE_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE).setType(meta.mime).putExtra(Intent.EXTRA_TITLE, meta.name).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION); startActivityForResult(call, intent, "saveChosen"); }
        catch (RuntimeException error) { picking.set(false); call.reject("The system save picker is unavailable", "unavailable"); }
      });
    } catch (IOException error) { call.reject(error.getMessage(), "corrupt"); }
  }); }
  @ActivityCallback private void saveChosen(PluginCall call, ActivityResult result) {
    picking.set(false); Uri uri = result.getData() == null ? null : result.getData().getData();
    io.execute(() -> {
      String key = call == null ? null : call.getString("saveId");
      try {
        Meta meta = key == null ? null : saves.get(key);
        if (result.getResultCode() != Activity.RESULT_OK || uri == null) { if (call != null) { JSObject out = new JSObject(); out.put("status", "cancelled"); call.resolve(out); } return; }
        if (meta == null || meta.cancelled.get() || meta.generation != generation.get()) throw new IOException("Download closed");
        File staged = store.verify(key); byte[] buffer = new byte[64 * 1024];
        try (InputStream input = new FileInputStream(staged); OutputStream output = getContext().getContentResolver().openOutputStream(uri, "w")) {
          if (output == null) throw new IOException("The destination could not be opened");
          AttachmentFileStore.copy(input, output, buffer, () -> meta.cancelled.get() || meta.generation != generation.get());
        }
        MessageDigest digest = AttachmentFileStore.sha256();
        try (InputStream saved = getContext().getContentResolver().openInputStream(uri)) { if (saved == null) throw new IOException("Saved content could not be checked"); for (int count; (count = saved.read(buffer)) >= 0;) { if (meta.cancelled.get() || meta.generation != generation.get()) throw new IOException("Download cancelled"); if (count > 0) digest.update(buffer, 0, count); } }
        if (!AttachmentFileStore.hex(digest.digest()).equals(store.digest(key))) throw new IOException("Saved content check failed");
        if (meta.cancelled.get() || meta.generation != generation.get()) throw new IOException("Download cancelled");
        JSObject out = new JSObject(); out.put("status", "saved"); call.resolve(out);
      } catch (Exception error) {
        boolean removed = uri == null;
        if (uri != null) try { removed = DocumentsContract.deleteDocument(getContext().getContentResolver(), uri); } catch (Exception ignored) { removed = false; }
        if (!removed) getContext().getSharedPreferences("attachment-recovery", 0).edit().putString(UUID.randomUUID().toString(), uri.toString()).apply();
        if (call != null) call.reject(removed ? "The file could not be saved" : "The provider kept an incomplete file. Remove it from the selected destination.", removed ? "unavailable" : "cleanup_failed");
      } finally { if (key != null) { saves.remove(key); try { store.cancel(key); } catch (IOException ignored) { /* Retried from private staging on startup. */ } } }
    });
  }
  @PluginMethod public void preview(PluginCall call) {
    try {
      JSObject info = call.getObject("info"); if (info == null || previews.size() >= 2) throw new IOException("Two previews are already open");
      Meta meta = new Meta(info, generation.get()); String key = UUID.randomUUID().toString(); previews.put(key, meta);
      if (meta.generation != generation.get()) { closePreview(key); throw new IOException("Preview closed"); }
      JSObject result = new JSObject(); result.put("previewId", key); result.put("url", "https://localhost/_attachments/" + key); call.resolve(result);
    } catch (IOException error) { call.reject(error.getMessage(), "unavailable"); }
  }
  @PluginMethod public void closePreview(PluginCall call) {
    try { closePreview(id(call, "previewId")); call.resolve(); }
    catch (IOException error) { call.reject("Invalid preview handle", "bad_params"); }
  }
  @PluginMethod public void respond(PluginCall call) {
    try {
      String requestId = id(call, "requestId"), key = id(call, "previewId"); Pending request = pending.get(requestId);
      Meta meta = previews.get(key);
      if (request == null || !request.preview.equals(key) || request.generation != generation.get() || meta == null || meta.cancelled.get() || meta.generation != request.generation) throw new IOException("Preview response not authorized");
      byte[] bytes = decode(call.getString("bytes")); if (bytes.length != request.length) throw new IOException("Invalid preview bytes");
      if (!request.answer.complete(bytes)) { Arrays.fill(bytes, (byte) 0); throw new IOException("Preview closed"); } call.resolve();
    } catch (IOException error) { String requestId = call.getString("requestId"); Pending request = requestId == null ? null : pending.get(requestId); if (request != null && request.preview.equals(call.getString("previewId")) && request.generation == generation.get()) request.answer.completeExceptionally(new IOException("Preview read failed")); call.reject(error.getMessage(), "bad_params"); }
  }
  private final class PreviewReader implements AttachmentInputStream.Reader {
    private final String key; private final Meta meta; private final AtomicBoolean cancelled = new AtomicBoolean(); private final AtomicReference<String> current = new AtomicReference<>();
    final AttachmentInputStream stream;
    PreviewReader(String key, Meta meta, long start, long end) { this.key = key; this.meta = meta; stream = new AttachmentInputStream(meta.size, start, end, this); meta.streams.add(stream); if (!valid()) stream.close(); }
    public boolean valid() { return !cancelled.get() && !meta.cancelled.get() && previews.get(key) == meta && meta.generation == generation.get(); }
    public byte[] read(long offset, int length) throws IOException { return TjommieAttachmentPlugin.this.read(key, meta, offset, length, cancelled, current); }
    public void cancel() { cancelled.set(true); meta.streams.remove(stream); String id = current.get(); Pending request = id == null ? null : pending.remove(id); if (request != null) request.answer.completeExceptionally(new IOException("Preview closed")); }
  }
  private byte[] read(String key, Meta meta, long offset, int length, AtomicBoolean cancelled, AtomicReference<String> current) throws IOException {
    if (meta.queued.incrementAndGet() > 2) { meta.queued.decrementAndGet(); throw new IOException("Too many preview reads"); }
    boolean acquired = false; String requestId = UUID.randomUUID().toString();
    try {
      long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(30);
      while (!acquired && !cancelled.get() && !meta.cancelled.get() && System.nanoTime() < deadline) acquired = meta.serial.tryAcquire(100, TimeUnit.MILLISECONDS);
      if (!acquired || cancelled.get() || meta.cancelled.get() || previews.get(key) != meta || meta.generation != generation.get()) throw new IOException("Preview closed");
      Pending request = new Pending(key, length, meta.generation); current.set(requestId); pending.put(requestId, request); if (cancelled.get() || meta.cancelled.get()) throw new IOException("Preview closed");
      getActivity().runOnUiThread(() -> { if (cancelled.get() || meta.cancelled.get() || meta.generation != generation.get() || pending.get(requestId) != request) { request.answer.completeExceptionally(new IOException("Preview closed")); return; } JSObject event = new JSObject(); event.put("previewId", key); event.put("requestId", requestId); event.put("offset", offset); event.put("length", length); notifyListeners("read", event); });
      byte[] bytes = request.answer.get(30, TimeUnit.SECONDS); if (cancelled.get() || meta.cancelled.get() || previews.get(key) != meta || meta.generation != generation.get()) { Arrays.fill(bytes, (byte) 0); throw new IOException("Preview closed"); } return bytes;
    } catch (Exception error) { throw new IOException("The file host did not provide preview bytes"); }
    finally { pending.remove(requestId); current.compareAndSet(requestId, null); if (acquired) meta.serial.release(); meta.queued.decrementAndGet(); }
  }
  public static WebResourceResponse intercept(Bridge bridge, WebResourceRequest request) {
    Uri uri = request.getUrl(); if (!"https".equals(uri.getScheme()) || !"localhost".equals(uri.getAuthority()) || uri.getQuery() != null || uri.getFragment() != null || uri.getPath() == null || !uri.getPath().startsWith("/_attachments/")) return null;
    TjommieAttachmentPlugin plugin; synchronized (INSTANCES) { plugin = INSTANCES.get(bridge); }
    Map<String, String> headers = new HashMap<>(); headers.put("Cache-Control", "no-store"); headers.put("X-Content-Type-Options", "nosniff"); headers.put("Content-Security-Policy", "default-src 'none'; sandbox");
    String key = uri.getPath().substring("/_attachments/".length()); Meta meta = plugin == null ? null : plugin.previews.get(key);
    if (meta == null || !key.matches("[0-9a-f-]{36}")) return response(404, "Not Found", "application/octet-stream", headers, new ByteArrayInputStream(new byte[0]));
    if (!"GET".equals(request.getMethod()) && !"HEAD".equals(request.getMethod())) return response(405, "Method Not Allowed", meta.mime, headers, new ByteArrayInputStream(new byte[0]));
    String rangeHeader = null; for (Map.Entry<String, String> header : request.getRequestHeaders().entrySet()) if (header.getKey().equalsIgnoreCase("Range")) rangeHeader = header.getValue();
    long[] range;
    try { range = "HEAD".equals(request.getMethod()) ? null : AttachmentRanges.parse(rangeHeader, meta.size); }
    catch (IllegalArgumentException error) { headers.put("Content-Range", "bytes */" + meta.size); return response(416, "Range Not Satisfiable", meta.mime, headers, new ByteArrayInputStream(new byte[0])); }
    long start = range == null ? 0 : range[0], end = range == null ? meta.size - 1 : range[1];
    headers.putAll(previewHeaders(meta.size, start, end, range != null));
    InputStream input = "HEAD".equals(request.getMethod()) ? AttachmentInputStream.headerOnly(meta.size) : plugin.new PreviewReader(key, meta, start, end).stream;
    return response(range == null ? 200 : 206, range == null ? "OK" : "Partial Content", meta.mime, headers, input);
  }
  static Map<String, String> previewHeaders(long size, long start, long end, boolean partial) {
    Map<String, String> headers = new HashMap<>(); headers.put("Accept-Ranges", "bytes");
    // Chromium supplies Content-Length from the stream; appending it here creates duplicate values.
    if (partial) headers.put("Content-Range", "bytes " + start + "-" + end + "/" + size); return headers;
  }
  private static WebResourceResponse response(int status, String reason, String mime, Map<String, String> headers, InputStream input) { return new WebResourceResponse(mime, null, status, reason, headers, input); }
}
