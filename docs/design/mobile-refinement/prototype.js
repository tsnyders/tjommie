"use strict";
// Tjommie mobile: five design directions. Synthetic data only; no network, no storage, no real accounts.

// Icons: lucide-react 1.48.0 geometry, the set the app ships.
const ICONS = {"layout-grid":"<rect width='7' height='7' x='3' y='3' rx='1'/><rect width='7' height='7' x='14' y='3' rx='1'/><rect width='7' height='7' x='14' y='14' rx='1'/><rect width='7' height='7' x='3' y='14' rx='1'/>",
  "users":"<path d='M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2'/><path d='M16 3.128a4 4 0 0 1 0 7.744'/><path d='M22 21v-2a4 4 0 0 0-3-3.87'/><circle cx='9' cy='7' r='4'/>",
  "settings":"<path d='M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915'/><circle cx='12' cy='12' r='3'/>",
  "search":"<path d='m21 21-4.34-4.34'/><circle cx='11' cy='11' r='8'/>",
  "plus":"<path d='M5 12h14'/><path d='M12 5v14'/>",
  "ellipsis":"<circle cx='12' cy='12' r='1'/><circle cx='19' cy='12' r='1'/><circle cx='5' cy='12' r='1'/>",
  "chevron-left":"<path d='m15 18-6-6 6-6'/>",
  "chevron-right":"<path d='m9 18 6-6-6-6'/>",
  "chevron-down":"<path d='m6 9 6 6 6-6'/>",
  "arrow-up":"<path d='m5 12 7-7 7 7'/><path d='M12 19V5'/>",
  "rotate-ccw":"<path d='M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8'/><path d='M3 3v5h5'/>",
  "circle-alert":"<circle cx='12' cy='12' r='10'/><line x1='12' x2='12' y1='8' y2='12'/><line x1='12' x2='12.01' y1='16' y2='16'/>",
  "loader":"<path d='M12 2v4'/><path d='m16.2 7.8 2.9-2.9'/><path d='M18 12h4'/><path d='m16.2 16.2 2.9 2.9'/><path d='M12 18v4'/><path d='m4.9 19.1 2.9-2.9'/><path d='M2 12h4'/><path d='m4.9 4.9 2.9 2.9'/>",
  "pause":"<rect x='14' y='3' width='5' height='18' rx='1'/><rect x='5' y='3' width='5' height='18' rx='1'/>",
  "play":"<path d='M5 5a2 2 0 0 1 3.008-1.728l11.997 6.998a2 2 0 0 1 .003 3.458l-12 7A2 2 0 0 1 5 19z'/>",
  "hourglass":"<path d='M5 22h14'/><path d='M5 2h14'/><path d='M17 22v-4.172a2 2 0 0 0-.586-1.414L12 12l-4.414 4.414A2 2 0 0 0 7 17.828V22'/><path d='M7 2v4.172a2 2 0 0 0 .586 1.414L12 12l4.414-4.414A2 2 0 0 0 17 6.172V2'/>",
  "info":"<circle cx='12' cy='12' r='10'/><path d='M12 16v-4'/><path d='M12 8h.01'/>",
  "list-checks":"<path d='M13 5h8'/><path d='M13 12h8'/><path d='M13 19h8'/><path d='m3 17 2 2 4-4'/><path d='m3 7 2 2 4-4'/>",
  "check":"<path d='M20 6 9 17l-5-5'/>",
  "x":"<path d='M18 6 6 18'/><path d='m6 6 12 12'/>",
  "paperclip":"<path d='m16 6-8.414 8.586a2 2 0 0 0 2.829 2.829l8.414-8.586a4 4 0 1 0-5.657-5.657l-8.379 8.551a6 6 0 1 0 8.485 8.485l8.379-8.551'/>",
  "refresh-cw":"<path d='M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8'/><path d='M21 3v5h-5'/><path d='M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16'/><path d='M8 16H3v5'/>",
  "message-circle":"<path d='M2.992 16.342a2 2 0 0 1 .094 1.167l-1.065 3.29a1 1 0 0 0 1.236 1.168l3.413-.998a2 2 0 0 1 1.099.092 10 10 0 1 0-4.777-4.719'/>",
  "at-sign":"<circle cx='12' cy='12' r='4'/><path d='M16 8v5a3 3 0 0 0 6 0v-1a10 10 0 1 0-4 8'/>",
  "sliders-horizontal":"<path d='M10 5H3'/><path d='M12 19H3'/><path d='M14 3v4'/><path d='M16 17v4'/><path d='M21 12h-9'/><path d='M21 19h-5'/><path d='M21 5h-7'/><path d='M8 10v4'/><path d='M8 12H3'/>",
  "monitor":"<rect width='20' height='14' x='2' y='3' rx='2'/><line x1='8' x2='16' y1='21' y2='21'/><line x1='12' x2='12' y1='17' y2='21'/>",
  "key-round":"<path d='M2.586 17.414A2 2 0 0 0 2 18.828V21a1 1 0 0 0 1 1h3a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h1a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h.172a2 2 0 0 0 1.414-.586l.814-.814a6.5 6.5 0 1 0-4-4z'/><circle cx='16.5' cy='7.5' r='.5' fill='currentColor'/>",
  "chart-column":"<path d='M3 3v16a2 2 0 0 0 2 2h16'/><path d='M18 17V9'/><path d='M13 17V5'/><path d='M8 17v-3'/>",
  "bot":"<path d='M12 8V4H8'/><rect width='16' height='12' x='4' y='8' rx='2'/><path d='M2 14h2'/><path d='M20 14h2'/><path d='M15 13v2'/><path d='M9 13v2'/>",
  "square":"<rect width='18' height='18' x='3' y='3' rx='2'/>",
  "sparkles":"<path d='M11.017 2.814a1 1 0 0 1 1.966 0l1.051 5.558a2 2 0 0 0 1.594 1.594l5.558 1.051a1 1 0 0 1 0 1.966l-5.558 1.051a2 2 0 0 0-1.594 1.594l-1.051 5.558a1 1 0 0 1-1.966 0l-1.051-5.558a2 2 0 0 0-1.594-1.594l-5.558-1.051a1 1 0 0 1 0-1.966l5.558-1.051a2 2 0 0 0 1.594-1.594z'/><path d='M20 2v4'/><path d='M22 4h-4'/><circle cx='4' cy='20' r='2'/>",
  "gauge":"<path d='m12 14 4-4'/><path d='M3.34 19a10 10 0 1 1 17.32 0'/>",
  "shield":"<path d='M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z'/>",
  "folder":"<path d='M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z'/>",
  "clock":"<circle cx='12' cy='12' r='10'/><path d='M12 6v6l4 2'/>"};

const ic = (name, cls = "") => `<svg class="i ${cls}" viewBox="0 0 24 24" aria-hidden="true">${ICONS[name]}</svg>`;
const esc = (text) => { const d = document.createElement("div"); d.textContent = text; return d.innerHTML; };
// The app's own clock (lib/format.ts CLOCK): en-US, "6:21 PM".
const CLOCK = new Intl.DateTimeFormat("en-US", { hour: "numeric", minute: "2-digit" });
const clock = () => CLOCK.format(new Date()).split(String.fromCharCode(0x202f)).join(" ");

// Bot avatars: the exact shapes from packages/ui/components/bot-avatar.tsx.
const SHAPES = {
  circle: ["M20 2 C29.94 2 38 10.06 38 20 C38 29.94 29.94 38 20 38 C10.06 38 2 29.94 2 20 C2 10.06 10.06 2 20 2 Z", "M17 14 L16 20 M24.5 14 L23.5 20"],
  "rounded-square": ["M11 4 H29 C32.9 4 36 7.1 36 11 V29 C36 32.9 32.9 36 29 36 H11 C7.1 36 4 32.9 4 29 V11 C4 7.1 7.1 4 11 4 Z", "M17 14 L16 20 M24.5 14 L23.5 20"],
  pill: ["M12 10 H28 C33.5 10 38 14.5 38 20 C38 25.5 33.5 30 28 30 H12 C6.5 30 2 25.5 2 20 C2 14.5 6.5 10 12 10 Z", "M18 15.5 L17.3 21.5 M24.5 15.5 L23.8 21.5"],
  hexagon: ["M17 3.2 C18.9 2.1 21.1 2.1 23 3.2 L33.5 9.3 C35.4 10.4 36.5 12.4 36.5 14.6 V25.4 C36.5 27.6 35.4 29.6 33.5 30.7 L23 36.8 C21.1 37.9 18.9 37.9 17 36.8 L6.5 30.7 C4.6 29.6 3.5 27.6 3.5 25.4 V14.6 C3.5 12.4 4.6 10.4 6.5 9.3 Z", "M17 15 L16 21 M24.5 15 L23.5 21"],
  cloud: ["M11 34 C5 34 2 29.5 3.5 25 C2 19 7.5 15 12.5 16.5 C14 10 21 7 26 11 C32 9.5 37 14 35.5 20 C39 23.5 37 32 30.5 34 Z", "M17.5 19 L16.7 25 M24.5 19 L23.7 25"],
  droplet: ["M20 3 C20 3 33 16 33 25 C33 32.2 27.2 37 20 37 C12.8 37 7 32.2 7 25 C7 16 20 3 20 3 Z", "M17.5 21 L16.8 27 M24 21 L23.3 27"],
};
const avatarSvg = (b) => `<svg viewBox="0 0 40 40" aria-hidden="true"><path class="av-body" d="${SHAPES[b.shape][0]}" fill="${b.color}"/><path class="av-eyes" d="${SHAPES[b.shape][1]}" stroke="#fff" stroke-width="3" stroke-linecap="round" fill="none"/></svg>`;
const avw = (b, i = 0) => `<span class="avw" data-state="${b.state}" style="--c:${b.color};--i:${i}">${avatarSvg(b)}</span>`;
// The Team room mark (TeamMark in bot-avatar.tsx): three small avatars on a tile.
const teamMark = () => `<span class="avw team-mark" style="--s:34px"><svg viewBox="0 0 64 64" aria-hidden="true"><rect width="64" height="64" rx="16" fill="var(--tile)"/><g transform="translate(9.2 10.8) scale(0.56)"><path d="${SHAPES.circle[0]}" fill="#8C5BF3"/><path d="${SHAPES.circle[1]}" stroke="#fff" stroke-width="3" stroke-linecap="round"/></g><g transform="translate(31.5 11.3) scale(0.56)"><path d="${SHAPES.cloud[0]}" fill="#EC712E"/><path d="${SHAPES.cloud[1]}" stroke="#fff" stroke-width="3" stroke-linecap="round"/></g><g transform="translate(20.4 30) scale(0.56)"><path d="${SHAPES["rounded-square"][0]}" fill="#5BC57A"/><path d="${SHAPES["rounded-square"][1]}" stroke="#fff" stroke-width="3" stroke-linecap="round"/></g></svg></span>`;

// ---------------------------------------------------------------- Sample data (synthetic)
const BOTS = [
  { id: "night", name: "Night Shift", color: "#EC712E", shape: "hexagon", state: "waiting", task: "Publish v2.4.0 to npm?", age: "2m" },
  { id: "inbox", name: "Inbox Triage", color: "#EC4698", shape: "cloud", state: "working", task: "Sorting 38 new emails", age: "now" },
  { id: "notes", name: "Release Notes", color: "#5BC57A", shape: "rounded-square", state: "working", task: "Drafting notes for v2.4", age: "now" },
  { id: "research", name: "Researcher", color: "#3B82F6", shape: "circle", state: "idle", task: "Summarised the vendor shortlist.", age: "1h" },
  { id: "ledger", name: "Ledger", color: "#8C5BF3", shape: "pill", state: "paused", task: "", age: "Mon" },
  { id: "scout", name: "Scout", color: "#57B8A7", shape: "droplet", state: "limited", task: "", age: "3h" },
];
const bot = (id) => BOTS.find((b) => b.id === id);
const STATE_LABEL = { working: "Working", waiting: "Waiting for you", paused: "Paused by you", limited: "Limited until 10:40 PM", idle: "Idle" };
const FILTERS = [["all", "All"], ["working", "Working"], ["waiting", "Waiting"], ["paused", "Paused"]];
const count = (state) => (state === "all" ? BOTS.length : BOTS.filter((b) => b.state === state).length);
const BRAILLE = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";
const PEN = `<svg class="g-pen" viewBox="0 0 100 40" preserveAspectRatio="none" aria-hidden="true"><path pathLength="1" d="M9 23 C 6 9, 58 3, 90 11 C 103 16, 98 35, 52 36 C 17 37, 1 31, 7 17 C 9 12, 16 9, 24 8"/></svg>`;

const TEAM = [
  { k: "day", t: "Today" },
  { k: "user", t: "6:20 PM", html: `<span class="mention">@Night Shift</span> can you run the release checklist for v2.4? Loop in <span class="mention">@Release Notes</span> for the changelog.` },
  { k: "bot", b: "night", t: "6:21 PM", html: "On it. I'll ask Release Notes once the build is green." },
  { k: "handoff", a: "night", b: "notes" },
  { k: "bot", b: "notes", t: "6:34 PM", html: "Changelog drafted: 12 fixes and 3 features. I'll post it here when Night Shift publishes." },
  { k: "notice" },
  { k: "bot", b: "inbox", t: "6:41 PM", html: "Heads up: two customer emails mention the 2.3 login bug. It's already in the fixes list." },
];
const CHAT = [
  { k: "day", t: "Today" },
  { k: "user", t: "6:20 PM", html: "Run the release checklist for v2.4. Ask me before publishing anything." },
  { k: "act", summary: "5 steps: Tagged v2.4.0-rc.1", steps: ["Pulled main", "Ran npm test: 212 passed", "Built all packages", "Checked the changelog", "Tagged v2.4.0-rc.1"] },
  { k: "bot", b: "night", t: "6:31 PM", html: "Checklist is green: tests pass, the build is clean and the tag is ready. One step left." },
  { k: "appr" },
];
const APPROVAL = { summary: "Publish v2.4.0 to npm, dry run first", detail: "Bash · npm publish --dry-run", rule: "Always allow saves: Bash(npm publish:*), for Night Shift only" };

const DIRS = {
  hush: { n: 1, name: "Hush", for: "For minimalists", line: "Nothing on screen but your bots and what they're doing.",
    look: ["Today's black and white with less chrome: hairline rows instead of cards", "One accent colour, kept for “waiting for you”", "Today's bubbles stay (bot replies on #262626, radius 18); the chrome around them gets quieter", "The large title folds into the bar as you scroll"],
    motion: ["Short fades with a 6 px rise (180 to 300 ms), never a bounce", "The tab capsule glides between tabs", "Working bots breathe a small dot; nothing else loops", "Menus grow out of the ⋯ that opened them"],
    type: "System sans: Segoe here, Roboto on Android", filter: "Outline chips with counts, shown only for states that have bots",
    swatches: ["#070707", "#161616", "#f4f4f4", "#46b083", "#62a5ff"], cost: "Lowest. Today's components, restyled; no new fonts." },
  folio: { n: 2, name: "Folio", for: "For minimalists who love type", line: "Your bots, set like a calm morning paper.",
    look: ["Warm paper and ink, serif headlines, rules between entries", "A one-line lede says who needs you and who is working", "“Waiting for you” is circled in red pen", "Bot replies are set as reading text, not bubbles"],
    motion: ["Slower, softer easing (240 to 460 ms) with staggered entries", "The filter underline slides like a ruler", "The “Working” dots write themselves; the red-pen circle draws once", "Sheets slide up like a page"],
    type: "Serif display (Sitka here, Newsreader in production) with system sans", filter: "Text tabs with counts and a sliding underline",
    swatches: ["#f3efe6", "#1d1b17", "#b4412a", "#2f6b45", "#e4ddcd"], cost: "Medium. One bundled serif and a paper palette." },
  aurora: { n: 3, name: "Aurora", for: "For people who love polished, glossy apps", line: "Each bot casts its own light behind frosted glass.",
    look: ["Bot colours glow behind frosted glass panels", "A “Needs you” card puts the waiting bot first", "Working and waiting avatars wear a halo in their own colour", "Glass header, pill and composer float over the light"],
    motion: ["Springs, not curves: panels settle with a slight give", "The nav droplet stretches toward the next tab, then settles", "Light drifts slowly; the waiting bot's light glows", "Screens focus in from a soft blur"],
    type: "System sans with heavier display titles", filter: "One glass “Show” button that opens a menu with counts",
    swatches: ["#05060a", "#EC712E", "#EC4698", "#5BC57A", "#f5f7fc"], cost: "Highest. Backdrop blur needs a low-power fallback." },
  sticker: { n: 4, name: "Sticker", for: "For people who want bold, playful style", line: "Your bots as characters: loud colour, thick ink, springy motion.",
    look: ["Every bot gets a card in its own colour, two to a row", "Thick outlines and hard shadows; highlighter yellow marks the selection", "Poster headlines in a condensed face", "Buttons press down into their shadows"],
    motion: ["Bouncy springs when cards arrive and when you tap", "Avatars blink now and then; working bots bob", "The yellow block springs between tabs and the icon hops", "Menus and messages pop in"],
    type: "Condensed display (Franklin Gothic here, Archivo in production) with system sans", filter: "Sticker chips with count badges",
    swatches: ["#fbf6ec", "#141414", "#ffd43b", "#1f9d5c", "#2f6fed"], cost: "Medium. A card grid on Agents; all motion is CSS." },
  console: { n: 5, name: "Console", for: "For developers and power users", line: "Agents as processes: a calm terminal you can use with one thumb.",
    look: ["Monospace everywhere, dashed rules, amber selection", "Rows read like a process list: state, task and last activity", "Chats read like a log; your lines are marked “you ›”", "Approvals show the exact command"],
    motion: ["Steps instead of curves: wipes and line-by-line reveals", "A braille spinner for working bots, a blinking LED for waiting", "A faint amber scan passes when you switch screens", "The selection block jumps instead of gliding"],
    type: "Monospace (Cascadia here, JetBrains Mono in production)", filter: "Inverse-video tabs with counts",
    swatches: ["#0b0c0b", "#e7ece5", "#f2b445", "#7fc882", "#82b3ff"], cost: "Low to medium. One bundled mono font; denser lists." },
};
const ORDER = ["hush", "folio", "aurora", "sticker", "console"];

// ---------------------------------------------------------------- Springs, sampled into CSS linear() easings
function spring(zeta, omega) {
  const wd = omega * Math.sqrt(Math.max(0.000001, 1 - zeta * zeta));
  const x = (t) => 1 - Math.exp(-zeta * omega * t) * (Math.cos(wd * t) + ((zeta * omega) / wd) * Math.sin(wd * t));
  let settle = 0;
  for (let t = 0; t < 3; t += 0.004) if (Math.abs(x(t) - 1) > 0.003) settle = t;
  const end = settle + 0.02;
  const points = [];
  for (let i = 0; i <= 48; i++) points.push(Number(x((end * i) / 48).toFixed(4)));
  points[48] = 1;
  return { easing: `linear(${points.join(", ")})`, ms: `${Math.round(end * 1000)}ms` };
}
for (const [name, zeta, omega] of [["soft", 0.82, 15], ["pill", 0.6, 17], ["bouncy", 0.48, 18], ["firm", 0.8, 21]]) {
  const s = spring(zeta, omega);
  document.documentElement.style.setProperty(`--spring-${name}`, s.easing);
  document.documentElement.style.setProperty(`--spring-${name}-dur`, s.ms);
}

// ---------------------------------------------------------------- Shared pieces
function status(b) {
  const glyph = b.state === "paused" ? ic("pause") : b.state === "limited" ? ic("hourglass") : "";
  return `<span class="st st-${b.state}"><span class="st-ind" aria-hidden="true"><i class="g-dot"></i><i class="g-led"></i><i class="g-dots"><i></i><i></i><i></i></i><i class="g-braille"><b>${BRAILLE}</b></i>${ic("loader", "g-loader")}${glyph}</span><span class="st-label">${STATE_LABEL[b.state]}</span>${b.state === "waiting" ? PEN : ""}</span>`;
}
const row = (b, i) => `<li class="row stg" style="--i:${i + 2};--c:${b.color}" data-state="${b.state}">
  <a class="row-link" href="#" data-act="chat" data-bot="${b.id}">${avw(b, i)}<span class="row-main"><span class="row-name">${b.name}</span><span class="row-line"><span class="row-st">${b.state === "idle" ? "" : status(b)}</span><span class="row-task">${b.task}</span></span></span><span class="row-age">${b.age}</span></a>
  <button class="ib row-more" data-act="menu" data-menu="row" data-bot="${b.id}" aria-haspopup="menu" aria-expanded="false" aria-label="Menu for ${b.name}">${ic("ellipsis")}</button></li>`;

function summary() {
  const parts = [`${BOTS.length} bots`];
  if (count("working")) parts.push(`${count("working")} working`);
  if (count("waiting")) parts.push(`${count("waiting")} waiting for you`);
  return parts.join(" · ");
}
function lede() {
  const waiting = BOTS.filter((b) => b.state === "waiting").map((b) => `<b>${b.name}</b>`);
  const working = BOTS.filter((b) => b.state === "working").map((b) => b.name);
  const first = waiting.length ? `${waiting.join(" and ")} ${waiting.length > 1 ? "are" : "is"} waiting for you. ` : "";
  const second = working.length ? `${working.join(" and ")} ${working.length > 1 ? "are" : "is"} working.` : "Everyone else is quiet.";
  return first + second;
}

function msg(m, d) {
  if (m.k === "day") return `<p class="day">${m.t}</p>`;
  if (m.k === "user") return `<div class="msg msg-user${m.fresh ? " new-msg" : ""}"><div class="bubble"><p>${m.html}</p></div><time>${m.t}</time></div>`;
  if (m.k === "bot") { const b = bot(m.b); return `<div class="msg msg-bot${m.fresh ? " new-msg" : ""}" style="--c:${b.color}">${avw(b)}<div class="msg-body"><div class="msg-meta"><b>${b.name}</b><time>${m.t}</time></div><div class="bubble"><p>${m.html}</p></div></div></div>`; }
  if (m.k === "typing") { const b = bot(m.b); return `<div class="msg msg-bot new-msg" data-typing style="--c:${b.color}">${avw(b)}<div class="msg-body"><div class="msg-meta"><b>${b.name}</b></div><div class="bubble typing" aria-label="${b.name} is typing"><i></i><i></i><i></i></div></div></div>`; }
  if (m.k === "handoff") { const a = bot(m.a), b = bot(m.b); return `<div class="handoff"><span class="pair">${avw(a)}${avw(b)}</span><span><b>${a.name}</b> asked <b>${b.name}</b></span><button class="link" data-act="noop">Open exchange</button></div>`; }
  if (m.k === "notice") return `<div class="notice">${ic("circle-alert")}<span>Tjommie restarted while this was running.</span><button class="btn" data-act="retry">${ic("rotate-ccw")}Retry</button></div>`;
  if (m.k === "act") return `<button class="act" data-act="steps" aria-expanded="false">${ic("list-checks")}<span>${m.summary}</span>${ic("chevron-down")}</button><ul class="act-steps" hidden>${m.steps.map((s) => `<li>${ic("check")}${s}</li>`).join("")}</ul>`;
  if (m.k === "appr") return d.approval === "pending" ? `<div class="appr" role="group" aria-label="Waiting for you"><div class="appr-head">${ic("circle-alert")}Waiting for you</div><p class="appr-sum">${APPROVAL.summary}</p><p class="appr-detail">${APPROVAL.detail}</p><div class="appr-actions"><button class="btn btn-primary" data-act="allow">Allow once</button><button class="btn" data-act="allow">Always allow</button><button class="btn" data-act="deny">Deny</button></div><p class="appr-rule">${APPROVAL.rule}</p></div>`
    : `<div class="answered">${ic(d.approval === "denied" ? "x" : "check")}<span>${d.approval === "denied" ? "Denied" : "Allowed once"}: ${APPROVAL.summary}</span></div>`;
  return "";
}

const composer = (kind, b) => `<div class="composer" data-kind="${kind}">
  ${kind === "chat" ? `<button class="ib" data-act="noop" aria-label="Attach files">${ic("paperclip")}</button>` : `<button class="ib" data-act="mention" aria-label="Mention a bot">${ic("at-sign")}</button>`}
  <textarea class="comp-field" rows="1" placeholder="${kind === "chat" ? `Message ${b.name}` : "Message the Team room"}" aria-label="${kind === "chat" ? `Message ${b.name}` : "Message the Team room"}"></textarea>
  <button class="comp-send" data-act="send" aria-label="Send">${ic("arrow-up")}</button></div>`;

// ---------------------------------------------------------------- Screens
const VIEWS = {
  agents(p) {
    const shown = BOTS.filter((b) => p.filter === "all" || b.state === p.filter).filter((b) => !p.query || b.name.toLowerCase().includes(p.query));
    const chips = FILTERS.filter(([f]) => f === "all" || count(f) > 0 || p.filter === f)
      .map(([f, label]) => `<button class="chip" data-act="filter" data-f="${f}" aria-pressed="${p.filter === f}"><span>${label}</span><span class="chip-n">${count(f)}</span></button>`).join("");
    const label = FILTERS.find(([f]) => f === p.filter)[1];
    const waiting = BOTS.filter((b) => b.state === "waiting");
    const focus = p.dir === "aurora" && waiting.length && (p.filter === "all" || p.filter === "waiting") && !p.query
      ? `<section class="focus stg" style="--i:2">${avw(waiting[0])}<h2>${waiting[0].name} needs you</h2><p>${waiting[0].task}</p><button class="btn btn-primary" data-act="chat" data-bot="${waiting[0].id}">Review</button></section>` : "";
    return `<div class="bar"><div class="bar-title">${p.dir === "console" ? "<i>tjommie/</i>agents" : "Agents"}</div>
      <button class="ib" data-act="search" aria-label="Search bots">${ic("search")}</button><button class="ib ib-new" data-act="noop" aria-label="New bot">${ic("plus")}</button></div>
      <div class="scroll">
        <header class="hero stg" style="--i:0"><p class="hero-date">Wednesday 22 October</p><h1 class="hero-title">Agents</h1><p class="hero-sub">${summary()}</p><p class="hero-lede">${lede()}</p></header>
        ${p.query ? `<p class="hero-sub" style="padding:8px 20px 0">Showing bots matching “${esc(p.query)}”</p>` : `<div class="chips stg" role="group" aria-label="Show" style="--i:1">${chips}<i class="chips-ind" aria-hidden="true"></i></div>`}
        <button class="lens stg" style="--i:1" data-act="menu" data-menu="filter" aria-haspopup="menu" aria-expanded="false">Show <b>${label === "All" ? "all agents" : label.toLowerCase()}</b> · ${count(p.filter)}${ic("chevron-down")}</button>
        ${focus}
        <ul class="agents">${shown.map(row).join("")}</ul>
        ${shown.length ? "" : `<p class="empty">No bots match.</p>`}
      </div>`;
  },
  team(p) {
    const working = count("working");
    return `<div class="bar"><button class="bar-id" data-act="sheet" data-sheet="members" aria-label="Team room, ${BOTS.length} bots, ${working} working. Show members">${teamMark()}<span class="bar-id-text"><span class="bar-id-name">Team room</span><span class="bar-id-sub">${BOTS.length} bots · ${working} working</span></span></button>
      <button class="ib bar-more" data-act="menu" data-menu="team" aria-haspopup="menu" aria-expanded="false" aria-label="More">${ic("ellipsis")}</button></div>
      <div class="scroll msgs">${p.team.map((m) => msg(m, p)).join("")}</div>
      <div class="dock">${composer("team")}</div>`;
  },
  chat(p) {
    const b = { ...bot("night"), state: p.chatState };
    return `<div class="bar"><button class="ib" data-act="back" aria-label="Back to Agents">${ic("chevron-left")}</button>
      <button class="bar-id" data-act="sheet" data-sheet="details" aria-label="${b.name}, ${STATE_LABEL[b.state]}. Details" style="--c:${b.color}">${avw(b)}<span class="bar-id-text"><span class="bar-id-name">${b.name}</span>${status(b)}</span></button>
      <button class="ib bar-more" data-act="menu" data-menu="chat" aria-haspopup="menu" aria-expanded="false" aria-label="More">${ic("ellipsis")}</button></div>
      <div class="scroll msgs">${p.chat.map((m) => msg(m, p)).join("")}</div>
      <div class="dock">${composer("chat", b)}</div>`;
  },
  settings(p) {
    const rows = [["sliders-horizontal", "General", `Appearance: ${p.o.theme === "dark" ? "Dark" : "Light"}`, "noop"], ["bot", "Bots", "Bots can ask each other 3 times per chain", "noop"], ["key-round", "Accounts", "Claude Max · Codex Pro", "noop"], ["chart-column", "Usage", "This week: 3.8M tokens", "usage"]];
    return `<div class="bar"><div class="bar-title">${p.dir === "console" ? "<i>tjommie/</i>settings" : "Settings"}</div></div>
      <div class="scroll"><header class="hero stg" style="--i:0"><h1 class="hero-title">Settings</h1></header>
      <ul class="set-list">${rows.map(([icon, t, s, act], i) => `<li class="stg" style="--i:${i + 1}"><a class="set-row" href="#" data-act="${act}"><span class="set-ic">${ic(icon)}</span><span class="set-main"><b>${t}</b><span>${s}</span></span>${ic("chevron-right")}</a></li>`).join("")}</ul>
      <p class="set-foot stg" style="--i:6">Sample settings. Accounts and usage figures are made up for this preview.</p></div>`;
  },
  usage(p) {
    const ranges = [["today", "Today"], ["week", "This week"], ["month", "This month"], ["year", "This year"]];
    const lim = (label, value, v, i) => `<div class="lim"><div class="lim-top"><span>${label}</span><span>${value}</span></div><div class="meter"><i style="--v:${v};--i:${i}"></i></div></div>`;
    const by = [["night", "$1.84", 0.45], ["notes", "$1.10", 0.27], ["inbox", "$0.86", 0.21], ["research", "$0.32", 0.08]];
    return `<div class="bar"><button class="bar-back" data-act="back" aria-label="Back to Settings">${ic("chevron-left")}<span>Settings</span></button><div class="bar-title">${p.dir === "console" ? "<i>settings/</i>usage" : "Usage"}</div></div>
      <div class="scroll"><header class="hero stg" style="--i:0"><h1 class="hero-title">Usage</h1></header>
      <div class="chips range stg" role="radiogroup" aria-label="Range" style="--i:1">${ranges.map(([r, l]) => `<button class="chip" role="radio" data-act="range" data-r="${r}" aria-pressed="${p.range === r}" aria-checked="${p.range === r}">${l}</button>`).join("")}</div>
      <div class="sec-head stg" style="--i:2"><h2>Subscriptions</h2><button class="ib" data-act="refresh" aria-label="Refresh all limits">${ic("refresh-cw")}</button></div>
      <article class="card prov stg" style="--i:3"><div class="prov-head"><b>Claude Code</b><span class="tag">Max 20x</span><button class="ib" data-act="refresh" aria-label="Refresh Claude Code limits">${ic("refresh-cw")}</button></div>
        ${lim("Session", "12% · resets 10:40 PM", 0.12, 0)}${lim("Weekly", "31% · resets Wed 11:00 PM", 0.31, 1)}${lim("Weekly · Opus", "8% · resets Wed 11:00 PM", 0.08, 2)}
        <div class="lim-top" style="padding:8px 0 2px"><span>Extra usage</span><span>Off</span></div><p class="acct">Claude · you@example.com</p></article>
      <article class="card prov stg" style="--i:4"><div class="prov-head"><b>Codex</b><span class="tag">Pro</span><button class="ib" data-act="refresh" aria-label="Refresh Codex limits">${ic("refresh-cw")}</button></div>
        ${lim("Weekly", "4% · resets 29 October 8:30 PM", 0.04, 3)}<div class="lim-top" style="padding:8px 0 2px"><span>Credits</span><span>1,240 left</span></div><p class="acct">Codex · you@example.com</p></article>
      <div class="tiles stg" style="--i:5"><div class="tile"><span>Tokens</span><b>3.8M</b></div><div class="tile"><span>Combined cost estimate</span><b>$4.12</b><small>At least. 35 turns have no price.</small></div>
        <div class="tile"><span>Spent (API keys)</span><b>$0.00</b></div><div class="tile"><span>API value (subscriptions)</span><b>$4.12</b><small>API-equivalent value, not your bill</small></div></div>
      <div class="sec-head stg" style="--i:6"><h2>By bot</h2></div>
      <ul class="brk stg" style="--i:7">${by.map(([id, v, w], i) => `<li>${avw(bot(id))}<span class="nm">${bot(id).name}</span><span class="val">${v}</span><div class="meter"><i style="--v:${w};--i:${i + 4}"></i></div></li>`).join("")}</ul></div>`;
  },
};
const TAB_OF = { agents: "agents", team: "team", chat: null, settings: "settings", usage: "settings" };

const MENUS = {
  row(p, t) { const b = bot(t.dataset.bot); return item("message-circle", "Open chat", "chat", `data-bot="${b.id}"`) + item(b.state === "paused" ? "play" : "pause", b.state === "paused" ? "Resume bot" : "Pause bot") + item("settings", "Bot settings"); },
  chat() { return item("info", "Details", "sheet", `data-sheet="details"`) + item("pause", "Pause bot") + item("settings", "Bot settings"); },
  team() { return item("users", "Members", "sheet", `data-sheet="members"`) + item("settings", "Bots settings"); },
  filter(p) { return FILTERS.map(([f, l]) => `<button class="menu-item" role="menuitemradio" aria-checked="${p.filter === f}" data-act="filter" data-f="${f}"><span>${l}</span><b>${count(f)}</b>${ic("check", "chk")}</button>`).join(""); },
};
const item = (icon, label, act = "close", extra = "") => `<button class="menu-item" role="menuitem" data-act="${act}" ${extra}>${ic(icon)}<span>${label}</span></button>`;

const SHEETS = {
  members: () => ["Members", `<ul class="members">${BOTS.map((b) => `<li><a href="#" data-act="chat" data-bot="${b.id}">${avw(b)}<span class="nm">${b.name}</span>${status(b)}</a></li>`).join("")}</ul>
    <p class="note">${ic("info")}<span>Bots can ask each other up to 3 times per chain. <button class="link" data-act="noop" style="padding:0">Change</button></span></p>`],
  // Everything today's bot panel has: summary (account · model, Today, limit, autonomy, folder), Browser, Routines with Add, Recent activity.
  // A client without the desktop bridge gets the honest Browser card; with the bridge the sheet keeps the native Details/Browser pane.
  details: (p) => { const b = { ...bot("night"), state: p.chatState }; return ["Details", `<section class="det-sum">${avw(b)}<div class="det-main"><div class="det-top"><b class="det-name">${b.name}</b>${b.state === "idle" ? "" : `<span class="tag">${STATE_LABEL[b.state]}</span>`}</div>
    <ul class="det-rows"><li>${ic("sparkles")}<span>Claude Max · Claude · Sonnet (latest)</span></li><li>${ic("chart-column")}<a href="#" data-act="noop">Today: 212K tokens · API value at least $0.84</a></li><li>${ic("gauge")}<span>31% of Weekly limit, resets Wed 11:00 PM</span></li><li>${ic("shield")}<span>Always ask: asks before every change</span></li><li>${ic("folder")}<span>Home folder</span></li></ul></div></section>
    <div class="pc-card">${ic("monitor")}<div><b>Browser</b><span>Runs on your PC. Open Tjommie on that PC to watch ${b.name} or take over.</span></div></div>
    <div class="sec-row"><h3 class="sub-h">Routines</h3><button class="link" data-act="noop" aria-label="Add a routine">Add</button></div>
    <ul class="routines"><li>${ic("clock")}<div><b>Nightly checklist</b><span>Every day at 2:00 AM</span></div></li><li>${ic("clock")}<div><b>Release dry run</b><span>Fridays at 4:00 PM</span></div></li></ul>
    <h3 class="sub-h">Recent activity</h3><ul class="steps">${CHAT[2].steps.slice().reverse().map((s, i) => `<li>${ic("check")}${s}<time>6:${30 - i * 2} PM</time></li>`).join("")}</ul>`]; },
};

const IME = `<div class="ime" aria-hidden="true"><div class="ime-sug"><span>the</span><span>I</span><span>it’s</span></div><div class="ime-row">${"<i></i>".repeat(10)}</div><div class="ime-row">${"<i></i>".repeat(9)}</div><div class="ime-row"><i class="w"></i>${"<i></i>".repeat(7)}<i class="w"></i></div><div class="ime-row"><i class="w"></i><i></i><i class="sp"></i><i></i><i class="w"></i></div></div>`;
const SYS_TOP = `<div class="sys-top" aria-hidden="true"><span>6:42</span><span class="sys-cam"></span><span class="sys-icons">
  <svg viewBox="0 0 16 16"><path d="M8 13.8 5.7 11.4a3.3 3.3 0 0 1 4.6 0z"/><path d="M3.4 9.1a6.5 6.5 0 0 1 9.2 0l-1.25 1.25a4.7 4.7 0 0 0-6.7 0z"/><path d="M1.1 6.8a9.8 9.8 0 0 1 13.8 0l-1.25 1.25a8 8 0 0 0-11.3 0z"/></svg>
  <svg viewBox="0 0 16 16"><rect x="1" y="10" width="2.6" height="5" rx="1"/><rect x="5" y="7.3" width="2.6" height="7.7" rx="1"/><rect x="9" y="4.6" width="2.6" height="10.4" rx="1"/><rect x="13" y="1.8" width="2.6" height="13.2" rx="1" opacity=".35"/></svg>
  <svg viewBox="0 0 27 16" style="width:25px"><rect x="1" y="3" width="21.5" height="10" rx="3" fill="none" stroke="currentColor" stroke-width="1.3" opacity=".55"/><rect x="2.9" y="4.9" width="14" height="6.2" rx="1.6"/><rect x="23.6" y="6" width="2" height="4" rx="1" opacity=".55"/></svg></span></div>`;

// ---------------------------------------------------------------- One interactive phone
class Phone {
  constructor(o) {
    this.o = { dir: "hush", theme: "dark", device: "s25", screen: "agents", st: "default", bars: "after", ...o };
    this.dir = this.o.dir; this.filter = "all"; this.query = ""; this.range = "week";
    this.approval = "pending"; this.chatState = "waiting";
    this.team = TEAM.map((m) => ({ ...m })); this.chat = CHAT.map((m) => ({ ...m }));
    this.el = document.createElement("div");
    this.el.className = "phone";
    Object.assign(this.el.dataset, { dir: this.o.dir, theme: this.o.theme, device: this.o.device, bars: this.o.bars });
    const blobs = this.dir === "aurora" ? `<i class="blob b1 hot" style="--c:#EC712E"></i><i class="blob b2" style="--c:#EC4698"></i><i class="blob b3" style="--c:#5BC57A"></i><i class="blob b4" style="--c:#3B82F6"></i>` : "";
    const tabs = [["agents", "layout-grid", "Agents", 1], ["team", "users", "Team", 2], ["settings", "settings", "Settings", 3]];
    this.el.innerHTML = `<div class="screen"><div class="backdrop">${blobs}</div><div class="views"></div>
      <nav class="pill" aria-label="Tjommie"><i class="pill-ind" aria-hidden="true"></i>${tabs.map(([id, icon, label, key]) => `<button class="pill-tab" data-act="tab" data-tab="${id}" aria-label="${id === "agents" ? `Agents, ${count("waiting")} waiting for you` : id === "team" ? "Team room" : label}">${ic(icon)}<span data-key="${key}">${label}</span>${id === "agents" && count("waiting") ? `<i class="pill-dot" aria-hidden="true"></i>` : ""}</button>`).join("")}</nav>
      <div class="layer"></div>${IME}${SYS_TOP}<div class="sys-bottom" aria-hidden="true"><i></i></div></div>`;
    this.screen = this.el.querySelector(".screen");
    this.views = this.el.querySelector(".views");
    this.pill = this.el.querySelector(".pill");
    this.layer = this.el.querySelector(".layer");
    this.bind();
  }
  mount(host) {
    host.append(this.el);
    this.go(this.o.screen, "none");
    requestAnimationFrame(() => requestAnimationFrame(() => this.applyState(this.o.st)));
    return this;
  }
  get w() { return this.screen.offsetWidth; }

  go(name, how = "tab") {
    this.closeOverlay(true);
    this.setKb(false);
    const prev = this.view;
    const v = document.createElement("div");
    v.className = `v v-${name}`;
    this.name = name;
    v.innerHTML = VIEWS[name](this);
    this.views.append(v);
    if (how !== "none") v.classList.add("anim", `in-${how}`);
    if (prev) { prev.classList.add("out"); setTimeout(() => prev.remove(), 320); }
    this.view = v;
    this.pill.classList.toggle("off", !TAB_OF[name]);
    if (TAB_OF[name]) this.setTab(TAB_OF[name], how !== "none");
    const scroll = v.querySelector(".scroll");
    if (name === "team" || name === "chat") scroll.scrollTop = scroll.scrollHeight;
    scroll.addEventListener("scroll", () => this.onScroll(), { passive: true });
    this.onScroll();
    this.placeUnderline(false);
    this.markOverflow();
    if (this.dir === "console" && how !== "none") { const s = document.createElement("i"); s.className = "scanline"; this.screen.append(s); setTimeout(() => s.remove(), 450); }
  }
  onScroll() {
    const scroll = this.view.querySelector(".scroll");
    this.view.classList.toggle("scrolled", scroll.scrollTop > 4);
    this.view.classList.toggle("titled", scroll.scrollTop > 46);
  }
  setTab(tab, animate) {
    const tabs = [...this.pill.querySelectorAll(".pill-tab")];
    const el = tabs.find((t) => t.dataset.tab === tab);
    tabs.forEach((t) => (t === el ? t.setAttribute("aria-current", "page") : t.removeAttribute("aria-current")));
    const left = el.offsetLeft, right = this.pill.clientWidth - el.offsetLeft - el.offsetWidth;
    const old = parseFloat(this.pill.style.getPropertyValue("--ind-l") || left);
    const lag = this.dir === "aurora" ? 90 : this.dir === "sticker" ? 55 : 0;
    this.pill.style.setProperty("--dl", `${left > old ? lag : 0}ms`);
    this.pill.style.setProperty("--dr", `${left < old ? lag : 0}ms`);
    if (!animate) this.pill.querySelector(".pill-ind").style.transition = "none";
    this.pill.style.setProperty("--ind-l", `${left}px`);
    this.pill.style.setProperty("--ind-r", `${right}px`);
    if (!animate) requestAnimationFrame(() => (this.pill.querySelector(".pill-ind").style.transition = ""));
  }
  placeUnderline(animate = true) {
    const chips = this.view && this.view.querySelector(".chips:not(.range)");
    if (!chips) return;
    const on = chips.querySelector('[aria-pressed="true"]');
    const ind = chips.querySelector(".chips-ind");
    if (!on || !ind) return;
    if (!animate) ind.style.transition = "none";
    const first = on.firstElementChild, last = on.lastElementChild;
    chips.style.setProperty("--ul", `${on.offsetLeft + first.offsetLeft}px`);
    chips.style.setProperty("--uw", `${last.offsetLeft + last.offsetWidth - first.offsetLeft}px`);
    if (!animate) requestAnimationFrame(() => (ind.style.transition = ""));
  }
  markOverflow() {
    requestAnimationFrame(() => this.view && this.view.querySelectorAll(".chips").forEach((c) => c.classList.toggle("overflow", c.scrollWidth > c.clientWidth + 1)));
  }
  rerenderAgents() {
    const v = this.view;
    const fresh = document.createElement("div");
    fresh.innerHTML = VIEWS.agents(this);
    v.classList.add("anim");
    v.classList.remove("in-tab", "in-push", "in-pop");
    const keepChips = v.querySelector(".chips");
    const nextChips = fresh.querySelector(".chips");
    if (keepChips && nextChips) {
      keepChips.querySelectorAll(".chip").forEach((c) => c.setAttribute("aria-pressed", String(c.dataset.f === this.filter)));
      keepChips.classList.remove("stg");
    }
    for (const sel of [".agents", ".lens", ".focus", ".empty"]) {
      const a = v.querySelector(sel), b = fresh.querySelector(sel);
      if (a && b) a.replaceWith(b); else if (a) a.remove(); else if (b) v.querySelector(".agents").before(b);
    }
    v.querySelectorAll(".hero, .chips").forEach((e) => e.classList.remove("stg"));
    this.placeUnderline(true);
  }

  // Overlays: menus anchor to their trigger; sheets rise from the bottom; Back or Escape closes them.
  rel(el) {
    const s = this.screen.getBoundingClientRect(), r = el.getBoundingClientRect(), k = s.width / this.screen.offsetWidth;
    return { x: (r.left - s.left) / k, y: (r.top - s.top) / k, w: r.width / k, h: r.height / k };
  }
  openMenu(kind, trigger) {
    this.closeOverlay(true);
    const r = this.rel(trigger);
    const leftSide = r.x + r.w / 2 < this.w / 2;
    const pos = leftSide ? `left:${Math.max(8, r.x)}px;transform-origin:top left` : `right:${Math.max(8, this.w - r.x - r.w)}px`;
    this.layer.innerHTML = `<div class="scrim clear" data-act="close"></div><div class="menu" role="menu" style="top:${r.y + r.h + 6}px;${pos}">${MENUS[kind](this, trigger)}</div>`;
    trigger.setAttribute("aria-expanded", "true");
    this.trigger = trigger;
  }
  openSheet(kind) {
    this.closeOverlay(true);
    const [title, body] = SHEETS[kind](this);
    this.layer.innerHTML = `<div class="scrim" data-act="close"></div><aside class="sheet" aria-label="${title}"><div class="sheet-grip"></div><div class="sheet-head"><h2 class="sheet-title">${title}</h2><button class="ib" data-act="close" aria-label="Close">${ic("x")}</button></div><div class="sheet-body">${body}</div></aside>`;
  }
  closeOverlay(now = false) {
    if (this.trigger) { this.trigger.setAttribute("aria-expanded", "false"); this.trigger = null; }
    if (!this.layer.children.length) return;
    if (now) { this.layer.innerHTML = ""; return; }
    [...this.layer.children].forEach((c) => c.classList.add("leaving"));
    clearTimeout(this.leaveTimer);
    this.leaveTimer = setTimeout(() => (this.layer.innerHTML = ""), 210);
  }
  setKb(on) {
    this.screen.classList.toggle("kb", on);
    const pop = this.view && this.view.querySelector(".mention-pop");
    if (!on && pop) pop.remove();
  }
  mentionPop(show) {
    const comp = this.view.querySelector(".composer");
    const old = comp.parentElement.querySelector(".mention-pop");
    if (old) old.remove();
    if (!show) return;
    comp.insertAdjacentHTML("beforebegin", `<div class="mention-pop" role="listbox" aria-label="Mention a bot">${BOTS.slice(0, 4).map((b) => `<button role="option" data-act="pick" data-bot="${b.id}">${avw(b)}<span>${b.name}</span><small>${STATE_LABEL[b.state].split(" ")[0]}</small></button>`).join("")}</div>`);
  }

  addMsg(list, m) {
    const scroll = this.view.querySelector(".scroll");
    (list === "team" ? this.team : this.chat).push(m);
    scroll.insertAdjacentHTML("beforeend", msg({ ...m, fresh: true }, this));
    scroll.scrollTo({ top: scroll.scrollHeight, behavior: document.documentElement.dataset.motion === "on" ? "smooth" : "auto" });
  }
  send() {
    const field = this.view.querySelector(".comp-field");
    const text = field.value.trim();
    if (!text) return;
    field.value = "";
    this.view.querySelector(".composer").classList.remove("has-text");
    this.mentionPop(false);
    const list = this.name === "team" ? "team" : "chat";
    this.addMsg(list, { k: "user", t: clock(), html: esc(text) });
    const who = list === "team" ? "inbox" : "night";
    setTimeout(() => this.view.querySelector(".scroll") && this.addMsg(list, { k: "typing", b: who }), 500);
    setTimeout(() => {
      const scroll = this.view.querySelector(".scroll");
      const typing = scroll && scroll.querySelector("[data-typing]");
      if (!typing) return;
      typing.remove();
      (list === "team" ? this.team : this.chat).pop();
      this.addMsg(list, { k: "bot", b: who, t: clock(), html: list === "team" ? "Noted. I'll keep an eye on replies about it." : "Got it. I'll hold the publish until you say so." });
    }, 1700);
  }
  answer(decision, instant = false) {
    this.approval = decision;
    const card = this.view.querySelector(".appr");
    if (!card) return;
    card.outerHTML = msg({ k: "appr" }, this).replace('class="answered"', `class="answered${instant ? "" : " new-msg"}"`);
    if (decision === "denied") return;
    this.chatState = "working";
    const id = this.view.querySelector(".bar-id");
    id.querySelector(".st").outerHTML = status({ ...bot("night"), state: "working" });
    id.setAttribute("aria-label", "Night Shift, Working. Details");
    const scroll = this.view.querySelector(".scroll");
    scroll.insertAdjacentHTML("beforeend", `<div class="live${instant ? "" : " new-msg"}">${status({ ...bot("night"), state: "working" })}<span class="code">npm publish --dry-run</span></div>`);
    scroll.scrollTo({ top: scroll.scrollHeight });
    if (instant) return;
    setTimeout(() => {
      const live = this.view.querySelector(".live");
      if (!live) return;
      live.remove();
      this.chatState = "waiting";
      this.addMsg("chat", { k: "bot", b: "night", t: clock(), html: "Dry run passed: 41 files, 312 kB. Say the word and I'll publish for real." });
    }, 2600);
  }

  bind() {
    this.screen.addEventListener("click", (e) => {
      const t = e.target.closest("[data-act]");
      if (!t || !this.screen.contains(t)) return;
      e.preventDefault();
      const a = t.dataset.act;
      if (a === "tab") { if (TAB_OF[this.name] !== t.dataset.tab) this.go(t.dataset.tab, "tab"); else this.view.querySelector(".scroll").scrollTo({ top: 0, behavior: "smooth" }); }
      else if (a === "chat") { this.closeOverlay(true); this.from = this.name; this.go("chat", "push"); }
      else if (a === "back") this.go(this.name === "usage" ? "settings" : this.from && this.from !== "chat" ? this.from : "agents", "pop");
      else if (a === "usage") this.go("usage", "push");
      else if (a === "menu") { if (this.trigger === t) this.closeOverlay(); else this.openMenu(t.dataset.menu, t); }
      else if (a === "sheet") this.openSheet(t.dataset.sheet);
      else if (a === "close") this.closeOverlay();
      else if (a === "filter") { this.filter = t.dataset.f; this.closeOverlay(); this.rerenderAgents(); }
      else if (a === "range") { this.range = t.dataset.r; this.view.querySelectorAll(".range .chip").forEach((c) => { c.setAttribute("aria-pressed", String(c === t)); c.setAttribute("aria-checked", String(c === t)); }); }
      else if (a === "allow") this.answer("allowed");
      else if (a === "deny") this.answer("denied");
      else if (a === "steps") { const open = t.getAttribute("aria-expanded") !== "true"; t.setAttribute("aria-expanded", String(open)); t.nextElementSibling.hidden = !open; }
      else if (a === "retry") { t.classList.add("spinning"); setTimeout(() => { const n = t.closest(".notice"); if (n) n.outerHTML = `<div class="answered new-msg">${ic("check")}<span>Retried. Night Shift picked the run back up.</span></div>`; }, 1100); }
      else if (a === "refresh") { const svg = t.querySelector(".i"); svg.animate([{ transform: "rotate(0)" }, { transform: "rotate(360deg)" }], { duration: 700, easing: "cubic-bezier(.65,0,.35,1)" }); }
      else if (a === "send") this.send();
      else if (a === "mention") { const f = this.view.querySelector(".comp-field"); f.focus(); f.value += (f.value && !f.value.endsWith(" ") ? " " : "") + "@"; this.view.querySelector(".composer").classList.add("has-text"); this.mentionPop(true); }
      else if (a === "pick") { const f = this.view.querySelector(".comp-field"); f.value = f.value.replace(/@$/, "") + `@${bot(t.dataset.bot).name} `; this.mentionPop(false); f.focus(); }
      else if (a === "search") this.toggleSearch(true);
    });
    this.screen.addEventListener("input", (e) => {
      if (e.target.matches(".comp-field")) {
        const v = e.target.value;
        this.view.querySelector(".composer").classList.toggle("has-text", v.trim().length > 0);
        if (this.name === "team") this.mentionPop(v.endsWith("@"));
      }
      if (e.target.matches(".bar-search")) { this.query = e.target.value.trim().toLowerCase(); this.rerenderAgents(); }
    });
    this.screen.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && !e.shiftKey && e.target.matches(".comp-field")) { e.preventDefault(); this.send(); }
      if (e.key === "Escape") { if (this.layer.children.length) this.closeOverlay(); else if (e.target.matches(".bar-search")) this.toggleSearch(false); else e.target.blur(); }
    });
    this.screen.addEventListener("focusin", (e) => { if (e.target.matches(".comp-field")) this.setKb(true); });
    this.screen.addEventListener("focusout", (e) => {
      if (e.target.matches(".comp-field")) setTimeout(() => { if (!this.screen.contains(document.activeElement) || !document.activeElement.matches(".comp-field, .mention-pop button, .comp-send")) this.setKb(false); }, 150);
      if (e.target.matches(".bar-search") && !e.target.value) setTimeout(() => this.toggleSearch(false), 100);
    });
  }
  toggleSearch(on) {
    const bar = this.view.querySelector(".bar");
    const existing = bar.querySelector(".bar-search");
    if (on && !existing) {
      bar.insertAdjacentHTML("afterbegin", `<input class="bar-search" type="search" placeholder="Search bots" aria-label="Search bots">`);
      bar.classList.add("searching");
      bar.querySelector(".bar-search").focus();
    } else if (!on && existing) {
      existing.remove(); bar.classList.remove("searching"); this.query = ""; this.rerenderAgents();
    }
  }

  // Static states for review and screenshots.
  applyState(st) {
    const v = this.view;
    if (st === "filter") { if (this.dir === "aurora") this.openMenu("filter", v.querySelector(".lens")); else { this.filter = "working"; this.rerenderAgents(); } }
    if (st === "rowmenu") this.openMenu("row", v.querySelector('.row-more[data-bot="night"]'));
    if (st === "menu") this.openMenu(this.name, v.querySelector(".bar-more"));
    if (st === "members") this.openSheet("members");
    if (st === "details") this.openSheet("details");
    if (st === "allowed") this.answer("allowed", true);
    if (st === "keyboard") {
      this.setKb(true);
      const f = v.querySelector(".comp-field");
      f.value = this.name === "team" ? "Thanks, all. @" : "Looks good. Go ahead with the dry run";
      v.querySelector(".composer").classList.add("has-text");
      if (this.name === "team") this.mentionPop(true);
      const scroll = v.querySelector(".scroll");
      setTimeout(() => (scroll.scrollTop = scroll.scrollHeight), 300);
    }
  }
}

// ---------------------------------------------------------------- Presenter
const STATES = {
  agents: [["default", "Default"], ["filter", "Filter in use"], ["rowmenu", "Row menu"]],
  team: [["default", "Default"], ["menu", "⋯ menu"], ["members", "Members sheet"], ["keyboard", "Keyboard"]],
  chat: [["default", "Approval"], ["allowed", "After Allow once"], ["menu", "⋯ menu"], ["details", "Details sheet"], ["keyboard", "Keyboard"]],
  settings: [["default", "Default"]],
  usage: [["default", "Default"]],
};
const SCREENS = [["agents", "Agents", "a"], ["team", "Team room", "t"], ["chat", "Chat", "c"], ["settings", "Settings", "s"], ["usage", "Usage", "u"]];
const REVIEW_STATES = [["agents", "filter", "Filter in use"], ["agents", "rowmenu", "Row menu"], ["team", "menu", "Team ⋯ menu"], ["team", "members", "Members sheet"], ["team", "keyboard", "Keyboard open"], ["chat", "menu", "Chat ⋯ menu"], ["chat", "details", "Details sheet"], ["chat", "keyboard", "Keyboard open"]];

const params = new URLSearchParams(location.search);
const P = {
  view: params.get("view") || "single", dir: params.get("dir") || "hush", theme: params.get("theme") || "dark", device: params.get("device") || "s25",
  screen: params.get("screen") || "agents", st: params.get("st") || "default", bars: params.get("bars") || "after",
  motion: params.get("motion") || (matchMedia("(prefers-reduced-motion: reduce)").matches ? "reduced" : "on"), fit: params.get("fit") !== "0",
};

function button(group, value, label, pressed) { return `<button class="pz-btn" data-g="${group}" data-v="${value}" aria-pressed="${pressed}">${label}</button>`; }
function controls() {
  const g = (name, items) => `<div class="pz-group"><b>${name}</b>${items}</div>`;
  const states = STATES[P.screen] || [["default", "Default"]];
  return `<div class="pz-brand">Tjommie mobile<small>Five directions · synthetic data</small></div>`
    + g("View", [["single", "One phone"], ["screens", "All screens"], ["states", "States"], ["compare", "Compare five"]].map(([v, l]) => button("view", v, l, P.view === v)).join(""))
    + g("Direction", ORDER.map((d) => button("dir", d, `<kbd>${DIRS[d].n}</kbd>${DIRS[d].name}`, P.dir === d)).join(""))
    + g("Theme", button("theme", "dark", "Dark", P.theme === "dark") + button("theme", "light", "Light", P.theme === "light"))
    + g("Size", button("device", "s25", "S25 · 411", P.device === "s25") + button("device", "narrow", "Narrow · 360", P.device === "narrow"))
    + g("Screen", SCREENS.map(([s, l]) => button("screen", s, l, P.screen === s)).join(""))
    + g("State", states.map(([s, l]) => button("st", s, l, P.st === s)).join(""))
    + g("Bars", button("bars", "after", "Canvas (new)", P.bars === "after") + button("bars", "before", "Grey (today)", P.bars === "before"))
    + g("Motion", [["on", "On"], ["reduced", "Reduced"], ["off", "Off"]].map(([m, l]) => button("motion", m, l, P.motion === m)).join("") + `<button class="pz-btn" data-g="replay">Replay</button>`);
}
function info(d) {
  const x = DIRS[d];
  return `<div class="pz-info"><h2>${x.n}. ${x.name}</h2><p class="pz-for">${x.for}</p><p class="pz-line">${x.line}</p>
    <div class="pz-swatches">${x.swatches.map((c) => `<i style="background:${c}"></i>`).join("")}</div>
    <h3>Look</h3><ul>${x.look.map((l) => `<li>${l}</li>`).join("")}</ul>
    <h3>Motion</h3><ul>${x.motion.map((l) => `<li>${l}</li>`).join("")}</ul>
    <h3>Type</h3><ul><li>${x.type}</li></ul>
    <h3>Agents filter</h3><ul><li>${x.filter}</li></ul>
    <h3>Build cost</h3><ul><li>${x.cost}</li></ul>
    <div class="pz-note">In every direction: the status and gesture bars are the app's own canvas, the nav pill floats with nothing behind it, header actions sit at fixed places (⋯ always at the far right), and filters are at least 44 px to tap. Tap anything in the phone: tabs, rows, ⋯, the title, Allow once, the composer (it raises a keyboard).</div></div>`;
}
const phones = [];
function phone(o, host) { const p = new Phone({ dir: P.dir, theme: P.theme, device: P.device, bars: P.bars, ...o }).mount(host); phones.push(p); return p; }
function render() {
  document.documentElement.dataset.motion = P.motion;
  document.body.classList.toggle("bare", params.get("bare") === "1");
  document.querySelector(".pz-top").innerHTML = controls();
  const main = document.querySelector(".pz-main");
  main.innerHTML = "";
  phones.length = 0;
  const cell = (label, host = main) => { const c = document.createElement("div"); c.className = "pz-cell"; c.innerHTML = `<p>${label}</p>`; host.append(c); return c; };
  if (P.view === "single") {
    const wrap = document.createElement("div"); wrap.className = "pz-single"; main.append(wrap);
    const stage = document.createElement("div"); stage.className = "pz-stage"; wrap.append(stage);
    phone({ screen: P.screen, st: P.st }, stage);
    wrap.insertAdjacentHTML("beforeend", info(P.dir));
    if (P.fit) stage.style.setProperty("--fit", Math.min(1, (innerHeight - document.querySelector(".pz-top").offsetHeight - 50) / (P.device === "s25" ? 909 : 818)).toFixed(3));
  } else {
    const title = document.createElement("p"); title.className = "pz-title";
    const row = document.createElement("div"); row.className = "pz-row";
    main.append(title, row);
    if (P.view === "screens") { title.innerHTML = `<b>${DIRS[P.dir].n}. ${DIRS[P.dir].name}</b> · ${DIRS[P.dir].for} · ${P.theme} · ${P.device === "s25" ? "411" : "360"} px`; SCREENS.forEach(([s, l]) => phone({ screen: s }, cell(`<b>${l}</b>`, row))); }
    if (P.view === "states") { title.innerHTML = `<b>${DIRS[P.dir].n}. ${DIRS[P.dir].name}</b> · states · ${P.theme}`; REVIEW_STATES.forEach(([s, st, l]) => phone({ screen: s, st }, cell(`<b>${l}</b>`, row))); }
    if (P.view === "compare") { title.innerHTML = `<b>${SCREENS.find(([s]) => s === P.screen)[1]}</b> in all five directions · ${P.theme}`; ORDER.forEach((d) => phone({ dir: d, screen: P.screen, st: P.st }, cell(`<b>${DIRS[d].n}. ${DIRS[d].name}</b> · ${DIRS[d].for.replace("For ", "for ")}`, row))); }
  }
  const q = new URLSearchParams({ view: P.view, dir: P.dir, theme: P.theme, device: P.device, screen: P.screen, st: P.st, bars: P.bars, motion: P.motion });
  history.replaceState(null, "", `?${q}`);
}
document.addEventListener("click", (e) => {
  const b = e.target.closest(".pz-btn");
  if (!b) return;
  if (b.dataset.g === "replay") { render(); return; }
  P[b.dataset.g] = b.dataset.v;
  if (b.dataset.g === "screen") P.st = "default";
  render();
});
document.addEventListener("keydown", (e) => {
  if (e.target.matches("input, textarea") || e.ctrlKey || e.metaKey || e.altKey) return;
  const n = Number(e.key);
  if (n >= 1 && n <= 5) { P.dir = ORDER[n - 1]; render(); return; }
  const s = SCREENS.find(([, , key]) => key === e.key.toLowerCase());
  if (s) { P.screen = s[0]; P.st = "default"; render(); return; }
  if (e.key === "d" || e.key === "l") { P.theme = e.key === "d" ? "dark" : "light"; render(); }
  if (e.key === "r") render();
});
render();
