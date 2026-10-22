import { describe, expect, it } from "vitest";
import { addressToUrl, containedRect, framePoint, framePointClamped, frameViewport, googleSignInRejected, imageReady, keyRoute, mouseButton, pictureKey, resizeWorthIt, viewportSize, wheelPixels } from "./browser-geometry";

const frame = { width: 800, height: 600 };

describe("containedRect", () => {
  it("letterboxes a frame that is wider or taller than its box, and fills one that matches", () => {
    expect(containedRect({ width: 400, height: 300 }, frame)).toEqual({ left: 0, top: 0, width: 400, height: 300 });
    expect(containedRect({ width: 800, height: 300 }, frame)).toEqual({ left: 200, top: 0, width: 400, height: 300 });
    expect(containedRect({ width: 400, height: 600 }, frame)).toEqual({ left: 0, top: 150, width: 400, height: 300 });
    expect(containedRect({ width: 0, height: 10 }, frame).width).toBe(0);
  });
});

describe("framePoint", () => {
  const box = { left: 100, top: 50, width: 800, height: 300 }; // drawn at 400 x 300, centered: x 300..700
  it("maps a pointer over the drawn picture to the frame's pixels", () => {
    expect(framePoint(box, frame, 500, 200)).toEqual({ x: 400, y: 300 });
    expect(framePoint(box, frame, 300, 50)).toEqual({ x: 0, y: 0 });
  });
  it("ignores the letterbox", () => {
    expect(framePoint(box, frame, 150, 200)).toBeNull();
    expect(framePoint(box, frame, 900, 200)).toBeNull();
  });
  it("never reaches the frame's own width or height", () => {
    expect(framePoint(box, frame, 700, 350)).toEqual({ x: 799, y: 599 });
  });
  it("maps onto the page's own size when the picture was scaled down", () => {
    const wide = { width: 1280, height: 720 }; // the encoded picture
    const page = { width: 1920, height: 1080 }; // the page it shows
    const bigBox = { left: 0, top: 0, width: 1280, height: 720 };
    expect(framePoint(bigBox, wide, 640, 360, page)).toEqual({ x: 960, y: 540 });
    expect(framePoint(bigBox, wide, 1280, 720, page)).toEqual({ x: 1919, y: 1079 });
  });
  it("has no point while there is no drawn picture", () => {
    expect(framePoint({ ...box, width: 0 }, frame, 5, 5)).toBeNull();
  });
});

describe("viewportSize and resizeWorthIt", () => {
  it("asks for what the viewport shows, within the daemon's limits", () => {
    expect(viewportSize(512.4, 400)).toEqual({ width: 512, height: 400 });
    expect(viewportSize(100, 50)).toEqual({ width: 240, height: 180 });
    expect(viewportSize(9000, 9000)).toEqual({ width: 2560, height: 1600 });
  });
  it("skips sizes within a step of the last one asked for", () => {
    const last = { width: 500, height: 400 };
    expect(resizeWorthIt(null, last)).toBe(true);
    expect(resizeWorthIt(last, { width: 508, height: 410 })).toBe(false);
    expect(resizeWorthIt(last, { width: 516, height: 400 })).toBe(true);
  });
});

describe("moved helpers", () => {
  it("names mouse buttons, and ignores the side buttons", () => {
    expect([0, 1, 2, 3].map(mouseButton)).toEqual(["left", "middle", "right", null]);
  });
  it("turns wheel lines and pages into pixels within the daemon's limit", () => {
    expect([wheelPixels(3, 1), wheelPixels(1, 2), wheelPixels(-99999, 0)]).toEqual([48, 400, -10_000]);
  });
  it("reads the address bar as an address, a host, this computer, or a search", () => {
    expect(addressToUrl("example.com/path")).toBe("https://example.com/path");
    expect(addressToUrl("localhost:3000/a")).toBe("http://localhost:3000/a");
    expect(addressToUrl("cheap flights")).toBe("https://duckduckgo.com/?q=cheap%20flights");
  });
  it("lets Shift+Tab leave the page, waits on paste, and sends other keys", () => {
    const plain = { shiftKey: false, ctrlKey: false, metaKey: false };
    expect(keyRoute({ ...plain, key: "Tab", shiftKey: true })).toBe("leave");
    expect(keyRoute({ ...plain, key: "V", ctrlKey: true })).toBe("paste");
    expect(keyRoute({ ...plain, key: "a" })).toBe("send");
  });
});

describe("googleSignInRejected", () => {
  it("is true only on Google's rejected sign-in page", () => {
    expect(googleSignInRejected("https://accounts.google.com/v3/signin/rejected?continue=x")).toBe(true);
    expect(googleSignInRejected("https://accounts.google.com/v3/signin/identifier")).toBe(false);
    expect(googleSignInRejected("https://evil.test/signin/rejected")).toBe(false);
    expect(googleSignInRejected("not a url")).toBe(false);
  });
});

describe("frameViewport", () => {
  it("is the page size a frame names, else the picture's own", () => {
    expect(frameViewport({ width: 1280, height: 720 })).toEqual({ width: 1280, height: 720 });
    expect(frameViewport({ width: 1280, height: 720, viewportWidth: 1920, viewportHeight: 1080 } as { width: number; height: number })).toEqual({ width: 1920, height: 1080 });
  });
});

describe("framePointClamped", () => {
  const box = { left: 100, top: 50, width: 800, height: 300 }; // drawn x 300..700
  it("puts a pointer that left the picture on its nearest edge, so a release still reaches the page", () => {
    expect(framePoint(box, frame, 150, 200)).toBeNull();
    expect(framePointClamped(box, frame, 150, 200)).toEqual({ x: 0, y: 300 });
    expect(framePointClamped(box, frame, 5000, -40)).toEqual({ x: 799, y: 0 });
  });
  it("agrees with framePoint inside the picture", () => {
    expect(framePointClamped(box, frame, 500, 200)).toEqual(framePoint(box, frame, 500, 200));
  });
});

describe("imageReady and pictureKey", () => {
  const src = "data:image/jpeg;base64,AAAA";
  const shown = { complete: true, naturalWidth: 800, naturalHeight: 600, currentSrc: src };
  it("is ready only for a decoded picture of this frame's size and source", () => {
    expect(imageReady(shown, frame, src)).toBe(true);
    expect(imageReady({ ...shown, complete: false }, frame, src)).toBe(false);
    expect(imageReady({ ...shown, naturalWidth: 0 }, frame, src)).toBe(false);
    expect(imageReady({ ...shown, naturalWidth: 640, naturalHeight: 480 }, frame, src)).toBe(false); // an older, smaller picture
    expect(imageReady({ ...shown, currentSrc: "data:image/jpeg;base64,OLD" }, frame, src)).toBe(false);
  });
  it("changes with the session, page, layout or stream, but not with each frame", () => {
    const state = { generation: "g1", activePageId: "p1" };
    const key = pictureKey({ sequence: 1 }, state);
    expect(pictureKey({ sequence: 2 }, state)).toBe(key);
    expect(pictureKey({ sequence: 1 }, { ...state, generation: "g2" })).not.toBe(key);
    expect(pictureKey({ sequence: 1 }, { ...state, activePageId: "p2" })).not.toBe(key);
    expect(pictureKey({ sequence: 1, viewportRevision: 3 } as { sequence: number }, state)).not.toBe(key);
    expect(pictureKey({ sequence: 1, streamId: "s2" } as { sequence: number }, state)).not.toBe(key);
  });
});
