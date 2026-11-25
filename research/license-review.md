# Revisión de Licencias y Cumplimiento Legal (`research/license-review.md`)

**Fecha:** 25 de noviembre de 2026  
**Propósito:** Evaluar riesgos de licencias OSS, copyleft, patentes de códecs, términos de IA/OAuth y requisitos de firma en Windows antes de iniciar la implementación en Rust.

---

## 1. Competidores y regla Clean-Room

| Proyecto | Licencia verificada | Riesgo para nuestro producto | Decisión / Regla obligatoria |
| :--- | :--- | :--- | :--- |
| **RustDesk** (`rustdesk/rustdesk` y `rustdesk/rustdesk-server`) | **AGPL-3.0** (`[Documentado]` vía GitHub 2026-11-25) | **Crítico (Copyleft de red):** Si se reutiliza o deriva código de RustDesk en cliente o servidor ofrecido en red/SaaS/self-hosted comercial, toda la obra derivada queda obligada a publicarse bajo AGPL-3.0. | **Prohibido copiar o adaptar código de RustDesk.** Diseño e implementación 100% *clean-room* en Rust. |
| **MeshCentral** (`Ylianst/MeshCentral`) | **Apache-2.0** (`[Documentado]`) | Permisiva (solo referencia conceptual; su stack es Node.js, descartado por PRD). | No copiar código JS; usar solo como referencia de arquitectura de protocolos/gestión. |
| **TeamViewer** (`15.83.7.0`) | Propietario + OSS atribuidos en `CopyRights.txt` (`[Observado]`) | Prohibida copia de código o ingeniería inversa para extracción de secretos/evasión. | Solo análisis de capacidades observables e interfaz Win32 estándar (`RES-04`). |

---

## 2. Stack UI en Rust (Web y Escritorio)

| Framework | Licencia verificada | Uso comercial sin regalías | Impacto y recomendación |
| :--- | :--- | :---: | :--- |
| **Leptos** (Frontend Web SSR/WASM) | **MIT** | **Sí** | **Aprobado (P0).** Sin restricciones copyleft; ideal para portal público, consola administrativa y `/app`. |
| **egui** (+ `eframe` + `wgpu` + `accesskit`) | **MIT OR Apache-2.0** | **Sí** | **Recomendado para cliente nativo.** Totalmente permisiva; integra `wgpu` directamente para render de video de baja latencia y `AccessKit` para accesibilidad Windows UIA (`WCAG`). |
| **iced** | **MIT** | **Sí** | **Alternativa viable.** Licencia permisiva; madurez de tablas densas tipo Linear inferior a web/egui. |
| **Tauri** (Rust + WebView2) | **MIT OR Apache-2.0** | **Sí** | **Alternativa válida para UI de administración/bandeja**, pero depende de Edge WebView2 (~100–200 MB RAM por ventana) y requiere puente separado para render de video `wgpu` a 60 FPS sin copias GPU→CPU. |
| **Slint** | **Triple licencia: GPLv3 / Royalty-Free / Commercial** (`[Documentado]`) | Condicional | **Requiere decisión comercial.** La versión Royalty-Free exige atribución pública obligatoria (`AboutSlint` / badge visible) y prohíbe sistemas embebidos sin licencia paga. Si se busca marca blanca total en clientes personalizados (`WEB-01`/`WEB-03`), usar `egui` o pagar licencia comercial de Slint. |

---

## 3. Códecs de video y audio (`ENG-02`)

| Códec | Implementación evaluada | Licencia / Patentes | Decisión propuesta |
| :--- | :--- | :--- | :--- |
| **H.264 (AVC)** | **Windows Media Foundation (`IMFTransform` / hardware NVENC/QSV/AMF)** proporcionado por el sistema operativo Windows | El binario Rust llama al API del SO sin redistribuir `x264` (GPL) ni binaries OpenH264. El hardware/SO cubre las licencias del decodificador/codificador del usuario en Windows. | **Aprobado para P0 en Windows.** Evita `x264` (GPLv2) y evita cargar peso extra en QuickSupport. |
| **H.264 Software fallback** | **Cisco OpenH264** (`BSD-2-Clause`, descargable/dinámico) vs `x264` (`GPL`) | Cisco cubre regalías de patente solo si se usa su binario oficial firmado descargado desde Cisco; compilarlo estáticamente pierde ese amparo. `x264` es GPL (incompatible con código propietario). | No incluir `x264`. Evaluar OpenH264 dinámico solo si un endpoint carece de MFT H.264. |
| **H.265 (HEVC)** | Media Foundation / hardware | Fragmentación severa de pools de patentes (MPEG LA, HEVC Advance, Velos) y soporte desigual en Windows sin extensión HEVC instalada. | **Diferir a P1/P2** como opcional si el SO ya posee MFT HEVC. |
| **VP9 / AV1** | `libvpx` (`BSD-3-Clause`) / `rav1e` (`BSD-2-Clause`) / `dav1d` (`BSD-2-Clause`) / hardware MFT AV1 | Libres de regalías (Alliance for Open Media / Google BSD); ideales para software fallback y futuro visor web `WebCodecs`. | **Aprobado para evaluación en M0.5 / P1** junto con H.264 MF. |
| **Opus** (audio) | `opus` (`BSD-3-Clause`) + `WASAPI` | Libre de regalías (IETF RFC 6716). | **Aprobado para P0/P1.** |

---

## 4. Crates core de Rust

| Crate / Familia | Licencia | Riesgo |
| :--- | :--- | :--- |
| `tokio`, `axum`, `tower`, `sqlx`, `serde`, `prost`, `quinn`, `rustls`, `snow` (Noise), `ed25519-dalek`, `argon2`, `wgpu`, `windows` (`windows-rs`), `tracing`, `prometheus`, `utoipa` (OpenAPI) | **MIT / Apache-2.0 / BSD** | **Bajo.** Compatible tanto con producto propietario como open-source permisivo. Generar SBOM automático en CI con `cargo-deny` y `cargo-cyclonedx`. |
| Driver Virtual Monitor Windows (IDD, `ENG-06` P2) | WDK / UMDF2 (`MIT` o código propio) | **Requiere firma WHQL / EV Attestation Signing de Microsoft** para cargar en Windows 10/11 con Secure Boot activado. |

---

## 5. Integración ChatGPT / OAuth (`AI-02`)

- **Estado documentado (`2026-11-25`):** OpenAI dispone de **"Sign in with ChatGPT"** (OAuth 2.0 + PKCE + OIDC contra `https://auth.openai.com/.well-known/openid-configuration`).
- **Restricción crítica de elegibilidad:** La delegación de cuota de suscripción ChatGPT Plus/Pro para terceros tiene acceso abierto para proyectos *open-source/personales*, pero **los socios comerciales requieren aprobación explícita / programa piloto de OpenAI**.
- **Regla obligatoria:**
  1. Implementar **OpenAI Platform API oficial** + **Ollama / `llama.cpp` local** como ruta garantizada en P1.
  2. Registrar `Sign in with ChatGPT` como integración opcional pendiente de elegibilidad comercial; **prohibido** usar endpoints privados de ChatGPT web, scraping o reutilización de tokens de Codex CLI.

---

## 6. Firma de código y distribución Windows (`SEC-05`)

- Desde junio 2023, las firmas Authenticode estándar exigen clave privada almacenada en **HSM compatible con FIPS 140-2 Nivel 2+ o Azure Trusted Signing / DigiCert KeyLocker**.
- Windows SmartScreen requiere reputación o certificado **EV / Trusted Signing** para evitar alertas bloqueantes en `QuickSupport.exe` descargado desde el portal `/downloads`.
- Todos los manifiestos de actualización deben firmarse adicionalmente con **Ed25519** verificable offline por el agente antes de ejecutar cualquier binario nuevo.
