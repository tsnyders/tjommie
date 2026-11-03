$ErrorActionPreference = 'Stop'
[Console]::InputEncoding = New-Object System.Text.UTF8Encoding($false)
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$workerProcess = $null
$jobHandle = [IntPtr]::Zero
$workerInput = [Console]::OpenStandardInput()
$globalProcessSource = @'
using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Collections.Generic;
public static class TjWorkerProcessSet {
  [StructLayout(LayoutKind.Sequential)] struct Row { public uint Session,Pid; public IntPtr Name,Sid; }
  [DllImport("wtsapi32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool WTSEnumerateProcessesW(IntPtr server,uint reserved,uint version,out IntPtr rows,out uint count);
  [DllImport("wtsapi32.dll")] static extern void WTSFreeMemory(IntPtr rows);
  public static void Validate(string workerSid,int[] pids,string[] sids) {
    if(pids.Length!=sids.Length) throw new Exception();
    for(int i=0;i<pids.Length;i++) {
      // PID 0 is Idle and PID 4 is the Windows kernel; neither can be a worker user process.
      if(pids[i]==0 || pids[i]==4) continue;
      if(String.IsNullOrEmpty(sids[i]) || sids[i]==workerSid) throw new Exception();
    }
  }
  public static void AssertQuiescent(string workerSid) {
    IntPtr rows; uint count;
    if(!WTSEnumerateProcessesW(IntPtr.Zero,0,1,out rows,out count)) throw new Exception();
    try {
      var pids=new List<int>(); var sids=new List<string>(); var seen=new HashSet<int>(); int size=Marshal.SizeOf(typeof(Row));
      for(int i=0;i<count;i++) {
        var row=(Row)Marshal.PtrToStructure(IntPtr.Add(rows,i*size),typeof(Row)); int pid=checked((int)row.Pid);
        if(!seen.Add(pid)) throw new Exception(); pids.Add(pid); sids.Add(row.Sid==IntPtr.Zero ? null : new SecurityIdentifier(row.Sid).Value);
      }
      Validate(workerSid,pids.ToArray(),sids.ToArray());
      // WTS can omit other users without sufficient query rights. Never accept a partial result.
      foreach(var process in Process.GetProcesses()) using(process) {
        if(seen.Contains(process.Id)) continue;
        try { using(var live=Process.GetProcessById(process.Id)) { if(!live.HasExited) throw new InvalidOperationException(); } }
        catch(ArgumentException) { /* Process exited between snapshots. */ }
      }
    } finally { WTSFreeMemory(rows); }
  }
}
'@
function Read-PrivateLine([IO.Stream]$pipe) {
  $bytes = New-Object 'Collections.Generic.List[byte]'
  while ($true) { $next = $pipe.ReadByte(); if ($next -lt 0) { throw 'input closed' }; if ($next -eq 10) { break }; if ($bytes.Count -ge 1048576) { throw 'input limit' }; $bytes.Add([byte]$next) }
  return [Text.Encoding]::UTF8.GetString($bytes.ToArray())
}
try {
  $launch = (Read-PrivateLine $workerInput) | ConvertFrom-Json
  $config = $launch.config
  if ([Security.Principal.WindowsIdentity]::GetCurrent().User.Value -ne $config.ownerSid -or $config.ownerSid -eq $config.workerSid) { throw 'boundary' }
  $account = Get-LocalUser -Name $config.workerName
  if ($account.SID.Value -ne $config.workerSid -or -not $account.Enabled) { throw 'boundary' }
  $boot = (Get-CimInstance Win32_OperatingSystem).LastBootUpTime.ToUniversalTime()
  if ($boot -le [DateTime]::Parse($config.provisionedAt).ToUniversalTime()) { throw 'Restart Windows after provisioning before accepting this boundary.' }
  if ($launch.quiescent) { Add-Type -TypeDefinition $globalProcessSource; [TjWorkerProcessSet]::AssertQuiescent([string]$config.workerSid) }
  foreach ($candidate in Get-CimInstance Win32_Process -Filter "Name='claude.exe' OR Name='codex.exe' OR Name='node.exe'") {
    if ($candidate.Name -match '^node(\.exe)?$' -and [String]::IsNullOrWhiteSpace($candidate.CommandLine)) { throw 'An existing Node process could not be identified safely.' }
    $provider = $candidate.Name -match '^(claude|codex)\.exe$' -or ($candidate.Name -match '^node(\.exe)?$' -and $candidate.CommandLine -match '([\\/]@anthropic-ai[\\/]claude-code[\\/]|[\\/]@openai[\\/]codex[\\/])')
    if (-not $provider) { continue }
    $owner = Invoke-CimMethod -InputObject $candidate -MethodName GetOwnerSid
    if ($owner.ReturnValue -ne 0 -or $owner.Sid -eq $config.ownerSid) { throw 'An owner-level provider is still running. Close older Tjommie/provider apps before enabling Logins.' }
    if ($owner.Sid -eq $config.workerSid -and @($launch.allowedWorkerPids) -notcontains $candidate.ProcessId) { throw 'An untracked worker provider is running. Reconcile older hosts before enabling Logins.' }
  }
  foreach ($protected in @($config.ownerDataRoot,$config.runtimeRoot)) {
    $acl = Get-Acl -LiteralPath $protected
    foreach ($rule in $acl.Access) {
      if ($rule.AccessControlType -ne [Security.AccessControl.AccessControlType]::Allow) { continue }
      $sid = $rule.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value
      if (@($config.ownerSid,'S-1-5-18','S-1-5-32-544') -contains $sid) { continue }
      if ($protected -eq $config.runtimeRoot -and $sid -eq $config.workerSid -and (($rule.FileSystemRights -band [Security.AccessControl.FileSystemRights]::Write) -eq 0) -and (($rule.FileSystemRights -band [Security.AccessControl.FileSystemRights]::ChangePermissions) -eq 0) -and (($rule.FileSystemRights -band [Security.AccessControl.FileSystemRights]::TakeOwnership) -eq 0)) { continue }
      throw 'protected ACL'
    }
  }
  if ($launch.mode -eq 'verify') { [Console]::Out.Write('verified'); exit 0 }
  Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.IO;
using System.Text;
using Microsoft.Win32.SafeHandles;
public static class TjWorkerSecurity {
  [DllImport("advapi32.dll", SetLastError=true)] static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
  [DllImport("advapi32.dll", SetLastError=true)] static extern bool GetTokenInformation(IntPtr token,int kind,IntPtr value,int size,out int needed);
  [DllImport("kernel32.dll", SetLastError=true)] public static extern bool CloseHandle(IntPtr handle);
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern IntPtr CreateJobObject(IntPtr attributes,string name);
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool SetInformationJobObject(IntPtr job,int kind,IntPtr value,uint size);
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool AssignProcessToJobObject(IntPtr job,IntPtr process);
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool TerminateJobObject(IntPtr job,uint code);
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool QueryInformationJobObject(IntPtr job,int kind,IntPtr value,uint size,out uint returned);
  [StructLayout(LayoutKind.Sequential)] struct Basic { public long PerProcess; public long PerJob; public uint Flags; public UIntPtr Minimum; public UIntPtr Maximum; public uint Active; public UIntPtr Affinity; public uint Priority; public uint Scheduling; }
  [StructLayout(LayoutKind.Sequential)] struct IO { public ulong ReadOps,WriteOps,OtherOps,ReadBytes,WriteBytes,OtherBytes; }
  [StructLayout(LayoutKind.Sequential)] struct Limits { public Basic Basic; public IO IO; public UIntPtr ProcessMemory,JobMemory,PeakProcess,PeakJob; }
  public static void Verify(IntPtr process,string sid) {
    IntPtr token;
    if(!OpenProcessToken(process,8,out token)) throw new Exception();
    try {
      using(var identity = new WindowsIdentity(token)) {
        if(identity.User.Value != sid || identity.Groups == null) throw new Exception();
        foreach(var group in identity.Groups) if(group.Value == "S-1-5-32-544" || group.Value == "S-1-5-18") throw new Exception();
      }
      int needed; var elevation=Marshal.AllocHGlobal(4);
      try { if(!GetTokenInformation(token,20,elevation,4,out needed) || Marshal.ReadInt32(elevation) != 0) throw new Exception(); }
      finally { Marshal.FreeHGlobal(elevation); }
      GetTokenInformation(token,3,IntPtr.Zero,0,out needed);
      var privileges=Marshal.AllocHGlobal(needed);
      try {
        if(!GetTokenInformation(token,3,privileges,needed,out needed)) throw new Exception();
        int count=Marshal.ReadInt32(privileges);
        // Only ordinary user shutdown, traverse, undock, working-set and time-zone rights are permitted.
        for(int i=0;i<count;i++) { int low=Marshal.ReadInt32(privileges,4+i*12); int high=Marshal.ReadInt32(privileges,8+i*12); if(high!=0 || (low!=19 && low!=23 && low!=25 && low!=33 && low!=34)) throw new Exception(); }
      } finally { Marshal.FreeHGlobal(privileges); }
    } finally { CloseHandle(token); }
  }
  public static IntPtr BindJob(IntPtr process) {
    var job=CreateJobObject(IntPtr.Zero,null); if(job==IntPtr.Zero) throw new Exception();
    var limits=new Limits(); limits.Basic.Flags=0x2000;
    int size=Marshal.SizeOf(limits); var pointer=Marshal.AllocHGlobal(size);
    try { Marshal.StructureToPtr(limits,pointer,false); if(!SetInformationJobObject(job,9,pointer,(uint)size) || !AssignProcessToJobObject(job,process)) throw new Exception(); return job; }
    catch { CloseHandle(job); throw; } finally { Marshal.FreeHGlobal(pointer); }
  }
  public static bool StopJob(IntPtr job) {
    if(!TerminateJobObject(job,71)) return false;
    var pointer=Marshal.AllocHGlobal(48);
    try { var deadline=DateTime.UtcNow.AddSeconds(5); do { uint returned; if(!QueryInformationJobObject(job,1,pointer,48,out returned)) return false; if(Marshal.ReadInt32(pointer,40)==0) return true; System.Threading.Thread.Sleep(10); } while(DateTime.UtcNow<deadline); return false; }
    finally { Marshal.FreeHGlobal(pointer); }
  }
  public static string Quote(string value) {
    if(value.Length>0 && value.IndexOfAny(new char[]{' ','\t','\n','\v','"'})<0) return value;
    var result=new System.Text.StringBuilder("\""); int slash=0;
    foreach(char c in value) { if(c=='\\') { slash++; continue; } if(c=='"') { result.Append('\\',slash*2+1); result.Append('"'); } else { result.Append('\\',slash); result.Append(c); } slash=0; }
    result.Append('\\',slash*2); result.Append('"'); return result.ToString();
  }
  public static System.Threading.Tasks.Task CopyInput(Stream source,Stream target) {
    return System.Threading.Tasks.Task.Run(async delegate { try { await source.CopyToAsync(target); } finally { target.Dispose(); } });
  }
}
public sealed class TjNativeWorker : IDisposable {
  public static bool UnconfirmedChild=false;
  [StructLayout(LayoutKind.Sequential)] struct SA { public int Length; public IntPtr Security; [MarshalAs(UnmanagedType.Bool)] public bool Inherit; }
  [StructLayout(LayoutKind.Sequential,CharSet=CharSet.Unicode)] struct SI { public int Size; public string Reserved,Desktop,Title; public uint X,Y,Width,Height,CharsX,CharsY,Fill,Flags; public ushort Show,ReservedSize; public IntPtr ReservedBytes,Input,Output,Error; }
  [StructLayout(LayoutKind.Sequential)] struct PI { public IntPtr Process,Thread; public uint Id,ThreadId; }
  [DllImport("advapi32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool CreateProcessWithLogonW(string user,string domain,string password,uint logon,string app,StringBuilder command,uint flags,IntPtr environment,string cwd,ref SI start,out PI process);
  [DllImport("advapi32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool ConvertStringSecurityDescriptorToSecurityDescriptor(string text,uint revision,out IntPtr descriptor,out uint size);
  [DllImport("kernel32.dll",SetLastError=true)] static extern bool CreatePipe(out IntPtr read,out IntPtr write,ref SA attributes,uint size);
  [DllImport("kernel32.dll",SetLastError=true)] static extern bool SetHandleInformation(IntPtr handle,uint mask,uint flags);
  [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr value);
  [DllImport("kernel32.dll",SetLastError=true)] static extern uint ResumeThread(IntPtr thread);
  [DllImport("kernel32.dll",SetLastError=true)] static extern uint WaitForSingleObject(IntPtr handle,uint milliseconds);
  [DllImport("kernel32.dll",SetLastError=true)] static extern bool GetExitCodeProcess(IntPtr process,out uint code);
  [DllImport("kernel32.dll",SetLastError=true)] static extern bool TerminateProcess(IntPtr process,uint code);
  [DllImport("user32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern IntPtr CreateWindowStation(string name,uint flags,uint access,ref SA attributes);
  [DllImport("user32.dll",SetLastError=true)] static extern IntPtr GetProcessWindowStation();
  [DllImport("user32.dll",SetLastError=true)] static extern bool SetProcessWindowStation(IntPtr station);
  [DllImport("user32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern IntPtr CreateDesktop(string name,IntPtr device,IntPtr mode,uint flags,uint access,ref SA attributes);
  [DllImport("user32.dll")] static extern bool CloseDesktop(IntPtr desktop);
  [DllImport("user32.dll")] static extern bool CloseWindowStation(IntPtr station);
  public IntPtr Handle,Thread,Station,Desktop;
  public uint Id;
  public FileStream Input,Output,Error;
  public static TjNativeWorker Start(string user,string password,string file,string arguments,string cwd,string environment,string ownerSid,string workerSid) {
    var result=new TjNativeWorker(); IntPtr descriptor=IntPtr.Zero,env=IntPtr.Zero,childInput=IntPtr.Zero,childOutput=IntPtr.Zero,childError=IntPtr.Zero,parentInput=IntPtr.Zero,parentOutput=IntPtr.Zero,parentError=IntPtr.Zero;
    try {
      uint size;
      if(!ConvertStringSecurityDescriptorToSecurityDescriptor("D:(A;;GA;;;"+ownerSid+")(A;;GA;;;"+workerSid+")(A;;GA;;;SY)",1,out descriptor,out size)) throw new Exception();
      var secured=new SA {Length=Marshal.SizeOf(typeof(SA)),Security=descriptor,Inherit=false};
      string stationName="TjommieWorker-"+Guid.NewGuid().ToString("N");
      result.Station=CreateWindowStation(stationName,0,0x37f,ref secured); if(result.Station==IntPtr.Zero) throw new Exception();
      var previous=GetProcessWindowStation();
      try { if(!SetProcessWindowStation(result.Station)) throw new Exception(); result.Desktop=CreateDesktop("default",IntPtr.Zero,IntPtr.Zero,0,0x1ff,ref secured); if(result.Desktop==IntPtr.Zero) throw new Exception(); }
      finally { if(!SetProcessWindowStation(previous)) throw new Exception(); }
      var pipes=new SA {Length=Marshal.SizeOf(typeof(SA)),Inherit=true};
      if(!CreatePipe(out childInput,out parentInput,ref pipes,0) || !CreatePipe(out parentOutput,out childOutput,ref pipes,0) || !CreatePipe(out parentError,out childError,ref pipes,0)) throw new Exception();
      if(!SetHandleInformation(parentInput,1,0) || !SetHandleInformation(parentOutput,1,0) || !SetHandleInformation(parentError,1,0)) throw new Exception();
      var start=new SI {Size=Marshal.SizeOf(typeof(SI)),Desktop=stationName+"\\default",Flags=0x101,Show=0,Input=childInput,Output=childOutput,Error=childError};
      env=Marshal.StringToHGlobalUni(environment); PI process;
      if(!CreateProcessWithLogonW(user,Environment.MachineName,password,1,file,new StringBuilder(TjWorkerSecurity.Quote(file)+" "+arguments),0x08000404,env,cwd,ref start,out process)) throw new Exception();
      result.Handle=process.Process; result.Thread=process.Thread; result.Id=process.Id;
      result.Input=new FileStream(new SafeFileHandle(parentInput,true),FileAccess.Write,4096,false); parentInput=IntPtr.Zero;
      result.Output=new FileStream(new SafeFileHandle(parentOutput,true),FileAccess.Read,4096,false); parentOutput=IntPtr.Zero;
      result.Error=new FileStream(new SafeFileHandle(parentError,true),FileAccess.Read,4096,false); parentError=IntPtr.Zero;
      return result;
    } catch { result.Kill(); if(result.Handle!=IntPtr.Zero && !UnconfirmedChild) { try { result.Wait(); } catch { UnconfirmedChild=true; } } result.Dispose(); throw; }
    finally { foreach(var handle in new[]{childInput,childOutput,childError,parentInput,parentOutput,parentError}) if(handle!=IntPtr.Zero) TjWorkerSecurity.CloseHandle(handle); if(descriptor!=IntPtr.Zero) LocalFree(descriptor); if(env!=IntPtr.Zero) Marshal.ZeroFreeGlobalAllocUnicode(env); }
  }
  public void Resume() { if(ResumeThread(Thread)==0xffffffff) throw new Exception(); TjWorkerSecurity.CloseHandle(Thread); Thread=IntPtr.Zero; }
  public int Wait() { if(WaitForSingleObject(Handle,0xffffffff)!=0) throw new Exception(); uint code; if(!GetExitCodeProcess(Handle,out code)) throw new Exception(); return unchecked((int)code); }
  public void Kill() { if(Handle!=IntPtr.Zero && !TerminateProcess(Handle,71)) UnconfirmedChild=true; }
  public void Dispose() { if(Input!=null) Input.Dispose(); if(Output!=null) Output.Dispose(); if(Error!=null) Error.Dispose(); if(Thread!=IntPtr.Zero) TjWorkerSecurity.CloseHandle(Thread); if(Handle!=IntPtr.Zero) TjWorkerSecurity.CloseHandle(Handle); if(Desktop!=IntPtr.Zero) CloseDesktop(Desktop); if(Station!=IntPtr.Zero) CloseWindowStation(Station); }
}
'@
  $environment = @{}
  foreach ($property in $launch.env.PSObject.Properties) { $environment[$property.Name] = [string]$property.Value }
  $environment['SystemRoot'] = $env:SystemRoot
  $environment['WINDIR'] = $env:SystemRoot
  $environment['ProgramFiles'] = [Environment]::GetFolderPath('ProgramFiles')
  $environment['ProgramData'] = [Environment]::GetFolderPath('CommonApplicationData')
  $environment['USERPROFILE'] = $config.workerRoot
  $environment['HOME'] = $config.workerRoot
  $environment['APPDATA'] = [IO.Path]::Combine($config.workerRoot,'AppData','Roaming')
  $environment['LOCALAPPDATA'] = [IO.Path]::Combine($config.workerRoot,'AppData','Local')
  $environment['TEMP'] = [IO.Path]::Combine($config.workerRoot,'tmp')
  $environment['TMP'] = $environment['TEMP']
  $block = (($environment.Keys | Sort-Object | ForEach-Object { $_ + '=' + $environment[$_] }) -join [char]0) + [char]0 + [char]0
  $arguments = (($launch.args | ForEach-Object { [TjWorkerSecurity]::Quote([string]$_) }) -join ' ')
  $workerProcess = [TjNativeWorker]::Start([string]$config.workerName,[string]$launch.password,[string]$launch.file,$arguments,[string]$launch.cwd,$block,[string]$config.ownerSid,[string]$config.workerSid)
  $launch.password = $null
  [TjWorkerSecurity]::Verify($workerProcess.Handle,[string]$config.workerSid)
  $jobHandle = [TjWorkerSecurity]::BindJob($workerProcess.Handle)
  $receipt = @{ version=1; ownerSid=$config.ownerSid; workerSid=$config.workerSid; wrapperPid=$PID; workerPid=$workerProcess.Id; elevated=$false; privileged=$false; jobBound=$true } | ConvertTo-Json -Compress
  [IO.File]::WriteAllText(($launch.receiptPath + '.pending'),$receipt,(New-Object Text.UTF8Encoding($false)))
  [IO.File]::Move(($launch.receiptPath + '.pending'),$launch.receiptPath)
  if ((Read-PrivateLine $workerInput) -ne 'continue') { throw 'launch cancelled' }
  $output = $workerProcess.Output.CopyToAsync([Console]::OpenStandardOutput())
  $errors = $workerProcess.Error.CopyToAsync([Console]::OpenStandardError())
  $inputCopy = [TjWorkerSecurity]::CopyInput($workerInput,$workerProcess.Input)
  $workerProcess.Resume()
  $workerExit = $workerProcess.Wait()
  if (-not [TjWorkerSecurity]::StopJob($jobHandle)) { throw 'worker children did not stop' }
  [Threading.Tasks.Task]::WaitAll(@($output,$errors))
  exit $workerExit
} catch {
  if ($null -ne $workerProcess) { try { $workerProcess.Kill() } catch {} }
  [Console]::Error.WriteLine('Tjommie could not start the verified Windows worker.')
  exit 71
} finally {
  $stopped = $true
  if ('TjNativeWorker' -as [type]) { if ([TjNativeWorker]::UnconfirmedChild) { $stopped=$false } }
  if ($jobHandle -ne [IntPtr]::Zero) { $stopped = [TjWorkerSecurity]::StopJob($jobHandle); [void][TjWorkerSecurity]::CloseHandle($jobHandle) }
  elseif ($null -ne $workerProcess) { try { $workerProcess.Kill(); [void]$workerProcess.Wait() } catch { $stopped=$false } }
  if ($null -ne $workerProcess) { $workerProcess.Dispose() }
  if ($null -ne $launch -and $launch.receiptPath) {
    $closed = @{ version=1; wrapperPid=$PID; stopped=$stopped } | ConvertTo-Json -Compress
    [IO.File]::WriteAllText(($launch.receiptPath + '.closed'),$closed,(New-Object Text.UTF8Encoding($false)))
  }
}
