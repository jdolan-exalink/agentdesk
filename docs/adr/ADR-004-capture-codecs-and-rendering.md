# ADR-004: Captura de Pantalla, Códecs y Renderizado en Windows

- **Estado:** Propuesto (pendiente benchmark M0.5)
- **Fecha:** 2026-11-25
- **Requisitos:** `ENG-01`, `ENG-02`, `ENG-03`, `license-review.md`

## Contexto
TeamViewer `15.83.7.0` separa un servicio Session 0 (`TeamViewer_Service.exe`) de un worker por sesión (`TeamViewer_Desktop.exe`) que gestiona captura, cursor, cambio de escritorio (`Winlogon`/UAC) y privacidad (`MAGNIFICATION.dll`). Necesitamos 1080p60 (4K60 objetivo P2) con mínima latencia y sin dependencias GPL.

## Decisión
1. **Arquitectura de captura en Windows (`crates/capture` + `crates/platform-windows`):**
   - **Ruta primaria:** **DXGI Desktop Duplication (`IDXGIOutputDuplication`)** sobre Direct3D 11 (`windows-rs`), extrayendo únicamente **regiones modificadas (`DirtyRects` / `MoveRects`)** y forma/posición del **cursor separado** para evitar reenviar toda la pantalla cuando solo se mueve el puntero.
   - **Ruta secundaria / fallback:** **Windows Graphics Capture (WGC)** para escenarios con DRM/ventanas específicas o restricciones de DXGI, y **BitBlt GDI** únicamente como último recurso en sesiones sin GPU compatible.
   - **DPI, rotación y multimonitor:** Soporte nativo `PerMonitorV2` en manifiesto (igual que `TeamViewer.exe`), enumeración de salidas DXGI por adaptador y cambio dinámico de monitor sin cortar la sesión.
   - **Secure Desktop / UAC:** Host Service en Session 0 lanza `agentdesk-session.exe` en la sesión WTS activa (`WTSQueryUserToken` + `CreateProcessAsUserW`). Al detectar cambio a escritorio `Winlogon` / UAC, el worker autorizado conmuta mediante `OpenInputDesktop` + `SetThreadDesktop` (o relanza en el escritorio de entrada con privilegios `SYSTEM` en modo Host, respetando la política de UAC en QuickSupport).
2. **Códecs (`crates/codec`):**
   - **P0 (Windows):** **H.264 Low-Latency vía Windows Media Foundation (`IMFTransform`)** aprovechando aceleración GPU (NVENC, Intel QSV, AMD AMF) provista por el driver del sistema operativo, sin incluir `x264` (GPL).
   - **Evaluación paralela M0.5 / P1:** **VP9 / AV1** (`libvpx` / `rav1e` / `dav1d` o MFT AV1 hardware) para endpoints sin encoder hardware H.264 o para futuros clientes web `WebCodecs`.
   - **Audio (`crates/audio`):** Captura loopback **WASAPI** + codificación **Opus** (`BSD-3-Clause`, 20 ms frames).
3. **Renderizado en Visor (`crates/renderer`):**
   - Pipeline **`wgpu` (Direct3D 12 / Vulkan)** presentando texturas YUV/NV12 mediante shader WGSL directamente al swapchain, evitando copias CPU innecesarias.

## Criterios de salida en M0.5
Medir en esta máquina con el prototipo Rust: tiempo de `AcquireNextFrame` + encode MF H.264 + decode + render `wgpu` en 1080p60, verificando latencia end-to-end loopback `≤ 16 ms` (dejando margen amplio para red LAN `≤ 40 ms` p95).
