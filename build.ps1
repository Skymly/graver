$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

cargo test --workspace
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

dotnet build .\Graver.slnx
exit $LASTEXITCODE
