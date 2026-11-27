# Plan de refactor UI/UX de AgentDesk

## Objetivo

Unificar la experiencia del cliente de escritorio, la consola local y el portal. La ficha del dispositivo será el punto de acceso al inventario, historial, diagnóstico y sesiones remotas. Se conserva el motor Rust y se migran los flujos existentes por etapas.

## Decisiones de diseño

- Usar azul como acción principal, verde como estado saludable y superficies neutras.
- Mantener Inter para la interfaz e IBM Plex Mono para identificadores, comandos y terminal.
- Ofrecer tema claro, oscuro y automático. Persistir la preferencia del usuario.
- Los iconos tendrán tooltip, nombre accesible, foco de teclado y área clickeable suficiente.
- Mantener texto en acciones de impacto, permisos, conexión y detención.
- Mantener egui durante la migración. Evaluar Tauri 2 en una prueba administrativa acotada antes de decidir sobre otra interfaz. El visor remoto mantiene captura, decodificación y renderizado fuera del camino IPC de JavaScript.
- No habilitar acciones de IA o automatización fuera de los permisos explícitos del usuario y las políticas del servidor.

## Etapas

### 0. Línea base, bandeja y recorridos actuales

**Entregas**

- Registrar versiones y rutas de binarios, estado del servicio, PostgreSQL, health y portal.
- Comprobar clic izquierdo, menú contextual, restauración, ocultamiento, copia de ID y cierre para cliente y consola.
- Comprobar operación de consola con servidor como proceso local y como servicio.

**Cierre**

- Cliente y consola restauran su formulario con clic izquierdo desde la bandeja.
- El menú contextual sigue procesando sus acciones con la ventana oculta.
- Abrir, cerrar y reiniciar una instancia no deja procesos huérfanos ni afecta sesiones activas sin aviso.

### 1. Sistema visual compartido

**Entregas**

- Paletas claro y oscuro con roles para fondo, texto, texto secundario, borde, foco, selección, éxito, aviso y error.
- Inter e IBM Plex Mono embebidas con licencia.
- Espaciado, escala tipográfica, esquinas, estados de botones, campos y paneles.
- Barra superior y navegación común, estados visibles y controles por icono accesibles.

**Cierre**

- Texto normal y controles alcanzan contraste WCAG AA, con foco visible.
- Probar a 100%, 125%, 150% y 200% de escala y con ventana 1366×768.
- Teclado puede recorrer navegación, cuadros de diálogo, menús y búsqueda.

### 2. Inicio, dispositivos y ficha

**Entregas**

- Inicio con conexión rápida, identificador propio, recientes y favoritos.
- Dispositivos en tabla con búsqueda, estado, grupo, filtros y acciones.
- Ficha del dispositivo con General, Hardware, Rendimiento, Software, Procesos, Servicios, Red, Seguridad, Archivos, Terminal, Historial e IA según las APIs disponibles.
- Abrir la ficha no inicia una sesión.

**Cierre**

- Buscar por nombre e ID, filtrar, abrir detalle y conectar en modo control o solo lectura.
- Estados de vacío, carga, error, sin red y dispositivo desconectado.
- Las operaciones del dispositivo respetan RBAC, auditoría y confirmación según impacto.

### 3. Sesiones y visor remoto

**Entregas**

- Barra de sesión compacta con equipo, estado y controles de monitor, calidad, pantalla, portapapeles, transferencia y chat.
- Progreso y error de transferencia, reconexión y cambio de estado.
- Modelo de estado por sesión preparado para pestañas y ventanas desacoplables.
- Instrumentación para comparar FPS, latencia, CPU y memoria antes y después.

**Cierre**

- Conectar, controlar, solo ver, cambiar monitor, usar portapapeles, transferir y desconectar.
- Validar reconexión tras reinicio y desconexión de red.
- El refactor visual no reduce el rendimiento del pipeline de captura y renderizado.

### 4. Centro de soporte y Agenda

**Entregas**

- Cola con filtros, prioridad, asignación, vista de detalle y contexto del dispositivo.
- Agenda integrada con ticket, cliente y sesión.
- Recorrido QuickSupport y cierre con problema, diagnóstico, acciones, duración y resultado.

**Cierre**

- Crear, asignar, tomar, atender y cerrar una solicitud.
- Un único técnico reclama cada caso.
- El historial enlaza ticket, sesión y acciones auditadas.

### 5. Consola local y portal

**Entregas**

- Consola local con estado del proceso, PostgreSQL, health, URL pública y logs.
- Acciones para iniciar, detener, reiniciar, firewall y servicio con estados claros.
- Portal web para inventario, organizaciones, usuarios, seguridad, políticas y auditoría.
- Completar el reto TOTP en login cuando MFA esté habilitado.

**Cierre**

- Diagnóstico del arranque sin depender del acceso a logs externos.
- La consola distingue proceso local y servicio.
- Roles y límites multiempresa se aplican en interfaz y servidor.

### 6. Copiloto y salud del equipo

**Entregas**

- Panel contextual opcional de IA en ficha, sesión y ticket.
- Diagnóstico de solo lectura con datos autorizados y redactados, y resumen de sesión editable.
- Tendencias y alertas de telemetría con severidad, deduplicación, reconocimiento e historial.
- Integración de proveedor validada y modo de producto funcional sin proveedor.

**Cierre**

- El técnico revisa y aprueba el resumen antes de guardarlo.
- Acciones propuestas requieren aprobación externa al modelo y quedan auditadas.
- Ningún dato de otro tenant aparece en contexto o resultados.

### 7. Distribución y validación de lanzamiento

**Entregas**

- Instalador firmado para Windows, actualización con rollback y paquetes de host/QuickSupport.
- Instalación limpia, actualización y recuperación documentadas.
- Validación en LAN, servidor self-hosted, dos clientes, base dedicada y perfiles RBAC.
- Evaluación documentada de Tauri para administración, si la prueba acotada se justifica.

**Cierre**

- Instalación nueva y actualización desde la versión anterior verificadas.
- Rollback recupera una actualización fallida.
- Recorrido ticket → QuickSupport → sesión → cierre funciona con permisos y auditoría.

## Brecha funcional investigada para después de 0–7

Las etapas 0–7 priorizan confiabilidad, experiencia de soporte, seguridad y una primera capa RMM/IA. En las páginas oficiales de TeamViewer y Splashtop aparecen capacidades adicionales de soporte remoto que no conviene mezclar con el cierre de este refactor:

| Prioridad | Capacidad candidata | Motivo y condición para iniciarla |
|---|---|---|
| P1 | Grabación de sesión con aviso/consentimiento, controles de retención y evidencia de auditoría | Mejora trazabilidad; definir almacenamiento, acceso y borrado antes de grabar. |
| P1 | Tareas/scripts remotos reutilizables con aprobación, alcance por grupo, resultado y auditoría | Evita trabajo repetido; primero cerrar RBAC, revisión de acciones y permisos. |
| P1 | Registro/exportación de sesiones y métricas por técnico/equipo | Aporta operación y reportes; requiere enlazar cada sesión al ticket y conservar métricas con política de retención. |
| P2 | Descubrimiento de activos, inventario de software y monitoreo de red | Expande RMM; requiere modelo de datos, agentes y alertas deduplicadas. |
| P2 | Gestión de parches y políticas de cumplimiento | Alto impacto operativo; necesita anillos, ventanas, rollback y aprobación por organización. |
| P2 | Experiencia móvil/web de soporte multiplataforma y voz en sesión | Amplía acceso; requiere evaluar protocolos y UX por dispositivo antes de prometer paridad. |

La documentación de producto de TeamViewer destaca monitorización, alertas, scripting dentro y fuera de sesión, gestión de parches y resúmenes IA; Splashtop destaca grabación, chat, reinicio remoto y acciones de fondo como gestión de procesos, servicios y comandos ([TeamViewer Remote Support](https://www.teamviewer.com/en-us/products/remote/use-cases/remote-support/), [TeamViewer Remote Management](https://www.teamviewer.com/apac/legal/product-descriptions/teamviewer-remote-management/), [Splashtop Remote Support](https://www.splashtop.com/api/assets/7AZy4Zc0gA1yHUU6ZBJyQ/Datasheet_-_Remote_Support_2025_2.pdf)). Son referencias para priorizar, no compromisos de paridad ni de alcance para esta entrega.

## Orden de trabajo y dependencias

0 → 1 → 2 → 3 → 4 → 5 → 6 → 7

Las APIs y permisos necesarios se adelantan cuando bloqueen una pantalla. El diseño se implementa junto con datos reales y estados de error. Cada etapa deja el producto compilable y utilizable.

## Seguimiento de ejecución

- **Etapa 0 — en curso:** cliente y consola enrutan clic izquierdo y opciones del menú mediante callbacks de `tray-icon`; el callback restaura directamente el `HWND` nativo y luego encola la acción y despierta eframe con `request_repaint`. Para ocultar se usa `SW_HIDE` en el HWND capturado al crear la ventana, conservándolo válido para restauración. La revisión de `tray-icon 0.19.3` encontró que, al asociar un menú, el clic izquierdo abre ese menú por defecto; se desactivó ese comportamiento en ambos productos para reservar el clic izquierdo a abrir el formulario y mantener el menú contextual con clic derecho. Este cambio compila en `cargo check`, pero requiere un Release nuevo para probarlo con el shell. En ambos Release previos se verificó el ciclo ocultar → callback exacto de clic izquierdo de la dependencia Windows de `tray-icon` (`WM_USER_TRAYICON`, ID 1, `WM_LBUTTONUP`) → formulario visible, con PID y clase de ventana comprobados y restauración confirmada por Orca. También se comprobó que cerrar/ocultar conserva vivo el proceso y que `ShowWindow(SW_RESTORE)` sobre el HWND inicial recupera el formulario. Esto valida el manejador y la restauración, aunque no sustituye la prueba física del clic ni la apertura del menú contextual del shell. Se eliminó el sondeo y el `WM_PAINT` manual. Falta validar icono, menú y recorrido de servicio con binarios actualizados.
- **Etapa 1 — en curso:** fuentes Inter e IBM Plex Mono, iconos accesibles de tema/copiar/visibilidad de contraseña y roles de paleta ya están en el cliente. La navegación lateral suma iconos vectoriales dibujados en egui, etiquetas visibles, selección por estado y borde de foco; el ancho pasó a 204 px para conservar la legibilidad de “Diagnóstico y RMM”. Las acciones de recarga en inventario, ficha de dispositivo, alertas RMM y recursos remotos ahora comparten un botón vectorial con tooltip y nombre accesible. Estos ajustes pasan `cargo check --offline --workspace`, pero todavía no se inspeccionaron en Release. El cliente ofrece tema Sistema/Claro/Oscuro; la paleta azul y neutra está aplicada. En el preview Release se verificó visualmente el cambio claro→oscuro→claro y la navegación de teclado hasta Dispositivos antes de estos ajustes. La revisión de pares de texto normal, secundario y estados semánticos detectó rojo de error claro apenas bajo AA y se ajustó; falta completar escalas, 1366×768 y recorrer teclado sobre la navegación nueva y pantallas completas.
- **Etapa 2 — en curso:** inventario con búsqueda por equipo/ID/propietario, estado, grupo y favoritos, con acciones separadas de control y solo lectura. La carga concurrente muestra progreso/error y permite reintentar; la ficha carga historial y ahora incluye Hardware, Rendimiento, Servicios y Sistema y red con consultas bajo demanda al agente, estados de carga/error y reintento. La API filtra equipos por pertenencia/compartición de grupo; `read_only` permite consulta, `read_write`/`full_control` habilitan control, y cliente/servidor aplican el permiso en sesiones, diagnósticos, comandos, telemetría, historial y exportación. Los roles `client` y `auditor` pueden abrir solo sesiones de vista y no iniciar control. La asociación nativa usa `devices.group_id`, se puede cambiar desde la ficha del cliente y desde la tabla del portal y queda auditada; el contador cuenta equipos directos. Se corrigió la incompatibilidad heredada de grupos: la 0001 ya creaba `device_groups`, así que el `CREATE TABLE IF NOT EXISTS` de la 0004 nunca añadía `owner_user_id`. La nueva 0007 agrega el campo, asigna grupos antiguos a un usuario de su organización cuando hay uno y conserva grupos huérfanos sin propietario. El DTO/API ahora admiten grupo sin propietario; esos grupos quedan administrables por roles de administración o por permisos compartidos. También se alineó el predicado administrativo y los filtros SQL para incluir `instance_admin`, que es el rol que crea el bootstrap. Falta validar la matriz con usuarios y servicio activos.
- **Etapa 3 — en curso:** barra del visor reorganizada para dejar visibles seguridad, equipo, métricas, monitor, calidad y cierre, y reunir acciones secundarias bajo Herramientas. El envío de archivos ahora calcula SHA-256 y transmite bloques en segundo plano con progreso/error asociado al id de sesión y de transferencia. La lista e informe CSV de sesiones respetan visibilidad por grupo; detener una sesión requiere permiso de escritura. Al crear una sesión, el ticket se valida dentro de la organización y, si ya tiene equipo asignado, debe coincidir con el dispositivo elegido. Falta verificar transferencia y reconexión con una sesión real, y comparar métricas antes/después.
- **Etapa 4 — en curso:** la cola tiene filtros por estado, contadores y contexto de solicitante. El cierre pide resultado y resumen de diagnóstico/resolución; el servidor guarda la nota interna junto al ticket, agrega las sesiones vinculadas con equipo, modo, estado y duración cuando está disponible, audita el resultado y emite `ticket.closed`. Cliente y portal muestran el historial; errores de guardado conservan el formulario. La API de comentarios ahora valida que el ticket pertenezca a la organización, rechaza contenido vacío o mayor a 4000 caracteres, prohíbe notas internas a `client`/`auditor` y audita los comentarios creados. Falta asociar un resumen de sesión explícito y validar el comportamiento con base de datos activa.
- **Etapa 5 — en curso:** consola con tema claro/oscuro persistente e identidad azul compartida; el health distingue proceso vivo de readiness y la consola muestra el estado derivado de PostgreSQL sin iniciar una segunda instancia si la base cae. Ya no imprime la contraseña de la cuenta inicial en pantalla; conserva visible el correo y deriva a la configuración local. Reutiliza Inter e IBM Plex Mono embebidas por el cliente; tema y ocultado a Systray usan botones de icono con nombres accesibles y tooltip, y “Salir y Detener” conserva texto explícito. Los colores de estado y mensaje se adaptan al tema; su contraste calculado en las superficies principales es 4.63:1–9.0:1 en oscuro y 6.7:1–7.6:1 en claro. El portal asocia por `for`/`id` todas las etiquetas HTML de sus formularios (solicitud, setup, login, agenda, grupos, asignación y cambio de contraseña); la búsqueda estática no encuentra etiquetas sin asociación. El portal conserva superficies, tipografía de sistema, foco visible, tablas responsivas y tema persistido por usuario. Los tokens de texto, enlace, foco y estados semánticos están diferenciados por tema; falta una comprobación visual completa de sus páginas con el servidor activo. Estas modificaciones pasan `cargo check --offline --workspace`; falta verificarlas en Release. El login web y de escritorio verifican TOTP cuando MFA está habilitado; falta validarlo con base de datos y revisar las vistas de seguridad.
- **Etapa 6 — en curso:** acciones propuestas muestran riesgo, motivo y comando; la ejecución exige elegir expresamente el dispositivo y queda auditada. Desde la cola se abre el Copiloto con el ticket seleccionado; el backend carga contexto de ticket y sesiones dentro de la organización, aplica la redacción PII configurada y rechaza inconsistencias de dispositivo. El Copiloto puede preparar un borrador de cierre editable; el técnico lo pasa al formulario, revisa resultado y resumen, y confirma antes de guardar. La nota final enumera sesiones vinculadas y duración; Rendimiento grafica CPU, RAM y disco usando las 12 muestras más recientes. El servidor toma métricas periódicamente de agentes conectados (300 s por defecto; `AGENTDESK_RMM_METRICS_INTERVAL_SECS`, limitado a 60–86400 s), con concurrencia máxima de ocho. Hay alertas persistentes/deduplicadas para CPU, RAM y espacio libre de C:, resolución al volver al rango normal y reconocimiento auditado. Los administradores pueden configurar por organización los umbrales de aviso/crítico desde Ajustes; se corrigió el control de permisos para que incluya el rol real `instance_admin` junto con `org_admin`/`admin`. Los cambios se validan, se auditan y aplican en la siguiente muestra. Falta validar SQL, permisos y flujo con base de datos activa.
- **Etapa 7 — en curso:** los previews Release del cliente y la consola con ocultado/restauración mediante HWND y los ajustes RMM están en `target/ui-refactor-preview-v12/release/`. El cliente y la consola arrancaron en la sesión real de Windows y se revisaron con Orca; el servidor pasa `cargo check`, pero no se pudo generar el binario Release en este entorno Windows GNU porque faltan `gcc.exe` y `dlltool.exe`; tampoco está disponible `cl.exe` para MSVC. El instalador firmado, actualización/rollback y recorrido de aceptación en servidor real siguen pendientes.

El preview Release actual de escritorio es `target/ui-refactor-preview-v12/release/agentdesk-desktop.exe`; la consola es `target/ui-refactor-preview-v12/release/agentdesk-server-console.exe`. Ambos se abrieron en la sesión de Windows real y se inspeccionaron con Orca; la consola ya no imprime la contraseña inicial. En los dos procesos se verificó ocultar el HWND principal y enviar el callback exacto de clic izquierdo a su ventana de bandeja: ambos pasaron de ocultos a visibles, con los HWND/PID verificados, y Orca encontró los formularios. La consola indica que el servidor está detenido y que el ejecutable `agentdesk-server.exe` no está junto al preview; no arrancó servicio. La bandeja del shell no está expuesta a la automatización, así que el icono físico y el menú contextual aún requieren una prueba manual. El cliente muestra reintentos contra `http://10.1.1.156:8080`; el servidor LAN sigue sin responder. PostgreSQL local está saludable; la base original `agentdesk` sigue en versión 4, sin `device_groups.owner_user_id`, y conserva 1 organización, 1 usuario y 1 dispositivo; sus datos no se alteraron. En el scratch `agentdesk_uiux_check` se aplicaron manualmente las migraciones 0001–0007 sin error; se verificó la columna y las tablas de alertas, y se comprobó el backfill legacy con filas ficticias (grupo asignado al admin si hay usuario, grupo conservado con propietario nulo si no hay usuarios). También se ejecutaron las consultas de listado para grupo con propietario, grupo huérfano y el rol `instance_admin`, y devolvieron los resultados esperados. El servidor actualizado aún no se pudo iniciar: `cargo build --offline -p agentdesk-server` falla porque no existe `dlltool.exe`. El cambio de consola pasa `cargo check --offline --workspace`; su Release no se pudo regenerar por el mismo toolchain GNU incompleto, por lo que el preview v12 sigue sin cambios. Tampoco se pudo regenerar el Release del servidor; no hay `gcc.exe`, `dlltool.exe` ni toolchain MSVC (`cl.exe`). No se instaló ni reemplazó ninguna versión de servidor.

En la revisión visual posterior se abrió el cliente v12 y quedó visible en la vista Conectar. La herramienta de automatización pudo enfocar los controles de egui, pero sus clics no activaron el cambio de tema ni navegación; Alt+F4 sí cerró el proceso. El cliente se volvió a abrir y quedó visible. Por ello, la inspección visual confirma la composición de la pantalla, pero el toggle, los clics del área de notificaciones y el menú contextual siguen pendientes de una prueba manual con mouse o de un Release actualizado.

Las entregas mantienen el cliente Rust/egui y el portal Axum existentes. Cada cambio se compila offline antes de continuar; los recorridos reales de bandeja, TOTP y sesión dependen de la validación con el server y la base activos.

## Actualización visual y de instancia única

- Se incorporó el isotipo de logos/agentdesk-icono.png al encabezado del cliente y la consola, y como icono de ventana y bandeja de ambos productos.
- Todas las opciones actuales de la sidebar del cliente ya muestran iconos vectoriales. Agenda, Calendario, Grupos y Usuarios/MFA ahora tienen símbolos distintos para reconocer mejor cada sección.
- Se añadió al cliente un mutex de instancia única: un segundo inicio restaura la ventana existente y termina. El código compila, pero falta probar el segundo inicio sin mantener dos clientes abiertos.
- El Release v13 de cliente y consola se compiló en target/ui-refactor-preview-v13/release/. Se abrió solo una instancia del cliente y se revisaron el isotipo y la sidebar. La consola no se abrió porque puede iniciar el servidor automáticamente.

## Estado al iniciar el refactor

- Etapa 0: la lógica de restauración de ventana y señal de repintado está en cambios locales. Clic de bandeja aún requiere validación real.
- Etapa 1: fuentes embebidas, tema básico e iconos de tema/copiar existen en cambios locales. La aplicación de colores y revisión de contraste requiere una pasada consistente.
- Etapas 2–7: pendientes de implementación.
- Limitaciones conocidas: el cliente mantiene una sola sesión visible; el empaquetado firmado y la actualización con rollback están pendientes.
