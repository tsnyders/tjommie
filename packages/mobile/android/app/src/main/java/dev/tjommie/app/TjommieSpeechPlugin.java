package dev.tjommie.app;

import android.Manifest;
import android.content.Intent;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.speech.RecognitionListener;
import android.speech.ModelDownloadListener;
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
  private final SpeechSession.Preparation preparation = new SpeechSession.Preparation();
  private final Handler handler = new Handler(Looper.getMainLooper());
  private final List<PluginCall> statusCalls = new ArrayList<>();
  private List<String> installed = new ArrayList<>();
  private SpeechRecognizer recognizer;
  private SpeechRecognizer query;
  private SpeechRecognizer modelRequest;
  private PluginCall pendingStart;
  private PluginCall pendingModel;
  private SpeechSession.Preparation.Confirmation capturePreparation;
  private boolean foreground = true;
  private boolean cleanupFailed;
  private boolean modelCleanupFailed;
  private WebViewListener pageListener;
  private Runnable deadline;
  private Runnable queryDeadline;
  private Runnable modelDeadline;
  private String supportProblem = "language_unverified";
  private String modelProblem;

  @Override public void load() {
    pageListener = new WebViewListener() {
      @Override public void onPageStarted(WebView web) { invalidatePage(); }
      @Override public void onReceivedError(WebView web) { invalidatePage(); }
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
  private JSObject diagnostic(String code) {
    if (code == null) code = "language_unverified";
    String message;
    switch (code) {
      case "support_uncheckable": message = "This Android on-device service does not allow support queries (14). Request English (United States) preparation; Start still needs a fresh SDK confirmation before recording."; break;
      case "english_not_installed": message = "The support query succeeded, but reported no installed English on-device language. You can request English (United States) preparation."; break;
      case "language_unsupported": message = "The Android on-device service reports this language is unsupported (12). No online recognizer was used."; break;
      case "language_unavailable": message = "The on-device language is supported but currently unavailable (13). Request its system download or check Android speech settings."; break;
      case "support_timeout": message = "The Android on-device support query timed out. No installed language was confirmed. Refresh or request English (United States) preparation."; break;
      case "support_exception": message = "The Android on-device support query could not be started. Refresh or check Android speech settings."; break;
      case "support_failed": message = "The Android on-device support query failed. No installed language was confirmed. You can keep typing."; break;
      case "download_pending": message = "Android is preparing English (United States). No recording has started. Leaving these controls detaches the observer; the system download may continue."; break;
      case "download_scheduled": message = "Android scheduled the English (United States) download. It has not confirmed completion and will send no more updates for this request. Request preparation again later."; break;
      case "download_unobservable": message = "This Android service does not support model-download callbacks (15). Tjommie cannot confirm completion. Check Android speech settings, then refresh."; break;
      case "download_timeout": message = "Android did not confirm the English (United States) download within two minutes. Observation ended; the system download may continue."; break;
      case "download_api_unavailable": message = "Observed on-device language downloads need Android 14 or later. Manage offline languages in Android speech settings, then refresh."; break;
      case "download_failed": message = "Android could not confirm the English (United States) download. Check system speech settings and network, then request preparation again."; break;
      default: message = "No English on-device language has been confirmed. Refresh or request English (United States) preparation. No online recognizer is substituted.";
    }
    return problem(code, message);
  }
  private void statusEvent() { JSObject event = new JSObject(); event.put("kind", "status"); event.put("status", statusValue()); notifyListeners("speechEvent", event); }
  private JSObject statusValue() {
    JSObject value = new JSObject();
    boolean supported = trusted() && onDevice();
    PermissionState permission = getPermissionState("microphone");
    boolean granted = permission == PermissionState.GRANTED;
    String permissionCode = permissionProblem(permission);
    boolean prepared = supported && granted && preparation.confirmed() != null;
    value.put("engine", "android-on-device"); value.put("capture", "native"); value.put("available", supported && granted && (!installed.isEmpty() || prepared) && !cleanupFailed && !modelCleanupFailed);
    value.put("permission", granted ? "granted" : permission == PermissionState.DENIED ? "denied" : "prompt");
    JSArray languages = new JSArray();
    if (supported && granted) for (String tag : installed) { JSObject item = new JSObject(); item.put("tag", tag); item.put("label", java.util.Locale.forLanguageTag(tag).getDisplayName()); languages.put(item); }
    value.put("languages", languages);
    JSObject model = new JSObject(); model.put("state", "system-managed"); model.put("bytes", 0); model.put("received", 0);
    if (prepared) { JSObject confirmation = new JSObject(); confirmation.put("language", "en-US"); confirmation.put("source", "android-download-success"); model.put("confirmation", confirmation); }
    value.put("model", model);
    if (cleanupFailed || modelCleanupFailed) value.put("problem", problem("cleanup_failed", "The previous local speech session could not be released. Close and reopen the app before dictating again."));
    else if (!supported) value.put("problem", problem("on_device_unavailable", "Local dictation needs Android 13 or later and an explicit on-device speech service. You can keep typing."));
    else if (permissionCode != null) value.put("problem", problem(permissionCode, permission == PermissionState.DENIED ? "Microphone access is denied. Allow it in Android app settings before dictating again." : "Start dictation to request microphone access. Your draft will stay editable."));
    else if (modelProblem != null) value.put("problem", diagnostic(modelProblem));
    else if (installed.isEmpty()) value.put("problem", diagnostic(supportProblem));
    return value;
  }

  @PluginMethod public void status(PluginCall call) {
    getActivity().runOnUiThread(() -> {
      boolean granted = getPermissionState("microphone") == PermissionState.GRANTED;
      if (!granted) { installed.clear(); clearPreparation(); }
      if (!SpeechSession.canQuerySupport(trusted(), onDevice(), granted, sessions.current() == null && modelRequest == null)) { call.resolve(statusValue()); return; }
      if (statusCalls.size() >= 8) { call.reject("Speech status is busy. Try again.", "busy"); return; }
      statusCalls.add(call);
      if (query != null) return;
      try {
        query = SpeechRecognizer.createOnDeviceSpeechRecognizer(getContext());
        SpeechRecognizer expected = query;
        query.setRecognitionListener(emptyListener());
        queryDeadline = () -> { if (query == expected) { installed.clear(); supportProblem = "support_timeout"; closeQuery(); } };
        handler.postDelayed(queryDeadline, 5000);
        query.checkRecognitionSupport(intent("en-US"), getContext().getMainExecutor(), new RecognitionSupportCallback() {
          @Override public void onSupportResult(RecognitionSupport support) {
            if (query != expected) return;
            if (!trusted() || getPermissionState("microphone") != PermissionState.GRANTED) { installed.clear(); clearPreparation(); closeQuery(); return; }
            installed = confirmedEnglishLanguages(support == null ? null : support.getInstalledOnDeviceLanguages());
            supportProblem = installed.isEmpty() ? "english_not_installed" : null;
            if (installed.contains("en-US")) modelProblem = null;
            closeQuery();
          }
          @Override public void onError(int error) { if (query == expected) { installed.clear(); supportProblem = SpeechSession.supportProblem(error); closeQuery(); } }
        });
      } catch (Exception ignored) { installed.clear(); supportProblem = "support_exception"; closeQuery(); }
    });
  }

  private void closeQuery() {
    if (queryDeadline != null) { handler.removeCallbacks(queryDeadline); queryDeadline = null; }
    if (query != null) { try { query.destroy(); } catch (Exception ignored) {} query = null; }
    for (PluginCall call : statusCalls) call.resolve(statusValue());
    statusCalls.clear();
  }

  @PluginMethod public void downloadModel(PluginCall call) {
    getActivity().runOnUiThread(() -> {
      if (!trusted() || !onDevice()) { call.reject("The explicit on-device speech service is unavailable.", "unavailable"); return; }
      if (getPermissionState("microphone") != PermissionState.GRANTED) { clearPreparation(); call.reject("Start dictation in a conversation to grant microphone access first.", "permission_required"); return; }
      if (Build.VERSION.SDK_INT < 34) { modelProblem = "download_api_unavailable"; statusEvent(); call.reject(diagnostic(modelProblem).getString("message"), modelProblem); return; }
      if (sessions.current() != null || pendingModel != null || cleanupFailed || modelCleanupFailed) { call.reject("Local speech is busy or needs to be reopened before preparation.", "busy"); return; }
      SpeechSession.Preparation.Request request;
      try { request = preparation.begin(call.getString("requestId")); }
      catch (Exception ignored) { call.reject("Another model request is active or the request is invalid.", "invalid_request"); return; }
      pendingModel = call; modelProblem = "download_pending"; closeQuery();
      try {
        modelRequest = SpeechRecognizer.createOnDeviceSpeechRecognizer(getContext());
        SpeechRecognizer expected = modelRequest;
        modelRequest.setRecognitionListener(emptyListener());
        modelDeadline = () -> finishModel(request, expected, false, "download_timeout");
        handler.postDelayed(modelDeadline, 120000);
        statusEvent();
        modelRequest.triggerModelDownload(intent("en-US"), getContext().getMainExecutor(), new ModelDownloadListener() {
          @Override public void onProgress(int percent) { /* Progress is never evidence that audio may start. */ }
          @Override public void onSuccess() { finishModel(request, expected, true, null); }
          @Override public void onScheduled() { finishModel(request, expected, false, "download_scheduled"); }
          @Override public void onError(int error) { finishModel(request, expected, false, SpeechSession.downloadProblem(error)); }
        });
      } catch (Exception ignored) { finishModel(request, modelRequest, false, "download_failed"); }
    });
  }

  private void finishModel(SpeechSession.Preparation.Request request, SpeechRecognizer expected, boolean success, String code) {
    if (modelRequest != expected || !preparation.owns(request)) return;
    if (!trusted() || getPermissionState("microphone") != PermissionState.GRANTED) { clearPreparation(); return; }
    if (!preparation.finish(request, success)) return;
    modelProblem = code;
    PluginCall call = pendingModel; pendingModel = null;
    closeModelObserver(); statusEvent();
    if (call != null) { if (success || "download_scheduled".equals(code)) call.resolve(); else call.reject(diagnostic(code).getString("message"), code); }
  }
  private void closeModelObserver() {
    if (modelDeadline != null) { handler.removeCallbacks(modelDeadline); modelDeadline = null; }
    SpeechRecognizer old = modelRequest; modelRequest = null;
    if (old != null) try { old.destroy(); } catch (Exception ignored) { modelCleanupFailed = true; }
  }
  private void clearPreparation() {
    preparation.clear(); closeModelObserver(); modelProblem = null;
    PluginCall call = pendingModel; pendingModel = null;
    if (call != null) call.reject("Speech preparation observation ended. The system download may continue.", "detached");
  }
  @PluginMethod public void releaseModelRequest(PluginCall call) {
    getActivity().runOnUiThread(() -> {
      String id = call.getString("requestId");
      boolean waitingOnOwner = preparation.awaitedBy(sessions, sessions.current(), capturePreparation, id);
      if (preparation.release(id)) { clearPreparation(); if (waitingOnOwner) cancelActive(); statusEvent(); }
      call.resolve();
    });
  }
  private void invalidatePage() { cancelActive(); clearPreparation(); installed.clear(); supportProblem = "language_unverified"; closeQuery(); }

  @PluginMethod public void start(PluginCall call) {
    getActivity().runOnUiThread(() -> {
      if (!trusted() || !onDevice()) { call.reject("The explicit local speech service is unavailable.", "unavailable"); return; }
      if (pendingModel != null) { call.reject("Wait for Android speech preparation before starting dictation.", "busy"); return; }
      if (modelCleanupFailed) { call.reject("The previous speech observer could not be released. Close and reopen the app.", "cleanup_failed"); return; }
      if (cleanupFailed && !closeRecognizer()) { call.reject("The previous speech session could not be released. Restart the app.", "cleanup_failed"); return; }
      SpeechSession.Ticket ticket;
      try { ticket = sessions.begin(call.getString("sessionId"), call.getString("language")); }
      catch (Exception ignored) { call.reject("Another dictation is active or its request is invalid.", "invalid_session"); return; }
      pendingStart = call;
      if (getPermissionState("microphone") != PermissionState.GRANTED) { clearPreparation(); installed.clear(); requestPermissionForAlias("microphone", call, "microphoneResult"); }
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
      SpeechRecognizer expected = recognizer;
      Intent recognitionIntent = intent(ticket.language);
      recognizer.setRecognitionListener(listener(ticket));
      SpeechSession.Preparation.Confirmation confirmation = preparation.confirmed();
      if (Build.VERSION.SDK_INT >= 34 && "en-US".equals(ticket.language) && preparation.owns(confirmation)) {
        capturePreparation = confirmation;
        // A past download success only opts in to another SDK check. This recognizer must confirm the exact intent afresh.
        deadline = () -> failPreparedCapture(ticket, "download_timeout");
        handler.postDelayed(deadline, 120000);
        recognizer.triggerModelDownload(recognitionIntent, getContext().getMainExecutor(), new ModelDownloadListener() {
          @Override public void onProgress(int percent) {}
          @Override public void onSuccess() {
            if (!currentCapture(ticket, expected)) return;
            if (!preparation.owns(confirmation)) { fail(ticket, "stale", "Speech preparation is no longer active. Request preparation again."); return; }
            listen(ticket, expected, recognitionIntent);
          }
          @Override public void onScheduled() { if (currentCapture(ticket, expected)) failPreparedCapture(ticket, "download_scheduled"); }
          @Override public void onError(int error) { if (currentCapture(ticket, expected)) failPreparedCapture(ticket, SpeechSession.downloadProblem(error)); }
        });
        return;
      }
      deadline = () -> { installed.clear(); supportProblem = "support_timeout"; fail(ticket, "support_timeout", diagnostic("support_timeout").getString("message")); };
      handler.postDelayed(deadline, 5000);
      recognizer.checkRecognitionSupport(recognitionIntent, getContext().getMainExecutor(), new RecognitionSupportCallback() {
        @Override public void onSupportResult(RecognitionSupport support) {
          if (!currentCapture(ticket, expected)) return;
          installed = confirmedEnglishLanguages(support == null ? null : support.getInstalledOnDeviceLanguages());
          supportProblem = installed.isEmpty() ? "english_not_installed" : null;
          if (!supportedLanguage(ticket.language, installed)) { fail(ticket, "language_missing", "The selected local English language is not installed. Manage Android speech language data, then try again."); return; }
          listen(ticket, expected, recognitionIntent);
        }
        @Override public void onError(int error) {
          if (!currentCapture(ticket, expected)) return;
          installed.clear(); supportProblem = SpeechSession.supportProblem(error);
          fail(ticket, supportProblem, diagnostic(supportProblem).getString("message"));
        }
      });
    } catch (Exception ignored) { if (capturePreparation != null) failPreparedCapture(ticket, "download_failed"); else fail(ticket, "start_failed", "The local speech service could not start. You can keep typing."); }
  }

  private boolean currentCapture(SpeechSession.Ticket ticket, SpeechRecognizer expected) {
    if (!sessions.awaitingConfirmation(ticket) || recognizer != expected) return false;
    if (!trusted() || getPermissionState("microphone") != PermissionState.GRANTED) {
      clearPreparation(); installed.clear(); fail(ticket, "permission_denied", "Speech permission or foreground access ended. Your draft is unchanged."); return false;
    }
    return true;
  }
  private void failPreparedCapture(SpeechSession.Ticket ticket, String code) {
    if (!sessions.owns(ticket)) return;
    preparation.clear(); modelProblem = code;
    fail(ticket, code, diagnostic(code).getString("message")); statusEvent();
  }
  private void listen(SpeechSession.Ticket ticket, SpeechRecognizer expected, Intent recognitionIntent) {
    if (!currentCapture(ticket, expected)) return;
    if (deadline != null) handler.removeCallbacks(deadline);
    try {
      if (!sessions.listening(ticket)) return;
      capturePreparation = null;
      expected.startListening(recognitionIntent);
      sessionEvent(ticket.id, "listening", null);
      PluginCall call = pendingStart; pendingStart = null; if (call != null) call.resolve();
      deadline = () -> stopTicket(ticket);
      handler.postDelayed(deadline, 60000);
    } catch (Exception ignored) { fail(ticket, "start_failed", "Could not start the on-device recognizer. Check microphone permission and try again."); }
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
        if (getPermissionState("microphone") != PermissionState.GRANTED) { clearPreparation(); installed.clear(); fail(ticket, "permission_denied", "Microphone permission ended. Your draft is unchanged."); return; }
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
    capturePreparation = null;
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
  @Override protected void handleOnPause() { foreground = false; invalidatePage(); closeRecognizer(); }
  @Override protected void handleOnResume() { foreground = true; }
  @Override protected void handleOnDestroy() { foreground = false; invalidatePage(); closeRecognizer(); handler.removeCallbacksAndMessages(null); if (pageListener != null) getBridge().removeWebViewListener(pageListener); }
}
