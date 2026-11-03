import { verifySpeechResources } from "../src/speech-resources.ts";

if (process.argv.length !== 3) throw new Error("Pass the CI-built speech resource directory.");
const manifest = verifySpeechResources(process.argv[2]);
console.log(`Verified pinned speech runtime and ${manifest.files.length - 1} notices.`);
