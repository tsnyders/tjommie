import type { Session, WebContents } from "electron";

const configured = new WeakSet<Session>();

/** Keep denials installed for the Session lifetime, including after its last tab closes. */
export function denyNativeBrowserPermissions(profile: Session, notice?: (permission: string) => void): void {
  profile.setPermissionCheckHandler(() => false);
  profile.setPermissionRequestHandler((_contents, permission, callback) => { callback(false); notice?.(permission); });
  profile.setDevicePermissionHandler(() => false);
  profile.setDisplayMediaRequestHandler((_request, callback) => callback({}));
  profile.setBluetoothPairingHandler((_details, callback) => callback({ confirmed: false }));
  if (configured.has(profile)) return;
  configured.add(profile);
  profile.on("select-hid-device", (event, _details, callback) => { event.preventDefault(); callback(); });
  profile.on("select-serial-port", (event, _ports, _contents, callback) => { event.preventDefault(); callback(""); });
  profile.on("select-usb-device", (event, _details, callback) => { event.preventDefault(); callback(); });
}

export function denyNativeBrowserBluetooth(contents: WebContents): void {
  contents.on("select-bluetooth-device", (event, _devices, callback) => { event.preventDefault(); callback(""); });
}
