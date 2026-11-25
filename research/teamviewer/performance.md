# Rendimiento y Benchmarks — TeamViewer (`15.83.7.0`) vs. Metas del PRD

- **Base de evidencia:** [evidence.md](file:///d:/DEVs/AgentDesk/research/teamviewer/evidence.md)
- **Hardware / SO medido:** Windows 11 x64 (`LocalSystem` servicio + Session 1 usuario)

## 1. Mediciones observadas en reposo (Observado)

Muestra tomada en 3 intervalos de 2 segundos (`Get-Process "*TeamViewer*"`) sin sesiones remotas activas:

| Proceso | Sesión | Memoria Privada (MB) | Working Set (MB) | Handles | Hilos | Delta CPU (4s) | Meta PRD Sección 6 |
| :--- | :---: | ---: | ---: | ---: | ---: | ---: | :--- |
| `TeamViewer_Service.exe` | 0 | **29.12 MB** | 44.02 MB | 665 | 51 | ~0.00 s | Host inactivo **≤ 40 MB priv** y **≤ 0.5% CPU** |
| `TeamViewer.exe` (GUI) | 1 | **280.24 MB** | 265.46 MB | 1,528 | 109 | ~0.09 s | — (GUI principal) |
| **Total residente (Host + GUI)** | 0+1 | **309.36 MB** | **309.48 MB** | **2,193** | **160** | — | Host unattended debe correr sin GUI abierta |

**Lectura crítica para nuestra arquitectura:**
- `TeamViewer_Service.exe` cumple el umbral de **≤ 40 MB** (`29.12 MB` privados) precisamente porque delega la UI a `TeamViewer.exe` y el trabajo gráfico de sesión a `TeamViewer_Desktop.exe` (que solo se lanza bajo demanda o en sesión activa).
- Por el contrario, `TeamViewer.exe` residente eleva el consumo total a **~309 MB** por su pila UI (`sciter.dll` + `WebView2` + recursos). En nuestro **Host desatendido**, el servicio Windows (`agentdesk-host`) debe mantenerse estrictamente en **< 25 MB** privados en reposo (sin runtime web embebido), lanzando el worker gráfico (`agentdesk-session`) únicamente cuando exista una sesión remota activa.

## 2. Métricas en sesión activa (No verificado en este hito)

Las siguientes métricas de TeamViewer y competidores **no se han medido aún** en laboratorio controlado (requieren VM aislada con red controlada / emulador de pérdida según `RES-01` y Sección 6 del PRD):

| Métrica | TeamViewer `15.83.7.0` | Meta PRD (Propia) | Plan de medición (M0.5 / M2) |
| :--- | :---: | :---: | :--- |
| Latencia LAN input-to-photon (p95) | **No verificado** | **≤ 40 ms** (1080p60) | Timestamp de `SendInput` vs detección de cambio de píxel en visor receptor (hardware reference clock / loop local) |
| Tiempo de conexión LAN tras autorización (p95) | **No verificado** | **≤ 2 s** | Cronometrado desde `POST /sessions` aprobado hasta primer keyframe renderizado |
| FPS sostenido 1080p / 4K con GPU | **No verificado** | **1080p60** (4K60 P2) | Contador de frames decodificados/presentados en `wgpu` durante reproducción de video + escritorio |
| Ancho de banda con pérdida 1–5% y RTT 20–150 ms | **No verificado** | Adaptativo sin congelamientos | `clumsy` / `tc netem` en VM Windows/Linux comparando QUIC datagrams/streams vs WebRTC |
| Huella en disco de instalación completa | **~290 MB** (`C:\Program Files\TeamViewer` + `Update\`) — **Observado** | **< 40 MB** (Host/Full) | Medición directa de artefactos MSI/EXE compilados en release LTO |

## 3. Mediciones reales del prototipo AgentDesk M0.5 (Observado el 2026-11-25)

Ejecutado en este equipo Windows 11 x64 (`2560×1440`) con `target\release\agentdesk-host.exe --benchmark 30` (`crates/capture` DXGI Desktop Duplication + `image` JPEG calidad 70):

| Métrica | Valor medido (AgentDesk M0.5) | Objetivo PRD (M2) | Conclusión técnica |
| :--- | :--- | :--- | :--- |
| **Backend captura** | `dxgi` (`IDXGIOutputDuplication` tras `OpenInputDesktop` + `SetThreadDesktop`) | DXGI + WGC fallback | **Validado:** DXGI funciona incluso desde sesiones/escritorios separados al adjuntar el thread al escritorio de entrada. |
| **Resolución probada** | `2560×1440` | `1920×1080` (y 4K60 P2) | Captura completa nativa 1440p sin errores. |
| **FPS efectivos / tiempo por frame** | **`20.3 FPS`** · avg `48.6 ms` · p95 `50.62 ms` (JPEG CPU) | **60 FPS** · p95 **≤ 40 ms** | El cuello de botella actual es **codificación JPEG por CPU** a 1440p (`~313.9 KB/frame`). Confirma `ADR-004`: migrar a **H.264 hardware (Media Foundation) + dirty rects** en M2 para alcanzar 60 FPS y ≤ 40 ms. |
| **Memoria privada durante captura activa** | `82.76 MB` (incluye staging texture 1440p + buffer RGBA) | Host inactivo **≤ 40 MB** | Separar servicio Session 0 (inactivo sin buffers DXGI) del worker Session N mantiene el servicio base liviano. |
