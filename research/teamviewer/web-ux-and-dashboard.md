# TeamViewer Web UX, QuickSupport & Management Console Research

Investigación sobre la arquitectura de interfaz web/escritorio de **TeamViewer (Remote / Classic / Management Console)** aplicada al diseño de **AgentDesk**.

## 1. Cliente Desktop ("TeamViewer Remote" vs Classic)
- **Barra superior global (Global Search Bar):** Cuadro de búsqueda omnipresente en el header para buscar por **ID de conexión (9 dígitos)**, **nombre de host**, **alias**, **grupos** o **conexiones recientes**, con conexión en 1 clic.
- **Menú de usuario arriba a la derecha:** Perfil del usuario autenticado, selector de tema claro/oscuro, cambio de contraseña de cuenta, configuración de seguridad/MFA y cierre de sesión (en AgentDesk además homologado al patrón desplegable de **OpenVMS Claro** / **Dracula Oscuro**).
- **Pestaña Home / Asistencia Remota:**
  - Bloque *"Permitir control remoto"*: **Su ID** (`NNN NNN NNN`) con botón de copia rápida, **Contraseña aleatoria** con botón de rotación/visibilidad, y opción de **Contraseña personal fija** (fija o en combinación con contraseña aleatoria para acceso no supervisado).
  - Bloque *"Controlar un equipo remoto"*: Entrada de ID + selector de modo (**Control remoto** / **Transferencia de archivos**) + lista de **Histórico de conexiones recientes** (re-conexión rápida dentro de ventana de confianza, solicitando contraseña nuevamente tras expirar ~1 hora).
  - Indicador visual de **Systray** diferenciado entre consola del servidor y cliente de usuario final, manteniendo el cliente residente al cerrar con `X`, con soporte de **Inicio automático con Windows** y **Servicio Windows de pre-inicio de sesión (Session 0)**.

## 2. QuickSupport y Service Queue (Cola de Asistencia)
- **QuickSupport (Ejecución sin instalación):** Genera un **Código de Sesión / Ticket** (`AD-XXXXXX`) en la cola de servicio del portal (`/support` en AgentDesk), muestra estado en vivo (`pending`, `assigned`, `active`, `closed`) y permite aceptación/conexión directa del técnico.
- **Limitación conocida de QuickSupport vs Host Service:**
  - QuickSupport corre en sesión de usuario interactiva: si se reinicia Windows o se cierra la sesión sin servicio instalado, el canal se pierde hasta que un usuario inicie sesión y vuelva a abrirlo.
  - Con **Host Service (`AgentDeskHost` en Session 0)** es posible **Reiniciar PC** de forma remota, **Bloquear pantalla**, **Cerrar sesión / Cambiar de usuario de Windows**, operar en pantalla de login UAC/Winlogon y ejecutar **Reboot & Reconnect** automático.

## 3. Acciones de Sesión Remota, Cambio de Usuario y Reboot & Reconnect
- En la barra de acciones del visor remoto (`Actions` en TeamViewer):
  - **Ctrl+Alt+Supr (SAS)**, **Bloquear equipo**, **Cerrar sesión / Cambiar de usuario de Windows** (selector de cuentas locales y sesiones WTS activas/desconectadas en el equipo remoto).
  - **Reiniciar PC (Reboot & Reconnect):** Diálogo de confirmación explícito para reiniciar la máquina remota con bandera de reconexión automática; además, si la sesión se desconecta de manera inesperada o por reinicio pendiente, el cliente pregunta *"¿Activar auto-reconexión cuando vuelva a estar en línea?"* y realiza sondeo periódico usando un `reconnect_token` efímero emitido por el servidor (expiración 1 hora, sin persistir contraseñas en texto plano).

## 4. Management Console: Agenda, Grupos, Compartición y Tickets
- **Device Groups (Grupos de Equipos):** Organización jerárquica/lógica de equipos agendados (`address_book_entries` vinculados a `device_groups`).
- **Group Sharing (Compartición entre usuarios registrados):** Permisos granulares por grupo:
  - `read_only` (Solo lectura / visibilidad)
  - `read_write` (Lectura, conexión y edición de entradas)
  - `full_control` (Control total, compartición y gestión)
- **Asignación / Registro de Equipos:** Alta o vinculación de equipos tanto por **ID de conexión (9 dígitos)** como por **Correo electrónico de usuario registrado** (`owner_email`), heredando acceso desde el primer arranque del cliente/host.
- **Tickets / Casos de Servicio y Calendario:** Cola `/support` integrada con comentarios de técnicos, historial de sesiones, exportación CSV y programación de asistencias/mantenimientos.

## 5. Monitorización de Hardware y Recursos (RMM)
- Panel en vivo accesible desde la ficha del equipo (RMM) y como panel acoplable dentro del visor remoto:
  - **CPU:** Modelo, núcleos lógicos y carga porcentual actual.
  - **Memoria RAM:** Total, usada, disponible (GB) y porcentaje de consumo.
  - **Discos:** Unidades locales (`C:`, `D:`, etc.), espacio libre/total (GB), porcentaje ocupado y estado de salud.
  - **GPU:** Nombre del adaptador, VRAM disponible (GB), versión de driver y utilización 3D/Compute cuando está disponible vía contadores Windows.
  - **Usuarios y Sesiones Windows:** Cuentas locales y sesiones WTS activas para soporte multiusuario y cambio rápido de sesión.
