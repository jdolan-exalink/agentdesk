# Matriz Competitiva y Propuesta de Valor (`research/feature-matrix.md`)

**Leyenda de clasificación (exigida por PRD Sección 1):**
- **Observado:** Verificado directamente mediante REA / inspección binaria o proceso en este equipo (`TeamViewer 15.83.7.0`).
- **Documentado:** Respaldado por documentación oficial pública del proveedor / repositorio.
- **Inferido:** Deducido técnicamente a partir de dependencias/arquitectura observada.
- **No verificado:** Pendiente de medición en laboratorio (VM + captura en vivo).

---

## 1. Comparativa funcional y arquitectónica

| Dimensión | TeamViewer (`15.83.7.0`) | AnyDesk | RustDesk (OSS / Pro) | ScreenConnect | MeshCentral | NinjaOne | Splashtop / Parsec | **Nuestra Plataforma (Rust)** |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **Self-hosting integral (web + relay + auth)** | No (nube del fabricante) `[Documentado]` | No (cloud; on-prem solo enterprise) `[Documentado]` | Sí (OSS `hbbs`/`hbbr`, consola web solo en Pro) `[Documentado]` | Sí (Windows/Linux servidor único) `[Documentado]` | Sí (Node.js todo-en-uno) `[Documentado]` | No (SaaS) `[Documentado]` | No (SaaS) `[Documentado]` | **Sí (P0): binario único modular Rust + PostgreSQL** |
| **Licencia / Riesgo reutilización** | Propietario `[Observado]` | Propietario `[Documentado]` | **AGPL-3.0** (OSS) / Pro comercial `[Documentado]` | Propietario `[Documentado]` | Apache-2.0 `[Documentado]` | Propietario `[Documentado]` | Propietario `[Documentado]` | **Clean-room Rust; sin copiar código AGPL** |
| **Stack nativo / UI cliente** | C++ + `sciter.dll` + `WebView2` (`280 MB` RAM reposo GUI) `[Observado]` | C++ nativo ligero `[Documentado]` | Rust + Flutter (`~100+ MB` runtime) `[Documentado]` | .NET + clientes nativos `[Documentado]` | Node.js + Web UI (sin cliente nativo rápido) `[Documentado]` | Agente nativo + Web `[Documentado]` | Nativo GPU/Direct3D de baja latencia `[Documentado]` | **Rust puro: Host <25 MB + web Leptos SSR/WASM + desktop nativo (`wgpu`)** |
| **Asistencia web sin cuenta (Web → Ticket → QuickSupport)** | Enlaces de sesión cloud `[Documentado]` | Enlace/ID manual `[Documentado]` | ID + contraseña manual `[Documentado]` | Excelente flujo web de códigos/invitados `[Documentado]` | Enlaces de invitación web `[Documentado]` | Mediante ticket + agente `[Documentado]` | SOS app con código 9 dígitos `[Documentado]` | **P0: Portal público + ticket atómico + token seguimiento + QuickSupport firmado** |
| **Multi-tenant / Jerarquía** | Licencias por compañía `[Documentado]` | Address book por equipo `[Documentado]` | Grupos/tags en Pro `[Documentado]` | Grupos de sesión por cliente `[Documentado]` | Dominios / grupos de dispositivos `[Documentado]` | Org → Sede → Política `[Documentado]` | Equipos/carpetas `[Documentado]` | **P0: Org → Cliente → Sede → Grupo + etiquetas/carpetas sin duplicar equipo** |
| **Mesa de ayuda + Calendario integrado** | Integraciones externas / Servicecamp `[Documentado]` | No nativo `[Documentado]` | No nativo `[Documentado]` | Básico (extensiones) `[Documentado]` | No `[Documentado]` | Ticketing RMM `[Documentado]` | No `[Documentado]` | **P0/P1: Tickets integrados + CalDAV/Google/Graph + conflictos de técnicos** |
| **RMM + Terminal + Scripts** | Add-on pago `[Documentado]` | No `[Documentado]` | No `[Documentado]` | Backstage/Shell nativo `[Documentado]` | Terminal/archivos/AMT `[Documentado]` | Referente RMM completo `[Documentado]` | Básico `[Documentado]` | **P1/P2: Inventario diferencial, métricas, terminal PowerShell/CMD, scripts firmados** |
| **IA copiloto + Redacción PII + MCP** | Asistente de resumen (cloud) `[Documentado]` | No `[Documentado]` | No `[Documentado]` | No `[Documentado]` | No `[Documentado]` | IA básica en tickets `[Documentado]` | No `[Documentado]` | **P1/P2: OpenAI API + Ollama local + redacción PII + motor de permisos + MCP propio** |
| **E2EE con Relay Ciego** | Cifrado extremo a extremo documentado `[Documentado]` | TLS + RSA `[Documentado]` | NaCl/Curve25519 `[Documentado]` | TLS/AES `[Documentado]` | TLS (servidor termina sesión web) `[Documentado]` | TLS `[Documentado]` | DTLS/AES `[Documentado]` | **P0: Ed25519 + Noise IK/XX E2EE; el relay jamás ve claves ni video** |
| **UX Admin (referente Linear)** | Densa pero fragmentada (`Sciter` + web) `[Observado/Documentado]` | Minimalista pero sin consola RMM `[Documentado]` | Funcional básica en Pro `[Documentado]` | Potente pero interfaz legacy `[Documentado]` | Funcional pero estética antigua `[Documentado]` | Compleja/densa `[Documentado]` | Orientada a sesión `[Documentado]` | **Estilo Fluent/Linear: `Ctrl+K`, tablas densas, tema claro/oscuro, ≤3 clics** |

---

## 2. Comparativa de métricas operativas y rendimiento

| Indicador | TeamViewer `15.83.7.0` | Referentes rápidos (Parsec / AnyDesk / ScreenConnect) | Meta Nuestra Plataforma | Estado de verificación |
| :--- | :--- | :--- | :--- | :--- |
| **Clics para conectar a equipo desde Inicio** | 2–3 clics desde lista de dispositivos `[Documentado]` | 1–2 clics (AnyDesk/Parsec); 2–3 en web (ScreenConnect) `[Documentado]` | **≤ 3 acciones** desde Inicio (Dashboard o `Ctrl+K`) | **Documentado / Meta P0** |
| **RAM privada Host/Servicio en reposo** | `29.12 MB` (`TeamViewer_Service.exe`), `+280.24 MB` si GUI abierta `[Observado]` | `No verificado` en este equipo | **≤ 40 MB** servicio inactivo (objetivo `< 25 MB`) | **Observado (TV)** / **Meta P0** |
| **CPU promedio Host en reposo** | `~0.0%` en intervalo 4s `[Observado]` | `No verificado` | **≤ 0.5%** | **Observado (TV)** / **Meta P0** |
| **Latencia LAN input-to-photon (p95)** | `No verificado` | Parsec referencia baja latencia GPU `[Documentado]` | **≤ 40 ms** (1080p60 GPU) | **No verificado** (medir en M0.5) |
| **Tiempo de conexión LAN tras autorización (p95)** | `No verificado` | `< 2 s` típico en LAN `[Documentado]` | **≤ 2 s** | **No verificado** (medir en M0.5) |
| **Soporte bajo pérdida 1–5% y RTT 20–150 ms** | `No verificado` | `No verificado` | Adaptación dinámica de bitrate/FPS sin caída de input | **No verificado** (medir en M0.5/M2) |
