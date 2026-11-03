package dev.tjommie.app;

import android.app.Activity;
import android.content.Intent;
import android.net.Uri;
import android.os.SystemClock;
import android.provider.DocumentsContract;
import android.util.Base64;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebView;
import android.widget.Toast;
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

/** Destination-first saves and opaque WebView streams; no arbitrary filesystem reader or HTTP listener. */
@CapacitorPlugin(name = "TjommieAttachments")
public class TjommieAttachmentPlugin extends Plugin {
  private static final Map<Bridge, TjommieAttachmentPlugin> INSTANCES = new WeakHashMap<>();
  private static final class Meta {
    final String name, mime, digest; final long size, generation; final Semaphore serial = new Semaphore(1); final AtomicInteger queued = new AtomicInteger(); final AtomicBoolean cancelled = new AtomicBoolean();
    final Set<AttachmentInputStream> streams = ConcurrentHashMap.newKeySet();
    final AtomicBoolean answered = new AtomicBoolean(), warned = new AtomicBoolean();
    volatile AttachmentSaveLifetime lifetime; volatile Uri uri; volatile String staged, recovery; volatile boolean published;
    PluginCall pickerCall;
    Meta(JSObject info, long generation) throws IOException {
      name = info.optString("name", "file").replace('/', '_').replace('\\', '_').replace('\0', '_'); digest = info.optString("sha256", "");
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
  private final AtomicLong generation = new AtomicLong(), page = new AtomicLong();
  private final AtomicReference<Meta> picking = new AtomicReference<>();
  private final AtomicBoolean resumed = new AtomicBoolean();
  private final AttachmentSaveLifetime.CleanupNotice cleanupNotice = new AttachmentSaveLifetime.CleanupNotice();
  private final Map<String, Meta> saves = new ConcurrentHashMap<>(), previews = new ConcurrentHashMap<>();
  private final Set<Meta> cleanupPending = ConcurrentHashMap.newKeySet();
  private final Map<String, Pending> pending = new ConcurrentHashMap<>();
  private AttachmentFileStore store;
  private WebViewListener pageListener;

  @Override public void load() {
    try { store = new AttachmentFileStore(new File(getContext().getCacheDir(), "attachment-stage")); } catch (IOException error) { store = null; }
    synchronized (INSTANCES) { INSTANCES.put(getBridge(), this); }
    pageListener = new WebViewListener() { @Override public void onPageStarted(WebView web) { invalidatePage(null, false, false); } };
  }
  @Override protected void handleOnStart() { getBridge().removeWebViewListener(pageListener); getBridge().addWebViewListener(pageListener); }
  @Override protected void handleOnResume() { resumed.set(true); showCleanupWarning(); for (Meta meta : saves.values()) completeSelection(meta); }
  @Override protected void handleOnPause() { resumed.set(false); invalidatePage(null, true); }
  @Override protected void handleOnStop() { closePreviews(); }
  @Override protected void handleOnDestroy() {
    invalidatePage(null, false, false); getBridge().removeWebViewListener(pageListener); synchronized (INSTANCES) { INSTANCES.remove(getBridge()); } io.shutdown();
  }
  private void invalidatePage(PluginCall call, boolean transfersOnly) {
    invalidatePage(call, transfersOnly, true);
  }
  private void invalidatePage(PluginCall call, boolean transfersOnly, boolean reply) {
    generation.incrementAndGet(); if (!transfersOnly) page.incrementAndGet(); closePreviews();
    Set<Meta> clearing = new java.util.HashSet<>(cleanupPending);
    for (Meta meta : saves.values()) if (meta.lifetime.invalidate(transfersOnly)) { if (!reply) meta.answered.set(true); cancel(meta, "invalidated"); clearing.add(meta); }
    // Reserve cleanup before a later activation or executor shutdown. No main-thread provider IO.
    try { io.execute(() -> {
      boolean cleared = true; for (Meta meta : clearing) if (!cleanup(meta)) cleared = false;
      if (!cleared) { if (call != null) call.reject("The save provider kept an incomplete file. Remove it from the selected destination.", "cleanup_failed"); }
      else clearTemporary(store, call);
    }); } catch (RejectedExecutionException error) { if (call != null) call.reject("Temporary file cleanup is unavailable", "cleanup_failed"); }
  }
  static void clearTemporary(AttachmentFileStore store, PluginCall call) {
    try { if (store == null) throw new IOException("Download staging unavailable"); store.clear(); if (call != null) call.resolve(); }
    catch (IOException error) { if (call != null) call.reject("Temporary file cleanup failed. Try again or reopen the app.", "cleanup_failed"); }
  }
  private void closePreview(String key) {
    Meta meta = previews.remove(key);
    if (meta != null) { meta.cancelled.set(true); for (AttachmentInputStream stream : meta.streams) stream.close(); meta.streams.clear(); }
    for (Map.Entry<String, Pending> entry : pending.entrySet()) if (entry.getValue().preview.equals(key)) { entry.getValue().answer.completeExceptionally(new IOException("Preview closed")); pending.remove(entry.getKey(), entry.getValue()); }
  }
  private void closePreviews() { for (String key : previews.keySet()) closePreview(key); for (Pending request : pending.values()) request.answer.completeExceptionally(new IOException("Preview closed")); pending.clear(); }
  @PluginMethod public void invalidate(PluginCall call) { invalidatePage(call, "transfers".equals(call.getString("scope"))); }
  private String id(PluginCall call, String key) throws IOException { String value = call.getString(key); if (value == null || !value.matches("[0-9a-f-]{36}")) throw new IOException("Invalid file handle"); return value; }
  private static byte[] decode(String encoded) throws IOException {
    if (encoded == null || encoded.isEmpty() || encoded.length() > 349528 || encoded.length() % 4 != 0 || !encoded.matches("[A-Za-z0-9+/]*={0,2}")) throw new IOException("Invalid file chunk");
    byte[] bytes; try { bytes = Base64.decode(encoded, Base64.NO_WRAP); } catch (IllegalArgumentException error) { throw new IOException("Invalid file chunk"); }
    if (bytes.length > AttachmentInputStream.CHUNK || !Base64.encodeToString(bytes, Base64.NO_WRAP).equals(encoded)) throw new IOException("Invalid file chunk"); return bytes;
  }
  private boolean saveValid(Meta meta) { return resumed.get() && !meta.cancelled.get() && meta.lifetime != null && meta.lifetime.valid(page.get(), generation.get(), SystemClock.elapsedRealtime()); }
  private void completeSelection(Meta meta) {
    if (!resumed.get() || meta.cancelled.get() || meta.lifetime == null) return;
    if (!meta.lifetime.selected(page.get(), SystemClock.elapsedRealtime())) { if (meta.lifetime.phase() == AttachmentSaveLifetime.Phase.CLOSED) { cancel(meta, "timeout"); discard(meta); } return; }
    if (meta.answered.compareAndSet(false, true)) { JSObject out = new JSObject(); out.put("saveId", meta.lifetime.id); meta.pickerCall.resolve(out); }
  }
  private void cancel(Meta meta, String reason) {
    meta.cancelled.set(true); meta.lifetime.cancel(reason); saves.remove(meta.lifetime.id, meta); cleanupPending.add(meta);
    if (meta.answered.compareAndSet(false, true) && meta.pickerCall != null) meta.pickerCall.resolve(new JSObject());
  }
  private void showCleanupWarning() {
    Activity activity = getActivity();
    if (cleanupNotice.take(resumed.get() && activity != null && !activity.isFinishing() && !activity.isDestroyed()))
      Toast.makeText(activity, "File cleanup failed. Remove any incomplete file from the chosen destination.", Toast.LENGTH_LONG).show();
  }
  private boolean cleanup(Meta meta) {
    boolean cleared = true;
    if (meta.staged != null) try { if (store != null) store.cancel(meta.staged); else throw new IOException("Download staging unavailable"); meta.staged = null; } catch (IOException error) { cleared = false; }
    if (!meta.published && meta.uri != null) {
      try { if (!DocumentsContract.deleteDocument(getContext().getContentResolver(), meta.uri)) throw new IOException("Provider retained document"); meta.uri = null; }
      catch (Exception error) {
        cleared = false; if (meta.recovery == null) { meta.recovery = UUID.randomUUID().toString(); getContext().getSharedPreferences("attachment-recovery", 0).edit().putString(meta.recovery, meta.uri.toString()).apply(); }
        // A late picker/cancel may no longer have a JS consumer. Never leave retained provider data silent.
        if (meta.warned.compareAndSet(false, true)) { cleanupNotice.retain(); Activity activity = getActivity(); if (activity != null) activity.runOnUiThread(this::showCleanupWarning); }
      }
    }
    if (cleared) { cleanupPending.remove(meta); if (meta.recovery != null) getContext().getSharedPreferences("attachment-recovery", 0).edit().remove(meta.recovery).apply(); }
    else cleanupPending.add(meta);
    return cleared;
  }
  private void discard(Meta meta) {
    try { io.execute(() -> cleanup(meta)); } catch (RejectedExecutionException ignored) { cleanupPending.add(meta); }
  }
  @PluginMethod public void startSave(PluginCall call) {
    try {
      JSObject info = call.getObject("info"); if (info == null) throw new IOException("Invalid file metadata");
      String key = id(call, "requestId"); Meta meta = new Meta(info, generation.get());
      if (!meta.digest.matches("[a-f0-9]{64}")) throw new IOException("Invalid file digest");
      meta.lifetime = new AttachmentSaveLifetime(key, page.get(), SystemClock.elapsedRealtime()); meta.pickerCall = call;
      if (saves.size() >= 2 || cleanupPending.size() >= 2 || !picking.compareAndSet(null, meta)) throw new IOException("Another save picker is open or cleanup is pending");
      if (saves.putIfAbsent(key, meta) != null) { picking.compareAndSet(meta, null); throw new IOException("Another save picker is open"); }
      getActivity().runOnUiThread(() -> {
        try {
          if (!resumed.get() || getActivity().isFinishing() || getActivity().isDestroyed() || meta.cancelled.get() || !meta.lifetime.pending(page.get(), SystemClock.elapsedRealtime())) throw new IllegalStateException("Save closed");
          Intent intent = new Intent(Intent.ACTION_CREATE_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE).setType(meta.mime).putExtra(Intent.EXTRA_TITLE, meta.name).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
          startActivityForResult(call, intent, "saveChosen");
        } catch (RuntimeException error) { picking.compareAndSet(meta, null); cancel(meta, "unavailable"); discard(meta); }
      });
    } catch (IOException error) { call.reject("The system save picker is unavailable", "unavailable"); }
  }
  @ActivityCallback private void saveChosen(PluginCall call, ActivityResult result) {
    Uri uri = result.getData() == null ? null : result.getData().getData();
    String key = call == null ? null : call.getString("requestId"); Meta meta = picking.get();
    if (meta == null || !meta.lifetime.id.equals(key)) {
      if (uri != null) { try { JSObject info = new JSObject(); info.put("name", "file"); info.put("size", 0); Meta orphan = new Meta(info, generation.get()); orphan.uri = uri; discard(orphan); } catch (IOException ignored) { /* Fixed metadata is valid. */ } }
      return;
    }
    picking.compareAndSet(meta, null); meta.uri = uri;
    if (result.getResultCode() != Activity.RESULT_OK || uri == null || meta.cancelled.get() || !meta.lifetime.choose(page.get(), SystemClock.elapsedRealtime())) { cancel(meta, "cancelled"); discard(meta); return; }
    completeSelection(meta); // The result can precede onResume; never start a transfer behind the picker.
  }
  @PluginMethod public void activateSave(PluginCall call) {
    final long document = page.get(), transfer = generation.get();
    io.execute(() -> {
      Meta meta = null;
      try {
        meta = saves.get(id(call, "saveId"));
        if (meta == null || meta.cancelled.get() || !resumed.get() || page.get() != document || generation.get() != transfer || meta.uri == null || !meta.lifetime.activate(document, transfer, SystemClock.elapsedRealtime())) throw new IOException("Download closed");
        if (store == null) throw new IOException("Download staging unavailable"); meta.staged = store.start(meta.size, meta.digest);
        if (!saveValid(meta)) throw new IOException("Download closed"); call.resolve();
      } catch (IOException error) { if (meta != null) { cancel(meta, "unavailable"); cleanup(meta); } call.reject("The download could not be started", "unavailable"); }
    });
  }
  @PluginMethod public void appendSave(PluginCall call) { io.execute(() -> {
    try {
      Meta meta = saves.get(id(call, "saveId")); Object value = call.getData().opt("offset");
      if (meta == null || !saveValid(meta) || meta.lifetime.phase() != AttachmentSaveLifetime.Phase.ACTIVE || meta.staged == null || !(value instanceof Number) || ((Number) value).doubleValue() != Math.floor(((Number) value).doubleValue())) throw new IOException("Download closed or invalid offset");
      store.append(meta.staged, ((Number) value).longValue(), decode(call.getString("bytes"))); call.resolve();
    } catch (IOException error) { call.reject(error.getMessage(), "corrupt"); }
  }); }
  @PluginMethod public void cancelSave(PluginCall call) {
    try {
      String key = id(call, "saveId"); Meta found = saves.get(key);
      if (found == null) for (Meta retained : cleanupPending) if (retained.lifetime != null && retained.lifetime.id.equals(key)) { found = retained; break; }
      final Meta meta = found; if (meta != null) cancel(meta, "cancelled");
      io.execute(() -> { if (meta == null || cleanup(meta)) call.resolve(); else call.reject("The save provider kept an incomplete file. Remove it from the selected destination.", "cleanup_failed"); });
    } catch (IOException error) { call.reject("Invalid file handle", "bad_params"); }
  }
  @PluginMethod public void finishSave(PluginCall call) { io.execute(() -> {
    Meta meta = null;
    try {
      meta = saves.get(id(call, "saveId")); if (meta == null || !saveValid(meta) || meta.staged == null) throw new IOException("Download closed");
      File staged = store.verify(meta.staged); if (!meta.lifetime.copy(page.get(), generation.get(), SystemClock.elapsedRealtime())) throw new IOException("Download closed");
      final Meta copying = meta; byte[] buffer = new byte[64 * 1024];
      try (InputStream input = new FileInputStream(staged); OutputStream output = getContext().getContentResolver().openOutputStream(meta.uri, "w")) {
        if (output == null) throw new IOException("The destination could not be opened"); AttachmentFileStore.copy(input, output, buffer, () -> !saveValid(copying));
      }
      MessageDigest digest = AttachmentFileStore.sha256();
      try (InputStream saved = getContext().getContentResolver().openInputStream(meta.uri)) { if (saved == null) throw new IOException("Saved content could not be checked"); for (int count; (count = saved.read(buffer)) >= 0;) { if (!saveValid(meta)) throw new IOException("Download cancelled"); if (count > 0) digest.update(buffer, 0, count); } }
      if (!AttachmentFileStore.hex(digest.digest()).equals(meta.digest) || !saveValid(meta)) throw new IOException("Saved content check failed");
      meta.published = true; JSObject out = new JSObject(); out.put("status", "saved"); call.resolve(out);
    } catch (Exception error) {
      if (meta != null) cancel(meta, "unavailable"); boolean removed = meta == null || cleanup(meta);
      call.reject(removed ? "The file could not be saved" : "The provider kept an incomplete file. Remove it from the selected destination.", removed ? "unavailable" : "cleanup_failed");
    } finally { if (meta != null) { cancel(meta, "finished"); cleanup(meta); } }
  }); }
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
