# Evidencia REA — TeamViewer Desktop Windows (`15.83.7.0`)

- **Objetivo analizado:** Instalación local legítima en `C:\Program Files\TeamViewer`
- **Versión observada:** `15.83.7.0` (`crashpad_handler.exe` `0.9.1`, `WriteDump.exe` `15.0.0.0`)
- **Fecha de inspección:** 2026-11-25
- **Herramienta:** `rea-agents@6.10.1` sobre Node.js `26.13.1` (Windows 11 x64)
- **Alcance ético y de privacidad (RES-04):** Se inspeccionaron exclusivamente binarios distribuidos, encabezados PE, certificados Authenticode, recursos, exportaciones, manifiestos embebidos, configuración de servicio SCM, `CopyRights.txt` público y telemetría agregada de memoria/CPU en reposo. **No** se leyeron archivos `.log`, `%APPDATA%\TeamViewer`, `%LOCALAPPDATA%\TeamViewer`, identificadores de cuenta ni tráfico privado.

---

## 1. Registro de Evidencias REA (`Evidence IDs`)

| Artefacto | Operación REA | `evidence_id` | SHA-256 del artefacto | Tamaño (bytes) | Clasificación |
| :--- | :--- | :--- | :--- | ---: | :--- |
| `C:\Program Files\TeamViewer` | `inspect_desktop_application` (summary) | `ev_12d40408ce01c342153c7969c2db660443d566c73457018812c7e2bd04590ac0` (padre `ev_86df7f4a80774d9723965512bd816d58d29b1e049ba64d03a4eb034205e8377a`) | `c443807bcbfe01fcc6d8e54559dca6301dd14dd4c70a98d94af913aae48b1bca` (snapshot directorio) | 110 entradas | **Observado** |
| `TeamViewer.exe` | `inspect_pe_imports` | `ev_94d3b4fc95cf049ce9cbfc35c60555521abb2db79ccffd33a3ea7a54a3e23ebe` | `476ff0a56c6fafc86e291ab0abc073696cc79d95650f65fa7cff5ef908526799` | 87,873,360 | **Observado** |
| `TeamViewer.exe` | `inspect_binary_hardening` | `ev_b34c4d540f4b238f77b2ffe9e1ea041853217b051aadccb5471cc75a420cf3fa` | `476ff0a56c6fafc86e291ab0abc073696cc79d95650f65fa7cff5ef908526799` | 87,873,360 | **Observado** |
| `TeamViewer.exe` | `inspect_pe_load_config` | `ev_ff8991f45f36b5d431a5451c46c3fbc3660275deba0c1f0d8ac9a4908502795d` | `476ff0a56c6fafc86e291ab0abc073696cc79d95650f65fa7cff5ef908526799` | 87,873,360 | **Observado** |
| `TeamViewer.exe` | `inspect_pe_certificates` | `ev_5dc031a5c819f3e065a102ddeb4a96e88ca542e026b5ac26f4bccda973e18c48` | `476ff0a56c6fafc86e291ab0abc073696cc79d95650f65fa7cff5ef908526799` | 87,873,360 | **Observado** |
| `TeamViewer_Service.exe` | `inspect_pe_imports` | `ev_1408f3784881fbce42522a41a44324940de3f59d96bb8446f7068e369f8a31ab` | `701816bcf532bca27fc8a0799833fbff586bcade3c3a0656d7843478e8a254d0` | 28,495,184 | **Observado** |
| `TeamViewer_Service.exe` | `inspect_binary_hardening` | `ev_9df4d4eaa2031cf9114e4a345b55370e0c37000d21aafb15b264ff5ed1fdbbad` | `701816bcf532bca27fc8a0799833fbff586bcade3c3a0656d7843478e8a254d0` | 28,495,184 | **Observado** |
| `TeamViewer_Service.exe` | `inspect_pe_load_config` | `ev_fb43746fca2b4e4c5b2edf04bc8f3e5609db65e472753ffb7884e2e56da1f49c` | `701816bcf532bca27fc8a0799833fbff586bcade3c3a0656d7843478e8a254d0` | 28,495,184 | **Observado** |
| `TeamViewer_Service.exe` | `inspect_pe_certificates` | `ev_2c31e1d8e50ec3cd38279df4bef79a784119dece129ff8cde43ea639b37db47d` | `701816bcf532bca27fc8a0799833fbff586bcade3c3a0656d7843478e8a254d0` | 28,495,184 | **Observado** |
| `TeamViewer_Service.exe` | `inspect_pe_resources` | `ev_1b8f2787699e67710175df3ee55fb6faeafa789bfd35f949291b99a4e8d609b4` | `701816bcf532bca27fc8a0799833fbff586bcade3c3a0656d7843478e8a254d0` | 28,495,184 | **Observado** |
| `TeamViewer_Desktop.exe` | `inspect_pe_imports` | `ev_0599c9c53ebb2b3eff883e359e0abb2e25b5a8f7e8a9ff8dfdfd7e6f9ef071f9` | `f42a3ccbe735c8f22c74ae46f632c40ad7912cf625196d1c89e8f9b0d05238c1` | 20,688,208 | **Observado** |
| `TeamViewer_Desktop.exe` | `inspect_binary_hardening` | `ev_1b1bab33bb2172f53106c911c9a2a5ed4de6356d4a34592f0796e2aec2c4e5b0` | `f42a3ccbe735c8f22c74ae46f632c40ad7912cf625196d1c89e8f9b0d05238c1` | 20,688,208 | **Observado** |
| `TeamViewer_Desktop.exe` | `inspect_pe_load_config` | `ev_08d17bb78e48d4623727a3c964a1bf2c180db3ad63562e6b8e817d24982e5608` | `f42a3ccbe735c8f22c74ae46f632c40ad7912cf625196d1c89e8f9b0d05238c1` | 20,688,208 | **Observado** |
| `TeamViewer_Desktop.exe` | `inspect_pe_resources` | `ev_9fd81e8c6e5807833ff57faaf3a9755b0b27509b211de19967de564e86c8c7c6` | `f42a3ccbe735c8f22c74ae46f632c40ad7912cf625196d1c89e8f9b0d05238c1` | 20,688,208 | **Observado** |
| `TVWebRTC.dll` | `inspect_pe_imports` | `ev_7d84d941a33e01dc5d1e1816911965bf2f98b4b11d3c45858c94ebebb5bb8ea8` | `4e2e23a65211956f59e389f7397c8424ef979905346d8d85ff14a6792dce2db5` | 10,917,200 | **Observado** |
| `TVWebRTC.dll` | `inspect_native_exports` (summary) | `ev_5e1e2f8e40f1385068d919d3cac1af5a1a7f9506100b7a69dd806082d464ce31` (padre `ev_1829bf9054eb60b9ca8d4bd73a019b0c16ae81e528f80c912e844de84d13dd8e`) | `4e2e23a65211956f59e389f7397c8424ef979905346d8d85ff14a6792dce2db5` | 10,917,200 | **Observado** |
| `sciter.dll` | `inspect_native_exports` (summary) | `ev_0fbb68e6bc15791a44cccf28e617704455814e3daa9b4a243e4887d422f66ea7` (padre `ev_513faa62ba1fb13731325897628dcf5c070bcf3688c6d0a2bec34e2ee1efddb3`) | `eff38ced6612e161754589c2236ad53f08dbbb3834963de02bae5a2ee1bd5c05` | 17,839,952 | **Observado** |
| `tv_x64.exe` | `inspect_pe_imports` | `ev_307672b58ced2bc55fabdbb74fc4ca3323624298fb76d58fc472c8020a629a6b` | `b3daf896fc368ba3a9e9d50e8c04dfe4ced64b2d6fd5b44c7d32d01dcc82746a` | 457,552 | **Observado** |
| `tv_x64.dll` | `inspect_native_exports` (summary) | `ev_408f2bcdd8029b22ea6b2a33e543ca27f5e325a27067ecdd7d75f13b748211c4` (padre `ev_9d380044ccf90c1bc2ee95848339b7e3444dafa75220e810d912a19a62f1bb08`) | `8e4ae7e80ffe15e8a208fe4ac3f8c6882e3649c9d8220518f75b9bca755ea608` | 195,408 | **Observado** |

Otros hashes SHA-256 medidos mediante PowerShell `Get-FileHash`:
- `x64\TVVirtualMonitorDriver.dll` (72,056 bytes): `cd858c1a37d9599181285ce55e38bbd7cef8637f8df1d3ba425b78c2670e345e` (**Observado**)

---

## 2. Arquitectura de binarios observada (PE Imports y Recursos)

### A. Separación multi-proceso en Windows
1. **`TeamViewer_Service.exe` (Session 0, `LocalSystem`)** — **Observado** (`sc.exe qc TeamViewer` + `ev_1408f378...`):
   - Servicio Windows `WIN32_OWN_PROCESS`, `AUTO_START`, dependencias `RpcSs`, `Tcpip`, `NDIS`, `AFD`.
   - Importa `ADVAPI32.dll` (`StartServiceCtrlDispatcherW`, `RegisterServiceCtrlHandlerExW`, `CreateProcessAsUserW`, `CreateProcessWithTokenW`, `CreateProcessWithLogonW`, `DuplicateTokenEx`, `LogonUserW`, `LsaStorePrivateData`, `AdjustTokenPrivileges`), `WTSAPI32.dll` (`WTSEnumerateSessionsW`, `WTSQueryUserToken`, `WTSQuerySessionInformationW`, `WTSEnumerateProcessesW`).
   - Gestión de red y proxy: `WINHTTP.dll` (18 APIs incluyendo `WinHttpGetProxyForUrl`, `WinHttpGetIEProxyConfigForCurrentUser`), `WS2_32.dll` (53 APIs), `IPHLPAPI.DLL` (39 APIs), `SensApi.dll`.
   - Criptografía y almacén local: `CRYPT32.dll` (`CryptProtectData` / `CryptUnprotectData` = DPAPI, `CertGetCertificateChain`, `PFXImportCertStore`), `bcrypt.dll` (`BCryptGenRandom`), `WinSCard.dll` (13 APIs, redirección de tarjetas inteligentes).
   - Instalación/drivers/actualizaciones: `msi.dll` (11), `SETUPAPI.dll` (19), `newdev.dll` (1), `CFGMGR32.dll` (2).
   - **Nota:** Solo importa `GetDesktopWindow` de `USER32.dll` y no importa `GDI32.dll` ni APIs de captura gráfica: el servicio en Session 0 no captura la pantalla directamente, sino que lanza/coordina procesos en sesiones de usuario interactivas (**Inferido** a partir de `WTSQueryUserToken` + `CreateProcessAsUserW` y ausencia de GDI/DWM/DXGI).

2. **`TeamViewer_Desktop.exe` (worker gráfico/input por sesión)** — **Observado** (`ev_0599c9c5...`):
   - **Cambio de escritorio y Secure Desktop (UAC/Winlogon):** importa `USER32.dll` (`OpenInputDesktop`, `OpenDesktopW`, `SetThreadDesktop`, `GetThreadDesktop`, `CloseDesktop`, `EnumDesktopWindows`) y `ADVAPI32.dll` (`CreateProcessWithTokenW`, `LogonUserW`), más `WTSAPI32.dll` (`WTSQueryUserToken`, `WTSRegisterSessionNotification`).
   - **Captura, privacidad y composición:** importa `MAGNIFICATION.dll` (7 funciones — usada típicamente en Windows para exclusión de ventanas / cortina negra / pantalla de privacidad durante sesiones remotas, **Inferido**), `dwmapi.dll` (3), `d2d1.dll` (1), `GDI32.dll` (`BitBlt` + 33 APIs), `gdiplus.dll` (25) y `api-ms-win-core-winrt-l1-1-0.dll` (WinRT). `dxgi.dll` y `d3d11.dll` no figuran en el IAT estático, por lo que si usa Desktop Duplication o WGC se cargan dinámicamente en tiempo de ejecución (**Inferido** / **No verificado** sin trazado en runtime).
   - **Entrada, cursor, accesibilidad y portapapeles:** `USER32.dll` (`SendInput`, `BlockInput`, `SetWindowsHookExW`, `GetPhysicalCursorPos`, `GetCursorInfo`, `AddClipboardFormatListener`, `OpenClipboard`/`GetClipboardData`), `OLEACC.dll` (3).
   - **Audio:** `ksuser.dll` (1), `WINMM.dll` (17).
   - **Manifiesto (`RT_MANIFEST`):** `requestedExecutionLevel level="asInvoker" uiAccess="false"`, `dpiAware: True/PM` (**Observado**).

3. **`TeamViewer.exe` (cliente principal GUI en Session 1)** — **Observado** (`ev_94d3b4fc...`):
   - `imphash`: `6d2b810fc8e69ceef0e191294134e058`, 45 DLLs estándar/delay, 1,239 funciones importadas en total (347 estándar + 892 delay-load).
   - Incluye `WINSPOOL.DRV` (14), `ACTIVEDS.dll` (6, Active Directory), `WinSCard.dll` (13), `msi.dll` (11), `urlmon.dll`, `WININET.dll`, `WINHTTP.dll`.
   - Manifiesto (`RT_MANIFEST`): `requestedExecutionLevel level="asInvoker" uiAccess="false"`, `dpiAware: true`, `dpiAwareness: PerMonitorV2` (**Observado**).

4. **`TVWebRTC.dll` y `sciter.dll` y `tv_x64.exe`/`tv_x64.dll`** — **Observado**:
   - `TVWebRTC.dll`: `10.92 MB`, exporta 1 símbolo (`ev_5e1e2f8e...`), importa únicamente 7 DLLs estándar (`KERNEL32`, `WS2_32`, `CRYPT32`, `bcrypt`, `ADVAPI32`, `IPHLPAPI`, `USER32`), sin `delay_imports`. Motor de transporte/medios WebRTC aislado en módulo dedicado (**Observado/Inferido**).
   - `sciter.dll`: `17.84 MB`, `276` exportaciones (`ev_0fbb68e6...`), motor HTML/CSS/script embebido para parte de la UI nativa. Adicionalmente la carpeta `utils\` incluye `MicrosoftEdgeWebview2Setup.exe` (`1.78 MB`) y `CopyRights.txt` declara `webview2@1.0.4129.50` (**Observado**).
   - `tv_x64.exe` (`457 KB`) / `tv_x64.dll` (`195 KB`, 2 exportaciones) y `tv_w32.*`: helpers por arquitectura para puente/hooks de ventana o arquitectura mixta (**Inferido**).

5. **Drivers virtuales e integraciones** — **Observado** (listado de directorio `C:\Program Files\TeamViewer\`):
   - `x64\TVVirtualMonitorDriver.dll` + `.inf`/`.cat`: Indirect Display Driver (IDD) para monitores virtuales / sesiones headless.
   - `x64\TV_Audio.sy_` + `.inf`/`.cat`: driver virtual de audio.
   - `x64\TeamViewerVPN.sy_` + `.inf`/`.cat`: adaptador VPN virtual.
   - `Printer\TeamViewer_XPSDriverFilter.dll` + `.gpd`/`.inf`: filtro de impresión remota XPS.
   - `outlook\ManagedAggregator.dll` y `TeamViewerMeetingAddIn.dll`: complementos `.NET` detectados por REA (`dsc_85bf0b14...`).

---

## 3. Endurecimiento binario y firma Authenticode

| Binario | ASLR / HighEntropyVA | NX | CFG (`GuardFlags`) | XFG ptr | SafeSEH | Firma Authenticode (observada) |
| :--- | :---: | :---: | :---: | :---: | :---: | :--- |
| `TeamViewer.exe` | Sí / Sí | Sí | Sí (`135,375` fns, `EXPORT_SUPPRESSION`, `LONGJUMP`) | Presente (`0x143c3eb18`) | N/A (PE32+) | Válida (`CN=TeamViewer Germany GmbH`, DigiCert Trusted G4 RSA4096 SHA384 2021 CA1, `2026-03-17`..`2027-05-06`) |
| `TeamViewer_Service.exe` | Sí / Sí | Sí | Sí (`43,804` fns, `EXPORT_SUPPRESSION`, `LONGJUMP`) | Presente (`0x1412b0a08`) | N/A (PE32+) | Válida (`CN=TeamViewer Germany GmbH`) |
| `TeamViewer_Desktop.exe` | Sí / Sí | Sí | Sí (`31,137` fns, `EXPORT_SUPPRESSION`, `LONGJUMP`) | Presente (`0x140e06e40`) | N/A (PE32+) | Válida (`CN=TeamViewer Germany GmbH`) |

*Nota REA:* `FORCE_INTEGRITY` está deshabilitado en los 3 ejecutables principales y `IMAGE_DIRECTORY_ENTRY_DEBUG` está presente (`crashpad_handler.exe` `0.9.1`). `inspect_pe_certificates` decodifica estructuralmente el bloque PKCS#7; la validez de cadena OS se confirmó adicionalmente con `Get-AuthenticodeSignature` (`Valid`).

---

## 4. Componentes OSS declarados en `CopyRights.txt` (muestra verificada)

A partir de `C:\Program Files\TeamViewer\CopyRights.txt` (3,121,915 bytes, **Observado**):
- Medios/transporte: `WebRTC`, `opus@1.4`, `libjpeg-turbo`, `abseil-cpp@20250814.2`, `protobuf@7.34.1`, `curl@8.22.0`
- Criptografía/compresión/datos: `openssl@3.6.4`, `zstd@1.5.7`, `zlib/libpng`, `sqlite@3.53.4`, `boost@1.92.0`
- UI/web: `webview2@1.0.4129.50` (+ `sciter.dll` en directorio)

---

## 5. Pasos reproducibles y limitaciones

### Pasos reproducibles
1. Ejecutar `inspect_desktop_application` con `path: "C:\\Program Files\\TeamViewer"`, `detail: "summary"`.
2. Ejecutar `inspect_pe_imports`, `inspect_binary_hardening`, `inspect_pe_load_config`, `inspect_pe_certificates`, `inspect_pe_resources` e `inspect_native_exports` sobre los ejecutables/DLLs listados (usando `max_file_bytes: 134217728` para `TeamViewer.exe`).
3. Consultar configuración de servicio con `sc.exe qc TeamViewer` y telemetría de proceso en reposo con `Get-Process "*TeamViewer*"`.

### Limitaciones explícitas
- El análisis estático PE (IAT/exports/load-config/resources) **no** observa carga dinámica vía `LoadLibrary`/`GetProcAddress`, servidores COM/WinRT ni tráfico de red en sesión activa.
- Los flujos de portal web, QuickSupport portable, códecs negociados en caliente y latencia/ancho de banda en sesión activa permanecen **No verificados** en este entorno (solo se evaluó la instalación local en reposo).
