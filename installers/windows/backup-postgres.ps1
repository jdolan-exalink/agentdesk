param(
    [string]$ContainerName = "agentdesk-postgres",
    [string]$DbUser = "agentdesk",
    [string]$DbName = "agentdesk",
    [string]$BackupDir = "$env:ProgramData\AgentDesk\backups"
)

$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $BackupDir | Out-Null
$ts = Get-Date -Format "yyyyMMdd_HHmmss"
$outFile = Join-Path $BackupDir "agentdesk_${ts}.sql"

docker exec $ContainerName pg_dump -U $DbUser -d $DbName --clean --if-exists | Out-File -FilePath $outFile -Encoding utf8
if ($LASTEXITCODE -ne 0) { throw "pg_dump failed ($LASTEXITCODE)" }

$hash = (Get-FileHash -Algorithm SHA256 -Path $outFile).Hash
[System.IO.File]::WriteAllText("$outFile.sha256", "$hash  $(Split-Path -Leaf $outFile)", [System.Text.UTF8Encoding]::new($false))
Write-Host "Backup completado y verificado:" -ForegroundColor Green
Write-Host "  Archivo: $outFile"
Write-Host "  SHA-256: $hash"
