# Backlog Maestro Trazable (`docs/backlog.md`)

Trazabilidad completa de todos los requisitos del PRD Maestro v1.0 organizados por hito (`M0`–`M4`), con IDs, prioridades, dependencias y criterios de aceptación verificables.

---

## Hito M0 — Investigación, Arquitectura y Prototipos (Gate de inicio)

| ID | Título | Prioridad | Dependencias | Criterios de aceptación |
| :--- | :--- | :---: | :--- | :--- |
| `RES-01..04` | Laboratorio REA e investigación TeamViewer `15.83.7.0` | P0 | — | Evidencia REA guardada con `evidence_id`, SHA-256, clasificación `Observado/Documentado/Inferido/No verificado`, sin leer logs/secretos locales (**Completado en M0**). |
| `M0-MAT` | Matriz competitiva y revisión de licencias | P0 | `RES-01..04` | `research/feature-matrix.md` y `research/license-review.md` aprobados sin dependencias AGPL/GPL bloqueantes. |
| `M0-SEC` | Threat model y ADRs iniciales | P0 | `M0-MAT` | `docs/security/threat-model.md` + `ADR-001..005` revisados y aprobados. |
| `M0-PROTO` | Prototipo M0.5 captura DXGI + MF H.264 + QUIC/wgpu | P0 | `ADR-001..004` | Benchmark reproducible en Windows x64 midiendo FPS, latencia loopback/LAN, RAM en reposo y pérdida 1–5% antes de iniciar M2. |

---

## Hito M1 — Fundaciones, Identidad y Portal Web (`P0`)

| ID | Título | Prioridad | Dependencias | Criterios de aceptación |
| :--- | :--- | :---: | :--- | :--- |
| `OPS-01..03` | Empaquetado servidor Linux/Windows + PostgreSQL + Asistente inicial | P0 | `ADR-001` | Instalación limpia en Debian 13/Ubuntu 24.04 y Windows sin servicios externos obligatorios; check DNS/TLS/DB, backup cifrado y restore probado en servidor limpio. |
| `IAM-01` | Bootstrap de instancia y gestión de usuarios web | P0 | `OPS-01` | Secreto de primer inicio crea `instance_admin`; alta, invitación, bloqueo, baja lógica y revocación de sesiones operan desde `/admin`. |
| `IAM-02` | RBAC granular + MFA + denegación por defecto | P0 | `IAM-01` | 7 roles implementados; separación administración vs acceso a equipos; MFA TOTP funcional; tests negativos de escalada de privilegios. |
| `IAM-03` | Jerarquía multi-tenant Org → Cliente → Sede → Grupo → Dispositivo | P0 | `IAM-02`, `SEC-04` | Etiquetas y carpetas virtuales sin duplicar dispositivo; tests negativos prueban que técnico de empresa A no descubre ni accede a recursos de empresa B. |
| `IAM-04` | Agenda personal/compartida e importación CSV | P0 | `IAM-03` | Favoritos, recientes, contactos, alias, búsqueda e import/export CSV; cero contraseñas remotas en texto plano. |
| `SEC-02`, `SEC-04` | Argon2id, cookies seguras, CSRF, rate limit, auditoría append-only | P0 | `IAM-01` | Auditoría UTC inmutable registra actor, alcance, acción, correlación y resultado; sin secretos en logs. |
| `WEB-01..04` | Portal público: Inicio, Centro de Descargas, Docs editables, Branding | P0 | `OPS-01`, `IAM-01` | Rutas `/`, `/downloads`, `/docs`; muestra SHA-256/firma; branding configurable se refleja en web; páginas de docs auditadas. |
| `REL-01` | Dashboard de releases y verificación de firma/hash | P0 | `WEB-02`, `SEC-05` | Subida/importación CI, validación SHA-256/Ed25519, canales estable/beta y retiro de release. |
| `M1-DASH` | Dashboard administrativo base (Leptos SSR/WASM, estilo Linear) | P0 | `IAM-02`, `WEB-01` | Tema claro/oscuro, `Ctrl+K`, 4 indicadores accionables, tablas densas, usable en `1366×768` y móvil. |

---

## Hito M2 — Motor Remoto MVP y Asistencia Web E2E (`P0`)

| ID | Título | Prioridad | Dependencias | Criterios de aceptación |
| :--- | :--- | :---: | :--- | :--- |
| `SEC-01`, `SEC-03` | Identidad Ed25519 de dispositivo, E2EE Noise y permisos granulares | P0 | `ADR-003`, `ADR-005` | Relay ciego verificado (captura PCAP muestra payload cifrado); consentimiento local y botón Terminar visibles. |
| `ENG-01` | Captura DXGI Desktop Duplication + WGC fallback + multimonitor + `wgpu` | P0 | `M0-PROTO` | 1080p60 en GPU compatible, cursor separado, cambio de monitor y soporte DPI/rotación. |
| `ENG-02` | Códec H.264 Media Foundation + bitrate/FPS adaptativo + audio WASAPI/Opus | P0 | `ENG-01` | Calidad adaptativa estable bajo pérdida simulada 1–5% y RTT 20–150 ms. |
| `ENG-03` | Control teclado/mouse, portapapeles, servicio Host Session 0 + UAC/Secure Desktop | P0 | `ENG-01` | Host inactivo **≤ 40 MB RAM privada** y **≤ 0.5% CPU**; soporta UAC/bloqueo/inicio de sesión usando APIs documentadas Windows. |
| `ENG-04` | Transporte LAN directo, P2P NAT traversal y Relay TLS 443 fallback | P0 | `SEC-01` | Conexión LAN p95 **≤ 2 s**; latencia LAN input-to-photon p95 **≤ 40 ms**; reconexión ante cambio de red. |
| `ENG-05` | Transferencia de archivos reanudable con SHA-256, chat, cambio calidad/monitor | P0 | `ENG-04` | Reanudación verificada tras corte; verificación de hash post-transferencia y límites por política. |
| `WEB-03`, `REL-02` | Paquetes preconfigurados (URL + pin), enrolamiento Host y auto-update con rollback | P0 | `REL-01`, `SEC-05` | Firmado Authenticode; manifiesto Ed25519; actualización respeta sesiones activas y hace rollback ante fallo. |
| `SUP-01..06`, `WEB-05`, `TKT-01(P0)` | Flujo completo Web → Ticket → QuickSupport/Deep Link → Sesión → Cierre | P0 | `M1-DASH`, `ENG-04` | Solicitud atómica con token alta entropía; reclamo atómico sin doble asignación; SSE/WS en tiempo real; cierre revoca acceso temporal sin persistencia; ≤ 3 acciones desde Inicio. |

---

## Hito M3 — Plataforma Profesional: Calendario, RMM, Tickets Completos e IA (`P1`)

| ID | Título | Prioridad | Dependencias | Criterios de aceptación |
| :--- | :--- | :---: | :--- | :--- |
| `WEB-06`, `WEB-07` | Portal autenticado de clientes y página pública de estado agregada | P1 | `M2` | Cliente ve solo sus dispositivos/tickets/citas; `/support/status` no expone topología interna ni datos de terceros. |
| `IAM-05` | AD/LDAP, OIDC, passkeys y aprovisionamiento de grupos | P1 | `IAM-02` | Login OIDC/LDAP funcional con cuenta de recuperación local protegida y revocación al desvincular grupo. |
| `CAL-01`, `CAL-02` | Calendario de técnicos + CalDAV, Google Calendar y Microsoft Graph | P1 | `IAM-03` | Detección de conflictos en reservas simultáneas; sincronización idempotente vía OAuth oficial de cada proveedor. |
| `TKT-01(P1)`, `TKT-02`, `REP-01` | Tickets completos, SLA, notas internas, reportes CSV/PDF y webhooks/SMTP | P1 | `SUP-01..06` | Flujo solicitud → ticket → sesión → tiempo trabajado → resolución visible; webhooks firmados sin secretos. |
| `RMM-01`, `RMM-02`, `RMM-03` | Inventario diferencial, métricas/alertas con histéresis y Terminal PowerShell/CMD | P1 | `ENG-03` | Inventario hardware/software/AV/updates; alertas deduplicadas; terminal auditable con redacción y elevación separada. |
| `AI-01..05` | Copiloto IA (OpenAI API + Ollama local), redacción PII, niveles de aprobación y resúmenes | P1 | `RMM-01`, `TKT-02`, `SEC-04` | Funciona con API oficial y local; modo sin IA 100% operativo; rechaza acciones no autorizadas; `ChatGPT OAuth` documentado según elegibilidad comercial. |
| `SUP-07`, `SEC-06` | Lanzamiento nativo desde web, retención/exportación/eliminación y grabaciones cifradas | P1 | `M2` | Políticas de retención ejecutables y verificables; grabaciones cifradas en endpoint con consentimiento explícito. |

---

## Hito M4 — Capacidades Enterprise y Alta Disponibilidad (`P2`)

| ID | Título | Prioridad | Dependencias | Criterios de aceptación |
| :--- | :--- | :---: | :--- | :--- |
| `ENG-06` | Audio bidireccional, anotaciones, colaboración/traspaso, impresión, WoL, túneles, headless IDD | P2 | `M2`, `M3` | Driver IDD firmado WHQL/EV para headless; túneles limitados a destino/puerto aprobado por política. |
| `SUP-07(P2)` | Evaluación visor web WebRTC/WebCodecs con seguridad equivalente | P2 | `ENG-02`, `SEC-01` | PoC evaluada garantizando E2EE y sin degradar modelo de permisos. |
| `RMM-04` | Scripts versionados/firmados por lotes, ventanas de mantenimiento y parches por anillos | P2 | `RMM-03`, `REL-01` | Ejecución por lotes con timeout, cancelación y rollback cuando aplique. |
| `AI-06` | Servidor MCP propio para agentes externos (delegado, scopes, auditoría) | P2 | `AI-04`, `IAM-02` | Herramientas tipadas con aprobación humana fuera del modelo. |
| `OPS-04` | Core replicado HA (objetivo 99,95%), relays regionales y observabilidad Prometheus | P2 | `M1..M3` | Pruebas de carga por niveles (100 → 20,000 dispositivos) y failover validado en laboratorio. |
