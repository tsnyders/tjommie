class TjommiePcm extends AudioWorkletProcessor {
  constructor() {
    super();
    this.samples = new Int16Array(2048);
    this.used = 0;
    this.total = 0;
    this.stopped = false;
    this.port.onmessage = (event) => {
      if (event.data?.kind !== "flush") return;
      this.stopped = true;
      this.flush();
      this.port.postMessage({ kind: "flushed" });
    };
  }

  flush() {
    if (!this.used) return;
    const buffer = new ArrayBuffer(this.used * 2);
    const view = new DataView(buffer);
    for (let index = 0; index < this.used; index += 1) view.setInt16(index * 2, this.samples[index], true);
    this.used = 0;
    this.samples.fill(0);
    this.port.postMessage({ kind: "pcm", buffer }, [buffer]);
  }

  process(inputs, outputs) {
    for (const output of outputs) for (const channel of output) channel.fill(0);
    if (this.stopped) return true;
    const input = inputs[0]?.[0];
    if (!input) return true;
    for (const value of input) {
      if (this.total >= 960000) {
        this.stopped = true;
        this.flush();
        this.port.postMessage({ kind: "limit" });
        break;
      }
      const sample = Number.isFinite(value) ? Math.max(-1, Math.min(1, value)) : 0;
      this.samples[this.used++] = Math.round(sample * (sample < 0 ? 32768 : 32767));
      this.total += 1;
      if (this.used === this.samples.length) this.flush();
    }
    return true;
  }
}

registerProcessor("tjommie-pcm", TjommiePcm);
