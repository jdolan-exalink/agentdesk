# ADR-002: UI Web (Leptos) y Cliente Nativo (egui vs. Slint vs. Tauri)

- **Estado:** Aceptado (`egui + wgpu + AccessKit` aprobado el 2026-11-25)
- **Fecha:** 2026-11-25
- **Requisitos:** PRD §2, §4, §6, §7 (`Leptos`, `Slint frente a egui`), `license-review.md`

## Contexto
1. La web es la única fuente de verdad administrativa (`WEB-01..07`, Dashboard estilo Fluent/Linear, `WCAG 2.2 AA`, `Ctrl+K`, tablas densas).
2. El PRD prohíbe Electron por defecto y exige evaluar `Slint` vs `egui` considerando **licencias** y **accesibilidad**, además de medir el peso en reposo (TeamViewer GUI ocupa `280.24 MB` privados por usar `sciter.dll` + `WebView2`).

## Decisión
1. **Frontend Web (Aprobado P0): `Leptos` (Rust SSR + Hydration WASM)** servido directamente por `axum` con CSS moderno (tokens Fluent/Linear, tema claro/oscuro, sin bundler JS complejo ni Node en runtime). Puentes JS mínimos exclusivamente para APIs de navegador sin binding directo (portapapeles Web, WebAuthn en P1, `WebCodecs` evaluado en P2).
2. **Cliente Nativo Desktop / QuickSupport (Propuesta recomendada: `egui` + `eframe` + `wgpu` + `accesskit`)**:
   - **Licencia:** `MIT OR Apache-2.0` (100% libre de regalías, marca blanca total sin atribución obligatoria en UI de clientes personalizados `WEB-03`).
   - **Rendimiento:** Render directo en GPU vía `wgpu` sin copias GPU→CPU ni motor browser embebido (~10–25 MB en reposo vs `280 MB` de TeamViewer GUI; tamaño binario QuickSupport estimado ~8–15 MB).
   - **Accesibilidad:** Integración nativa con **`AccessKit`** (expone el árbol de accesibilidad Windows UI Automation para lectores de pantalla y navegación por teclado).
3. **Alternativas evaluadas y descartadas por defecto:**
   - **Slint:** Excelente estética declarativa, pero su licencia gratuita (*Royalty-Free*) exige **atribución pública obligatoria (`AboutSlint`)** y restringe uso embebido salvo compra de licencia comercial. Elegible solo si el usuario aprueba adquirir licencia comercial o mostrar atribución.
   - **Tauri (Rust + WebView2):** Útil para vistas puramente administrativas, pero en Windows depende de Edge WebView2 (~150–250 MB de RAM en procesos Chromium hijos al abrir ventana) y complica el render de video 4K60 zero-copy en `wgpu`.

## Criterios de validación (M0.5)
- Medir tamaño del ejecutable `QuickSupport.exe` compilado con `egui + wgpu` (release + LTO + strip).
- Validar navegación por teclado (`Tab`, `Ctrl+K`) y lectura básica mediante Windows Narrator / UIA con `AccessKit`.
