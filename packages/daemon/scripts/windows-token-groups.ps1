#requires -Version 5.1

# Inspect only the current process token. Output contains SID strings and raw group attributes.
# https://learn.microsoft.com/en-us/windows/win32/api/winnt/ne-winnt-token_information_class
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if (-not ('Tjommie.WindowsTokenGroups' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Security.Principal;

namespace Tjommie
{
    public sealed class TokenSidEntry
    {
        public string Sid { get; set; }
        public uint Attributes { get; set; }
    }

    public static class WindowsTokenGroups
    {
        private const int ErrorInsufficientBuffer = 122;

        private enum TokenInformationClass
        {
            TokenGroups = 2,
            TokenRestrictedSids = 11,
            TokenCapabilities = 30,
            TokenAppContainerSid = 31
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct SidAndAttributes
        {
            public IntPtr Sid;
            public uint Attributes;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct TokenGroupsHeader
        {
            public uint GroupCount;
            public SidAndAttributes FirstGroup;
        }

        [DllImport("advapi32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool GetTokenInformation(
            IntPtr token,
            TokenInformationClass informationClass,
            IntPtr information,
            uint informationLength,
            out uint returnLength);

        private static IntPtr ReadInformation(
            IntPtr token,
            TokenInformationClass informationClass,
            out uint length)
        {
            bool sized = GetTokenInformation(token, informationClass, IntPtr.Zero, 0, out length);
            int error = Marshal.GetLastWin32Error();
            if (!sized && error != ErrorInsufficientBuffer)
                throw new Win32Exception(error);
            if (length == 0 || length > Int32.MaxValue)
                throw new InvalidOperationException("Windows returned an invalid token information length.");

            IntPtr buffer = Marshal.AllocHGlobal(checked((int)length));
            try
            {
                uint returnedLength;
                if (!GetTokenInformation(token, informationClass, buffer, length, out returnedLength))
                    throw new Win32Exception(Marshal.GetLastWin32Error());
                if (returnedLength > length)
                    throw new InvalidOperationException("Windows returned token information beyond the buffer.");
                length = returnedLength;
                return buffer;
            }
            catch
            {
                Marshal.FreeHGlobal(buffer);
                throw;
            }
        }

        private static TokenSidEntry[] ReadGroups(IntPtr token, TokenInformationClass informationClass)
        {
            uint length;
            IntPtr buffer = ReadInformation(token, informationClass, out length);
            try
            {
                if (length < sizeof(uint))
                    throw new InvalidOperationException("Windows returned an incomplete token group count.");
                int count = Marshal.ReadInt32(buffer);
                if (count < 0)
                    throw new InvalidOperationException("Windows returned an invalid token group count.");
                if (count == 0)
                    return new TokenSidEntry[0];

                int offset = Marshal.OffsetOf(typeof(TokenGroupsHeader), "FirstGroup").ToInt32();
                int stride = Marshal.SizeOf(typeof(SidAndAttributes));
                if ((long)offset + (long)count * stride > length)
                    throw new InvalidOperationException("Windows returned incomplete token group information.");

                TokenSidEntry[] groups = new TokenSidEntry[count];
                for (int index = 0; index < count; index++)
                {
                    IntPtr address = IntPtr.Add(buffer, checked(offset + index * stride));
                    SidAndAttributes group = (SidAndAttributes)Marshal.PtrToStructure(address, typeof(SidAndAttributes));
                    groups[index] = new TokenSidEntry
                    {
                        Sid = new SecurityIdentifier(group.Sid).Value,
                        Attributes = group.Attributes
                    };
                }
                return groups;
            }
            finally
            {
                Marshal.FreeHGlobal(buffer);
            }
        }

        public static TokenSidEntry[] Groups(IntPtr token)
        {
            return ReadGroups(token, TokenInformationClass.TokenGroups);
        }

        public static TokenSidEntry[] RestrictedSids(IntPtr token)
        {
            return ReadGroups(token, TokenInformationClass.TokenRestrictedSids);
        }

        public static TokenSidEntry[] Capabilities(IntPtr token)
        {
            return ReadGroups(token, TokenInformationClass.TokenCapabilities);
        }

        public static string AppContainerSid(IntPtr token)
        {
            uint length;
            IntPtr buffer = ReadInformation(token, TokenInformationClass.TokenAppContainerSid, out length);
            try
            {
                if (length < IntPtr.Size)
                    throw new InvalidOperationException("Windows returned incomplete AppContainer information.");
                IntPtr sid = Marshal.ReadIntPtr(buffer);
                return sid == IntPtr.Zero ? null : new SecurityIdentifier(sid).Value;
            }
            finally
            {
                Marshal.FreeHGlobal(buffer);
            }
        }
    }
}
'@
}

$tokenIdentity = [System.Security.Principal.WindowsIdentity]::GetCurrent(
    [System.Security.Principal.TokenAccessLevels]::Query
)
try {
    [ordered]@{
        TokenGroups = @([Tjommie.WindowsTokenGroups]::Groups($tokenIdentity.Token))
        TokenRestrictedSids = @([Tjommie.WindowsTokenGroups]::RestrictedSids($tokenIdentity.Token))
        TokenCapabilities = @([Tjommie.WindowsTokenGroups]::Capabilities($tokenIdentity.Token))
        TokenAppContainerSid = [Tjommie.WindowsTokenGroups]::AppContainerSid($tokenIdentity.Token)
    } | ConvertTo-Json -Depth 4 -Compress
}
finally {
    $tokenIdentity.Dispose()
}
