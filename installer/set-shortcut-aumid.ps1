# Sets a shortcut's AppUserModelID, for the Tjommie installer (T8.2). Windows shows a notification from an app only if the app's
# Start-menu shortcut carries the same AppUserModelID the app uses (`System.AppUserModel.ID`), and NSIS's CreateShortCut cannot
# write that property. The installer makes the shortcut, then runs this on it; R1's installer does the same with a plugin, and
# the T8.1 facts show the result: both shortcuts carry com.tsnyders.tjommie.
#
# Works on an existing .lnk only, and changes nothing but that property. Exit 0 when it is set; anything else, it is not.
param(
    [Parameter(Mandatory)][string]$Path,
    [Parameter(Mandatory)][string]$AppUserModelId
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

try {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "There is no shortcut at $Path." }

    # The shell link object implements IPersistFile (load and save the .lnk) and IPropertyStore (its properties).
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class TjommieShortcutAumid
{
    [ComImport, Guid("00021401-0000-0000-C000-000000000046")]
    private class ShellLink { }

    [ComImport, InterfaceType(ComInterfaceType.InterfaceIsIUnknown), Guid("0000010B-0000-0000-C000-000000000046")]
    private interface IPersistFile
    {
        void GetClassID(out Guid classId);
        [PreserveSig] int IsDirty();
        void Load([MarshalAs(UnmanagedType.LPWStr)] string fileName, uint mode);
        void Save([MarshalAs(UnmanagedType.LPWStr)] string fileName, [MarshalAs(UnmanagedType.Bool)] bool remember);
        void SaveCompleted([MarshalAs(UnmanagedType.LPWStr)] string fileName);
        void GetCurFile([MarshalAs(UnmanagedType.LPWStr)] out string fileName);
    }

    [StructLayout(LayoutKind.Sequential, Pack = 4)]
    private struct PropertyKey
    {
        public Guid FormatId;
        public uint PropertyId;
    }

    // A PROPVARIANT is 16 bytes in a 32-bit process and 24 in a 64-bit one; only the type and the pointer are used here.
    [StructLayout(LayoutKind.Explicit, Size = 24)]
    private struct PropVariant
    {
        [FieldOffset(0)] public ushort Type;
        [FieldOffset(8)] public IntPtr Pointer;
    }

    [ComImport, InterfaceType(ComInterfaceType.InterfaceIsIUnknown), Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99")]
    private interface IPropertyStore
    {
        [PreserveSig] int GetCount(out uint count);
        [PreserveSig] int GetAt(uint index, out PropertyKey key);
        [PreserveSig] int GetValue(ref PropertyKey key, out PropVariant value);
        [PreserveSig] int SetValue(ref PropertyKey key, ref PropVariant value);
        [PreserveSig] int Commit();
    }

    private const ushort VtLpwstr = 31;
    private const uint StgmReadWrite = 2;

    private static void Check(int result, string step)
    {
        if (result != 0) throw new InvalidOperationException(step + " failed with 0x" + result.ToString("X8"));
    }

    public static void Set(string path, string appUserModelId)
    {
        object link = new ShellLink();
        try
        {
            ((IPersistFile)link).Load(path, StgmReadWrite);
            IPropertyStore store = (IPropertyStore)link;
            PropertyKey key = new PropertyKey();
            key.FormatId = new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"); // PKEY_AppUserModel_ID
            key.PropertyId = 5;
            PropVariant value = new PropVariant();
            value.Type = VtLpwstr;
            value.Pointer = Marshal.StringToCoTaskMemUni(appUserModelId);
            try
            {
                Check(store.SetValue(ref key, ref value), "SetValue");
                Check(store.Commit(), "Commit");
            }
            finally { Marshal.FreeCoTaskMem(value.Pointer); }
            ((IPersistFile)link).Save(path, true);
        }
        finally { Marshal.FinalReleaseComObject(link); }
    }
}
'@

    [TjommieShortcutAumid]::Set((Resolve-Path -LiteralPath $Path).ProviderPath, $AppUserModelId)
} catch {
    Write-Output $_.Exception.Message
    exit 1
}
