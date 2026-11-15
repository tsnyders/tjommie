<#
The UI boundary rule (Phase 3 plan T0.4, widened by the M0 review's F5): in crates/desktop/src, every handler
registration runs its whole closure inside guarded( (or guarded_async( for a spawned task), so a panic in it becomes a
"Something went wrong" toast instead of a crash. A registration is a method call: .on_*, .observe*, .subscribe*,
.spawn, .spawn_in, .defer or .defer_in. A closure inside cx.listener(...) counts; a method passed by name doesn't (wrap
it in a guarded closure). Nothing may follow the guarded call but .await, a semicolon and the closing brace. Not
checked: our primitives' .on_press and .on_toggle (ui/primitives runs them inside its own guarded handlers; gpui-kit
has neither name), std::thread::spawn and background_spawn (off the UI thread, where a panic ends only that thread),
Command::spawn(), definitions, and calls of a callback held in a variable (on_press(window, cx)). A registration that
must not be guarded says why on the line above it: // unguarded: <reason>

    .\tools\check-guarded.ps1             # from tjommie-rs: lists each unguarded call and exits 1
    .\tools\check-guarded.ps1 -SelfTest   # this script, on planted handlers
#>
param([string]$Path = (Join-Path (Split-Path $PSScriptRoot -Parent) 'crates\desktop\src'), [switch]$SelfTest)
$ErrorActionPreference = 'Stop'

# A registration: a method call with at least one argument, maybe with a turbofish, but not a primitive's builder.
$call = [regex]'(?<=\.)(?!(?:on_press|on_toggle)\()(?:on_[a-z_]+|observe[a-z_]*|subscribe[a-z_]*|spawn(?:_in)?|defer(?:_in)?)(?:::<[^>]*>)?\((?!\s*\))'
# Where the closure must start, after any plain leading arguments (window, &entity, MouseButton::Left):
# [cx.listener(] [async] [move] |params| [{] guarded( or guarded_async(
$guardedStart = [regex]'\G\s*(?:[^,|(){}]+,\s*)*(?:cx\.listener\(\s*)?(?:async\s+)?(?:move\s+)?\|[^|]*\|\s*(?<brace>\{\s*)?guarded(?:_async)?\('
# After the guarded call's closing parenthesis: [.await], then the end of the closure.
$tailBrace = [regex]'\G\s*(?:\.await)?\s*;?\s*\}\s*\)'
$tailBare = [regex]'\G\s*(?:\.await)?\s*\)'

# The rule is for GPUI handlers (a panic in one aborts the process). A file that has no GPUI in it has none: the view models
# (src/model) are plain Rust with their own subscribe, on_* and spawn methods, and are exempt. What counts as GPUI is the name
# (use gpui::..., gpui_kit) or the one handler idiom a file can have through a re-export, cx.listener(. Comments don't count.
$gpuiReference = [regex]'\bgpui|\bcx\.listener\('
function Remove-Comments([string]$text) { [regex]::Replace($text, '//[^\r\n]*', '') }  # keeps the newlines: lines still line up

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
        if (-not $gpuiReference.IsMatch((Remove-Comments $text))) { continue }  # no GPUI here: nothing to guard
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

# The exemption above is only sound while the models stay GPUI-free, so that is checked too, not assumed: every file under
# <root>/model that names gpui or uses cx.listener( is listed. (A comment that mentions gpui is not a use of it.)
function Find-GpuiInModel([string]$Root) {
    $model = Join-Path $Root 'model'
    if (-not (Test-Path $model)) { return }
    foreach ($file in Get-ChildItem $model -Recurse -Filter *.rs) {
        $code = Remove-Comments (Get-Content $file.FullName -Raw)
        foreach ($match in $gpuiReference.Matches($code)) {
            '{0}:{1}' -f $file.FullName, ($code.Substring(0, $match.Index).Split("`n").Count)
        }
    }
}

if ($SelfTest) {
    $dir = Join-Path ([IO.Path]::GetTempPath()) "check-guarded-$([guid]::NewGuid())"
    New-Item -ItemType Directory $dir | Out-Null
    try {
        Set-Content (Join-Path $dir 'guarded.rs') @'
use gpui::prelude::*; pub fn on_click(self, handler: impl Fn()) -> Self { self }
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
    div().on_click(move |_, window, cx| guarded("press", || on_press(window, cx)));
    Pill::new("cancel", "Cancel").on_press(|window, cx| window.close_dialog(cx));
    Toggle::new("wifi", "Wi-Fi", on).on_toggle(move |on, _, cx| set(on, cx));
    // unguarded: the notice sink
    cx.spawn(async move |cx| { loop { show().await } }).detach();
}
'@
        Set-Content (Join-Path $dir 'planted.rs') @'
use gpui::prelude::*; fn view(cx: &mut Context<Shell>) {
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
    Slider::new("volume").on_change(|value, _, cx| set(value, cx));
    Popover::new("menu").on_open_change(move |open, _, cx| { shield(*open, cx) });
}
'@
        # The rule is for GPUI handlers. A file that does not use GPUI (the view models) is exempt, however its own
        # on_*, subscribe and spawn methods are named ...
        Set-Content (Join-Path $dir 'plain.rs') @'
fn wire(store: &mut Store, env: &Env) {
    store.on_change(|state| state.touch());
    store.subscribe(move |event| event.apply());
    env.spawn(async move { poll().await });
}
'@
        # ... but a file that does is still held to it,
        Set-Content (Join-Path $dir 'gpuiview.rs') @'
use gpui::Context;
fn view(cx: &mut Context<Shell>) {
    div().on_click(|_, _, _| send());
}
'@
        # even when it only has GPUI through a re-export and shows it by its handler idiom, cx.listener(.
        Set-Content (Join-Path $dir 'reexport.rs') @'
use crate::ui::prelude::*;
fn view(cx: &mut Context<Shell>) {
    div().on_click(cx.listener(|this, _, window, cx| this.send(window, cx)));
}
'@
        # The exemption is only safe while the models stay GPUI-free: a file under model/ that names gpui or uses its
        # handler idiom fails the check. A comment that mentions gpui is not a use of it.
        $model = Join-Path $dir 'model'
        New-Item -ItemType Directory $model | Out-Null
        Set-Content (Join-Path $model 'ok.rs') @'
// gpui is deliberately not used here: gpui::App would make this file a UI file.
fn wire(store: &mut Store) { store.on_change(|state| state.touch()); }
'@
        Set-Content (Join-Path $model 'leak.rs') @'
use gpui::App;
fn f(app: &App) {}
'@
        Set-Content (Join-Path $model 'listener.rs') @'
fn f(cx: &mut Context<Shell>) {
    let handler = cx.listener(|this, _, _, _| ());
}
'@
        $planted = Join-Path $dir 'planted.rs'
        $expected = @(@(2, 3, 4, 5, 6, 7, 8, 9, 11, 12, 13 | ForEach-Object { "${planted}:$_" }) + @("$(Join-Path $dir 'gpuiview.rs'):3", "$(Join-Path $dir 'reexport.rs'):3"))
        $found = @(Find-Unguarded $dir)
        if (Compare-Object $expected $found) { Write-Host "FAIL: expected $($expected -join ', '); found $($found -join ', ')"; exit 1 }
        $expectedModel = @("$(Join-Path $model 'leak.rs'):1", "$(Join-Path $model 'listener.rs'):2")
        $foundModel = @(Find-GpuiInModel $dir)
        if (Compare-Object $expectedModel $foundModel) { Write-Host "FAIL: expected GPUI in model at $($expectedModel -join ', '); found $($foundModel -join ', ')"; exit 1 }
        Write-Host 'PASS: the planted unguarded handlers were found, the guarded and the GPUI-free ones passed, and GPUI in model/ was found.'
        exit 0
    } finally { Remove-Item $dir -Recurse -Force }
}

$found = @(Find-Unguarded $Path)
$leaks = @(Find-GpuiInModel $Path)
if ($found -or $leaks) {
    $found | ForEach-Object { Write-Host "Unguarded handler (run its whole closure in guarded): $_" }
    $leaks | ForEach-Object { Write-Host "GPUI in the view models (src/model must stay GPUI-free, or the guard exemption is unsound): $_" }
    exit 1
}
Write-Host "Every GPUI handler under $Path is guarded, and src/model is GPUI-free."
exit 0
