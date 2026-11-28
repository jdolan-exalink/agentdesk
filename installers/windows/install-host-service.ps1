param(
    [string]$ServerUrl = "http://10.1.1.156:8080",
    [string]$EnrollToken = "",
    [string]$BinaryPath = "",
    [string]$ServiceName = "AgentDeskHost"
)

$ErrorActionPreference = "Stop"

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host "ERROR: Este script requiere permisos de Administrador." -ForegroundColor Red
    Write-Host "Abre PowerShell como Administrador y ejecuta:" -ForegroundColor Yellow
    Write-Host "  powershell -ExecutionPolicy Bypass -File `"$PSCommandPath`" -ServerUrl `"$ServerUrl`" -EnrollToken `"<TOKEN>`"" -ForegroundColor Cyan
    exit 1
}

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Resolve-Path (Join-Path $scriptDir "..\..")
if ([string]::IsNullOrWhiteSpace($BinaryPath)) {
    $BinaryPath = Join-Path $repoRoot "target\release\agentdesk-host.exe"
}
if (-not (Test-Path $BinaryPath)) {
    throw "No se encontró el binario del agente en: $BinaryPath"
}

$programFilesDir = Join-Path $env:ProgramFiles "AgentDesk\host"
$programDataLogs = Join-Path $env:ProgramData "AgentDesk\logs"
New-Item -ItemType Directory -Force -Path $programFilesDir | Out-Null
New-Item -ItemType Directory -Force -Path $programDataLogs | Out-Null

$targetExe = Join-Path $programFilesDir "agentdesk-host.exe"
Copy-Item -Force $BinaryPath $targetExe

& $targetExe --configure-server $ServerUrl
if (-not [string]::IsNullOrWhiteSpace($EnrollToken)) {
    # El token NO va en la linea de comandos del servicio (ImagePath es legible por cualquier
    # usuario local): queda en un archivo de un solo uso, solo SYSTEM y Administradores pueden
    # leerlo, y el servicio lo borra al enrolarse en el primer arranque.
    $tokDir = Join-Path $env:ProgramData "AgentDesk"
    New-Item -ItemType Directory -Force -Path $tokDir | Out-Null
    $tokFile = Join-Path $tokDir "host-enroll.token"
    Set-Content -Path $tokFile -Value $EnrollToken.Trim() -NoNewline -Encoding ASCII
    & icacls.exe $tokFile /inheritance:r /grant:r "*S-1-5-18:F" "*S-1-5-32-544:F" | Out-Null
    Write-Host "Token de enrolamiento guardado para el primer arranque del servicio."
}

$existing = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
if ($existing) {
    if ($existing.Status -ne "Stopped") {
        Stop-Service -Name $ServiceName -Force -ErrorAction SilentlyContinue
        $existing.WaitForStatus("Stopped", [TimeSpan]::FromSeconds(15))
    }
    & sc.exe delete $ServiceName | Out-Null
    Start-Sleep -Seconds 2
}

$binArg = "`"$targetExe`" --service --server `"$ServerUrl`" --auto-accept"
& sc.exe create $ServiceName "binPath=" $binArg "start=" auto "depend=" "Tcpip" "DisplayName=" "AgentDesk Host Service" | Out-Null
if ($LASTEXITCODE -ne 0) { throw "Fallo sc.exe create ($LASTEXITCODE)" }
& sc.exe description $ServiceName "Agente desatendido AgentDesk (Session 0) para soporte remoto E2EE." | Out-Null
& sc.exe failure $ServiceName "reset=" 86400 "actions=" "restart/2000/restart/2000/restart/5000" | Out-Null

Start-Service -Name $ServiceName
(Get-Service -Name $ServiceName).WaitForStatus("Running", [TimeSpan]::FromSeconds(15))
Write-Host "Servicio '$ServiceName' instalado y ejecutándose (estilo TeamViewer_Service.exe)." -ForegroundColor Green
