// Recolor a cloned WF frame from the dark tokens to the light tokens (from the macOS references).
// Runs inside the OpenPencil app: npx --prefix design/tools openpencil eval --stdin < design/openpencil/recolor-light.js
// Role-aware: TEXT, icon vectors and surfaces use different maps. Bot avatars (instances), white eyes,
// the Chrome logo and the remote "Live screen" content are left untouched.
const ROOT_ID = globalThis.__RECOLOR_ROOT ?? '0:1915'

const SURFACE = {
  '#070707': '#FCFCFC', '#111111': '#F5F5F5', '#262626': '#EFEFEF', '#181818': '#F0F0F0',
  '#2F2F2F': '#FFFFFF', '#3A3A3A': '#F0F0F0', '#303030': '#E1E1E1', '#1A1A1A': '#EFEFEF',
  '#F5F5F5': '#0A0A0A', '#101722': '#EEF4FF', '#3A2020': '#FDECEC', '#252525': '#E4E4E4',
  '#080808': '#EBEBEB', '#1C1C1C': '#EBEBEB', '#3FA37A': '#17804F', '#459FFF': '#2563EB',
  '#EA4144': '#DC2626',
}
const STROKE = {
  '#252525': '#E4E4E4', '#080808': '#EBEBEB', '#1C1C1C': '#EBEBEB', '#111111': '#F5F5F5',
  '#1E3A5F': '#C7DBFF', '#EA4144': '#DC2626', '#303030': '#D9D9D9', '#2A2A2A': '#D6D6D6',
  '#070707': '#FCFCFC', '#459FFF': '#2563EB', '#3FA37A': '#17804F', '#262626': '#E4E4E4',
}
const TEXT = {
  '#F5F5F5': '#0A0A0A', '#FCFCFC': '#0A0A0A', '#070707': '#FFFFFF', '#BDBDBD': '#404040',
  '#989898': '#6B6B6B', '#909090': '#6B6B6B', '#9C9C9C': '#6B6B6B', '#6E6E6E': '#767676',
  '#ACACAC': '#5C5C5C', '#3FA37A': '#17804F', '#459FFF': '#2563EB', '#EA4144': '#DC2626',
}
const ICON = {
  '#ACACAC': '#5C5C5C', '#989898': '#6B6B6B', '#909090': '#6B6B6B', '#F5F5F5': '#0A0A0A',
  '#070707': '#FFFFFF', '#3FA37A': '#17804F', '#459FFF': '#2563EB', '#EA4144': '#DC2626',
  '#BDBDBD': '#404040',
}

const hex = (c) => '#' + [c.r, c.g, c.b].map((v) => Math.round(v * 255).toString(16).padStart(2, '0')).join('').toUpperCase()
const rgb = (h) => ({ r: parseInt(h.slice(1, 3), 16) / 255, g: parseInt(h.slice(3, 5), 16) / 255, b: parseInt(h.slice(5, 7), 16) / 255 })

const unmapped = {}
let changedNodes = 0
const special = { liveScreen: [], composer: [], voice: [], cards: [], attachment: [], targetSwitch: [] }

function remap(list, map, node, kind) {
  if (!Array.isArray(list) || !list.length) return null
  let changed = false
  const out = list.map((p) => {
    const color = p.color
    if (!color || (p.type && p.type !== 'SOLID')) return p
    const h = hex(color)
    const to = map[h]
    if (!to) {
      if (h !== '#FFFFFF') unmapped[`${kind}:${h}`] = (unmapped[`${kind}:${h}`] ?? 0) + 1
      return p
    }
    changed = true
    return { ...p, color: { ...rgb(to), a: color.a ?? 1 } }
  })
  return changed ? out : null
}

function insideIcon(node, root) {
  for (let p = node.parent; p && p !== root; p = p.parent) if (/^Icon \//.test(p.name)) return true
  return false
}

function walk(node, root, inTargetSwitch) {
  if (node.type === 'INSTANCE') return // bot avatars keep their colors
  if (/^Icon \/ logos:/.test(node.name)) return // multicolor brand logo
  if (node.name === 'Live screen') { special.liveScreen.push(node.id); return } // remote Chrome content stays as is
  if (/composer/i.test(node.name) && node.type === 'FRAME') special.composer.push(node.id)
  if (/voice/i.test(node.name)) special.voice.push(node.id)
  if (/profile card|logins card/i.test(node.name)) special.cards.push(node.id)
  if (/attachment|\.pdf|file card/i.test(node.name) && node.type === 'FRAME') special.attachment.push(node.id)
  const inSwitch = inTargetSwitch || /target switch/i.test(node.name)
  if (/target switch/i.test(node.name)) special.targetSwitch.push(node.id)

  let changed = false
  if (node.type === 'TEXT') {
    const f = remap(node.fills, TEXT, node, 'text')
    if (f) { node.fills = f; changed = true }
  } else if (insideIcon(node, root)) {
    const f = remap(node.fills, ICON, node, 'icon')
    if (f) { node.fills = f; changed = true }
    const s = remap(node.strokes, ICON, node, 'icon-stroke')
    if (s) { node.strokes = s; changed = true }
  } else if (node !== undefined) {
    const surfaceMap = inSwitch ? { ...SURFACE, '#303030': '#FFFFFF' } : SURFACE
    const f = remap(node.fills, surfaceMap, node, 'surface')
    if (f) { node.fills = f; changed = true }
    const s = remap(node.strokes, STROKE, node, 'stroke')
    if (s) { node.strokes = s; changed = true }
  }
  if (changed) changedNodes++
  if ('children' in node && node.children) for (const c of node.children) walk(c, root, inSwitch)
}

const root = figma.getNodeById(ROOT_ID)
if (!root) return `root ${ROOT_ID} not found`
walk(root, root, false)
return { root: root.name, changedNodes, unmapped, special }
