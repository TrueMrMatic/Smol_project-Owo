# Pins Ruffle git dependencies to the exact commit currently recorded in Cargo.lock.
#
# Usage:
#   1) Build once (or run `cargo generate-lockfile` in rust/bridge) so Cargo.lock exists.
#   2) From repo root, run:  powershell -ExecutionPolicy Bypass -File scripts\pin_ruffle_rev.ps1

$lock = "rust/bridge/Cargo.lock"
$toml = "rust/bridge/Cargo.toml"

if (!(Test-Path $lock)) {
  Write-Error "${lock} not found. Run make once (or cargo generate-lockfile in rust/bridge) first."
  exit 1
}

$txt = Get-Content $lock -Raw
$re = 'git\+https://github\.com/ruffle-rs/ruffle[^#]*#([0-9a-f]{7,40})'
$m = [regex]::Match($txt, $re)
if (!$m.Success) {
  Write-Error "Could not find a ruffle-rs/ruffle git source in ${lock}."
  exit 1
}
$rev = $m.Groups[1].Value
Write-Host "Detected Ruffle rev: $rev"

$tomlText = Get-Content $toml -Raw
if ($tomlText -match 'git = "https://github.com/ruffle-rs/ruffle".*rev =') {
  Write-Host "Cargo.toml already contains a pinned rev. Nothing to do."
  exit 0
}

Copy-Item $toml "$toml.bak" -Force

$tomlText = $tomlText -replace 'git = "https://github.com/ruffle-rs/ruffle"', ('git = "https://github.com/ruffle-rs/ruffle", rev = "' + $rev + '"')
Set-Content -Path $toml -Value $tomlText -NoNewline

Write-Host "Pinned Ruffle git deps in $toml (backup: $toml.bak)"
Write-Host "Tip: commit Cargo.lock + Cargo.toml together for stability."
