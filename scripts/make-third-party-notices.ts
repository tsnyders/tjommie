import { existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Writes third-party-notices.txt, the file the installer puts beside Tjommie.exe and the About screen shows. It is one file with five parts:
//
//   1. the Rust crates in the Windows binaries, from cargo-about (installer/about.toml and about.hbs; the release workflow runs it)
//   2. pdfium, with its own licence; the texts of what is inside it ship as files in resources\pdfium-licenses
//   3. the speech runtime: whisper.cpp, ggml, OpenAI Whisper and the model notice, from the verified speech bundle
//   4. the fonts, Archivo and JetBrains Mono (SIL Open Font License, tjommie-rs/design/fonts/OFL.txt)
//   5. the icon set, Lucide (ISC, with the MIT text for the icons derived from Feather; installer/third-party/LUCIDE-LICENSE.txt)
//
//   node scripts/make-third-party-notices.ts --version <x.y.z> --crates <cargo-about's output> --speech <the speech bundle folder>
//        --out <third-party-notices.txt> [--pdfium-licenses <the folder fetch-pdfium.ps1 -Licenses wrote>] [--strict]
//
// --strict is what a release passes: pdfium's licences are then required (a dry run on a branch that has no pdfium yet may leave them out, and
// the file says so). Everything else is always required: a part that is missing is an error, never a quietly shorter file. It reads files and
// writes one file; it fetches nothing.

export type Parts = {
  version: string;
  crates: string;
  pdfium: { release: string; source: string; license: string } | undefined;
  speech: { name: string; text: string }[];
  fonts: string;
  lucide: string;
};

export const SPEECH_FILES = ["WHISPER-LICENSE.txt", "GGML-LICENSE.txt", "OPENAI-WHISPER-LICENSE.txt", "MODEL-NOTICE.txt"] as const;
const VERSION = /^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/;
const RULE = "*".repeat(80);

// LF, whatever the source had: the crates' licence files come with both, and the About screen shows the text as it is.
const lf = (text: string): string => text.replaceAll("\r\n", "\n").replaceAll("\r", "\n").trim();

export function renderNotices(parts: Parts): string {
  const heading = (number: number, title: string): string => `${RULE}\n${number}. ${title}\n${RULE}\n\n`;
  const pdfium = parts.pdfium
    ? `pdfium, the PDF engine behind the PDF viewer, release ${parts.pdfium.release} of the pdfium-binaries project (${parts.pdfium.source}).\n\n` +
      "pdfium's own licence follows. The licences of the libraries inside pdfium are installed beside this app, as files, in the folder\n" +
      "resources\\pdfium-licenses.\n\n" +
      lf(parts.pdfium.license)
    : "pdfium is not part of this build, so its licences are not included.";
  return (
    [
      `Tjommie ${parts.version} - third-party notices`,
      "",
      "Tjommie is made with the software below. Each part is used under its own licence, reproduced here or in the folder named.",
      "",
      "  1. Rust crates (the app and its daemon are written in Rust, and these are linked into them)",
      "  2. pdfium (the PDF viewer)",
      "  3. Speech recognition (whisper.cpp, ggml, OpenAI Whisper and the speech model)",
      "  4. Fonts (Archivo and JetBrains Mono)",
      "  5. Icons (Lucide)",
      "",
      heading(1, "Rust crates") + lf(parts.crates),
      "",
      heading(2, "pdfium") + pdfium,
      "",
      heading(3, "Speech recognition (whisper.cpp, ggml, OpenAI Whisper, the model)") +
        parts.speech.map((file) => `---- ${file.name} ----\n\n${lf(file.text)}`).join("\n\n"),
      "",
      heading(4, "Fonts (Archivo and JetBrains Mono)") + lf(parts.fonts),
      "",
      heading(5, "Icons (Lucide, https://lucide.dev)") +
        "The icons in the app are from the Lucide icon set, used under the ISC licence (and, for the icons derived from Feather, the MIT licence).\n\n" +
        lf(parts.lucide),
      "",
    ].join("\n")
  );
}

function argument(args: string[], name: string): string | undefined {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : undefined;
}

function read(file: string, what: string): string {
  if (!existsSync(file) || !statSync(file).isFile()) throw new Error(`${what} is missing: ${file}`);
  const text = readFileSync(file, "utf8");
  if (text.trim().length === 0) throw new Error(`${what} is empty: ${file}`);
  return text;
}

export function makeThirdPartyNotices(args: string[], repo = resolve(import.meta.dirname, "..")): string {
  const version = argument(args, "--version");
  const cratesFile = argument(args, "--crates");
  const speechDir = argument(args, "--speech");
  const out = argument(args, "--out");
  const pdfiumDir = argument(args, "--pdfium-licenses");
  const strict = args.includes("--strict");
  if (!version) throw new Error("--version is required.");
  if (!cratesFile) throw new Error("--crates is required (the output of cargo-about).");
  if (!speechDir) throw new Error("--speech is required (the speech bundle folder).");
  if (!out) throw new Error("--out is required.");
  if (!VERSION.test(version)) throw new Error(`The version must be x.y.z, not '${version}'.`);

  const crates = read(cratesFile, "The cargo-about output");
  // A file with no crates in it is cargo-about having failed quietly, not a notice: every licence section names who uses it.
  if (!/^Used by:$/m.test(crates.replaceAll("\r\n", "\n"))) throw new Error(`The cargo-about output has no crates in it: ${cratesFile}`);

  let pdfium: Parts["pdfium"];
  if (pdfiumDir) {
    const recorded = JSON.parse(readFileSync(join(repo, "installer", "tool-hashes.json"), "utf8")) as { pdfium?: { version?: string | null; source?: string | null } };
    if (!recorded.pdfium?.version || !recorded.pdfium.source) throw new Error("installer/tool-hashes.json does not record pdfium's release and source.");
    pdfium = { release: recorded.pdfium.version, source: recorded.pdfium.source, license: read(join(pdfiumDir, "LICENSE"), "pdfium's LICENSE") };
  } else if (strict) {
    throw new Error("pdfium's licences are required for a release (--pdfium-licenses) and none were given.");
  }

  const parts: Parts = {
    version,
    crates,
    pdfium,
    speech: SPEECH_FILES.map((name) => ({ name, text: read(join(speechDir, name), `The speech bundle's ${name}`) })),
    fonts: read(join(repo, "tjommie-rs", "design", "fonts", "OFL.txt"), "The fonts' licence (OFL.txt)"),
    lucide: read(join(repo, "installer", "third-party", "LUCIDE-LICENSE.txt"), "The Lucide licence"),
  };
  const text = renderNotices(parts);
  mkdirSync(dirname(resolve(out)), { recursive: true });
  writeFileSync(out, text);
  return `Wrote ${out}: ${Buffer.byteLength(text)} bytes${pdfium ? "" : " (without pdfium)"}`;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    console.log(makeThirdPartyNotices(process.argv.slice(2)));
  } catch (error: unknown) {
    console.error(error instanceof Error ? error.message : "Could not write the third-party notices.");
    process.exitCode = 1;
  }
}
