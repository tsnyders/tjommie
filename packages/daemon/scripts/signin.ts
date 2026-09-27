// One-time setup: sign Tjommie's own Claude and Codex accounts in.
// The daemon and the real-CLI tests (`npm run test:real`) use these account folders.
import { readdirSync } from "node:fs";
import { openDb } from "../src/db.ts";
import { dataPaths } from "../src/paths.ts";
import { checkSetup, ensureAccounts, signIn } from "../src/setup.ts";

const paths = dataPaths();
if (readdirSync(paths.root).some((name) => /^tjommie\.db(?:$|[-.])/.test(name))) {
  throw new Error("Start the daemon once to move the existing database before signing in.");
}
const db = openDb(paths.db);
ensureAccounts(db, paths.accounts);

for (const setup of await checkSetup(db)) {
  if (!setup.installed) {
    console.log(`${setup.provider}: not installed, skipping.`);
    continue;
  }
  if (setup.signedIn) {
    console.log(`${setup.provider}: already signed in as ${setup.account}.`);
    continue;
  }
  console.log(`${setup.provider}: opening its sign-in. Finish it in the window or browser that opens.`);
  await new Promise<void>((resolve, reject) => {
    signIn(db, setup.provider, (ok) => (ok ? resolve() : reject(new Error(`${setup.provider} sign-in did not finish within 10 minutes`))))
      .then(({ url }) => {
        if (url) console.log(`If the browser didn't open, go to: ${url}`);
      })
      .catch(reject);
  });
}

for (const setup of await checkSetup(db)) {
  console.log(`${setup.provider}: ${setup.signedIn ? `signed in as ${setup.account} (${setup.plan})` : "not signed in"}`);
}
db.close();
