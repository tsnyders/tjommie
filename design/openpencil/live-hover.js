// Grokbot agent avatar: live hover inside the running OpenPencil app.
// Eyes follow the cursor, one blink on hover-enter, occasional idle blink, hover highlight fades in.
// Run:  npx --prefix design/tools openpencil eval --stdin < design/openpencil/live-hover.js
// Stop: npx --prefix design/tools openpencil eval --code "window.__grokbotAvatar?.stop(); return 'stopped'"
// Motion (Emil): transform-only moves, ~250ms follow settle, 165ms blink, no follow under reduced motion.

if (window.__grokbotAvatar) window.__grokbotAvatar.stop()

const PAGE = '0:3'
const AVATARS = [{ frame: '0:6', open: '0:20', half: '0:41', closed: '0:44', hover: '0:64' }]

const renderer = figma._renderer
const canvas = document.querySelector('canvas')
const reduce = matchMedia('(prefers-reduced-motion: reduce)').matches
const pointer = { x: 1e9, y: 1e9 }

function onMove(e) {
  const rect = canvas.getBoundingClientRect()
  const p = renderer.screenToCanvas(e.clientX - rect.left, e.clientY - rect.top)
  pointer.x = p.x
  pointer.y = p.y
}
function onLeave() {
  pointer.x = 1e9
  pointer.y = 1e9
}
window.addEventListener('pointermove', onMove, { capture: true, passive: true })
document.documentElement.addEventListener('pointerleave', onLeave)

const now = performance.now()
const avatars = AVATARS.map((ids) => {
  const node = (k) => figma.getNodeById(ids[k])
  return {
    frame: node('frame'),
    eyes: { open: node('open'), half: node('half'), closed: node('closed') },
    hover: node('hover'),
    eye: { x: 0, y: 0 },
    hoverAlpha: 0,
    inside: false,
    blinkAt: -1,
    nextIdle: now + 2500,
    shown: 'open',
    written: { x: 0, y: 0, a: 0 },
  }
})

function blinkPhase(t, a) {
  if (a.blinkAt < 0) return 'open'
  const e = t - a.blinkAt
  if (e < 45) return 'half'
  if (e < 120) return 'closed'
  if (e < 165) return 'half'
  a.blinkAt = -1
  return 'open'
}

let raf = 0
function tick(t) {
  raf = requestAnimationFrame(tick)
  if (renderer.pageId !== PAGE) return
  for (const a of avatars) {
    const b = a.frame.absoluteBoundingBox
    const size = b.width
    const inside = pointer.x >= b.x && pointer.x <= b.x + size && pointer.y >= b.y && pointer.y <= b.y + b.height

    if (inside && !a.inside && a.blinkAt < 0) a.blinkAt = t // blink once when the cursor arrives
    a.inside = inside
    if (!reduce && !inside && a.blinkAt < 0 && t > a.nextIdle) {
      a.blinkAt = t
      a.nextIdle = t + 3500 + Math.random() * 3000
    }

    // Look toward the cursor: direction scaled by proximity, max 7.5% of avatar size.
    let tx = 0, ty = 0
    if (!reduce) {
      const cx = b.x + size * 0.515, cy = b.y + size * 0.55
      const dx = pointer.x - cx, dy = pointer.y - cy
      const d = Math.hypot(dx, dy)
      if (d > 0.01 && d < size * 4) {
        const reach = Math.min(1, d / (size * 0.75))
        const max = size * 0.075
        tx = (dx / d) * max * reach
        ty = (dy / d) * max * reach * 0.8
      }
    }
    a.eye.x += (tx - a.eye.x) * 0.18 // ~ power3.out settle in ~250ms at 60fps
    a.eye.y += (ty - a.eye.y) * 0.18
    a.hoverAlpha += ((inside ? 1 : 0) - a.hoverAlpha) * 0.22

    const x = Math.round(a.eye.x * 10) / 10, y = Math.round(a.eye.y * 10) / 10
    if (x !== a.written.x || y !== a.written.y) {
      for (const g of Object.values(a.eyes)) { g.x = x; g.y = y }
      a.written.x = x
      a.written.y = y
    }
    const alpha = Math.round(a.hoverAlpha * 50) / 50
    if (a.hover && alpha !== a.written.a) {
      a.hover.opacity = alpha
      a.written.a = alpha
    }
    const phase = blinkPhase(t, a)
    if (phase !== a.shown) {
      for (const [k, g] of Object.entries(a.eyes)) g.visible = k === phase
      a.shown = phase
    }
  }
}
raf = requestAnimationFrame(tick)

window.__grokbotAvatar = {
  stop() {
    cancelAnimationFrame(raf)
    window.removeEventListener('pointermove', onMove, { capture: true })
    document.documentElement.removeEventListener('pointerleave', onLeave)
    for (const a of avatars) {
      for (const [k, g] of Object.entries(a.eyes)) { g.x = 0; g.y = 0; g.visible = k === 'open' }
      if (a.hover) a.hover.opacity = 0
    }
    delete window.__grokbotAvatar
  },
}
return `live hover running on ${avatars.length} avatar(s); reduced motion: ${reduce}`
