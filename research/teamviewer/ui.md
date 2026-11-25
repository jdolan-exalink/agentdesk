# Análisis de UI y UX — TeamViewer vs. Propuesta Propia

- **Producto analizado:** TeamViewer Desktop Windows `15.83.7.0`
- **Base de evidencia:** [evidence.md](file:///d:/DEVs/AgentDesk/research/teamviewer/evidence.md)

## 1. Pila de interfaz observada en TeamViewer

| Componente observado | Evidencia REA / Disco | Tamaño / Huella | Clasificación |
| :--- | :--- | ---: | :--- |
| `sciter.dll` | 276 símbolos exportados (`ev_0fbb68e6...`) | 17.84 MB | **Observado** |
| `TeamViewer_StaticRes.dll` | Recursos estáticos embebidos | 42.31 MB | **Observado** |
| `TeamViewer.exe` (proceso GUI Session 1) | `Get-Process`: `280.24 MB` privados, `109` hilos, `1,528` handles en reposo | 87.87 MB en disco / 280 MB RAM | **Observado** |
| `utils\MicrosoftEdgeWebview2Setup.exe` + `CopyRights.txt` | `webview2@1.0.4129.50` | 1.78 MB instalador | **Observado** |
| `TeamViewer_Resource_*.dll` (39 idiomas) | DLLs de localización satélite | ~14 MB acumulados | **Observado** |

## 2. Evaluación UX y hallazgos clave

1. **Hibridación pesada en memoria (**Observado**):** Mantener simultáneamente `TeamViewer.exe` (Sciter + recursos + puente WebView2) en Session 1 consume **~280 MB de memoria privada en reposo** y **109 hilos**, incluso cuando no hay ninguna sesión remota activa.
2. **Doble fuente de verdad UI (**Inferido**):** En TeamViewer coexisten paneles nativos/Sciter locales y vistas embebidas web/WebView2 de su portal cloud. Para los operadores esto suele traducirse en inconsistencias entre la configuración local y la consola web (**Documentado**).
3. **Objetivo de diseño Linear / Fluent para Nuestra Plataforma:**
   - **Web como única fuente de verdad (Leptos SSR + WASM):** Todas las pantallas administrativas, tickets, agenda, calendario, RMM, IA y descargas viven en una sola SPA/SSR rápida en `/app` y `/admin`.
   - **Navegación tipo Linear:** Barra lateral plegable, paleta de comandos `Ctrl+K` (buscar dispositivos, tickets, acciones en <100 ms), tablas densas con columnas configurables, filtros persistentes en URL y panel lateral deslizable de ficha de equipo.
   - **Cliente nativo liviano (`apps/desktop`):** Enfoque puro en visor de video `wgpu`, pestañas multimonitor, captura/control y lista operativa sincronizada vía API REST/WebSocket del servidor, evitando embeber navegadores completos en memoria cuando está minimizado en bandeja.
   - **Accesibilidad:** Contraste y navegación completa por teclado apuntando a **WCAG 2.2 AA** (punto clave a considerar al elegir `egui` + `AccessKit` vs `Slint` en `ADR-002`).
