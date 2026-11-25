# ADR-003: Protocolo de Transporte, NAT Traversal y Cifrado E2EE

- **Estado:** Propuesto (pendiente aprobación M0 / validación M0.5)
- **Fecha:** 2026-11-25
- **Requisitos:** `ENG-04`, `SEC-01`, PRD §6–7

## Contexto
Las sesiones remotas deben operar en LAN directa, P2P a través de NAT/CGNAT y detrás de proxies/firewalls restrictivos mediante relay, garantizando que **el relay no pueda descifrar video, input ni archivos**, y alcanzando latencia LAN input-to-photon p95 ≤ 40 ms y conexión p95 ≤ 2 s.

## Decisión
Adoptar una arquitectura de transporte dual sobre **QUIC (`quinn` + `rustls`)** con fallback **TLS/WebSocket sobre TCP 443**, protegiendo el payload extremo a extremo con **Noise Protocol (`snow`)**:

1. **Plano de Control y Rendezvous:**
   - Conexiones persistentes salientes desde Host/Full Client hacia el servidor (`HTTPS/WSS TLS 1.3`, puerto `443`) para presencia, heartbeats, entrega de tickets y señalización de candidatos de red (`LAN`, `UPnP/STUN`, `Relay`).
2. **Plano de Datos P2P / LAN:**
   - **QUIC sobre UDP** (puerto configurable, p. ej. UDP `21116` o rango efímero con *UDP hole punching* coordinado por Rendezvous).
   - Multiplexación nativa sin *Head-of-Line blocking* entre canales independientes:
     - **Stream/Datagram 0 (prioridad máxima):** Input (mouse/teclado) y eventos de control.
     - **Stream 1:** Video frames (descartables ante congestión / FEC o reintento acotado por GOP).
     - **Stream 2:** Audio Opus (datagramas QUIC no confiables de baja latencia).
     - **Stream 3:** Transferencia de archivos reanudable + portapapeles.
3. **Fallback Relay Ciego (UDP bloqueado / CGNAT simétrico):**
   - Si P2P falla en `< 800 ms`, conmutar automáticamente a **Relay QUIC** o **Relay TLS 443** (TCP/WebSocket binario) compatible con proxies corporativos.
   - **Cifrado E2EE (`SEC-01`):** Sobre el transporte (ya sea P2P o Relay), técnico y endpoint establecen una sesión **Noise_IK** o **Noise_XX** autenticada por sus claves **Ed25519** (identidad de dispositivo/técnico verificada vía servidor + huella visible). El relay solo lee el encabezado de enrutamiento de sesión (`session_route_id`) y reenvía bytes opacos.

## Consecuencias
- **Positivas:** Control total de congestión, multiplexación y latencia en Rust puro sin depender de un stack monolítico C++ tipo `libwebrtc` (~11 MB en `TVWebRTC.dll`), manteniendo además compatibilidad futura con WebTransport/WebRTC si se habilita visor web en P2.
- **Negativas / Riesgos:** El *hole punching* UDP en Rust requiere pruebas exhaustivas en redes reales CGNAT / pérdida 1–5% durante M0.5 y M2.
