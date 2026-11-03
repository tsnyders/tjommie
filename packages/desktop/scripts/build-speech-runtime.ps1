$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows' -or [string]::IsNullOrWhiteSpace($env:RUNNER_TEMP)) {
    throw 'The pinned speech runtime builds on Windows GitHub Actions only.'
}

$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../..'))
$commit = 'd1be6fde11ac6e0407606b4e42fe72d34add8037'
$output = Join-Path $repositoryRoot '.cache/speech-runtime'
if (Test-Path -LiteralPath $output) { throw 'The speech runtime output must be absent before a fresh build.' }
$runnerTempPath = [IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd('\', '/')
$nativeScratch = [IO.Path]::GetFullPath((Join-Path $runnerTempPath ('tjommie-whisper-' + [Guid]::NewGuid().ToString('N'))))
New-Item -ItemType Directory -Path $nativeScratch | Out-Null
try {
    $source = Join-Path $nativeScratch 'source'
    $build = Join-Path $nativeScratch 'build'
    & git init $source
    if ($LASTEXITCODE -ne 0) { throw 'Could not initialize the isolated speech source.' }
    & git -C $source remote add origin 'https://github.com/ggml-org/whisper.cpp.git'
    if ($LASTEXITCODE -ne 0) { throw 'Could not set the pinned upstream.' }
    & git -C $source fetch --depth 1 origin $commit
    if ($LASTEXITCODE -ne 0) { throw 'Could not fetch the pinned speech source.' }
    & git -C $source checkout --detach FETCH_HEAD
    if ($LASTEXITCODE -ne 0) { throw 'Could not select the pinned speech source.' }
    $actual = & git -C $source rev-parse HEAD
    if ($LASTEXITCODE -ne 0 -or $actual.Trim() -ne $commit) { throw 'Speech source commit mismatch.' }
    $flags = @(
        '-DBUILD_SHARED_LIBS=OFF', '-DGGML_STATIC=ON', '-DGGML_BACKEND_DL=OFF',
        '-DCMAKE_POLICY_DEFAULT_CMP0091=NEW', '-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded',
        '-DGGML_NATIVE=OFF', '-DGGML_SSE42=OFF', '-DGGML_AVX=OFF', '-DGGML_AVX2=OFF', '-DGGML_BMI2=OFF',
        '-DGGML_OPENMP=OFF', '-DGGML_BLAS=OFF', '-DGGML_CPU=ON',
        '-DGGML_CUDA=OFF', '-DGGML_VULKAN=OFF', '-DGGML_METAL=OFF',
        '-DWHISPER_BUILD_IS_DEV=OFF', '-DWHISPER_BUILD_TESTS=OFF', '-DWHISPER_BUILD_EXAMPLES=ON',
        '-DWHISPER_BUILD_SERVER=OFF', '-DWHISPER_CURL=OFF', '-DWHISPER_SDL2=OFF', '-DWHISPER_COREML=OFF', '-DWHISPER_OPENVINO=OFF'
    )
    & cmake -S $source -B $build -A x64 @flags
    if ($LASTEXITCODE -ne 0) { throw 'Speech runtime CMake configuration failed.' }
    & cmake --build $build --config Release --target whisper-cli --parallel 2
    if ($LASTEXITCODE -ne 0) { throw 'Speech runtime compilation failed.' }
    $binary = Join-Path $build 'bin/Release/whisper-cli.exe'
    if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw 'The CPU x64 speech executable is missing.' }
    if (@(Get-ChildItem -LiteralPath (Split-Path $binary) -Filter '*.dll' -Recurse).Count -ne 0) { throw 'The static speech build unexpectedly produced runtime DLLs.' }
    & $binary --help
    if ($LASTEXITCODE -ne 0) { throw 'The built speech executable did not start.' }
    New-Item -ItemType Directory -Path $output | Out-Null
    Copy-Item -LiteralPath $binary -Destination (Join-Path $output 'whisper-cli.exe')
    Copy-Item -LiteralPath (Join-Path $source 'LICENSE') -Destination (Join-Path $output 'WHISPER-LICENSE.txt')
    Copy-Item -LiteralPath (Join-Path $source 'ggml/LICENSE') -Destination (Join-Path $output 'GGML-LICENSE.txt')
    $openAiLicense = Join-Path $output 'OPENAI-WHISPER-LICENSE.txt'
    Invoke-WebRequest -Uri 'https://raw.githubusercontent.com/openai/whisper/v20250625/LICENSE' -OutFile $openAiLicense
    if ((Get-FileHash -LiteralPath $openAiLicense -Algorithm SHA256).Hash.ToLowerInvariant() -ne 'b5d65a59060e68c4ff940e1eddfa6f94b2d68fdf58ed7f4dd57721c997e35e9d') { throw 'Pinned OpenAI Whisper license text changed.' }
    $notice = "Whisper English model: ggml-base.en.bin`nModel weights: OpenAI, MIT; conversion: ggerganov/whisper.cpp, MIT.`nThe model is downloaded only at the user's request; it is not in this package.`nRevision: 5359861c739e955e79d9a303bcbc70fb988958b1`nBytes: 147964211`nSHA256: a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002`nSource: https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.en.bin`nSee OPENAI-WHISPER-LICENSE.txt for the model license.`n"
    [IO.File]::WriteAllText((Join-Path $output 'MODEL-NOTICE.txt'), $notice, [Text.UTF8Encoding]::new($false))
    $files = @('whisper-cli.exe', 'WHISPER-LICENSE.txt', 'GGML-LICENSE.txt', 'OPENAI-WHISPER-LICENSE.txt', 'MODEL-NOTICE.txt') | ForEach-Object {
        $file = Join-Path $output $_
        @{ name = $_; bytes = (Get-Item -LiteralPath $file).Length; sha256 = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant() }
    }
    $manifest = @{ version = 1; repository = 'https://github.com/ggml-org/whisper.cpp'; commit = $commit; platform = 'win32'; arch = 'x64'; cpuOnly = $true; files = @($files) }
    [IO.File]::WriteAllText((Join-Path $output 'manifest.json'), (($manifest | ConvertTo-Json -Depth 5) + "`n"), [Text.UTF8Encoding]::new($false))
    & node (Join-Path $PSScriptRoot 'check-speech-resources.ts') $output
    if ($LASTEXITCODE -ne 0) { throw 'Speech runtime resource verification failed.' }
} finally {
    if ([IO.Path]::GetDirectoryName($nativeScratch) -ne $runnerTempPath -or [IO.Path]::GetFileName($nativeScratch) -notlike 'tjommie-whisper-*') { throw 'Refusing unowned speech build cleanup.' }
    Remove-Item -LiteralPath $nativeScratch -Recurse -Force -ErrorAction Stop
}
