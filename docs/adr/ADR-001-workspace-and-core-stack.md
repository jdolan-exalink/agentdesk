# ADR-001: Cargo Workspace y Stack Core en Rust

- **Estado:** Propuesto (pendiente aprobación M0)
- **Fecha:** 2026-11-25
- **Requisitos:** PRD §7 (`Stack propuesto`), `OPS-01`, `SEC-04`

## Contexto
La plataforma requiere un backend multiempresa desplegable en Linux (Debian 13 / Ubuntu 24.04) y Windows (servicios SCM), clientes nativos Windows x64 de alto rendimiento, un portal web integrado servido desde la misma instalación y arquitectura preparada para expansión multiplataforma.

## Decisión
Adoptar un **único Cargo Workspace en Rust estable** (probado en este entorno con `rustc 1.99.1` / `cargo 1.99.1`) organizado como **monolito modular** para el plano de control, con binarios independientes desplegables para `relay` y `rendezvous` cuando se escale:

- **Runtime asíncrono:** `tokio` (multi-threaded)
- **Servidor HTTP/WebSocket/SSE:** `axum` + `tower` + `tower-http` + `utoipa` (OpenAPI v1)
- **Base de datos:** `sqlx` con **PostgreSQL** como única fuente de verdad en servidor (migraciones versionadas), y **SQLite** (`sqlx` / `rusqlite`) exclusivamente para caché/cola local offline en clientes si es necesario
- **Serialización y contratos:** `serde` + `serde_json` para API HTTP/SSE; `prost` (Protobuf) o `postcard`/`rkyv` versionado para protocolo binario en plano de datos
- **Observabilidad:** `tracing` + `tracing-subscriber` (JSON estructurado sin secretos) + `metrics` / exportador `prometheus`
- **Plataforma Windows:** `windows` (`windows-rs` oficial de Microsoft) + `windows-service` para SCM en Session 0

## Consecuencias
- **Positivas:** Un solo lenguaje, tipado fuerte compartido en `crates/protocol` y `crates/policy`, despliegue de binario único sin dependencias de runtime (ni Node.js, ni Python, ni JVM en producción).
- **Negativas / Riesgos:** Tiempos de compilación inicial mayores al agregar `wgpu` y `leptos`; requiere pipeline CI bien cacheado (`sccache` / target cache) para Linux y Windows (`x86_64-pc-windows-msvc` recomendado para CI con Media Foundation + MSI).
- **Nota toolchain local:** En esta PC Cargo tiene instalado actualmente `x86_64-pc-windows-gnu`. Antes de M0.5/M1 conviene verificar/instalar también `x86_64-pc-windows-msvc` (Visual Studio Build Tools) si algún binding Win32/Media Foundation o `wgpu` lo requiere para firmar y generar PDBs estándar.
