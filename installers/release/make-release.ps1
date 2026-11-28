# Arma un release de AgentDesk en target\release-X.Y.Z:
#   exes versionados + servidor Linux + manifiesto firmado del actualizador
#   + deploy.ps1 / deploy-remote.sh listos para subir a 192.168.99.8.
#
# Uso (desde la raíz del repo, con la versión ya subida en Cargo.toml y su
# sección en CHANGELOG.md):
#   powershell -ExecutionPolicy Bypass -File installers\release\make-release.ps1
#   powershell -ExecutionPolicy Bypass -File installers\release\make-release.ps1 -SkipLinux
#
# La firma usa la clave de %LOCALAPPDATA%\AgentDesk\release-signing.key (DPAPI de
# este usuario de Windows). Sin esa clave los clientes no aceptan la actualización.
param(
    [switch]$SkipBuild,
    [switch]$SkipLinux,
    [string]$Server = 'root@192.168.99.8'
)
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path "$PSScriptRoot\..\..").Path
Set-Location $Root

$V = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
if (-not $V) { throw 'No encontré la versión en Cargo.toml' }
if (-not (Select-String -Path CHANGELOG.md -Pattern "^## $([regex]::Escape($V))\b" -Quiet)) {
    throw "CHANGELOG.md no tiene sección '## $V'. Escribila antes de publicar."
}
$Out = Join-Path $Root "target\release-$V"
New-Item -ItemType Directory -Force $Out | Out-Null
Write-Host "== AgentDesk $V -> $Out"

if (-not $SkipBuild) {
    cargo build --release -p agentdesk-desktop-gpui -p agentdesk-host -p agentdesk-quicksupport -p agentdesk-identity --bin agentdesk-desktop-gpui --bin agentdesk-host --bin agentdesk-quicksupport --bin agentdesk-release-sign
    if ($LASTEXITCODE -ne 0) { throw 'cargo build falló' }
}
Copy-Item target\release\agentdesk-desktop-gpui.exe "$Out\agentdesk-desktop-v$V-x64.exe" -Force
Copy-Item target\release\agentdesk-host.exe "$Out\agentdesk-host-v$V-x64.exe" -Force
Copy-Item target\release\agentdesk-quicksupport.exe "$Out\agentdesk-quicksupport-v$V-x64.exe" -Force

if (-not $SkipLinux) {
    $env:MSYS_NO_PATHCONV = '1'
    docker run --rm -v "${Root}:/src" -v agentdesk-cargo-cache:/usr/local/cargo/registry -v agentdesk-linux-target:/target -e CARGO_TARGET_DIR=/target -w /src rust:1-bookworm sh -c "cargo build --release -p agentdesk-server && cp /target/release/agentdesk-server /src/target/release-$V/agentdesk-server"
    if ($LASTEXITCODE -ne 0) { throw 'build Linux del servidor falló' }
}

# Manifiesto firmado del actualizador del cliente.
& target\release\agentdesk-release-sign.exe sign --exe "$Out\agentdesk-desktop-v$V-x64.exe" --version $V --changelog CHANGELOG.md --out "$Out\agentdesk-desktop-update.json"
if ($LASTEXITCODE -ne 0) { throw 'firma del manifiesto falló' }

$deploy = @"
# AgentDesk ${V}: sube servidor + exes + manifiesto a $Server y los instala (con respaldo).
`$ErrorActionPreference = 'Stop'
`$R = '$Server'
`$D = '$Out'
ssh `$R 'mkdir -p /root/agentdesk-$V-staging'
scp "`$D\agentdesk-server" "`$D\agentdesk-desktop-v$V-x64.exe" "`$D\agentdesk-host-v$V-x64.exe" "`$D\agentdesk-quicksupport-v$V-x64.exe" "`$D\agentdesk-desktop-update.json" "`$D\deploy-remote.sh" "`${R}:/root/agentdesk-$V-staging/"
ssh `$R "sed -i 's/\r`$//' /root/agentdesk-$V-staging/deploy-remote.sh && bash /root/agentdesk-$V-staging/deploy-remote.sh"
"@
Set-Content -Path "$Out\deploy.ps1" -Value $deploy -Encoding UTF8

$remote = @'
#!/usr/bin/env bash
# AgentDesk __V__ deploy (corre en el servidor). Staging: /root/agentdesk-__V__-staging
set -euo pipefail
V=__V__
STG=/root/agentdesk-$V-staging
ART=/opt/agentdesk/artifacts
BK=/root/agentdesk-backups/$(date +%Y%m%d-%H%M%S)-pre-$V

echo "== Respaldo en $BK"
mkdir -p "$BK"
cp -a /usr/local/bin/agentdesk-server "$BK/"
cp -a /etc/systemd/system/agentdesk-server.service "$BK/" 2>/dev/null || true
for f in agentdesk-desktop.exe agentdesk-host.exe agentdesk-quicksupport.exe agentdesk-desktop-update.json; do
  [ -f "$ART/$f" ] && cp -a "$ART/$f" "$BK/" || true
done
ls -la "$BK"

echo "== Instalando $V"
for c in desktop host quicksupport; do
  install -m 0644 "$STG/agentdesk-$c-v$V-x64.exe" "$ART/agentdesk-$c-v$V-x64.exe"
  install -m 0644 "$STG/agentdesk-$c-v$V-x64.exe" "$ART/agentdesk-$c.exe"
done
install -m 0755 "$STG/agentdesk-server" /usr/local/bin/agentdesk-server.new
mv -f /usr/local/bin/agentdesk-server.new /usr/local/bin/agentdesk-server
systemctl restart agentdesk-server
sleep 3
# El manifiesto va al final: los clientes solo ven la versión nueva cuando el
# servidor ya sirve el exe que anuncia.
install -m 0644 "$STG/agentdesk-desktop-update.json" "$ART/agentdesk-desktop-update.json"

echo "== Verificacion"
systemctl is-active agentdesk-server
for c in desktop host quicksupport; do
  echo "$c $(sha256sum "$ART/agentdesk-$c-v$V-x64.exe" | cut -c1-16)"
done
echo "manifiesto $(sha256sum "$ART/agentdesk-desktop-update.json" | cut -c1-16)"

echo "== Borrando versiones viejas"
find "$ART" -maxdepth 1 -name 'agentdesk-*-v*-x64.exe' ! -name "*-v$V-x64.exe" -print -delete
find /root -maxdepth 1 -type d -name 'agentdesk-*-staging' ! -name "agentdesk-$V-staging" -print -exec rm -rf {} +
ls -1dt /root/agentdesk-backups/*/ 2>/dev/null | tail -n +4 | while read -r d; do echo "$d"; rm -rf "$d"; done

echo "== Listo. Volver atras: cp -a $BK/agentdesk-server /usr/local/bin/ && cp -a $BK/*.exe $BK/*.json $ART/ && systemctl restart agentdesk-server"
'@
$remote = $remote.Replace('__V__', $V)
[IO.File]::WriteAllText("$Out\deploy-remote.sh", ($remote -replace "`r`n", "`n"))

Get-ChildItem $Out | Format-Table Name, Length
Write-Host "== Para publicar:  powershell -ExecutionPolicy Bypass -File `"$Out\deploy.ps1`""
