Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class TjWorkerCredential {
 [StructLayout(LayoutKind.Sequential,CharSet=CharSet.Unicode)] struct Credential { public uint Flags,Type; public string Target,Comment; public long Updated; public uint Size; public IntPtr Blob; public uint Persist,Count; public IntPtr Attributes; public string Alias,User; }
 [DllImport("advapi32.dll",EntryPoint="CredWriteW",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool Write(ref Credential value,uint flags);
 [DllImport("advapi32.dll",EntryPoint="CredDeleteW",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool Delete(string target,uint type,uint flags);
 public static void Put(string target,string value) {
  var bytes=Encoding.UTF8.GetBytes(value); if(bytes.Length>2560) throw new Exception(); var pointer=Marshal.AllocHGlobal(bytes.Length);
  try { Marshal.Copy(bytes,0,pointer,bytes.Length); var credential=new Credential {Type=1,Target=target,Size=(uint)bytes.Length,Blob=pointer,Persist=2,User="Tjommie isolated worker"}; if(!Write(ref credential,0)) throw new Exception(); }
  finally { for(int i=0;i<bytes.Length;i++) Marshal.WriteByte(pointer,i,0); Marshal.FreeHGlobal(pointer); Array.Clear(bytes,0,bytes.Length); }
 }
 public static void Remove(string target) { if(!Delete(target,1,0) && Marshal.GetLastWin32Error()!=1168) throw new Exception(); }
}
'@
