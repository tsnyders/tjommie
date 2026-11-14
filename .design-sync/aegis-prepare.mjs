// Builds the throwaway tokens-only package the design-sync converter reads (run before package-build.mjs).
// Aegis has no React components, so the converter's tokens-only mode builds the base files from this package.
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const SRC = 'design/claude-design/aegis';
const PKG = '.ds-sync/aegis-nm/tjommie-aegis';

// The cards' own Google Fonts link is the source of truth for the font families.
const card = readFileSync(join(SRC, 'components', 'foundations-type.html'), 'utf8');
const fonts = /href="(https:\/\/fonts\.googleapis\.com\/css2\?[^"]+)"/.exec(card)?.[1];
if (!fonts) throw new Error('foundations-type.html has no Google Fonts stylesheet link');

mkdirSync(PKG, { recursive: true });
writeFileSync(join(PKG, 'package.json'),
  JSON.stringify({ name: 'tjommie-aegis', private: true, type: 'module', main: 'index.js', module: 'index.js' }, null, 2) + '\n');
writeFileSync(join(PKG, 'index.js'), 'export {};\n');
writeFileSync(join(PKG, 'aegis.css'),
  `@import url("${fonts.replaceAll('&amp;', '&')}");\n` + readFileSync(join(SRC, 'tokens.css'), 'utf8'));
console.log(`prepared ${PKG}`);
