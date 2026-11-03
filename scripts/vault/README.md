# Logins worker setup and acceptance

Logins requires a separate standard Windows worker identity for **every** Tjommie provider launch. Full auto remains automatic within the explicitly assigned bot/worktree folders; it does not gain the owner's Windows identity. The worker cannot use owner Credential Manager entries, open owner process memory, or watch the owner's interactive desktop. The native vault entry window, secret store and browser fill stay with the owner. This is not protection from other software already running as the owner or an administrator, and the receiving website can expose a submitted value.

Setup is an explicit machine change, never an automatic app startup action. Review the scripts and obtain the owner's approval before executing. Close Tjommie first. In an elevated PowerShell window **as the same Windows owner**, run:

```powershell
& .\provision-worker.ps1 -OwnerDataRoot 'C:\Users\OWNER\AppData\Local\Tjommie'
```

The script creates one local standard account, protects the Tjommie metadata root, writes a random worker credential directly to the owner's namespaced Credential Manager entry, and creates `worker.json` plus a persistent `vault-protection-required` marker. It does not read existing credentials, copy account sign-ins, install a service, or restart Windows. Preserve the marker if configuration breaks; removing `worker.json` must not enable owner-level provider execution.

Install the supported Windows Node executable and both provider CLI distributions beneath the printed protected `runtime` folder. Keep `node.exe` directly in that folder. Native `claude.exe`/`codex.exe` may be directly in it or in matching `claude`/`codex` subfolders; npm distributions may use its `node_modules` folder. Install as the owner so the worker retains read/execute access and cannot replace runtime code. Do not copy owner CLI profiles or authentication. No global package installation is required by Tjommie.

Restart Windows after provisioning. The worker host refuses readiness if the current boot predates provisioning. Close older Tjommie/provider apps and disable their automatic startup before acceptance: every readiness check also rejects owner-level Claude/Codex processes and untracked worker provider processes. Restarting alone does not prove exclusivity when an older app can start again. The setup scripts do not stop those apps or change their startup settings. Then run the acceptance bundle using Node, before reopening Tjommie:

```powershell
& 'C:\ProgramData\Tjommie\Worker-NAMESPACE\runtime\node.exe' .\acceptance\main.mjs 'C:\Users\OWNER\AppData\Local\Tjommie' '..\daemon\windows-worker.ps1'
```

From a source checkout, `node scripts/vault/accept-worker.ts <dataRoot>` runs the same source. Acceptance creates only a unique synthetic owner marker and one synthetic namespaced credential, proves denied owner file/credential/process/desktop access and permitted worker writes, runs both real CLIs with `--version`, and cleans its fixtures. It neither enumerates nor reads owner credentials and starts no model turn. A protected receipt is written only if every assertion passes. This receipt does not prove successful paid provider conversations or all third-party CLI behavior.

Reopen Tjommie. Sign in separately to the new worker-owned provider accounts through Accounts. Existing owner provider profiles stay in place but are not used or copied. Host-native Logins unlock/save/fill controls become available only while the daemon supplies an authenticated live isolation lease. Closing/restarting/disconnecting the daemon locks Logins. Restart always starts locked. Clearing private fields leaves browser automation paused; the person explicitly returns control. Closing a private browser retains its profile and sign-ins.

Project review obtains a quiescent worker launch lease. It reports Busy while other worker jobs run, without stopping them. Before each serialized scoped launch, the owner helper checks a complete global Win32 process/SID snapshot; unknown ownership, an omitted live process or any unexplained worker process refuses review. Windows must permit this cross-user ownership observation; insufficient query rights fail closed. A fixed program under the worker identity checks that worker's Claude policy registry and hashes machine policy files before and after the review. Unrelated worker launches remain blocked for the entire check; unconfirmed termination keeps the lease and account reserved until the host is reconciled. Acceptance also exercises this entire policy lease before writing its receipt, so an incomplete global process observation cannot silently enable an incompatible worker setup.

To remove this setup, first delete all Logins entries in Tjommie and resolve pending deletions. Lock and close Tjommie. Have the owner remove the named standard account and its namespaced worker credential, then review the worker folders and protection marker. Do not delete a browser profile or any owner's account data as part of worker removal. Automated account removal is deliberately omitted until an exact empty-vault/owned-process teardown receipt is available.
