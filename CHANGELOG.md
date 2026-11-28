# Cambios de AgentDesk

Cada sección `## X.Y.Z — fecha` con viñetas `- ...` es lo que el cliente muestra
al ofrecer una actualización (lo firma `agentdesk-release-sign`). Escribí las
viñetas pensando en el técnico que las va a leer, no en el código.

## 0.1.8 — 2026-11-28
- Actualizaciones automáticas del cliente: avisa cuando hay una versión nueva, muestra qué cambió y solo instala si aceptás.
- Cada actualización viene firmada por AgentDesk y se verifica (firma y SHA-256) antes de instalarla.
- La versión instalada y la búsqueda de actualizaciones están en Ajustes → Acerca de.

## 0.1.7 — 2026-11-28
- La contraseña aleatoria de acceso es de un solo uso y cambia al terminar cada sesión.
- Bloqueo de un ID tras 8 contraseñas erróneas en 10 minutos.
- QuickSupport pide aceptar o rechazar cada conexión.
- El registro de equipos exige firma del host.

## 0.1.6 — 2026-11-28
- Diagnóstico de control remoto en el host y aviso cuando el equipo no puede recibir mouse o teclado.

## 0.1.5 — 2026-11-28
- El control remoto ya no se congela cada 10 segundos.
- Mouse correcto en equipos con varios monitores.
- La pizarra queda siempre visible sobre el monitor compartido.

## 0.1.4 — 2026-11-27
- Telemetría de calidad de conexión.

## 0.1.3 — 2026-11-27
- Conexión más rápida y sin cuelgues con el escritorio remoto quieto.
