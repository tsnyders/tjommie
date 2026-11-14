<#
The UI boundary rule (Phase 3 plan T0.4): in crates/desktop/src/ui, the closure of every on_click, on_action,
on_key_down and spawn call starts with guarded(, so a panic in it becomes a "Something went wrong" toast instead of a
crash. A closure inside cx.listener(...) counts; a method passed by name doesn't (wrap it in a guarded closure).

    .\tools\check-guarded.ps1             # from tjommie-rs: lists each unguarded call and exits 1
    .\tools\check-guarded.ps1 -SelfTest   # this script, on planted handlers
#>
param([string]$Path = (Join-Path (Split-Path $PSScriptRoot -Parent) 'crates\desktop\src\ui'), [switch]$SelfTest)
$ErrorActionPreference = 'Stop'

# A call to check, not a method definition (fn on_click(...)) and not background_spawn (off the UI thread).
$call = [regex]'(?<!fn\s)\b(?:on_click|on_action|on_key_down|spawn)\('
# Where the call's argument must start: [cx.listener(] [async] [move] |params| [{] guarded(
$guardedClosure = [regex]'\G\s*(?:cx\.listener\(\s*)?(?:async\s+)?(?:move\s+)?\|[^|]*\|\s*(?:\{\s*)?guarded\('

function Find-Unguarded([string]$Root) {
    if (-not (Test-Path $Root)) { return }
    foreach ($file in Get-ChildItem $Root -Recurse -Filter *.rs) {
        $text = Get-Content $file.FullName -Raw
        foreach ($match in $call.Matches($text)) {
            if (-not $guardedClosure.Match($text, $match.Index + $match.Length).Success) {
                '{0}:{1}' -f $file.FullName, ($text.Substring(0, $match.Index).Split("`n").Count)
            }
        }
    }
}

if ($SelfTest) {
    $dir = Join-Path ([IO.Path]::GetTempPath()) "check-guarded-$([guid]::NewGuid())"
    New-Item -ItemType Directory $dir | Out-Null
    try {
        Set-Content (Join-Path $dir 'guarded.rs') @'
pub fn on_click(self, handler: impl Fn()) -> Self { self }
fn view(cx: &mut Context<Shell>) {
    div().on_click(cx.listener(|this, _, window, cx| guarded("send", || this.send(window, cx))));
    div().on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
        guarded("send", || this.send(window, cx))
    }));
    cx.on_action(move |_: &ZoomIn, cx| guarded("zoom in", || zoom(cx)));
    cx.background_spawn(async move { work() }).detach();
    cx.spawn(async move |this, cx| guarded("tick", || tick())).detach();
}
'@
        Set-Content (Join-Path $dir 'planted.rs') @'
fn view(cx: &mut Context<Shell>) {
    div().on_click(|_, _, _| send());
    div().on_key_down(cx.listener(|this, event, window, cx| this.key(event, window, cx)));
    div().on_click(Self::send_clicked);
}
'@
        $planted = Join-Path $dir 'planted.rs'
        $expected = @("${planted}:2", "${planted}:3", "${planted}:4")
        $found = @(Find-Unguarded $dir)
        if (Compare-Object $expected $found) { Write-Host "FAIL: expected $($expected -join ', '); found $($found -join ', ')"; exit 1 }
        Write-Host 'PASS: the planted unguarded handlers were found and the guarded ones passed.'
        exit 0
    } finally { Remove-Item $dir -Recurse -Force }
}

$found = @(Find-Unguarded $Path)
if ($found) {
    $found | ForEach-Object { Write-Host "Unguarded UI handler (start its closure with guarded): $_" }
    exit 1
}
Write-Host "Every UI handler under $Path is guarded."
exit 0
