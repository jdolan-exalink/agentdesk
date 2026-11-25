param(
    [string]$BindAddr = "0.0.0.0:8080",
    [string]$DatabaseUrl = "postgres://agentdesk:agentdesk_dev_pass@127.0.0.1:5432/agentdesk",
    [string]$BinaryPath = "",
    [string]$ServiceName = "AgentDeskServer",
    [string]$BootstrapToken = "agentdesk-local-setup-2026",
    [string]$AdminEmail = "juan@dolan.com.ar",
    [string]$AdminPassword = "admin123",
    [switch]$InPlace
)

$ErrorActionPreference = "Stop"

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host "ERROR: Este script requiere permisos de Administrador." -ForegroundColor Red
    Write-Host "Abre PowerShell como Administrador y ejecuta:" -ForegroundColor Yellow
    Write-Host "  powershell -ExecutionPolicy Bypass -File `"$PSCommandPath`"" -ForegroundColor Cyan
    exit 1
}

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Resolve-Path (Join-Path $scriptDir "..\..")
if ([string]::IsNullOrWhiteSpace($BinaryPath)) {
    $BinaryPath = Join-Path $repoRoot "target\release\agentdesk-server.exe"
}
if (-not (Test-Path $BinaryPath)) {
    throw "No se encontró el binario del servidor en: $BinaryPath. Ejecuta 'cargo build --release --workspace' primero."
}

$sourceDir = Split-Path -Parent (Resolve-Path $BinaryPath)
$programFilesDir = Join-Path $env:ProgramFiles "AgentDesk\server"
$programDataDir = Join-Path $env:ProgramData "AgentDesk"
New-Item -ItemType Directory -Force -Path $programDataDir | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $programDataDir "logs") | Out-Null

if ($InPlace) {
    $targetExe = (Resolve-Path $BinaryPath).Path
    $artifactsDir = $sourceDir
} else {
    New-Item -ItemType Directory -Force -Path $programFilesDir | Out-Null
    Copy-Item -Force $BinaryPath (Join-Path $programFilesDir "agentdesk-server.exe")
    foreach ($extra in @("agentdesk-server-console.exe", "agentdesk-host.exe", "agentdesk-desktop.exe")) {
        $extraSrc = Join-Path $sourceDir $extra
        if (Test-Path $extraSrc) {
            Copy-Item -Force $extraSrc (Join-Path $programFilesDir $extra)
        }
    }
    $targetExe = Join-Path $programFilesDir "agentdesk-server.exe"
    $artifactsDir = $programFilesDir
}

$configPath = Join-Path $programDataDir "server_config.json"
$configObj = [ordered]@{
    bind_addr = $BindAddr
    database_url = $DatabaseUrl
    artifacts_dir = $artifactsDir
    bootstrap_token = $BootstrapToken
    auto_seed_admin = $true
    default_org_name = "AgentDesk"
    default_org_slug = "agentdesk"
    default_admin_email = $AdminEmail
    default_admin_name = "Juan Dolan"
    default_admin_password = $AdminPassword
}
$json = $configObj | ConvertTo-Json -Depth 4
[System.IO.File]::WriteAllText($configPath, $json, [System.Text.UTF8Encoding]::new($false))
Write-Host "Configuración escrita en: $configPath"

$existing = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
if ($existing) {
    Write-Host "Deteniendo y reemplazando servicio existente '$ServiceName'..."
    if ($existing.Status -ne "Stopped") {
        Stop-Service -Name $ServiceName -Force -ErrorAction SilentlyContinue
        $existing.WaitForStatus("Stopped", [TimeSpan]::FromSeconds(15))
    }
    & sc.exe delete $ServiceName | Out-Null
    Start-Sleep -Seconds 2
}

$binArg = "`"$targetExe`" --service --config `"$configPath`""
& sc.exe create $ServiceName "binPath=" $binArg "start=" auto "DisplayName=" "AgentDesk Remote Support Server" | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "Fallo sc.exe create ($LASTEXITCODE)"
}
& sc.exe description $ServiceName "AgentDesk central coordination server, REST API, Web Portal, and E2EE Noise_XX relay." | Out-Null
& sc.exe failure $ServiceName "reset=" 86400 "actions=" "restart/5000/restart/10000/restart/30000" | Out-Null

$port = 8080
if ($BindAddr -match ":(\d+)$") {
    $port = [int]$Matches[1]
}

$fwScript = Join-Path $scriptDir "allow-firewall.ps1"
if (Test-Path $fwScript) {
    & $fwScript -Port $port -BinaryPath $targetExe -NoElevate
} else {
    $fwRuleName = "AgentDesk Server TCP $port"
    Get-NetFirewallRule -DisplayName $fwRuleName -ErrorAction SilentlyContinue | Remove-NetFirewallRule -ErrorAction SilentlyContinue
    New-NetFirewallRule -DisplayName $fwRuleName -Direction Inbound -Protocol TCP -LocalPort $port -Action Allow -Profile Any | Out-Null
    Write-Host "Regla de Firewall creada: $fwRuleName (Profile Any)"
}

Start-Service -Name $ServiceName
$svc = Get-Service -Name $ServiceName
$svc.WaitForStatus("Running", [TimeSpan]::FromSeconds(15))

$healthUrl = "http://127.0.0.1:$port/health/ready"
try {
    $resp = Invoke-RestMethod -Uri $healthUrl -TimeoutSec 10
    Write-Host "Servicio '$ServiceName' activo y saludable: $($resp | ConvertTo-Json -Compress)" -ForegroundColor Green
} catch {
    Write-Warning "Servicio '$ServiceName' iniciado, pero falló verificación en ${healthUrl}: $_"
}
