# AgentDesk — PRD de conectividad, instalación y distribución
**Versión:** 1.0 | **Fecha:** 2026-11-26 | **Estado:** Planificación técnica
**Repositorio:** https://github.com/jdolan-exalink/agentdesk

## 1. Objetivo
Convertir el prototipo AgentDesk en una plataforma enterprise de soporte remoto self-hosted con servidor Windows/Linux, cliente multiservidor, sesiones siempre mediadas por el servidor, QuickSupport y Host personalizables, instalación simple y alta disponibilidad K3s/Kubernetes. Reutilizar los módulos Rust existentes.

## 2. Decisiones de arquitectura
- Servidores independientes y cliente multiservidor. Sin federación ni red global en esta etapa.
- Todas las sesiones atraviesan infraestructura del servidor correspondiente; **sin P2P**.
- WebRTC con política ICE `relay-only`, TURN/UDP preferente y TURN/TCP/TLS de respaldo.
- El servidor requiere dirección pública alcanzable (IP pública directa o NAT con publicación de puertos); los endpoints pueden estar detrás de Starlink/CGNAT.
- Cloudflare Zero Trust no es requisito ni se implementa gateway CGNAT propio.
- Modos standalone Windows/Linux y HA Kubernetes/K3s, mismos protocolos.
- Mantener E2EE autenticado entre técnico y equipo asistido, sin capacidad de descifrado en relay.

## 3. Auditoría del repositorio existente
El README declara servidor Axum + SQLx/PostgreSQL, portal web, API `/api/v1`, señalización `/ws/agent`, relay E2EE `/ws/relay/{session_id}`, servicio Windows, cliente egui, Host y QuickSupport. Cargo workspace: `crates/protocol`, `identity`, `capture`, `transport`, `rmm`, `ai`, `apps/server`, `server-console`, `agent-host`, `desktop`.
**Conflicto a resolver:** `docs/adr/ADR-003-transport-protocol-and-e2ee.md` propone QUIC/P2P. Crear nueva ADR que la reemplace con WebRTC/TURN relay-only; conservar la anterior como historial. No asumir que las funciones declaradas en README están verificadas en producción. Validar con tests al comenzar.
No reescribir captura DXGI/GDI, identidad Noise, UI ni RMM sin evidencia de necesidad. Mantener relay WebSocket actual bajo feature flag hasta completar migración.

## 4. Arquitectura técnica
### 4.1 Plano de control
API HTTPS, WSS, registro/presencia, usuarios, agenda, autorización, señalización SDP/ICE, auditoría y descargas. Diseñar procesos stateless para escalar con almacenamiento de estado compartido.

### 4.2 Plano de datos
TURN compatible con WebRTC, credenciales efímeras, direcciones anunciadas correctas, asignación de relays y política ICE relay-only en ambos extremos. Evaluar coturn inicialmente: el producto permanece desarrollado en Rust sin reimplementar innecesariamente el estándar TURN. Adaptar video, audio, teclado/ratón, portapapeles y archivos a medios/DataChannels, con prioridad, confiabilidad y control de congestión apropiados. No confundir STUN con transporte de datos.

### 4.3 Persistencia y seguridad
PostgreSQL con migraciones versionadas, backup/restore, auditoría, aislamiento entre organizaciones, RBAC, MFA administrativo, rate limits, certificados TLS, revocación y rotación de credenciales. Verificar autenticación criptográfica de extremos para E2EE; DTLS/TLS por sí solos no prueban que el relay no pueda interceptar una sesión si controla la señalización. Proteger secretos y no incrustar tokens permanentes en clientes.

## 5. Puertos
| Servicio | Puerto propuesto | Exposición |
|---|---|---|
| Portal/API/WSS | TCP 443 | Público |
| TURN UDP principal | UDP 3478 | Público |
| TURN TCP alternativo | TCP 3478 | Público |
| TURN TLS | TCP 5349 | Público |
| Relay TURN | UDP 49160–49260, configurable | Público; rango a dimensionar |
| PostgreSQL | TCP 5432 | Solo privado |

TURN/TLS en TCP 443 es opcional y requiere listener/IP separada o multiplexación compatible; no asumir que comparte automáticamente el mismo 443 del HTTPS. Kubernetes requiere balanceador L4 para UDP y anuncio correcto de direcciones, no solo Ingress HTTP. Confirmar capacidad del rango según concurrencia.

## 6. Instalación
### Linux
Distribución oficial desde la web con script firmado/verificable y comando simple:
```bash
curl -fsSLo agentdesk-install.sh https://DOMINIO-OFICIAL/install.sh
# Revisar procedencia y contenido
sudo bash agentdesk-install.sh
```
Soporte inicial Debian 13 y Ubuntu 24.04 LTS. Preflight de CPU, RAM, disco, DNS, certificados y puertos; wizard CLI, PostgreSQL local/externo, systemd, firewall con confirmación, configuración idempotente, healthchecks, backups, upgrade y uninstall. Modo no interactivo con secretos por archivo protegido o STDIN.

### Windows
MSI/EXE firmado para Windows Server (versiones exactas validadas en M0), wizard y silent install, servicio SCM, Windows Firewall, certificados, rutas ProgramData, logs/Event Log, upgrades y desinstalación con opción de conservar datos. No exigir compilación manual.

### Primer inicio
Crear administrador por flujo seguro, configurar dominio y TLS, comprobar accesibilidad externa de HTTPS y TURN, generar identidad de servidor y publicar URL estable. Diagnóstico guiado con mensajes accionables.

## 7. Cliente multiservidor
- Pantalla Servidores: URL, alias, identidad/certificado, credenciales independientes, conectar/desconectar y estado.
- Agenda visual consolidada con organización de origen, filtros, grupos, historial y búsqueda.
- Identificador real compuesto `server_identity + device_identity`. El ID numérico de nueve dígitos sigue siendo local a cada servidor.
- Si un ID coincide en varios servidores, ofrecer selección explícita; nunca adivinar.
- Aislar tokens, credenciales, permisos, caché, agenda y sesiones por servidor; usar almacén seguro del sistema operativo.
- Soportar sesiones simultáneas y que una caída de servidor no afecte las demás.
- Migrar configuración actual de una única URL sin perder identidad o agenda.

## 8. Branding y distribución desde el servidor
Panel **Marca y distribución** con nombre comercial, logo, ícono, colores, tema claro/oscuro, contacto, idioma, URL pública y políticas. Vista previa y catálogo de versiones/canales.
Generar y publicar:
1. **QuickSupport portable:** logo y servidor precargados, consentimiento, credenciales efímeras, sin instalación de servicio.
2. **Cliente completo:** instalador y perfiles preconfigurados; permitir agregar otros servidores según política.
3. **Host desatendido:** servicio Windows, enrolamiento con token de un solo uso/corta vida, permisos y revocación.
Entregar desde `/downloads` con versión, SHA-256, firma, manifest y auditoría. Usar binario base firmado + manifest/recursos firmados o recompilar y volver a firmar en pipeline protegido; nunca invalidar Authenticode editando binarios después de firmarlos. Claves de firma fuera de servidores de clientes.

## 9. Kubernetes / alta disponibilidad
Helm chart con configuración, secretos, readiness/liveness, réplicas API, anti-affinity y PodDisruptionBudgets. TURN en pods/nodos con red L4 y afinidad de asignaciones; balanceadores públicos redundantes. PostgreSQL HA y PITR con restauración ensayada. Múltiples hosts físicos: varios pods sobre un único Proxmox no son HA real.
**Objetivos propuestos para validar:** caída de réplica API sin cortar sesiones activas; recuperación de nuevas solicitudes ≤30 s; caída de relay con reconexión automática de sesiones afectadas ≤30 s en red saludable. No prometer continuidad sin interrupción al caer el relay activo.

## 10. Fases de desarrollo
| Fase | Entregables | Criterio de cierre |
|---|---|---|
| **M0 — Auditoría y ADR** | Inventario, CI Windows/Linux, matriz de compatibilidad, nueva ADR relay-only, amenazas, benchmark | Arquitectura aprobada y tests base reproducibles |
| **M1 — Instalación standalone** | Script Linux, MSI Windows, TLS, PostgreSQL, servicio, wizard, backups, healthcheck | Instalación limpia, reboot y upgrade sin pérdida de datos |
| **M2 — Transporte** | WebRTC/TURN, señalización, UDP/TCP/TLS, E2EE, reconexión, diagnóstico | ICE relay-only comprobado; pruebas Starlink/CGNAT y UDP bloqueado |
| **M3 — Cliente multiservidor** | Perfiles, agenda, credenciales aisladas, IDs locales, sesiones paralelas, UX | Tres servidores independientes sin colisiones ni fuga de permisos |
| **M4 — Branding y clientes** | Editor, manifests, QuickSupport, Desktop, Host, firma y descargas | Instaladores personalizados funcionales y verificables |
| **M5 — HA/Kubernetes** | Helm, réplicas, TURN redundante, PG HA, balanceo, chaos testing | Failover demostrado con tiempos medidos |
| **M6 — Hardening/release** | Pentest, carga, upgrades/rollback, documentación y RC | Checklist de producción y CI completo aprobados |

M0 es requisito previo. M1 y diseño M3 pueden avanzar parcialmente en paralelo. M2 es la ruta crítica del transporte; M5 no debe bloquear la primera instalación standalone.

## 11. Backlog por módulo
- `crates/transport`: interfaz de transporte, WebRTC, ICE relay-only, TURN y fallback, métricas.
- `crates/protocol`: señalización versionada, negociación de capacidades y compatibilidad.
- `crates/identity`: identidad servidor/equipo, claves verificadas y revocación.
- `apps/server`: bootstrap, signaling, API TURN, branding, descargas y control stateless.
- `apps/desktop`: perfiles, agenda unificada, aislamiento, sesiones paralelas.
- `apps/agent-host`: enrolamiento, QuickSupport, configuración firmada, reconexión.
- `apps/server-console`: diagnósticos, logs, estado de servicios y puertos.
- Nuevos `deploy/linux`, `deploy/windows`, `deploy/helm`, `packaging`, `docs/operations` y CI/CD.

## 12. Pruebas de aceptación
- A01: instalar desde cero Debian 13 y Ubuntu 24.04, sin compilar.
- A02: instalar Windows Server y reiniciar servicio tras reboot.
- A03: dominio HTTPS y certificados correctos.
- A04: QuickSupport personalizado apunta al servidor correcto.
- A05: tres servidores, con mismo ID local, sin confusión.
- A06: sesiones simultáneas; logout aislado.
- A07: captura ICE demuestra ausencia de P2P.
- A08: conexión desde endpoint detrás de Starlink/CGNAT.
- A09: UDP bloqueado activa TCP/TLS o informa bloqueo total.
- A10: relay no puede descifrar; identidad autenticada.
- A11: falla de API no corta sesión; falla de relay reconecta.
- A12: backup/restore, revocación, actualización y rollback.
- A13: binarios y manifests conservan firma válida después del branding.
- A14: benchmark de concurrencia, ancho de banda y latencia con objetivos fijados por medición.

## 13. Definición de terminado
Cada fase entrega PR revisado, tests automatizados, demo reproducible, documentación, métricas, changelog y plan de rollback. No retirar el transporte WebSocket previo antes de superar pruebas de compatibilidad. No introducir P2P, federación, red pública global ni gateway CGNAT sin nueva decisión.

## 14. Riesgos pendientes
Elegir stack WebRTC Rust con pruebas reales; preservar E2EE al cambiar transporte; dimensionar TURN y puertos por concurrencia; empaquetado de branding compatible con firma de código; validar versiones de Windows Server; asegurar redundancia de balanceador y base de datos; definir SLO/RTO/RPO mediante benchmark y no por estimación.

**Orden al equipo:** comenzar M0 sobre el código existente, generar issues trazables por fase y mantener compatibilidad.