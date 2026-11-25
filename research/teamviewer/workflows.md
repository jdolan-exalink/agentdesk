# Flujos Operativos — TeamViewer vs. Propuesta Propia

- **Producto analizado:** TeamViewer Desktop Windows `15.83.7.0`
- **Base de evidencia:** [evidence.md](file:///d:/DEVs/AgentDesk/research/teamviewer/evidence.md)

## 1. Arquitectura de procesos observada en Windows

```
[Session 0 - LocalSystem]
  TeamViewer_Service.exe (29.1 MB priv)
     ├── Red / Control / WinHTTP Proxy / DPAPI / MSI Updates
     ├── WTSEnumerateSessionsW + WTSQueryUserToken
     └── CreateProcessAsUserW ──► [Session N - Usuario interactivo / Winlogon]
                                     ├── TeamViewer_Desktop.exe (captura, input, clipboard, Secure Desktop)
                                     ├── tv_x64.exe / tv_w32.exe (helpers arquitectura)
                                     └── TeamViewer.exe (GUI Sciter/WebView2, 280.2 MB priv)
```

- **Clasificación:** Los binarios, sesiones (`SessionId 0` y `1`), dependencias del servicio e imports WTS/Token/Desktop son **Observados** (`ev_1408f378...`, `ev_0599c9c5...`, `sc.exe qc`). La coordinación IPC exacta entre `TeamViewer_Service.exe` y `TeamViewer_Desktop.exe` es **Inferida** (pipes nombrados observados en imports `api-ms-win-core-namedpipe-l1-1-0.dll`).

## 2. Flujo de atención puntual vs. acceso desatendido

### Cómo opera TeamViewer (Observado / Documentado)
1. **Acceso desatendido (Host / Servicio):** El servicio en Session 0 arranca con Windows (`AUTO_START`), mantiene conectividad saliente y puede proyectar cualquier sesión interactiva incluyendo pantalla de bloqueo y prompts UAC mediante `TeamViewer_Desktop.exe` (`OpenInputDesktop`, `SetThreadDesktop`). (**Observado** en imports y `sc.exe`; el detalle de enrolamiento en la nube es **Documentado**).
2. **Soporte puntual tradicional (ID + contraseña):** Requiere dictar un ID numérico y contraseña temporal o compartir un enlace de sesión generado desde la consola cloud del fabricante. (**Documentado**).
3. **Actualizaciones (`Update\update.exe`):** Incluye un binario separado de `85.66 MB` dentro de `Update\`, casi del mismo tamaño que `TeamViewer.exe` (`87.87 MB`), duplicando huella en disco. (**Observado**).

### Fricciones detectadas y propuesta propia (P0)
1. **Eliminar el intercambio telefónico de ID/contraseña (`SUP-01..06`):**
   - El usuario entra a `/support` en el portal del servidor → completa formulario + consentimiento → obtiene código público `SUP-XXXX` y descarga **QuickSupport portable (~5–8 MB estimado, objetivo sin runtime WebView)** o abre deep link `agentdesk://support?token=...`.
   - En la cola del Dashboard (`/app/support`), el técnico reclama el ticket de forma atómica y presiona **Conectar** (**<= 3 clics desde Inicio**).
   - QuickSupport muestra identidad verificada (certificado + nombre de organización y técnico), lista exacta de permisos solicitados y botón rojo persistente **Terminar sesión**. Al cerrar, se revoca el token efímero sin dejar servicio instalado.
2. **Separación estricta de binarios sin duplicar 85 MB en `Update/`:**
   - `agentdesk-host.exe` (servicio Session 0 liviano, objetivo `<= 40 MB`) + `agentdesk-session.exe` (worker Session N).
   - Actualizador incremental diferencial/firmado compartido en `crates/updater` sin empaquetar un segundo ejecutable completo de 85 MB en disco.
3. **UAC y Secure Desktop limpio (`ENG-03`):**
   - Replicar el patrón correcto observado: servicio en Session 0 lanzando worker en Session N; para interactuar con prompts UAC en `Winlogon` desktop, utilizar binario firmado con manifiesto `uiAccess="true"` instalado en `Program Files` o transición autorizada de escritorio de entrada (`OpenInputDesktop` + `SetThreadDesktop`) en modo Host/elevado, **sin** evadir UAC en QuickSupport no elevado (en QuickSupport el usuario local aprueba UAC explícitamente si requiere soporte elevado).
