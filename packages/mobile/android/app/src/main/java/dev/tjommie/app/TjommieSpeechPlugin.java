package dev.tjommie.app;

import android.Manifest;
import android.content.Intent;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.speech.RecognitionListener;
import android.speech.RecognitionSupport;
import android.speech.RecognitionSupportCallback;
import android.speech.RecognizerIntent;
import android.speech.SpeechRecognizer;
import android.webkit.WebView;
import com.getcapacitor.JSArray;
import com.getcapacitor.JSObject;
import com.getcapacitor.PermissionState;
import com.getcapacitor.Plugin;
import com.getcapacitor.PluginCall;
import com.getcapacitor.PluginMethod;
import com.getcapacitor.WebViewListener;
import com.getcapacitor.annotation.CapacitorPlugin;
import com.getcapacitor.annotation.Permission;
import com.getcapacitor.annotation.PermissionCallback;
import java.util.ArrayList;
import java.util.List;

@CapacitorPlugin(name = "TjommieSpeech", permissions = { @Permission(alias = "microphone", strings = { Manifest.permission.RECORD_AUDIO }) })
public class TjommieSpeechPlugin extends Plugin {
  private final SpeechSession sessions = new SpeechSession();
  private final Handler handler = new Handler(Looper.getMainLooper());
  private final List<PluginCall> statusCalls = new ArrayList<>();
  private List<String> installed = new ArrayList<>();
  private SpeechRecognizer recognizer;
  private SpeechRecognizer query;
  private PluginCall pendingStart;
  private boolean foreground = true;
  private boolean cleanupFailed;
  private WebViewListener pageListener;
  private Runnable deadline;

  @Override public void load() {
    pageListener = new WebViewListener() {
      @Override public void onPageStarted(WebView web) { cancelActive(); closeQuery(); installed.clear(); }
      @Override public void onReceivedError(WebView web) { cancelActive(); closeQuery(); }
    };
    getBridge().addWebViewListener(pageListener);
  }

  private boolean trusted() { return foreground && getActivity() != null && getBridge() != null && getBridge().getWebView() != null && SpeechSession.trustedOrigin(getBridge().getWebView().getUrl()); }
  private boolean onDevice() { if (Build.VERSION.SDK_INT < 33) return false; try { return SpeechRecognizer.isOnDeviceRecognitionAvailable(getContext()); } catch (Exception ignored) { return false; } }
  static boolean supportedLanguage(String language, List<String> installed) { return language != null && language.matches("en(?:-[A-Za-z0-9]{2,8})*") && installed.contains(language); }
  static String permissionProblem(PermissionState permission) { return SpeechSession.permissionProblem(permission == PermissionState.GRANTED, permission == PermissionState.DENIED); }
  static List<String> confirmedEnglishLanguages(List<String> languages) {
    List<String> verified = new ArrayList<>();
    if (languages != null) for (String tag : languages) if (tag != null && tag.matches("en(?:-[A-Za-z0-9]{2,8})*") && !verified.contains(tag) && verified.size() < 100) verified.add(tag);
    return verified;
  }

  private Intent intent(String language) {
    Intent intent = new Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH);
    intent.putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM);
    intent.putExtra(RecognizerIntent.EXTRA_LANGUAGE, language);
    intent.putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, false);
    intent.putExtra(RecognizerIntent.EXTRA_MAX_RESULTS, 1);
    return intent;
  }

  private JSObject problem(String code, String message) { JSObject value = new JSObject(); value.put("code", code); value.put("message", message); return value; }
  private JSObject statusValue() {
    JSObject value = new JSObject();
    boolean supported = trusted() && onDevice();
    PermissionState permission = getPermissionState("microphone");
    boolean granted = permission == PermissionState.GRANTED;
    String permissionCode = permissionProblem(permission);
    value.put("engine", "android-on-device"); value.put("capture", "native"); value.put("available", supported && granted && !installed.isEmpty() && !cleanupFailed);
    value.put("permission", granted ? "granted" : permission == PermissionState.DENIED ? "denied" : "prompt");
    JSArray languages = new JSArray();
    if (supported && granted) for (String tag : installed) { JSObject item = new JSObject(); item.put("tag", tag); item.put("label", java.util.Locale.forLanguageTag(tag).getDisplayName()); languages.put(item); }
    value.put("languages", languages);
    JSObject model = new JSObject(); model.put("state", "system-managed"); model.put("bytes", 0); model.put("received", 0); value.put("model", model);
    if (cleanupFailed) value.put("problem", problem("cleanup_failed", "The previous local speech session could not be released. Close and reopen the app before dictating again."));
    else if (!supported) value.put("problem", problem("on_device_unavailable", "Local dictation needs Android 13 or later and an explicit on-device speech service. You can keep typing."));
    else if (permissionCode != null) value.put("problem", problem(permissionCode, permission == PermissionState.DENIED ? "Microphone access is denied. Allow it in Android app settings before dictating again." : "Start dictation to request microphone access. Your draft will stay editable."));
    else if (installed.isEmpty()) value.put("problem", problem("language_unverified", "No supported English on-device language could be confirmed. Check Android speech language settings or refresh, then try again."));
    return value;
  }

  @PluginMethod public void status(PluginCall call) {
    getActivity().runOnUiThread(() -> {
      boolean granted = getPermissionState("microphone") == PermissionState.GRANTED;
      if (!SpeechSession.canQuerySupport(trusted(), onDevice(), granted, sessions.current() == null)) { if (!granted) installed.clear(); call.resolve(statusValue()); return; }
      if (statusCalls.size() >= 8) { call.reject("Speech status is busy. Try again.", "busy"); return; }
      statusCalls.add(call);
      if (query != null) return;
      try {
        query = SpeechRecognizer.createOnDeviceSpeechRecognizer(getContext());
        SpeechRecognizer expected = query;
        query.setRecognitionListener(emptyListener());
        deadline = () -> { if (query == expected) closeQuery(); };
        handler.postDelayed(deadline, 5000);
        query.checkRecognitionSupport(intent("en-US"), getContext().getMainExecutor(), new RecognitionSupportCallback() {
          @Override public void onSupportResult(RecognitionSupport support) {
            if (query != expected) return;
            if (!trusted() || getPermissionState("microphone") != PermissionState.GRANTED) { installed.clear(); closeQuery(); return; }
            installed = confirmedEnglishLanguages(support == null ? null : support.getInstalledOnDeviceLanguages());
            closeQuery();
          }
          @Override public void onError(int error) { if (query == expected) { installed.clear(); closeQuery(); } }
        });
      } catch (Exception ignored) { installed.clear(); closeQuery(); }
    });
  }

  private void closeQuery() {
    if (query != null) { try { query.destroy(); } catch (Exception ignored) {} query = null; }
    for (PluginCall call : statusCalls) call.resolve(statusValue());
    statusCalls.clear();
  }

  @PluginMethod public void start(PluginCall call) {
    getActivity().runOnUiThread(() -> {
      if (!trusted() || !onDevice()) { call.reject("The explicit local speech service is unavailable.", "unavailable"); return; }
      if (cleanupFailed && !closeRecognizer()) { call.reject("The previous speech session could not be released. Restart the app.", "cleanup_failed"); return; }
      SpeechSession.Ticket ticket;
      try { ticket = sessions.begin(call.getString("sessionId"), call.getString("language")); }
      catch (Exception ignored) { call.reject("Another dictation is active or its request is invalid.", "invalid_session"); return; }
      pendingStart = call;
      if (getPermissionState("microphone") != PermissionState.GRANTED) requestPermissionForAlias("microphone", call, "microphoneResult");
      else beginRecognition(ticket);
    });
  }

  @PermissionCallback private void microphoneResult(PluginCall call) {
    getActivity().runOnUiThread(() -> {
      SpeechSession.Ticket ticket = sessions.current();
      if (pendingStart != call || ticket == null || !ticket.id.equals(call.getString("sessionId"))) return;
      if (!trusted() || getPermissionState("microphone") != PermissionState.GRANTED) { fail(ticket, "permission_denied", "Microphone permission was not granted. Your draft is unchanged."); return; }
      beginRecognition(ticket);
    });
  }

  private void beginRecognition(SpeechSession.Ticket ticket) {
    if (!sessions.owns(ticket) || !trusted()) return;
    if (getPermissionState("microphone") != PermissionState.GRANTED) { fail(ticket, "permission_denied", "Microphone permission was not granted. Your draft is unchanged."); return; }
    if (Build.VERSION.SDK_INT < 33 || !onDevice()) { fail(ticket, "unavailable", "The explicit on-device speech service is unavailable."); return; }
    closeQuery();
    try {
      recognizer = SpeechRecognizer.createOnDeviceSpeechRecognizer(getContext());
      recognizer.setRecognitionListener(listener(ticket));
      deadline = () -> fail(ticket, "support_timeout", "Could not check the installed local speech language. Try again.");
      handler.postDelayed(deadline, 5000);
      recognizer.checkRecognitionSupport(intent(ticket.language), getContext().getMainExecutor(), new RecognitionSupportCallback() {
        @Override public void onSupportResult(RecognitionSupport support) {
          if (!sessions.owns(ticket) || !trusted()) return;
          if (getPermissionState("microphone") != PermissionState.GRANTED) { fail(ticket, "permission_denied", "Microphone permission was not granted. Your draft is unchanged."); return; }
          installed = confirmedEnglishLanguages(support == null ? null : support.getInstalledOnDeviceLanguages());
          if (!supportedLanguage(ticket.language, installed)) { fail(ticket, "language_missing", "The selected local English language is not installed. Manage Android speech language data, then try again."); return; }
          handler.removeCallbacks(deadline);
          try {
            if (!sessions.listening(ticket)) return;
            recognizer.startListening(intent(ticket.language));
            sessionEvent(ticket.id, "listening", null);
            PluginCall call = pendingStart; pendingStart = null; if (call != null) call.resolve();
            deadline = () -> stopTicket(ticket);
            handler.postDelayed(deadline, 60000);
          } catch (Exception ignored) { fail(ticket, "start_failed", "Could not start the on-device recognizer. Check microphone permission and try again."); }
        }
        @Override public void onError(int error) { fail(ticket, "support_unavailable", "The installed local language could not be verified. No online recognizer was used."); }
      });
    } catch (Exception ignored) { fail(ticket, "start_failed", "The local speech service could not start. You can keep typing."); }
  }

  private RecognitionListener emptyListener() { return listener(null); }
  private RecognitionListener listener(SpeechSession.Ticket ticket) {
    return new RecognitionListener() {
      @Override public void onReadyForSpeech(Bundle params) {}
      @Override public void onBeginningOfSpeech() {}
      @Override public void onRmsChanged(float rms) {}
      @Override public void onBufferReceived(byte[] buffer) {}
      @Override public void onPartialResults(Bundle results) {}
      @Override public void onEvent(int type, Bundle params) {}
      @Override public void onEndOfSpeech() { transcribing(ticket); }
      @Override public void onError(int error) { fail(ticket, "recognition_failed", "No usable local transcript was produced. Check permission, language data and background noise, then try again."); }
      @Override public void onResults(Bundle results) {
        if (!sessions.owns(ticket) || !trusted() || "permission".equals(ticket.phase)) return;
        transcribing(ticket);
        ArrayList<String> values = results == null ? null : results.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION);
        String text = values == null || values.isEmpty() || values.get(0) == null ? "" : values.get(0).trim();
        if (text.isEmpty() || text.length() > 8192) { fail(ticket, "no_speech", "No usable speech was detected. Try again."); return; }
        if (!sessions.finish(ticket)) return;
        if (!closeRecognizer()) { sessionEvent(ticket.id, "error", problem("cleanup_failed", "Local speech could not be released. Restart the app before dictating again.")); return; }
        JSObject event = new JSObject(); event.put("kind", "final"); event.put("sessionId", ticket.id); event.put("text", text); notifyListeners("speechEvent", event);
      }
    };
  }

  private void transcribing(SpeechSession.Ticket ticket) {
    if (!sessions.transcribing(ticket)) return;
    if (deadline != null) handler.removeCallbacks(deadline);
    sessionEvent(ticket.id, "transcribing", null);
    deadline = () -> fail(ticket, "transcription_timeout", "Local transcription took too long. Try a shorter recording.");
    handler.postDelayed(deadline, 120000);
  }
  private void stopTicket(SpeechSession.Ticket ticket) {
    if (!sessions.owns(ticket) || !"listening".equals(ticket.phase)) return;
    transcribing(ticket);
    try { recognizer.stopListening(); } catch (Exception ignored) { fail(ticket, "stop_failed", "Could not finish local dictation. Try again."); }
  }
  private void sessionEvent(String id, String phase, JSObject problem) { JSObject event = new JSObject(); event.put("kind", "session"); event.put("sessionId", id); event.put("phase", phase); if (problem != null) event.put("problem", problem); notifyListeners("speechEvent", event); }
  private boolean closeRecognizer() {
    if (deadline != null) handler.removeCallbacks(deadline);
    boolean released = true;
    if (recognizer != null) {
      SpeechRecognizer old = recognizer;
      try { old.cancel(); } catch (Exception ignored) { released = false; }
      try { old.destroy(); recognizer = null; } catch (Exception ignored) { released = false; }
    }
    cleanupFailed = !released;
    return released;
  }
  private void fail(SpeechSession.Ticket ticket, String code, String message) {
    if (!sessions.finish(ticket)) return;
    PluginCall call = pendingStart; pendingStart = null;
    boolean released = closeRecognizer();
    String safeMessage = released ? message : "Local speech could not be released. Restart the app before dictating again.";
    if (call != null) call.reject(safeMessage, released ? code : "cleanup_failed");
    sessionEvent(ticket.id, "error", problem(released ? code : "cleanup_failed", safeMessage));
  }
  private void cancelActive() {
    SpeechSession.Ticket ticket = sessions.current();
    if (ticket == null || sessions.cancel(ticket.id) == null) return;
    PluginCall call = pendingStart; pendingStart = null;
    boolean released = closeRecognizer();
    if (call != null) call.reject("Dictation cancelled.", "cancelled");
    sessionEvent(ticket.id, released ? "cancelled" : "error", released ? null : problem("cleanup_failed", "Local speech could not be released. Restart the app before dictating again."));
  }

  @PluginMethod public void stop(PluginCall call) { getActivity().runOnUiThread(() -> { SpeechSession.Ticket ticket = sessions.current(); if (ticket == null || !ticket.id.equals(call.getString("sessionId"))) { call.reject("This speech session is no longer active.", "stale"); return; } stopTicket(ticket); call.resolve(); }); }
  @PluginMethod public void cancel(PluginCall call) { getActivity().runOnUiThread(() -> { SpeechSession.Ticket ticket = sessions.current(); if (ticket != null && ticket.id.equals(call.getString("sessionId"))) cancelActive(); if (cleanupFailed) call.reject("The local recognizer could not be released. Restart the app.", "cleanup_failed"); else call.resolve(); }); }
  @Override protected void handleOnPause() { foreground = false; cancelActive(); closeRecognizer(); closeQuery(); }
  @Override protected void handleOnResume() { foreground = true; }
  @Override protected void handleOnDestroy() { foreground = false; cancelActive(); closeRecognizer(); closeQuery(); handler.removeCallbacksAndMessages(null); if (pageListener != null) getBridge().removeWebViewListener(pageListener); }
}
