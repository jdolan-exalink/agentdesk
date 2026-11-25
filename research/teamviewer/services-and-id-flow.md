# Servicios Windows e Intercambio ID/Contraseña — Evidencia REA en TeamViewer (`15.83.7.0`)

- **Objetivo analizado:** Instalación legítima en `C:\Program Files\TeamViewer`
- **Herramienta:** `rea-agents@6.10.1` (`inspect_pe_imports`, `inspect_pe_resources`) + SCM Windows (`sc.exe qc/qfailure`, `Get-CimInstance Win32_Service`, `Get-NetTCPConnection`, `Get-NetFirewallRule`)
- **Cumplimiento ético (RES-04):** Solo se inspeccionaron binarios distribuidos, tablas PE, configuración pública de servicio SCM, puertos locales loopback y reglas de firewall. No se leyeron logs ni credenciales ni datos de cuenta del usuario.

---

## 1. Evidencias REA Nuevas y Previas

| Artefacto | Operación | `evidence_id` / Fuente | Hallazgo principal |
| :--- | :--- | :--- | :--- |
| `TeamViewer_Service.exe` | `inspect_pe_imports` + `sc.exe qc TeamViewer` | `ev_1408f3784881fbce42522a41a44324940de3f59d96bb8446f7068e369f8a31ab` | Servicio único `TeamViewer` (`WIN32_OWN_PROCESS`, `AUTO_START`, `LocalSystem`), dependencias `RpcSs`, `Tcpip`, `NDIS`, `AFD`, recuperación `restart/2000ms`. Consumo en reposo observado: `29.5 MB` priv en `Session 0`. |
| `TeamViewer.exe` | `inspect_pe_imports` + `inspect_pe_resources` | `ev_94d3b4fc...`, `ev_8d34782d601219160376202f0c8fc29cd0e2799fa77a015a2552fb423dbb20c8` | GUI cliente (`87.87 MB`, recursos `REGISTRY`, `TYPELIB`, íconos, manifiesto `asInvoker` `PerMonitorV2`). En reposo: `280.9 MB` priv en `Session 1`. |
| `TeamViewer_Desktop.exe` | `inspect_pe_imports` | `ev_0599c9c53ebb2b3eff883e359e0abb2e25b5a8f7e8a9ff8dfdfd7e6f9ef071f9` | Worker gráfico por sesión (`OpenInputDesktop`, `SetThreadDesktop`, `BitBlt`, `SendInput`, `MAGNIFICATION.dll`). |
| `TeamViewer_Note.exe` | `inspect_pe_imports` + `inspect_pe_resources` | `ev_e57166b08cf10a40a356e65e820c859b5e07fc8a71c2c0a9104a6ec1e3475a5d`, `ev_aca35f039b50894bbcab12f1f6b89adf26adb98d524fd03f0e4e4f9099c0a91c` | Helper liviano Win32 (`643,920` bytes) para notas de sesión; verifica firma Authenticode (`WINTRUST`, `CRYPT32`). |
| `tv_x64.exe` / `tv_w32.exe` | `Get-Process` + `inspect_pe_imports` | `ev_307672b58ced2bc55fabdbb74fc4ca3323624298fb76d58fc472c8020a629a6b` | Procesos auxiliares ligeros por arquitectura en `Session 1` (`2.2 MB` y `2.9 MB` priv). |
| IPC Local y Firewall | `Get-NetTCPConnection` + `Get-NetFirewallRule` | Observación en vivo | `TeamViewer_Service.exe` (PID 27924, Session 0) escucha en `127.0.0.1:5939`; `TeamViewer.exe` (Session 1) mantiene conexión loopback hacia `127.0.0.1:5939`. Reglas de firewall inbound (`Private` + `Public`) para Service y Application. |

---

## 2. Cómo Replica AgentDesk esta Arquitectura en 100% Rust

| Capacidad observada en TeamViewer | Implementación en AgentDesk (Rust) |
| :--- | :--- |
| **Servicio central de coordinación / relay** | `agentdesk-server.exe` (`--service` SCM `AgentDeskServer`) + `agentdesk-server-console.exe` (Systray + logs en vivo + UAC Firewall/Servicio). |
| **Servicio Windows de acceso desatendido en Session 0** | `agentdesk-host.exe --service` (SCM `AgentDeskHost`, `AUTO_START`, recuperación `restart/2000`, acceso a `OpenInputDesktop` / `SetThreadDesktop` + captura DXGI/GDI). |
| **Cliente dual con ID numérico + contraseña aleatoria al abrir** | `agentdesk-desktop.exe` al iniciar sin login obligatorio: carga identidad `Ed25519` DPAPI, genera contraseña aleatoria CSPRNG de 8 caracteres, llama `POST /api/v1/clients/register` para obtener un **ID estable de 9 dígitos (`XXX XXX XXX`)**, abre `/ws/agent` para recibir sesiones y permite conectar a otros pares vía `POST /api/v1/sessions/connect-by-id` con túnel E2EE `Noise_XX`. |
| **Firewall multi-perfil (`Private`/`Public`/`Domain`)** | `installers/windows/allow-firewall.ps1` + botón **Permitir en Firewall (UAC)** en `agentdesk-server-console.exe` creando reglas `Profile Any` para TCP `8080` y binario. |
