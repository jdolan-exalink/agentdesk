param(
    [int]$Port = 8080,
    [string]$BinaryPath = "",
    [switch]$NoElevate
)

$ErrorActionPreference = "Stop"

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
$isAdmin = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

if (-not $isAdmin) {
    if ($NoElevate) {
        Write-Host "ERROR: Se requieren permisos de Administrador para modificar el Firewall de Windows." -ForegroundColor Red
        exit 1
    }
    $argsList = @("-ExecutionPolicy", "Bypass", "-File", "`"$PSCommandPath`"", "-Port", $Port)
    if (-not [string]::IsNullOrWhiteSpace($BinaryPath)) {
        $argsList += @("-BinaryPath", "`"$BinaryPath`"")
    }
    Write-Host "Solicitando elevación (UAC) para habilitar las reglas de Firewall TCP $Port..." -ForegroundColor Yellow
    $proc = Start-Process -FilePath "powershell.exe" -ArgumentList $argsList -Verb RunAs -Wait -PassThru
    exit $proc.ExitCode
}

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Resolve-Path (Join-Path $scriptDir "..\..")
if ([string]::IsNullOrWhiteSpace($BinaryPath)) {
    $candidate = Join-Path $repoRoot "target\release\agentdesk-server.exe"
    if (Test-Path $candidate) {
        $BinaryPath = (Resolve-Path $candidate).Path
    }
}

$tcpRuleName = "AgentDesk Server TCP $Port"
$appRuleName = "AgentDesk Server App"

foreach ($name in @($tcpRuleName, $appRuleName)) {
    Get-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue | Remove-NetFirewallRule -ErrorAction SilentlyContinue
}

New-NetFirewallRule `
    -DisplayName $tcpRuleName `
    -Direction Inbound `
    -Protocol TCP `
    -LocalPort $Port `
    -Action Allow `
    -Profile Any `
    -Description "Permite conexiones entrantes al servidor AgentDesk en puerto TCP $Port" | Out-Null

Write-Host "Regla de Firewall creada: $tcpRuleName (TCP $Port, perfiles Any)" -ForegroundColor Green

if (-not [string]::IsNullOrWhiteSpace($BinaryPath) -and (Test-Path $BinaryPath)) {
    $resolvedBin = (Resolve-Path $BinaryPath).Path
    New-NetFirewallRule `
        -DisplayName $appRuleName `
        -Direction Inbound `
        -Program $resolvedBin `
        -Action Allow `
        -Profile Any `
        -Description "Permite conexiones entrantes para $resolvedBin" | Out-Null
    Write-Host "Regla de Firewall creada: $appRuleName ($resolvedBin, perfiles Any)" -ForegroundColor Green
}
