$ErrorActionPreference = "Stop"

$Version = "23.4"
$Asset = "protoc-$Version-win64.zip"
$ExpectedSha256 = "a309c39442fb75f0db343cb22c111a00f91cdf0767f332e170644b9378e2bcc6"
$InstallRoot = Join-Path $env:RUNNER_TEMP "protoc-$Version"
$Archive = Join-Path $env:RUNNER_TEMP $Asset
$Url = "https://github.com/protocolbuffers/protobuf/releases/download/v$Version/$Asset"

Invoke-WebRequest -Uri $Url -OutFile $Archive
$ActualSha256 = (Get-FileHash -Path $Archive -Algorithm SHA256).Hash.ToLowerInvariant()
if ($ActualSha256 -ne $ExpectedSha256) {
    throw "Checksum mismatch for $Asset"
}

if (Test-Path $InstallRoot) {
    Remove-Item -Recurse -Force $InstallRoot
}
Expand-Archive -Path $Archive -DestinationPath $InstallRoot -Force
$BinPath = Join-Path $InstallRoot "bin"
$BinPath | Out-File -FilePath $env:GITHUB_PATH -Encoding utf8 -Append
& (Join-Path $BinPath "protoc.exe") --version
