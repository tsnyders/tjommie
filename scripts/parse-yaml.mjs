// Parses one YAML file with js-yaml (a real YAML parser) and prints it as JSON. Exits 1, printing the parser's message, if the
// file is not valid YAML. scripts/workflows.test.ts runs it over every GitHub workflow: GitHub refuses an invalid workflow
// before any job starts, with no log to read.
import { readFileSync } from "node:fs";
import { load } from "js-yaml";

try {
  process.stdout.write(JSON.stringify(load(readFileSync(process.argv[2], "utf8"))));
} catch (error) {
  process.stderr.write(error instanceof Error ? error.message : String(error));
  process.exit(1);
}
