const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const { mkdtempSync, rmSync } = require("node:fs");
const { tmpdir } = require("node:os");
const { basename, dirname, join, resolve } = require("node:path");
const electron = require("electron");

if (typeof electron === "string") {
  const profile = mkdtempSync(join(tmpdir(), "tjommie-p0-voice-"));
  const env = { ...process.env };
  delete env.ELECTRON_RUN_AS_NODE;
  try {
    const output = execFileSync(electron, [__filename, "--probe-profile", profile], {
      env, windowsHide: true, timeout: 30000, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"],
    });
    process.stdout.write(output);
  } finally {
    assert.equal(dirname(resolve(profile)), resolve(tmpdir()));
    assert(basename(profile).startsWith("tjommie-p0-voice-"));
    rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
} else {
  const { app, BrowserWindow, protocol, session } = electron;
  const profile = process.argv[process.argv.indexOf("--probe-profile") + 1];
  assert(profile && basename(profile).startsWith("tjommie-p0-voice-"));
  assert.equal(dirname(resolve(profile)), resolve(tmpdir()));
  app.setName("Tjommie P0 Voice Probe");
  app.setPath("userData", profile);
  app.setPath("sessionData", join(profile, "session"));
  app.setAppLogsPath(join(profile, "logs"));
  protocol.registerSchemesAsPrivileged([{ scheme: "app", privileges: { standard: true, secure: true, supportFetchAPI: true } }]);

  async function run() {
    await app.whenReady();
    session.defaultSession.setPermissionRequestHandler((_contents, _permission, callback) => callback(false));
    session.defaultSession.setPermissionCheckHandler(() => false);
    protocol.handle("app", (request) => {
      const isWorklet = new URL(request.url).pathname === "/worklet.js";
      const body = isWorklet
        ? "class SyntheticSilence extends AudioWorkletProcessor { process(_inputs, outputs) { for (const output of outputs) for (const channel of output) channel.fill(0); return true; } } registerProcessor('synthetic-silence', SyntheticSilence);"
        : "<!doctype html><title>Synthetic voice capability probe</title>";
      return new Response(body, { headers: { "Content-Type": isWorklet ? "text/javascript" : "text/html", "Content-Security-Policy": "default-src 'self'; script-src 'self'; connect-src 'none'" } });
    });
    const window = new BrowserWindow({ show: false, webPreferences: { nodeIntegration: false, contextIsolation: true, sandbox: true } });
    await window.loadURL("app://tjommie/");
    // No getUserMedia invocation, permission request, input device, speaker or real recording is used.
    const observed = await window.webContents.executeJavaScript(`(async () => {
      const context = new OfflineAudioContext(1, 16000, 16000);
      await context.audioWorklet.addModule('app://tjommie/worklet.js');
      new AudioWorkletNode(context, 'synthetic-silence').connect(context.destination);
      const buffer = await context.startRendering();
      return {
        secureContext: isSecureContext,
        microphoneApiPresent: typeof navigator.mediaDevices?.getUserMedia === 'function',
        audioWorkletPresent: typeof AudioWorkletNode === 'function',
        sampleRate: buffer.sampleRate,
        frames: buffer.length,
        syntheticSilence: buffer.getChannelData(0).every(value => value === 0),
      };
    })()`);
    assert.equal(observed.secureContext, true);
    assert.equal(observed.microphoneApiPresent, true);
    assert.equal(observed.audioWorkletPresent, true);
    assert.equal(observed.sampleRate, 16000);
    assert.equal(observed.frames, 16000);
    assert.equal(observed.syntheticSilence, true);
    console.log(JSON.stringify({ electron: process.versions.electron, chromium: process.versions.chrome, platform: process.platform, arch: process.arch, ...observed }));
    window.destroy();
    app.exit(0);
  }

  run().catch((error) => {
    console.error(error instanceof Error ? error.message : "Synthetic capture capability check failed.");
    app.exit(1);
  });
}
