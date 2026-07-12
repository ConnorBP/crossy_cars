# Load project-local .env into the current PowerShell process.
# Usage: . ./tools/load_env.ps1
$EnvFile = Join-Path $PSScriptRoot "../.env"
if (-not (Test-Path $EnvFile)) { throw "Missing .env; copy .env.example and fill values." }
Get-Content $EnvFile | ForEach-Object {
  $line = $_.Trim()
  if ($line -and -not $line.StartsWith("#") -and $line.Contains("=")) {
    $name, $value = $line.Split("=", 2)
    [Environment]::SetEnvironmentVariable($name.Trim(), $value, "Process")
  }
}
Write-Host "Loaded local Roady Car environment variables from .env"
