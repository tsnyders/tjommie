// Runs in the sandboxed renderer before the UI loads. The UI reaches Electron only through window.tjommie.
// A sandboxed preload can't be an ES module or TypeScript, so this one file is plain CommonJS.
const { contextBridge, ipcRenderer, webUtils } = require("electron");

function listen(channel, listener) {
  const handler = (_event, value) => listener(value);
  ipcRenderer.on(channel, handler);
  return () => ipcRenderer.removeListener(channel, handler);
}

contextBridge.exposeInMainWorld("tjommie", {
  connection: () => ipcRenderer.invoke("daemon:connection"),
  restartDaemon: () => ipcRenderer.invoke("daemon:restart"),
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
