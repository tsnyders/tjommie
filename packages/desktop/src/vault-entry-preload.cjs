const { contextBridge, ipcRenderer } = require("electron");
contextBridge.exposeInMainWorld("privateLogin", Object.freeze({
  init(listener) {
    ipcRenderer.once("tjommie:private-login-init", (_event, input) => listener(input));
    return ipcRenderer.invoke("tjommie:private-login-ready");
  },
  submit(value) { return ipcRenderer.invoke("tjommie:private-login-submit", value); },
}));
