// Renders the agent-avatar hover animation frame by frame with OpenPencil's engine.
// Usage: node design/tools/avatar-frames.mjs <outDir>   -> <outDir>/frame-000.png ...
// Motion rules (Emil): eyes only move via transform, max travel 5.5% of avatar size,
// follow smoothing ~ power3.out 250ms, one 133ms blink on pointer-enter, ease back to center on leave.
import { mkdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { SceneGraph } from '@open-pencil/scene-graph'
import { renderJSX } from '@open-pencil/core/design-jsx'
import { headlessRenderNodes } from '@open-pencil/core/io'
import { computeAllLayouts } from '@open-pencil/core/layout'

const outDir = process.argv[2] ?? 'design/out/motion/frames'
await mkdir(outDir, { recursive: true })

const W = 360, H = 240, SIZE = 120, AX = 120, AY = 58 // avatar box (40-unit viewBox scaled 3x)
const S = SIZE / 40
const FPS = 30, SECONDS = 4.2, N = Math.round(FPS * SECONDS)
const BODY = 'M11 34 C5 34 2 29.5 3.5 25 C2 19 7.5 15 12.5 16.5 C14 10 21 7 26 11 C32 9.5 37 14 35.5 20 C39 23.5 37 32 30.5 34 Z'
const EYES = {
  open: ['M17.5 19 L16.7 25', 'M24.5 19 L23.7 25', 3],
  half: ['M17.2 21 L16.9 23.6', 'M24.2 21 L23.9 23.6', 3],
  closed: ['M15.4 22.5 L18.8 22.5', 'M22.4 22.5 L25.8 22.5', 2.5],
}
const EYE_CENTER = { x: AX + 20.6 * S, y: AY + 22 * S } // between the eyes, canvas px
const MAX_TRAVEL = 3 * S // 7.5% of size
const NOTICE_RADIUS = 220 // eyes start tracking when the cursor is this close

// Cursor keyframes: [time s, x, y]
const PATH = [
  [0.0, 20, 232], [0.8, 60, 70], [1.5, 300, 52], [2.0, 206, 126],
  [2.7, 214, 136], [3.3, 352, 214], [3.6, 420, 260], [4.2, 420, 260],
]
const easeInOut = (t) => (t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2)
function cursorAt(t) {
  for (let i = 0; i < PATH.length - 1; i++) {
    const [t0, x0, y0] = PATH[i], [t1, x1, y1] = PATH[i + 1]
    if (t >= t0 && t <= t1) {
      const k = easeInOut((t - t0) / (t1 - t0 || 1))
      return { x: x0 + (x1 - x0) * k, y: y0 + (y1 - y0) * k }
    }
  }
  const last = PATH.at(-1)
  return { x: last[1], y: last[2] }
}
const inside = (p) => p.x > AX + 4 && p.x < AX + SIZE - 4 && p.y > AY + 20 && p.y < AY + SIZE - 12

const graph = new SceneGraph()
const page = graph.getPages()[0]
let eye = { x: 0, y: 0 }, hover = 0, wasInside = false, blinkStart = -1
const frames = []

for (let f = 0; f < N; f++) {
  const t = f / FPS
  const c = cursorAt(t)
  const isInside = inside(c)
  if (isInside && !wasInside) blinkStart = f // one blink on pointer-enter
  wasInside = isInside

  // target eye offset: toward the cursor, scaled by proximity, clamped
  const dx = c.x - EYE_CENTER.x, dy = c.y - EYE_CENTER.y
  const dist = Math.hypot(dx, dy)
  const near = c.x < W + 10 && dist < NOTICE_RADIUS
  const reach = near ? Math.min(1, dist / 70) : 0
  const target = dist > 0.001 ? { x: (dx / dist) * MAX_TRAVEL * reach, y: (dy / dist) * MAX_TRAVEL * reach * 0.8 } : { x: 0, y: 0 }
  eye.x += (target.x - eye.x) * 0.32 // ~quickTo(0.25s, power3.out) at 30fps
  eye.y += (target.y - eye.y) * 0.32
  hover += ((isInside ? 1 : 0) - hover) * 0.45

  const b = blinkStart >= 0 ? f - blinkStart : -1
  const state = b === 0 || b === 3 ? 'half' : b === 1 || b === 2 ? 'closed' : 'open'
  const [l, r, sw] = EYES[state]

  frames.push(`<Frame name="frame-${String(f).padStart(3, '0')}" w={${W}} h={${H}} bg="#070707" overflow="hidden">
  <Rectangle name="Hover bg" position="absolute" x={${AX - 16}} y={${AY - 16}} w={${SIZE + 32}} h={${SIZE + 32}} rounded={32} bg="#303030" opacity={${hover.toFixed(3)}} />
  <svg name="Body" position="absolute" x={${AX}} y={${AY}} viewBox="0 0 40 40" size={${SIZE}}><path d="${BODY}" fill="#EC712E" /></svg>
  <svg name="Eyes" position="absolute" x={${(AX + eye.x).toFixed(2)}} y={${(AY + eye.y).toFixed(2)}} viewBox="0 0 40 40" size={${SIZE}}><path d="${l}" stroke="#FFFFFF" strokeWidth={${sw}} fill="none" /><path d="${r}" stroke="#FFFFFF" strokeWidth={${sw}} fill="none" /></svg>
  <Icon name="lucide:mouse-pointer-2" size={26} color="#FFFFFF" position="absolute" x={${c.x.toFixed(1)}} y={${c.y.toFixed(1)}} />
</Frame>`)
}

const ids = []
for (let i = 0; i < frames.length; i++) {
  const [res] = await renderJSX(graph, frames[i], { parentId: page.id, x: (i % 12) * (W + 40), y: Math.floor(i / 12) * (H + 40) })
  ids.push(res.id)
}
computeAllLayouts(graph)
for (let i = 0; i < ids.length; i++) {
  const png = await headlessRenderNodes(graph, page.id, [ids[i]], { scale: 2 })
  await writeFile(join(outDir, `frame-${String(i).padStart(3, '0')}.png`), png)
}
console.log(`blinkStart=${blinkStart}`); console.log(`rendered ${ids.length} frames to ${outDir}`)
