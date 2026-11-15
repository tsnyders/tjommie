<#
The UI boundary rule (Phase 3 plan T0.4, widened by the M0 review's F5): in crates/desktop/src, every handler
registration runs its whole closure inside guarded( (or guarded_async( for a spawned task), so a panic in it becomes a
"Something went wrong" toast instead of a crash. A registration is any on_* method, observe*, subscribe*, spawn,
spawn_in, defer or defer_in call. A closure inside cx.listener(...) counts; a method passed by name doesn't (wrap it in
a guarded closure). Nothing may follow the guarded call but .await, a semicolon and the closing brace. Not checked:
std::thread::spawn and background_spawn (off the UI thread, where a panic ends only that thread), Command::spawn(),
method definitions. A registration that must not be guarded says why on the line above it: // unguarded: <reason>

    .\tools\check-guarded.ps1             # from tjommie-rs: lists each unguarded call and exits 1
    .\tools\check-guarded.ps1 -SelfTest   # this script, on planted handlers
#>
param([string]$Path = (Join-Path (Split-Path $PSScriptRoot -Parent) 'crates\desktop\src'), [switch]$SelfTest)
$ErrorActionPreference = 'Stop'

# A registration call with at least one argument, maybe with a turbofish; not `fn on_click(` and not `thread::spawn(`.
$call = [regex]'(?<!fn\s)(?<!thread::)\b(?:on_[a-z_]+|observe[a-z_]*|subscribe[a-z_]*|spawn(?:_in)?|defer(?:_in)?)(?:::<[^>]*>)?\((?!\s*\))'
# Where the closure must start, after any plain leading arguments (window, &entity, MouseButton::Left):
# [cx.listener(] [async] [move] |params| [{] guarded( or guarded_async(
$guardedStart = [regex]'\G\s*(?:[^,|(){}]+,\s*)*(?:cx\.listener\(\s*)?(?:async\s+)?(?:move\s+)?\|[^|]*\|\s*(?<brace>\{\s*)?guarded(?:_async)?\('
# After the guarded call's closing parenthesis: [.await], then the end of the closure.
$tailBrace = [regex]'\G\s*(?:\.await)?\s*;?\s*\}\s*\)'
$tailBare = [regex]'\G\s*(?:\.await)?\s*\)'

# The index just past the parenthesis that closes the one open before $start, skipping strings, chars and comments.
function Find-Close([string]$text, [int]$start) {
    $depth = 1
    for ($i = $start; $i -lt $text.Length; $i++) {
        $c = $text[$i]
        if ($c -eq '"') {
            for ($i++; $i -lt $text.Length -and $text[$i] -ne '"'; $i++) { if ($text[$i] -eq '\') { $i++ } }
        } elseif ($c -eq "'") {
            # A char literal ('(' or '\''); a lifetime ('a) is left alone.
            if ($i + 2 -lt $text.Length -and $text[$i + 2] -eq "'") { $i += 2 }
            elseif ($i + 1 -lt $text.Length -and $text[$i + 1] -eq '\') { $i = $text.IndexOf("'", $i + 3); if ($i -lt 0) { return -1 } }
        } elseif ($c -eq '/' -and $i + 1 -lt $text.Length -and $text[$i + 1] -eq '/') {
            $i = $text.IndexOf("`n", $i); if ($i -lt 0) { return -1 }
        } elseif ($c -eq '(') {
            $depth++
        } elseif ($c -eq ')') {
            $depth--; if ($depth -eq 0) { return $i + 1 }
        }
    }
    return -1
}

# Whether the line above the one at $index gives a reason not to guard: // unguarded: <reason>
function Test-Exempt([string]$text, [int]$index) {
    $lineStart = $text.LastIndexOf("`n", [Math]::Max($index - 1, 0))
    if ($lineStart -le 0) { return $false }
    $above = $text.LastIndexOf("`n", $lineStart - 1) + 1
    return $text.Substring($above, $lineStart - $above) -match '^\s*//\s*unguarded:\s*\S'
}

function Find-Unguarded([string]$Root) {
    if (-not (Test-Path $Root)) { return }
    foreach ($file in Get-ChildItem $Root -Recurse -Filter *.rs) {
        $text = Get-Content $file.FullName -Raw
        foreach ($match in $call.Matches($text)) {
            if (Test-Exempt $text $match.Index) { continue }
            $start = $guardedStart.Match($text, $match.Index + $match.Length)
            $guarded = $start.Success
            if ($guarded) {
                $close = Find-Close $text ($start.Index + $start.Length)
                $tail = if ($start.Groups['brace'].Success) { $tailBrace } else { $tailBare }
                $guarded = $close -ge 0 -and $tail.Match($text, $close).Success
            }
            if (-not $guarded) {
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
    div().on_click(cx.listener(|this, _, window, cx| {
        guarded("send (a; b) // c", || this.send(f(')'), &'a', window, cx));
    }));
    div().on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| guarded("press", || this.press(cx))));
    let hold = cx.on_focus_lost(window, |this, window, cx| {
        guarded("hold focus", || this.focus.focus(window, cx));
    });
    cx.observe_global::<Theme>(|cx| guarded("theme", || cx.refresh_windows())).detach();
    cx.on_action(move |_: &ZoomIn, cx| guarded("zoom in", || zoom(cx)));
    cx.background_spawn(async move { work() }).detach();
    cx.spawn(async move |this, cx| guarded_async("tick", async move { tick().await }).await).detach();
    cx.spawn(async move |cx| {
        guarded_async("serve", async move {
            serve(cx).await;
        })
        .await
    })
    .detach();
    std::thread::spawn(move || serve());
    let child = Command::new("x").spawn()?;
    // unguarded: the notice sink
    cx.spawn(async move |cx| { loop { show().await } }).detach();
}
'@
        Set-Content (Join-Path $dir 'planted.rs') @'
fn view(cx: &mut Context<Shell>) {
    div().on_click(|_, _, _| send());
    div().on_key_down(cx.listener(|this, event, window, cx| this.key(event, window, cx)));
    div().on_click(Self::send_clicked);
    div().on_mouse_down(MouseButton::Left, |_, _, _| press());
    div().on_click(cx.listener(|this, _, _, cx| { guarded("a", || this.a(cx)); this.b(cx) }));
    div().on_click(|_, _, cx| guarded("a", || a(cx)).map(|v| c(v)));
    cx.spawn(async move |cx| { loop { tick().await } }).detach();
    cx.subscribe(&entity, |this, _, event, cx| this.handle(event, cx)).detach();
    // unguarded:
    cx.spawn(async move |cx| { loop { show().await } }).detach();
}
'@
        $planted = Join-Path $dir 'planted.rs'
        $expected = @(2, 3, 4, 5, 6, 7, 8, 9, 11 | ForEach-Object { "${planted}:$_" })
        $found = @(Find-Unguarded $dir)
        if (Compare-Object $expected $found) { Write-Host "FAIL: expected $($expected -join ', '); found $($found -join ', ')"; exit 1 }
        Write-Host 'PASS: the planted unguarded handlers were found and the guarded ones passed.'
        exit 0
    } finally { Remove-Item $dir -Recurse -Force }
}

$found = @(Find-Unguarded $Path)
if ($found) {
    $found | ForEach-Object { Write-Host "Unguarded handler (run its whole closure in guarded): $_" }
    exit 1
}
Write-Host "Every handler under $Path is guarded."
exit 0
