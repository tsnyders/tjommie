// @vitest-environment happy-dom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { PdfFileViewer } from "./pdf-file-viewer";

const fixture = vi.hoisted(() => ({ text: vi.fn(), layers: vi.fn(), cleanup: vi.fn() }));
vi.mock("http://localhost:3000/pdfjs/build/pdf.mjs", () => ({
  GlobalWorkerOptions: {}, PDFDataRangeTransport: class {},
  getDocument: () => ({ promise: Promise.resolve({ numPages: 2, cleanup: async () => {}, getPage: async (page: number) => ({
    getViewport: () => ({ width: 100, height: 100, userUnit: 1 }), render: () => ({ promise: Promise.resolve(), cancel() {} }),
    getTextContent: () => fixture.text(page), cleanup: fixture.cleanup,
  }) }), destroy: async () => {} }),
  TextLayer: class {
    readonly options: { textContentSource: { items: { str: string }[] }; container: HTMLElement };
    constructor(options: { textContentSource: { items: { str: string }[] }; container: HTMLElement }) { this.options = options; fixture.layers(options.textContentSource); }
    async render() { this.options.container.textContent = this.options.textContentSource.items.map(item => item.str).join(""); }
    cancel() {}
  },
}));

it("keeps a late previous-page text result out of the current page", async () => {
  Reflect.set(globalThis, "IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("devicePixelRatio", 1);
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({} as CanvasRenderingContext2D);
  let finish!: (value: { items: { str: string }[] }) => void;
  fixture.text.mockImplementation((page: number) => page === 1 ? new Promise(resolve => { finish = resolve; }) : Promise.resolve({ items: [{ str: "Current page" }] }));
  const container = document.body.appendChild(document.createElement("div")), root = createRoot(container);
  const settle = () => act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
  try {
    await act(async () => root.render(<PdfFileViewer url="app://tjommie/_attachments/pdf" size={500} />)); await settle(); await settle();
    expect(fixture.text).toHaveBeenCalledWith(1);
    expect(container.querySelector<HTMLInputElement>('[aria-label="Search PDF"]')?.labels?.[0].textContent).toBe("Search PDF");
    await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="Next page"]')!.click()); await settle();
    expect(container.querySelector(".textLayer")?.textContent).toBe("Current page");
    await act(async () => finish({ items: [{ str: "Stale page" }] })); await settle();
    expect(container.querySelector(".textLayer")?.textContent).toBe("Current page");
    expect(fixture.layers).toHaveBeenCalledOnce();
  } finally { finish?.({ items: [] }); await act(async () => root.unmount()); container.remove(); fixture.text.mockReset(); fixture.layers.mockClear(); vi.restoreAllMocks(); vi.unstubAllGlobals(); }
});
