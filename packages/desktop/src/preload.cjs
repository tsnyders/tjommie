// Runs in the sandboxed renderer before the UI loads. The UI reaches Electron only through window.tjommie.
// A sandboxed preload can't be an ES module or TypeScript, so this one file is plain CommonJS.
const { contextBridge, ipcRenderer, webUtils } = require("electron");

function listen(channel, listener) {
  const handler = (_event, ...values) => listener(...values);
  ipcRenderer.on(channel, handler);
  return () => ipcRenderer.removeListener(channel, handler);
}

contextBridge.exposeInMainWorld("tjommie", {
  vault: { command: (action) => ipcRenderer.invoke("vault:command", action) },
  attachments: {
    startSave: (info) => ipcRenderer.invoke("attachments:start-save", info),
    appendSave: (id, offset, bytes) => ipcRenderer.invoke("attachments:append-save", id, offset, bytes),
    finishSave: (id) => ipcRenderer.invoke("attachments:finish-save", id),
    cancelSave: (id) => ipcRenderer.invoke("attachments:cancel-save", id),
    preview: (info) => ipcRenderer.invoke("attachments:preview", info),
    onRead: (listener) => listen("attachments:read", listener),
    respond: (previewId, requestId, bytes) => ipcRenderer.invoke("attachments:respond", previewId, requestId, bytes),
    closePreview: (id) => ipcRenderer.invoke("attachments:close-preview", id),
    invalidate: () => ipcRenderer.invoke("attachments:invalidate"),
  },
  speech: {
    status: () => ipcRenderer.invoke("speech:status"),
    downloadModel: () => ipcRenderer.invoke("speech:download-model"),
    cancelModelDownload: () => ipcRenderer.invoke("speech:cancel-model-download"),
    removeModel: () => ipcRenderer.invoke("speech:remove-model"),
    start: (input) => ipcRenderer.invoke("speech:start", input),
    appendPcm: (input) => ipcRenderer.invoke("speech:append-pcm", input),
    stop: (input) => ipcRenderer.invoke("speech:stop", input),
    cancel: (input) => ipcRenderer.invoke("speech:cancel", input),
    onEvent: (listener) => listen("speech:event", listener),
  },
  diagnostics: {
    info: () => ipcRenderer.invoke("diagnostics:info"),
    saveReport: (report) => ipcRenderer.invoke("diagnostics:save", report),
  },
  connection: () => ipcRenderer.invoke("daemon:connection"),
  restartDaemon: () => ipcRenderer.invoke("daemon:restart"),
  remote: {
    open: (endpoint, timeoutMs) => ipcRenderer.invoke("remote:open", endpoint, timeoutMs),
    send: (id, frame) => ipcRenderer.invoke("remote:send", id, frame),
    close: (id) => ipcRenderer.send("remote:close", id),
    onFrame: (listener) => listen("remote:frame", listener),
    onClosed: (listener) => listen("remote:closed", listener),
    seal: (name, value) => ipcRenderer.invoke("remote:seal", name, value),
    read: (name) => ipcRenderer.invoke("remote:read", name),
    delete: (name) => ipcRenderer.invoke("remote:delete", name),
  },
  loginItem: {
    get: () => ipcRenderer.invoke("login-item:get"),
    set: (on) => ipcRenderer.invoke("login-item:set", on),
  },
  updates: {
    state: () => ipcRenderer.invoke("updates:state"),
    onState: (listener) => listen("updates:state", listener),
    check: () => ipcRenderer.invoke("updates:check"),
    restart: () => ipcRenderer.send("updates:restart"),
  },
  nativeBrowser: {
    command: (botId, command) => ipcRenderer.invoke("native-browser:command", botId, command),
    mount: (botId, mount) => ipcRenderer.invoke("native-browser:mount", botId, mount),
    onState: (listener) => listen("native-browser:state", listener),
    onFocusReturn: (listener) => listen("native-browser:focus-return", listener),
  },
  connectGoogle: (services) => ipcRenderer.invoke("google:connect", services),
  onConnection: (listener) => listen("daemon:connection", listener),
  onDaemonFailed: (listener) => listen("daemon:failed", listener),
  onNavigate: (listener) => listen("navigate", listener),
  pickFiles: () => ipcRenderer.invoke("files:pick"),
  pathForFile: (file) => webUtils.getPathForFile(file),
  showFile: (path) => ipcRenderer.send("file:show", path),
  openFolder: (path) => ipcRenderer.invoke("folder:open", path),
  pickFolder: () => ipcRenderer.invoke("folder:pick"),
  openNotes: (homeFolder) => ipcRenderer.invoke("notes:open", homeFolder),
  setTheme: (theme) => ipcRenderer.send("theme:set", theme),
  setDialogScrim: (open) => ipcRenderer.send("dialog:scrim", open),
  info: () => ipcRenderer.invoke("app:info"),
});
