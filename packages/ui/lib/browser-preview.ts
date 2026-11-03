import type { BrowserFrame, BrowserState } from "@tjommie/protocol";

/** A visible card keeps one image and one request, with no queued video or persistent cache. */
export class BrowserPreview {
  #visible = false;
  #epoch = 0;
  #pending = false;
  #timer: ReturnType<typeof setTimeout> | undefined;
  constructor(private readonly pull: () => Promise<{ state: BrowserState; frame: BrowserFrame | null }>, private readonly show: (frame: BrowserFrame | null) => void) {}
  setVisible(visible: boolean): void {
    if (this.#visible === visible) return;
    this.#visible = visible; this.#epoch++;
    clearTimeout(this.#timer); this.show(null);
    if (visible) void this.#next();
  }
  async #next(): Promise<void> {
    if (!this.#visible || this.#pending) return;
    const epoch = this.#epoch; this.#pending = true;
    try {
      const { state, frame } = await this.pull();
      if (this.#visible && this.#epoch === epoch) this.show(state.privateActive ? null : frame);
    } catch { if (this.#visible && this.#epoch === epoch) this.show(null); }
    finally {
      this.#pending = false;
      if (this.#visible) this.#timer = setTimeout(() => { void this.#next(); }, this.#epoch === epoch ? 1000 : 0);
    }
  }
}
