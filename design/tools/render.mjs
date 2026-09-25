// Headless JSX -> .fig renderer for OpenPencil (no running app needed).
// Usage: node design/tools/render.mjs <out.fig> "<Page name>=<a.jsx>,<b.jsx>" ["<Page 2>=<c.jsx>"]...
// Frames on a page are laid out left to right with a 200px gap.
// Rasterize afterwards with: npx openpencil export <out.fig> -f png --page "<Page name>" -o <out.png>
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { SceneGraph } from '@open-pencil/scene-graph'
import { renderJSX } from '@open-pencil/core/design-jsx'
import { BUILTIN_IO_FORMATS, IORegistry, headlessRenderNodes } from '@open-pencil/core/io'
import { computeAllLayouts } from '@open-pencil/core/layout'
import { prepareGraphFonts } from '@open-pencil/core/text'

const [out, ...pageSpecs] = process.argv.slice(2)
if (!out || !pageSpecs.length) {
  console.error('usage: node render.mjs <out.fig> "<Page>=<a.jsx>,<b.jsx>" ...')
  process.exit(1)
}

const graph = new SceneGraph()
const existing = graph.getPages()
let firstPageFree = existing.length > 0

for (const spec of pageSpecs) {
  const eq = spec.indexOf('=')
  const name = spec.slice(0, eq)
  const files = spec.slice(eq + 1).split(',').filter(Boolean)
  let page
  if (firstPageFree) {
    page = existing[0]
    page.name = name
    firstPageFree = false
  } else {
    page = graph.addPage(name)
  }
  let x = 0
  for (const file of files) {
    const jsx = await readFile(file, 'utf8')
    const results = await renderJSX(graph, jsx, { parentId: page.id, x, y: 0 })
    computeAllLayouts(graph, page.id)
    for (const r of results) {
      for (const w of r.warnings ?? []) console.warn(`[${file}] warning: ${w}`)
      const node = graph.getNode(r.id)
      const width = node?.width ?? 1440
      console.log(`${name}: ${file} -> ${r.id} "${r.name}" ${Math.round(width)}x${Math.round(node?.height ?? 0)} at x=${x}`)
      x += width + 200
    }
  }
}

// Load the fonts the frames use (system fonts included), then re-measure text.
const fontStatus = await prepareGraphFonts(graph, graph.getPages().map((p) => p.id))
if (!fontStatus.faithful) {
  console.warn('font substitution: ' + fontStatus.issues.map((f) => `${f.family} ${f.style}`).join(', '))
}
computeAllLayouts(graph)
const io = new IORegistry(BUILTIN_IO_FORMATS)
const result = await io.writeDocument('fig', graph)
await mkdir(dirname(out), { recursive: true })
await writeFile(out, result.data)
console.log(`wrote ${out}`)

// PNG per top-level frame, next to the .fig: <out-dir>/png/<page>--<frame>.png
// (patched headless.js in design/tools fixes the CanvasKit wasm path on Windows)
const scale = Number(process.env.PNG_SCALE ?? 1)
const pngDir = join(dirname(out), 'png')
await mkdir(pngDir, { recursive: true })
const slug = (s) => s.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '')
for (const page of graph.getPages()) {
  for (const childId of page.childIds ?? []) {
    const node = graph.getNode(childId)
    const png = await headlessRenderNodes(graph, page.id, [childId], { scale })
    if (!png) continue
    const file = join(pngDir, `${slug(page.name)}--${slug(node?.name ?? childId)}.png`)
    await writeFile(file, png)
    console.log(`png ${file}`)
  }
}
