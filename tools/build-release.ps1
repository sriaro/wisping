$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
$sysroot = (& rustc --print sysroot).Trim()
$flags = @('-C', 'target-feature=+crt-static', '-C', 'link-arg=/PDBALTPATH:wisp.pdb')
foreach ($pair in @(@($root, 'wisp'), @($cargoHome, 'cargo'), @($sysroot, 'rust'))) {
    $flags += "--remap-path-prefix=$($pair[0])=$($pair[1])"
}
$env:CARGO_ENCODED_RUSTFLAGS = $flags -join [char]0x1f
Push-Location $root
try { cargo build --release } finally { Pop-Location }
