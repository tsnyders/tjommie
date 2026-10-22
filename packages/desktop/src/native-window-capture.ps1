param([Parameter(Mandatory=$true)][UInt64]$WindowHandle,[Parameter(Mandatory=$true)][UInt32]$OwnerPid,[Parameter(Mandatory=$true)][string]$OutputPath,[int]$CropWidth=0)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class NativeFixtureCapture {
  [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left,Top,Right,Bottom; }
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out Rect r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
}
'@
$handle = [IntPtr]::new([Int64]$WindowHandle)
$foundPid = [UInt32]0
[void][NativeFixtureCapture]::GetWindowThreadProcessId($handle,[ref]$foundPid)
if ($foundPid -ne $OwnerPid) { throw 'Capture window does not belong to the fixture.' }
$rectangle = New-Object NativeFixtureCapture+Rect
if (-not [NativeFixtureCapture]::GetWindowRect($handle,[ref]$rectangle)) { throw 'Fixture bounds unavailable.' }
$width = $rectangle.Right - $rectangle.Left
$height = $rectangle.Bottom - $rectangle.Top
if ($width -le 0 -or $height -le 0 -or $width -gt 8192 -or $height -gt 8192) { throw 'Invalid fixture capture dimensions.' }
$bitmap = New-Object System.Drawing.Bitmap($width,$height)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$dc = $graphics.GetHdc()
try { if (-not [NativeFixtureCapture]::PrintWindow($handle,$dc,2)) { throw 'Native fixture capture failed.' } }
finally { $graphics.ReleaseHdc($dc); $graphics.Dispose() }
try {
  $samples = 0
  for ($y=0;$y -lt $height;$y+=4) { for ($x=0;$x -lt $width;$x+=4) {
    $pixel = $bitmap.GetPixel($x,$y)
    if ([Math]::Abs($pixel.R-219) -le 2 -and [Math]::Abs($pixel.G-236) -le 2 -and [Math]::Abs($pixel.B-245) -le 2) { $samples++ }
  } }
  if ($CropWidth -gt 0) {
    $crop = $bitmap.Clone([System.Drawing.Rectangle]::new(0,0,[Math]::Min($CropWidth,$width),$height),$bitmap.PixelFormat)
    try { $crop.Save($OutputPath,[System.Drawing.Imaging.ImageFormat]::Png) } finally { $crop.Dispose() }
  } else { $bitmap.Save($OutputPath,[System.Drawing.Imaging.ImageFormat]::Png) }
  @{width=$width;height=$height;nativePixelSamples=$samples} | ConvertTo-Json -Compress
} finally { $bitmap.Dispose() }
