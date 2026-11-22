import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// The deliberate breakages scripts/release-workflow.test.ts is checked against (T8.3, and the review's F3). Each one replaces a single piece
// of .github/workflows/release.yml with something dangerous, and the test file must then FAIL: "caught". A mutation that is not caught is
// a hole in the test.
//
//   node scripts/release-workflow-mutations.ts                 every mutation
//   node scripts/release-workflow-mutations.ts "<some words>"  only the mutations whose label has those words in it
//
// It never edits release.yml. Each mutated copy is written to a temporary folder and the test is pointed at it with
// TJOMMIE_RELEASE_WORKFLOW, so stopping a run half way leaves nothing to restore. It runs vitest once per mutation (a few minutes), prints
// one line each and exits 1 if any was missed or no longer finds its text. scripts/release-workflow.test.ts checks that every `from` below is
// still found exactly once in release.yml, so an edit to the workflow that moves one shows up in the ordinary test run.

export type Mutation = { label: string; from: string; to: string };

const nl = "\n";
const installNsis = "      - name: Install NSIS" + nl;
// A step in front of "Install NSIS" (in the Windows job) that runs `command`. As a block scalar, so a command that starts with `&` or `[`
// (which YAML reads as an anchor or a list) is still one valid workflow, which is what a mutation has to be.
const stepBeforeNsis = (command: string): string => "      - name: A step that should not be here" + nl + "        run: |" + nl + "          " + command + nl + installNsis;

export const mutations: Mutation[] = [
  // The publish job's gate.
  { label: "release `if` loses the dry_run condition", from: "needs.verify.outputs.dry_run == 'false' && github.event_name == 'push' && startsWith", to: "github.event_name == 'push' && startsWith" },
  { label: "release `if` loses the tag condition", from: " && startsWith(github.ref, 'refs/tags/v') }}", to: " }}" },
  { label: "release `if` is made an OR with workflow_dispatch", from: "github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v') }}", to: "github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v') || github.event_name == 'workflow_dispatch' }}" },
  { label: "the in-step tag guard is removed", from: 'if [[ "$GITHUB_EVENT_NAME" != "push" || "$GITHUB_REF" != refs/tags/v* ]]; then', to: "if false; then" },
  // Publishing from somewhere else.
  { label: "a gh release step appears in the windows job", from: installNsis, to: stepBeforeNsis("gh release create v0.0.0 --notes x") },
  { label: "a gh.exe release step appears in the windows job", from: installNsis, to: stepBeforeNsis("gh.exe release create v0.0.0 --notes x") },
  { label: "a quoted gh appears in the windows job", from: installNsis, to: stepBeforeNsis("& 'gh' release create v0.0.0 --notes x") },
  { label: "a git push appears in the verify job", from: "      - name: Decide whether this run may publish" + nl, to: "      - name: A step that should not be here" + nl + "        run: git push origin HEAD" + nl + "      - name: Decide whether this run may publish" + nl },
  { label: "a .NET web client appears in the windows job", from: installNsis, to: stepBeforeNsis("[Net.WebClient]::new().UploadString('https://example.test', 'x')") },
  { label: "node -e with fetch appears in the windows job", from: installNsis, to: stepBeforeNsis(`node -e "fetch('https://api.github.com')"`) },
  { label: "npx octokit appears in the windows job", from: installNsis, to: stepBeforeNsis("npx octokit rest") },
  // Permissions and credentials.
  { label: "the windows job gets contents: write", from: "    timeout-minutes: 120" + nl, to: "    timeout-minutes: 120" + nl + "    permissions:" + nl + "      contents: write" + nl },
  { label: "a personal access token is given to the windows job", from: "      DRY_RUN: ${{ needs.verify.outputs.dry_run }}", to: "      DRY_RUN: ${{ needs.verify.outputs.dry_run }}" + nl + "      GH_TOKEN: ${{ secrets.RELEASE_PAT }}" },
  { label: "an Android secret is used in the verify job", from: '        run: node scripts/release-mode.ts >> "$GITHUB_OUTPUT"', to: "        env:" + nl + "          T: ${{ secrets.ANDROID_KEY_PASSWORD }}" + nl + '        run: node scripts/release-mode.ts >> "$GITHUB_OUTPUT"' },
  { label: "a checkout keeps its credentials", from: "          persist-credentials: false" + nl + "      # The installer script", to: "          persist-credentials: true" + nl + "      # The installer script" },
  { label: "an action is no longer pinned to a commit", from: "actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9 # v7.0.2" + nl + "        if: ${{ needs.verify.outputs.dry_run == 'true' }}" + nl + "        with:" + nl + "          name: release-dryrun-windows", to: "actions/upload-artifact@v7 # v7.0.2" + nl + "        if: ${{ needs.verify.outputs.dry_run == 'true' }}" + nl + "        with:" + nl + "          name: release-dryrun-windows" },
  // What a dry run uploads and builds.
  { label: "the windows-assets upload loses its gate", from: "        if: ${{ needs.verify.outputs.dry_run == 'false' }}" + nl + "        with:" + nl + "          name: windows-assets", to: "        with:" + nl + "          name: windows-assets" },
  { label: "the dry-run artifact is named windows-assets", from: "name: release-dryrun-windows", to: "name: windows-assets" },
  // The APK is built in every run, a dry run too (the owner's decision, 2026-11-22); only where it goes depends on the mode.
  { label: "the android job is skipped in a dry run again", from: "    needs: verify" + nl + "    runs-on: windows-latest" + nl + "    timeout-minutes: 60", to: "    needs: verify" + nl + "    if: ${{ needs.verify.outputs.dry_run == 'false' }}" + nl + "    runs-on: windows-latest" + nl + "    timeout-minutes: 60" },
  { label: "the dry-run APK is named android-assets", from: "          name: release-dryrun-apk", to: "          name: android-assets" },
  { label: "the android-assets upload loses its gate", from: "        if: ${{ needs.verify.outputs.dry_run == 'false' }}" + nl + "        with:" + nl + "          name: android-assets", to: "        with:" + nl + "          name: android-assets" },
  { label: "the dry-run APK is kept for 90 days", from: "          retention-days: 3" + nl, to: "          retention-days: 90" + nl },
  { label: "the pdfium gate fails open (the -Release switch needs an exact true)", from: "if ($env:DRY_RUN -ne 'true') { $more['Release'] = $true }" + nl + "          ./installer/assemble-payload.ps1", to: "if ($env:DRY_RUN -eq 'false') { $more['Release'] = $true }" + nl + "          ./installer/assemble-payload.ps1" },
  // pdfium and its licence texts: fetched into a folder of their own, checked, and handed to the assembler.
  { label: "pdfium is fetched into the payload folder", from: "$dir = Join-Path $env:RUNNER_TEMP 'pdfium'", to: "$dir = Join-Path $env:RUNNER_TEMP 'payload'" },
  { label: "the pdfium hash check is removed", from: '            ./installer/check-tool-hashes.ps1 -Tool pdfium -Path "$dir\\pdfium.dll" @more', to: "            Write-Output 'skipped'" },
  { label: "the pdfium hash check fails open on the tag path", from: "if ($env:DRY_RUN -ne 'true') { $more['Release'] = $true }" + nl + "          $fetch = ", to: "if ($env:DRY_RUN -eq 'false') { $more['Release'] = $true }" + nl + "          $fetch = " },
  { label: "a release goes on without the pdfium script", from: "} elseif ($env:DRY_RUN -ne 'true') {", to: "} elseif ($false) {" },
  { label: "the assembler is not given pdfium's licences", from: '; $more[\'PdfiumLicenses\'] = "$pdfium\\licenses"', to: "" },
  // The third-party notices: a pinned cargo-about, a licence nobody accepted stops the build, and the assembler is given the file.
  { label: "the notices step downloads cargo-about itself", from: 'run: ./installer/fetch-cargo-about.ps1 -Destination "$env:RUNNER_TEMP\\cargo-about"', to: 'run: Invoke-WebRequest https://example.test/cargo-about.exe -OutFile "$env:RUNNER_TEMP\\cargo-about.exe"' },
  { label: "a failed cargo-about does not stop the notices step", from: 'if ($status -ne 0) { throw "cargo-about failed', to: 'if ($false) { throw "cargo-about failed' },
  { label: "cargo-about covers only the root crate", from: " generate --manifest-path Cargo.toml --workspace --locked", to: " generate --manifest-path Cargo.toml --locked" },
  { label: "a release writes the notices without --strict", from: "if ($env:DRY_RUN -ne 'true') { $more += '--strict' }", to: "if ($false) { $more += '--strict' }" },
  { label: "the assembler is not given the notices", from: "          $more['ThirdPartyNotices'] = \"$env:RUNNER_TEMP\\third-party-notices.txt\"" + nl, to: "" },
  // The debug symbols: their own release asset, both PDBs required on the tag path.
  { label: "the symbols step fails open on the tag path", from: "if ($env:DRY_RUN -ne 'true') { $more['Release'] = $true }" + nl + "          ./installer/pack-symbols.ps1", to: "if ($env:DRY_RUN -eq 'false') { $more['Release'] = $true }" + nl + "          ./installer/pack-symbols.ps1" },
  { label: "the symbols zip is left out of the release", from: '            "release-assets/Tjommie-$version-symbols.zip" \\' + nl, to: "" },
  { label: "the symbols zip is not uploaded for the release", from: "            dist-desktop/installer/latest.yml" + nl + "            dist-desktop/installer/Tjommie-*-symbols.zip" + nl + "          if-no-files-found: error", to: "            dist-desktop/installer/latest.yml" + nl + "          if-no-files-found: error" },
  // The review's F1: the NSIS compiler is checked against recorded hashes, strictly on the tag path.
  { label: "the NSIS hash check fails open on the tag path", from: "if ($env:DRY_RUN -ne 'true') { $more['Release'] = $true }" + nl + "          New-Item -ItemType Directory -Force -Path dist-desktop", to: "if ($env:DRY_RUN -eq 'false') { $more['Release'] = $true }" + nl + "          New-Item -ItemType Directory -Force -Path dist-desktop" },
  { label: "the NSIS hash check is removed", from: '          ./installer/check-tool-hashes.ps1 -Tool nsis -Path "${env:ProgramFiles(x86)}\\NSIS" -Observed dist-desktop\\tool-hashes-nsis.json @more', to: "          Write-Output 'skipped'" },
  { label: "the manifest stops carrying the NSIS hashes", from: ' -ToolHashes "$root\\dist-desktop\\tool-hashes-nsis.json"', to: "" },
  { label: "the dry-run artifact stops carrying the NSIS hashes", from: "            dist-desktop/tool-hashes-nsis.json" + nl, to: "" },
  { label: "the kept NSIS hashes are uploaded on a release too", from: "if: ${{ always() && needs.verify.outputs.dry_run == 'true' }}", to: "if: ${{ always() }}" },
  { label: "the production identity is dropped from the cargo build", from: "          TJOMMIE_IDENTITY: production" + nl + "        run: cargo build", to: "          TJOMMIE_IDENTITY: dev" + nl + "        run: cargo build" },
  { label: "npm run package comes back", from: installNsis, to: "      - run: npm run package" + nl + installNsis },
  // The provider-fixtures review's F2: the daemon is built under the production identity, and its finished file is checked.
  {
    label: "the daemon is built outside the production identity",
    from: "        run: cargo build --release --locked -p daemon -p desktop",
    to: "        run: cargo build --release --locked -p desktop" + nl + "      - name: Build the daemon" + nl + "        working-directory: tjommie-rs" + nl + "        run: cargo build --release --locked -p daemon",
  },
  {
    label: "the production-binary check is removed",
    from: "        run: ./installer/check-production-binaries.ps1 -Desktop tjommie-rs\\target\\release\\tjommie-desktop.exe -Daemon tjommie-rs\\target\\release\\tjommie-daemon.exe",
    to: "        run: Write-Output 'skipped'",
  },
  {
    label: "the production-binary check looks at a daemon that was not built here",
    from: "-Desktop tjommie-rs\\target\\release\\tjommie-desktop.exe -Daemon tjommie-rs\\target\\release\\tjommie-daemon.exe" + nl + "      # The crates that ship",
    to: "-Desktop tjommie-rs\\target\\release\\tjommie-desktop.exe -Daemon tjommie-rs\\target\\debug\\tjommie-daemon.exe" + nl + "      # The crates that ship",
  },
  // The owner's F2 decision (2026-11-15): a release has to be strictly newer than the latest one.
  { label: "the version guard accepts the same version", from: 'if [[ "$verdict" == equal && -n "$pre" ]]; then verdict=newer; fi', to: 'if [[ "$verdict" == equal ]]; then verdict=newer; fi' },
  {
    label: "the version guard compares text, not numbers",
    from: "            if (( a > b )); then verdict=newer; break; fi" + nl + "            if (( a < b )); then verdict=older; break; fi",
    to: '            if [[ "$a" > "$b" ]]; then verdict=newer; break; fi' + nl + '            if [[ "$a" < "$b" ]]; then verdict=older; break; fi',
  },
  {
    label: "the version guard lets a failed read through",
    from: '            echo "Refusing to publish: the latest release could not be read: $(cat "$errors")" >&2' + nl + "            exit 1",
    to: '            echo "could not be read: $(cat "$errors")" >&2' + nl + "            exit 0",
  },
  { label: "the version guard lets a latest it cannot parse through", from: 'if [[ ! "${latest#v}" =~ ^$core\\.$core\\.$core$prerelease$build$ ]]; then', to: "if false; then" },
  { label: "the version guard is removed", from: "      - name: The release is newer than the latest one", to: "      - name: A step that checks nothing" },
  // The triggers and the mode.
  { label: "the push trigger takes every branch", from: "branches: ['release-dryrun-*']", to: "branches: ['**']" },
  { label: "the verify job stops writing the mode", from: '        run: node scripts/release-mode.ts >> "$GITHUB_OUTPUT"', to: '        run: echo dry_run=false >> "$GITHUB_OUTPUT"' },
];

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const repo = resolve(import.meta.dirname, "..");
  // LF, as the mutations are written: a Windows checkout may have CRLF.
  const original = readFileSync(join(repo, ".github", "workflows", "release.yml"), "utf8").replaceAll("\r\n", "\n");
  const dir = mkdtempSync(join(tmpdir(), "tjommie-mutations-"));
  let caught = 0;
  let problems = 0;
  const only = process.argv[2];
  const selected = only ? mutations.filter(({ label }) => label.includes(only)) : mutations;
  if (selected.length === 0) {
    console.log(`No mutation has "${only}" in its label.`);
    process.exit(1);
  }
  try {
    for (const { label, from, to } of selected) {
      const found = original.split(from).length - 1;
      if (found !== 1) {
        console.log(`STALE   ${label}: its text is found ${found} times in release.yml`);
        problems++;
        continue;
      }
      const file = join(dir, "release.yml");
      writeFileSync(file, original.replace(from, () => to));
      const run = spawnSync(process.execPath, [join(repo, "node_modules", "vitest", "vitest.mjs"), "run", "scripts/release-workflow.test.ts"], {
        cwd: repo,
        encoding: "utf8",
        windowsHide: true,
        env: { ...process.env, TJOMMIE_RELEASE_WORKFLOW: file },
      });
      const output = `${run.stdout}${run.stderr}`;
      const failed = /Tests\s+(\d+) failed/.exec(output);
      if (failed) {
        caught++;
        const names = [...output.matchAll(/^\s*×\s+(.+?)\s+\d+ms$/gm)].map((match) => match[1]).slice(0, 2);
        console.log(`CAUGHT  ${label}: ${failed[1]} failing, e.g. ${names.join(" | ")}`);
      } else if (/Test Files\s+\d+ failed/.test(output)) {
        // The test file could not even load: the mutated workflow is not valid YAML, so GitHub would refuse it and this proves nothing.
        console.log(`INVALID ${label}: the mutated workflow does not parse; make the mutation a valid workflow`);
        problems++;
      } else {
        console.log(`MISSED  ${label}`);
        problems++;
      }
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
  console.log(`caught ${caught} of ${selected.length}`);
  process.exitCode = problems > 0 ? 1 : 0;
}
