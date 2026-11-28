# Migración del cliente AgentDesk a GPUI Kit

## Decisión

El cliente de escritorio nuevo se desarrolla en `apps/desktop-gpui` usando GPUI Kit (`gpui-kit`), que incluye GPUI, GPUI Component y sus assets compatibles. El ejecutable eframe actual se conserva como referencia y respaldo hasta completar la migración funcional. Ambos comparten el mutex de instancia de Windows para impedir dos clientes simultáneos.

Las conexiones remotas son ventanas de primer nivel independientes del dashboard. Cada sesión tendrá su propio ciclo de vida y estado; cerrar una sesión no debe cerrar el shell ni otras sesiones.

## Etapas

1. **Base visual GPUI:** shell, navegación, tokens para tema claro/oscuro, iconografía, escalas tipográficas y estados vacíos. Está en `apps/desktop-gpui/src/main.rs`.
2. **Separar núcleo de UI:** mover configuración, identidad, autenticación, comandos, estado de conexión y adaptadores de red a módulos/crates de capacidad. El cliente existente contiene UI y servicios entrelazados en `apps/desktop/src/main.rs`; se debe migrar por límite funcional, no copiar el archivo entero.
3. **Inicio y dispositivos reales:** enlazar el servidor configurado, autenticación, estado de conectividad y listas de dispositivos/grupos, incluida búsqueda y filtros; retirar los datos de muestra.
4. **Sesiones separadas:** abrir una ventana GPUI por conexión, vincularla al UUID real de sesión y trasladar captura de frames, render de video y envío de teclado/ratón. El viewport debe escalar sin bloquear la UI. No mostrar resolución, FPS, RTT o cifrado como activos hasta recibir telemetría real.
5. **Herramientas de sesión:** migrar acciones de sistema, monitores, transferencia bidireccional de archivos, portapapeles, chat, audio, privacidad, grabación y auditoría. Mantener permisos, confirmaciones y feedback cerca de cada acción.
6. **Resto del producto:** migrar soporte/tickets, agenda, grupos, automatización, RMM/monitoreo, AI contextual, reportes, MFA/usuarios y ajustes por capacidad, priorizando lo que ya funciona en el backend.
7. **Bandeja, robustez y entrega:** bandeja con abrir/mostrar, estado, copiar ID y salir; single-instance coordinado con el cliente legado; restaurar/enfocar el shell al invocarlo; persistencia de tema/ventanas; instalador/actualización, migración de datos y rollback. Cambiar el nombre del ejecutable de producción solo cuando las rutas críticas pasen la revisión visual y funcional.

## Estado de este corte

- **Estado de producto:** este binario todavía no debe reemplazar al cliente eframe para pruebas de conexión real. El cliente funcional sigue siendo `agentdesk-desktop`; GPUI es una migración en curso, no un cliente equivalente. El host integrado que genera y rota el ID/contraseña temporal, el control remoto entrante y el visor de sesiones aún no fueron migrados. No se deben inventar ni reconstruir localmente esas credenciales desde la UI GPUI.
- La pantalla de inicio del cliente funcional conserva el panel “Compartir mi equipo” con el ID y la contraseña reales del host, conexión entrante, acceso desatendido, y el panel de conexión saliente. El rediseño debe reubicar estos flujos existentes antes de cambiar el binario de producción.

- Implementado: shell GPUI con ventana inicial de 1440×840, navegación con iconos y vistas específicas para Inicio, Dispositivos, Centro de soporte, Sesiones, Agenda, Monitoreo, Automatización, AgentDesk AI, Informes y Configuración.
- Implementado: panel AI lateral que se cierra desde su encabezado y vuelve a abrirse desde el botón de la barra superior; preferencias visuales para tema y visibilidad del panel.
- Implementado: comprobación real de `/health/ready`, configuración y persistencia de la URL de servidor, login con contraseña/MFA, lectura autenticada de `/api/v1/devices` y prueba real de autorización con `/api/v1/sessions/connect-by-id`. La sesión de prueba se termina inmediatamente desde la API para no dejar un relay huérfano mientras falte el visor.
- Implementado: el dashboard y la lista de dispositivos muestran datos del servidor o estados vacíos; se retiraron los equipos, conteos, tickets, agenda y métricas inventados.
- Límite de sesión: el endpoint real puede autorizar una sesión, pero el visor GPUI y el stream WebSocket de pantalla/entrada siguen pendientes. La ventana lo informa explícitamente.
- Las vistas de soporte, sesiones, agenda, monitoreo, automatización, AI e informes ya no presentan ejemplos; muestran que aún no están integradas al backend en el cliente GPUI.
- Salvaguardas: mutex de instancia compartido con el cliente existente; no abrir dos clientes a la vez.
- Verificación: `rustfmt --edition 2021 apps/desktop-gpui/src/main.rs` y `cargo build --offline -p agentdesk-desktop-gpui --target-dir target/gpui-connection-test` completan correctamente. El EXE de prueba está en `target/gpui-connection-test/debug/agentdesk-desktop-gpui.exe` y fue despojado de símbolos de depuración. Esta compilación de desarrollo todavía pesa aproximadamente 188 MB.
- Portabilidad: fuentes Inter y assets están embebidos. El EXE importa DLLs de Windows, incluido `icuuc.dll`; hace falta confirmar la ejecución en la PC destino. Para conectarse, el usuario debe ingresar la URL alcanzable del servidor, iniciar sesión y tener acceso de red/firewall.

## Criterios de aceptación

- Solo existe una instancia de AgentDesk entre el cliente legado y el GPUI.
- Inicio, búsqueda, tema, AI y navegación funcionan con teclado y ratón en ambos temas.
- Una conexión remota crea una ventana separada, activa y cerrable; el dashboard permanece disponible.
- El streaming y cada herramienta exponen estados reales de conexión, permiso, éxito y error.
- No quedan tarjetas, estados ni métricas simuladas en builds de producción.
- Bandeja, cierre, restauración, segundo lanzamiento y cierre de una sesión no dejan procesos o sesiones huérfanas.
