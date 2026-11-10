//! `WORKER_POLICY_OBSERVER` (`worker-policy-observer.ts`), copied byte for byte by a script: its sha256 goes into
//! every review policy receipt. A test checks it against the TypeScript source.

pub const WORKER_POLICY_OBSERVER: &str = r#"
const{spawnSync}=require('node:child_process');
const{existsSync,lstatSync,readFileSync}=require('node:fs');
const{join}=require('node:path');const{createHash}=require('node:crypto');
const command="$ErrorActionPreference='Stop';$found=$false;foreach($hive in @([Microsoft.Win32.Registry]::LocalMachine,[Microsoft.Win32.Registry]::CurrentUser)){$key=$hive.OpenSubKey('SOFTWARE\\Policies\\ClaudeCode',$false);if($null -ne $key){$found=$true;$key.Dispose()}};if($found){'present'}else{'absent'}";
const check=spawnSync(join(process.env.SystemRoot,'System32','WindowsPowerShell','v1.0','powershell.exe'),['-NoLogo','-NoProfile','-NonInteractive','-Command',command],{env:{SystemRoot:process.env.SystemRoot,WINDIR:process.env.SystemRoot},cwd:process.cwd(),windowsHide:true,encoding:'utf8',timeout:5000,maxBuffer:4096});
if(check.status!==0||check.stdout.trim()!=='absent')process.exit(73);
const folder=join(process.env.ProgramFiles||'C:\\Program Files','ClaudeCode');
const files=['managed-settings.json','managed-mcp.json'].map(name=>{const path=join(folder,name);if(!existsSync(path))return[name,null];const info=lstatSync(path);if(!info.isFile()||info.isSymbolicLink()||info.size>1048576)process.exit(73);return[name,createHash('sha256').update(readFileSync(path)).digest('hex')]});
process.stdout.write(JSON.stringify({version:1,registryAbsent:true,files}));
"#;
