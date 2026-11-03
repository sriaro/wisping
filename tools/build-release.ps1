$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
$sysroot = & rustc --print sysroot
if ($LASTEXITCODE -ne 0 -or -not $sysroot) { throw 'Unable to locate the Rust sysroot.' }
$sysroot = $sysroot.Trim()
$flags = @('-C', 'target-feature=+crt-static', '-C', 'control-flow-guard=yes', '-C', 'link-arg=/PDBALTPATH:wisping.pdb')
foreach ($pair in @(@($root, 'wisping'), @($cargoHome, 'cargo'), @($sysroot, 'rust'))) {
    $flags += "--remap-path-prefix=$($pair[0])=$($pair[1])"
}
$env:CARGO_ENCODED_RUSTFLAGS = $flags -join [char]0x1f
Push-Location $root
try {
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw "Release build failed with exit code $LASTEXITCODE." }
} finally { Pop-Location }
