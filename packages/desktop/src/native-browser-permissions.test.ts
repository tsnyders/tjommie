import { EventEmitter } from "node:events";
import type { Session, WebContents } from "electron";
import { expect, it, vi } from "vitest";
import { denyNativeBrowserBluetooth, denyNativeBrowserPermissions } from "./native-browser-permissions.ts";

it("explicitly cancels Bluetooth, HID, USB and serial selections without duplicate handlers", () => {
  const profile = Object.assign(new EventEmitter(), {
    setPermissionCheckHandler: vi.fn(), setPermissionRequestHandler: vi.fn(), setDevicePermissionHandler: vi.fn(),
    setDisplayMediaRequestHandler: vi.fn(), setBluetoothPairingHandler: vi.fn(),
  });
  denyNativeBrowserPermissions(profile as unknown as Session);
  denyNativeBrowserPermissions(profile as unknown as Session);
  const event = { preventDefault: vi.fn() }, hid = vi.fn(), usb = vi.fn(), serial = vi.fn(), bluetooth = vi.fn();
  profile.emit("select-hid-device", event, {}, hid);
  profile.emit("select-usb-device", event, {}, usb);
  profile.emit("select-serial-port", event, [], {}, serial);
  const contents = new EventEmitter();
  denyNativeBrowserBluetooth(contents as unknown as WebContents);
  contents.emit("select-bluetooth-device", event, [{ deviceId: "must-never-select" }], bluetooth);
  expect(hid.mock.calls).toEqual([[]]); expect(usb.mock.calls).toEqual([[]]);
  expect(serial.mock.calls).toEqual([[""]]); expect(bluetooth.mock.calls).toEqual([[""]]);
  expect(event.preventDefault).toHaveBeenCalledTimes(4);
});

it("keeps all permission handlers denying after close-time reconfiguration and rejects pairing", () => {
  const check = vi.fn<Session["setPermissionCheckHandler"]>();
  const request = vi.fn<Session["setPermissionRequestHandler"]>();
  const device = vi.fn<Session["setDevicePermissionHandler"]>();
  const display = vi.fn<Session["setDisplayMediaRequestHandler"]>();
  const pairing = vi.fn<Session["setBluetoothPairingHandler"]>();
  const profile = Object.assign(new EventEmitter(), { setPermissionCheckHandler: check, setPermissionRequestHandler: request,
    setDevicePermissionHandler: device, setDisplayMediaRequestHandler: display, setBluetoothPairingHandler: pairing });
  const notice = vi.fn();
  denyNativeBrowserPermissions(profile as unknown as Session, notice);
  denyNativeBrowserPermissions(profile as unknown as Session);
  // The exact callbacks installed at close remain denials, never Electron's permissive defaults.
  const allowed = vi.fn();
  request.mock.lastCall![0]!({} as WebContents, "geolocation", allowed, {} as Parameters<NonNullable<Parameters<Session["setPermissionRequestHandler"]>[0]>>[3]);
  expect(allowed).toHaveBeenCalledWith(false); expect(notice).not.toHaveBeenCalled();
  expect(check.mock.lastCall![0]).toBeTypeOf("function"); expect(device.mock.lastCall![0]).toBeTypeOf("function");
  const paired = vi.fn();
  pairing.mock.lastCall![0]!({} as Parameters<NonNullable<Parameters<Session["setBluetoothPairingHandler"]>[0]>>[0], paired);
  expect(paired).toHaveBeenCalledWith({ confirmed: false });
  const streams = vi.fn();
  display.mock.lastCall![0]!({} as Parameters<NonNullable<Parameters<Session["setDisplayMediaRequestHandler"]>[0]>>[0], streams);
  expect(streams).toHaveBeenCalledWith({});
});
