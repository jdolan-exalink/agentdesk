param(
    [string]$ServiceName = "AgentDeskServer",
    [int]$Port = 8080
)

$ErrorActionPreference = "Stop"

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host "ERROR: Este script requiere permisos de Administrador." -ForegroundColor Red
    exit 1
}

$existing = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
if ($existing) {
    if ($existing.Status -ne "Stopped") {
        Stop-Service -Name $ServiceName -Force -ErrorAction SilentlyContinue
        $existing.WaitForStatus("Stopped", [TimeSpan]::FromSeconds(15))
    }
    & sc.exe delete $ServiceName | Out-Null
    Write-Host "Servicio '$ServiceName' eliminado." -ForegroundColor Green
} else {
    Write-Host "Servicio '$ServiceName' no estaba instalado."
}

$fwRuleName = "AgentDesk Server TCP $Port"
if (Get-NetFirewallRule -DisplayName $fwRuleName -ErrorAction SilentlyContinue) {
    Remove-NetFirewallRule -DisplayName $fwRuleName
    Write-Host "Regla de Firewall '$fwRuleName' eliminada."
}
