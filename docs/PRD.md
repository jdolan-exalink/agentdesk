# PRD Maestro — Plataforma de Soporte Remoto Inteligente

- **Versión:** v1.0
- **Fecha:** 25 de noviembre de 2026
- **Destinatario:** Claude Code y equipo de desarrollo
- **Nombre comercial:** Pendiente
- **Modelo:** Producto self-hosted, multiempresa, con motor nativo Rust y portal web integrado

## Visión

Construir una alternativa profesional a TeamViewer, AnyDesk y RustDesk, con administración centralizada, mesa de ayuda, agenda, RMM e inteligencia artificial. El servidor debe ofrecer el sitio del producto, las descargas, la asistencia al usuario y toda la gestión administrativa desde una instalación única. El servicio remoto no dependerá de una nube del fabricante.

## Decisiones de alcance

- Servidor Linux y Windows; clientes Windows x64 inicialmente, incluido Windows Server como endpoint sujeto a pruebas. Arquitectura preparada para ARM64, macOS, Linux y móviles.
- Rust para lógica de servidor, motor y clientes; frontend web Rust/WASM con HTML/CSS y puentes JS mínimos cuando sean necesarios. Sin Electron como opción por defecto.
- **P0:** producto utilizable de extremo a extremo, incluyendo portal, identidad, agenda de dispositivos, asistencia web y motor remoto.
- **P1:** agenda de trabajo, IA, tickets ampliados, RMM.
- **P2:** funciones avanzadas y HA.
- La web es la fuente de verdad de la administración; los clientes nativos consumen la misma API. No duplicar reglas ni bases de datos.

## Mandato al equipo de desarrollo

Investigar antes de implementar, registrar evidencia y decisiones (ADR), construir prototipos medibles, entregar por hitos con tests reales. No declarar funcionalidades listas si son mocks. Separar requisitos obligatorios de hipótesis técnicas; elevar decisiones de licencia, privacidad y seguridad para aprobación.

---

## 1. Investigación obligatoria con REA

Repositorio oficial: `https://github.com/morluto/rea`. README consultado mediante GitHub el 25/11/2026. REA permite investigar aplicaciones mediante MCP o CLI, con evidencia y limitaciones; el análisis se realiza localmente, aunque el agente puede transmitir resultados a su proveedor de modelo.

### Preparación del laboratorio
- **RES-01:** Usar una VM Windows aislada con snapshots, cuentas de prueba y aplicaciones obtenidas legítimamente. Registrar versión, licencia, fecha y hash de cada instalador.
- **RES-02:** Instalar Node.js/npm en entorno confiable. Ejecutar `npx rea-agents setup`; seleccionar cliente, revisar y aprobar cambios, reiniciar el agente. Fijar la versión probada de REA para reproducibilidad.
- **RES-03:** Comprobar disponibilidad del MCP y proveedores. Para análisis nativo, seleccionar Ghidra, Hopper, IDA o Kuna según formato, plataforma y licencia. No asumir que todos son necesarios.
- **RES-04:** Investigar primero documentación y comportamiento observable; usar análisis más profundo únicamente cuando esté autorizado. No copiar código propietario, evadir licencias, extraer credenciales ni investigar sistemas ajenos sin autorización.

### Matriz de investigación
- **TeamViewer:** portal, QuickSupport, Host, agenda, permisos, sesiones y soporte IA.
- **AnyDesk:** conexión rápida, UX, calidad adaptativa, reconexión.
- **RustDesk:** self-hosting, rendezvous, relay, dispositivos y distribución; revisar AGPL y licencias de cada componente antes de reutilizar.
- **ScreenConnect, MeshCentral y NinjaOne:** consola, inventario, cola de soporte, administración masiva.
- **Splashtop y Parsec:** experiencia de video, latencia y uso de recursos.
- **Linear:** navegación, búsqueda y densidad visual.

### Entregables y puerta de salida
- `research/<producto>/{features,workflows,ui,performance,evidence}.md`
- `research/feature-matrix.md`
- `research/license-review.md`
- `docs/adr/`
- Cada hallazgo: versión, fuente, pasos reproducibles, evidencia, limitación y propuesta propia. Clasificar `Observado` / `Documentado` / `Inferido` / `No verificado`. Comparar clics, tiempos, CPU/RAM, resolución, latencia y ancho de banda. No iniciar implementación definitiva hasta aprobar matriz y prototipos.

---

## 2. Portal público del producto

El servidor incluye una web responsive, con branding configurable, accesible desde el dominio de cada instalación. Puede publicarse en Internet o restringirse a LAN/VPN. La consola privada puede residir en otro subdominio u origen sin cambiar el producto.

- **WEB-01 (P0):** Inicio: propuesta del producto, funciones, plataformas, botón Descargar, Solicitar asistencia e Iniciar sesión. Logo, colores, textos, datos de soporte y pie configurables.
- **WEB-02 (P0):** Centro de descargas: Full Client, Host MSI y QuickSupport portable. Mostrar arquitectura, versión, fecha, tamaño, notas, checksum SHA-256 y firma del editor. Detectar plataforma sin impedir selección manual.
- **WEB-03 (P0):** Los paquetes deben venir preconfigurados con URL del servidor y ancla de confianza verificada. No incluir secretos permanentes. Los Host personalizados usan invitaciones de enrolamiento de alcance limitado, expirables y revocables.
- **WEB-04 (P0):** Documentación y ayuda: instalación, inicio rápido, permisos, acceso desatendido, solución de problemas, privacidad y contacto. Páginas editables desde administración, con vista previa y publicación auditada.
- **WEB-05 (P0):** Página de asistencia pública con formulario, validación de contacto según política, código de seguimiento, descarga QuickSupport y estado de solicitud. No requiere cuenta para soporte puntual si el administrador lo permite.
- **WEB-06 (P1):** Portal autenticado de clientes: sus dispositivos autorizados, solicitudes, mensajes, adjuntos, citas, historial y documentos; posibilidad de invitar usuarios de su organización si poseen permiso.
- **WEB-07 (P1):** Página de estado de servicios con información agregada sin revelar topología interna ni identificadores de clientes. Historial de releases estables; beta solo para usuarios habilitados.

Rutas propuestas: `/` • `/downloads` • `/docs` • `/support` • `/support/status` • `/login` • `/portal` • `/app` • `/admin`. API versionada `/api/v1`.

---

## 3. Solicitud de asistencia desde la web

- **SUP-01 (P0):** Formulario con nombre, contacto, empresa opcional, descripción, categoría y consentimiento de tratamiento de datos. Adjuntos opcionales con límites y validación. Confirmación mediante enlace de un solo uso si la política lo exige.
- **SUP-02 (P0):** Crear solicitud y ticket de forma atómica; generar referencia pública no secreta y token de seguimiento separado, de alta entropía y vencimiento. Mostrar estado y posibilidad de cancelar.
- **SUP-03 (P0):** Si existe cliente instalado, abrir mediante deep link registrado con token de lanzamiento de corta duración, vinculado a solicitud e instalación. Si no existe, ofrecer QuickSupport firmado y un código de vinculación manual alternativo.
- **SUP-04 (P0):** QuickSupport registra dispositivo temporal, muestra identidad del técnico, permisos solicitados y consentimiento local. El técnico reclama el ticket desde la cola; una operación atómica evita doble asignación.
- **SUP-05 (P0):** La web actualiza estado en tiempo real (SSE o WebSocket); técnico puede enviar mensajes, pedir conexión, reagendar o cerrar. Si no hay técnicos disponibles, ofrecer solicitud pendiente y horario de atención.
- **SUP-06 (P0):** Al finalizar, revocar la autorización temporal, registrar duración y resultado; el cliente puede cerrar el programa sin que quede acceso desatendido. Ofrecer encuesta opcional.
- **SUP-07 (P1):** Técnico puede iniciar sesión desde web lanzando cliente nativo; en P2, evaluar visor web WebRTC/WebCodecs con seguridad equivalente.

Estados: `solicitada → verificada → en cola → asignada → esperando agente → esperando consentimiento → activa → finalizada` (alternativas: `cancelada`, `expirada`, `rechazada`, `fallida`, `reagendada`).

---

## 4. Dashboard administrativo y UX

Navegación propuesta: Inicio, Equipos y agenda, Soporte, Calendario, Clientes, Monitoreo, Automatización, IA, Informes, Administración. Estética sobria inspirada en Fluent/Linear; tema claro/oscuro, `Ctrl+K`, tablas densas, panel lateral, objetivo WCAG 2.2 AA. Criterio: localizar equipo y solicitar conexión en hasta 3 acciones desde Inicio.

---

## 5. Usuarios, organizaciones y agenda

- **IAM-01 (P0):** Asistente de primer inicio crea administrador mediante secreto de bootstrap de un solo uso. Gestión de usuarios y sesiones desde web.
- **IAM-02 (P0):** Roles: administrador de instancia, administrador de organización, supervisor, técnico, técnico limitado, auditor y cliente. Denegación por defecto, MFA y separación entre administración y acceso a equipos.
- **IAM-03 (P0):** Jerarquía Organización → cliente/empresa → sede → grupo → dispositivo, con etiquetas, carpetas virtuales y pertenencia múltiple sin duplicar dispositivo.
- **IAM-04 (P0):** Agenda personal y compartida: favoritos, recientes, contactos, alias, notas, búsqueda, CSV; sin almacenar contraseñas remotas en texto plano.
- **IAM-05 (P1):** AD/LDAP, OIDC, passkeys, mapeo de grupos y aprovisionamiento.
- **CAL-01 (P1):** Calendario diario/semanal/mensual de técnicos, citas vinculadas, turnos, recurrencia, SLA, recordatorios y detección de conflictos.
- **CAL-02 (P1):** Integración CalDAV, Google Calendar y Microsoft Graph mediante OAuth propio de cada proveedor y permisos mínimos.

---

## 6. Motor remoto y clientes nativos

- Clientes P0: **Full Client**, **Host** (servicio Windows + agente por sesión interactiva), **QuickSupport** (portable temporal).
- **ENG-01:** Captura DXGI Desktop Duplication y evaluar Windows Graphics Capture; regiones modificadas, cursor separado, DPI, rotación y multimonitor. Render `wgpu`/Direct3D.
- **ENG-02:** H.264 inicial con Media Foundation; evaluar H.265, AV1 y VP9 con pruebas y revisión de licencias. Bitrate/FPS/resolución adaptativos, priorización del input, audio WASAPI/Opus.
- **ENG-03:** Control teclado/mouse, portapapeles sujeto a política; mecanismos documentados de Windows para UAC, bloqueo e inicio de sesión sin saltar controles del sistema.
- **ENG-04:** Conexión LAN directa, P2P NAT traversal, relay cifrado como fallback (IPv4/IPv6, CGNAT, TCP/TLS 443 fallback). El relay no descifra video.
- **ENG-05:** Transferencia de archivos reanudable con hashes, chat, cambio de monitor, calidad, pantalla completa, captura y finalización inmediata.
- **ENG-06 (P1/P2):** Audio bidireccional, anotaciones, grabación consentida, colaboración, traspaso, impresión remota, Wake-on-LAN, túneles aprobados, soporte headless.
- **Objetivos de benchmark (no medidos):** 1080p60 (4K60 objetivo avanzado), latencia LAN input-to-photon p95 ≤ 40 ms, conexión LAN p95 ≤ 2 s, Host inactivo ≤ 40 MB memoria privada y ≤ 0,5% CPU promedio.

---

## 7. Arquitectura e interfaces

Separación entre plano de control y plano de datos. Monolito modular inicialmente; rendezvous y relay desplegables por separado. Stack propuesto sujeto a ADR: Rust estable, Cargo Workspace, Tokio, Axum, SQLx/PostgreSQL, SQLite local, `rustls`, Protobuf/Serde, QUIC + fallback TLS, Leptos SSR/WASM, Slint vs `egui` (evaluar licencias), `windows-rs`, `tracing`, Prometheus.

---

## 8. IA integrada y ChatGPT

- **AI-01 (P1):** Adaptadores para OpenAI API, modelos locales (`llama.cpp`/Ollama) y otros proveedores oficiales.
- **AI-02 (P1):** Evaluar OAuth oficial de ChatGPT si existe elegibilidad y autorización; API oficial es fallback obligatorio.
- **AI-03 (P1):** Diagnóstico contextual con redacción de secretos/PII antes del envío.
- **AI-04 (P1):** Niveles consulta → lectura diagnóstica → propuesta → ejecución aprobada → automatización limitada.
- **AI-05 (P1):** Resumen de sesión, borrador de resolución y base de conocimiento.
- **AI-06 (P2):** MCP propio para agentes externos con identidad delegada, scopes y auditoría.

---

## 9. RMM, tickets e informes

- **RMM-01 (P1):** Inventario hardware/software/servicios/antivirus/updates.
- **RMM-02 (P1):** Métricas CPU/RAM/disco/red/uptime y alertas con histéresis.
- **RMM-03 (P1):** Terminal PowerShell/CMD, explorador remoto y reinicio bajo autorización.
- **RMM-04 (P2):** Scripts versionados y firmados por lotes, ventanas de mantenimiento y parches por anillos.
- **TKT-01 (P0/P1):** Cola básica P0; tickets completos, SLA, escalamiento y KB P1.
- **TKT-02 (P1):** Estados de ticket, comentarios internos/públicos, historial inmutable.
- **REP-01 (P1):** Reportes de sesiones, SLA, tiempos facturables, CSV/PDF, webhooks/ERP.

---

## 10. Seguridad, privacidad y datos

- **SEC-01 (P0):** Identidad criptográfica por dispositivo, TLS control, E2EE autenticado para sesión (relay ciego).
- **SEC-02 (P0):** MFA, Argon2id, sesiones revocables, cookies HttpOnly/Secure/SameSite, CSRF, rate limiting, DPAPI/secret manager.
- **SEC-03 (P0):** Permisos granulares por capacidad, consentimiento visible, botón terminar.
- **SEC-04 (P0):** Aislamiento estricto por tenant y auditoría append-only UTC.
- **SEC-05 (P0):** Authenticode Windows, manifiestos de actualización firmados, SBOM, escaneo de dependencias.
- **SEC-06 (P1):** Retención, exportación/eliminación de datos y grabaciones cifradas en endpoint.

---

## 11. Instalación, releases y operación

- **OPS-01 (P0):** Linux Debian 13/Ubuntu 24.04 (systemd, Docker Compose, LXC Proxmox); Windows (MSI, SCM, PostgreSQL local/externo).
- **OPS-02 (P0):** Hostname, TLS/ACME, HTTPS 443, UDP/relay configurables, matriz de firewall.
- **OPS-03 (P0):** Asistente inicial, backups cifrados y restauración probada.
- **REL-01 (P0):** Dashboard de releases con verificación de hash/firma y canales estable/beta.
- **REL-02 (P0):** Actualización gradual de clientes con rollback seguro.
- **OPS-04 (P1/P2):** Observabilidad, salud de relays y HA (objetivo 99,95% sujeto a validación).

---

## 12. Roadmap y definición de terminado

- **M0:** Investigación REA, matriz competitiva, revisión de licencias, threat model, ADRs, benchmark de captura.
- **M1:** Fundaciones (servidor Linux/Windows, PostgreSQL, bootstrap, IAM, tenants, API, portal público, descargas, dashboard base).
- **M2:** Remote MVP (motor LAN/P2P/relay, Full/Host/QuickSupport, permisos, transferencia, flujo web→ticket→sesión→cierre).
- **M3:** Profesional (calendario, tickets completos, AD/OIDC, RMM, terminal, IA local/API).
- **M4:** Enterprise (automatización, impresión, colaboración, grabaciones, visor web evaluado, HA, reportes).
