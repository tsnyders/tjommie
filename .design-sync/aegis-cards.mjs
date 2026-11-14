// Adds the Aegis kit's hand-authored preview cards, screens, tokens and reference media to ds-bundle/
// (run after package-build.mjs, before package-validate.mjs). Source: design/claude-design/aegis.
import { copyFileSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { basename, join } from 'node:path';

const SRC = 'design/claude-design/aegis';
const OUT = 'ds-bundle';
const STYLES = '../../../styles.css'; // components/<group>/<Name>/<Name>.html -> ds-bundle/styles.css

const pascal = (s) => s.replace(/(^|-)([a-z0-9])/g, (_, __, c) => c.toUpperCase());
const slug = (group) => group.toLowerCase().replace(/[^a-z0-9]+/g, '-');
const htmlFiles = (dir) => readdirSync(join(SRC, dir)).filter((f) => f.endsWith('.html')).sort();

function place(file, group, html) {
  const name = pascal(basename(file, '.html'));
  const dir = join(OUT, 'components', slug(group), name);
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, `${name}.html`), html);
  return `${slug(group)}/${name}`;
}

const placed = [];
for (const f of htmlFiles('components')) {
  const html = readFileSync(join(SRC, 'components', f), 'utf8');
  const group = /^<!-- @dsCard group="([^"]+)" -->/.exec(html)?.[1];
  if (!group) throw new Error(`${f}: first line isn't a @dsCard comment`);
  placed.push(place(f, group, html.replaceAll('href="../tokens.css"', `href="${STYLES}"`)));
}

// Screens become cards in a "Screens" group; their links to each other point at the sibling screen cards.
for (const f of htmlFiles('screens')) {
  const html = readFileSync(join(SRC, 'screens', f), 'utf8')
    .replaceAll('href="../tokens.css"', `href="${STYLES}"`)
    .replace(/href="([a-z0-9-]+)\.html"/g, (_, s) => `href="../${pascal(s)}/${pascal(s)}.html"`);
  placed.push(place(f, 'Screens', `<!-- @dsCard group="Screens" -->\n${html}`));
}

mkdirSync(join(OUT, 'tokens'), { recursive: true });
for (const t of ['tokens.css', 'tokens.json']) copyFileSync(join(SRC, t), join(OUT, 'tokens', t));

for (const [dir, ext] of [['motion-clips', '.webm'], ['shots', '.png']]) {
  mkdirSync(join(OUT, 'guidelines', dir), { recursive: true });
  for (const f of readdirSync(join(SRC, dir)).filter((x) => x.endsWith(ext))) {
    copyFileSync(join(SRC, dir, f), join(OUT, 'guidelines', dir, f));
  }
}
copyFileSync(join(SRC, 'README.md'), join(OUT, 'guidelines', 'aegis-design-notes.md'));

// This is an off-script layout: the converter's verification anchor and story map describe only its own
// (card-less) build, so drop both rather than upload an anchor that vouches for files it never hashed.
// With no anchor, the next sync re-verifies everything, which is right for a kit this size.
rmSync(join(OUT, '_ds_sync.json'), { force: true });
rmSync(join(OUT, '.stories-map.json'), { force: true });

// The converter's local build metadata counts bundle components (0 for this tokens-only DS); record the
// hand-authored pattern cards it now ships so package-validate.mjs's preview count matches. Local only, never uploaded.
const metaPath = join(OUT, '.ds-build-meta.json');
const meta = JSON.parse(readFileSync(metaPath, 'utf8'));
writeFileSync(metaPath, JSON.stringify({ ...meta, componentCount: placed.length, handAuthoredCards: placed.length }, null, 2) + '\n');

console.log(`placed ${placed.length} cards:\n  ${placed.join('\n  ')}`);
