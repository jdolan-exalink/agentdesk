# ADR-005: Identidad, Multi-tenancy, Políticas y Privacidad IA

- **Estado:** Propuesto (pendiente aprobación M0)
- **Fecha:** 2026-11-25
- **Requisitos:** `IAM-01` .. `IAM-05`, `SEC-01` .. `SEC-04`, `AI-01` .. `AI-06`

## Contexto
La plataforma es multiempresa desde P0 (`Organización → Cliente → Sede → Grupo → Dispositivo`), exige que la web sea la única fuente de verdad, y requiere un motor de políticas independiente que regule tanto las sesiones humanas como las herramientas invocadas por el copiloto IA.

## Decisión
1. **Identidad de usuarios y autenticación (`crates/identity`):**
   - Primer arranque (`IAM-01`): bootstrap token criptográfico de un solo uso generado al iniciar el servidor por primera vez (impreso en consola/instalador), que habilita el asistente web para crear el primer `InstanceAdmin`.
   - Contraseñas hashadas con **Argon2id** (`argon2` crate, parámetros OWASP).
   - Sesiones web con cookies `HttpOnly; Secure; SameSite=Lax`, token de refresco rotatorio en PostgreSQL y revocación inmediata.
   - MFA TOTP obligatorio configurable por organización (WebAuthn passkeys + OIDC/LDAP en P1).
2. **Identidad criptográfica de dispositivos:**
   - Cada instalación Host/Full Client genera localmente un par **Ed25519** (`device_id` derivado de clave pública + UUID interno). La clave privada se almacena cifrada con **Windows DPAPI** (o secret service/permisos `0600` en Linux).
   - Enrolamiento de Host mediante tokens de invitación de alcance limitado (organización/grupo específico), con TTL y límite de usos, revocables desde `/admin`.
3. **Modelo RBAC + ABAC unificado (`crates/policy`):**
   - Roles base (`IAM-02`): `instance_admin`, `org_admin`, `supervisor`, `technician`, `limited_technician`, `auditor`, `client_user`.
   - Denegación por defecto: toda operación HTTP, WebSocket, inicio de sesión remota y herramienta IA consulta `PolicyEngine::authorize(actor, tenant_scope, action, resource)`.
   - Aislamiento multi-tenant estricto: ninguna consulta SQL puede ejecutarse sin predicado explícito de `organization_id` (salvo `instance_admin` en rutas `/admin` auditadas).
4. **Privacidad y gobierno de IA (`crates/ai`):**
   - Proveedores intercambiables mediante trait `LlmProvider` (`OpenAiApiProvider`, `LocalOllamaProvider`; `ChatGptOAuthProvider` diferido hasta verificación de elegibilidad comercial).
   - **Redactor PII/Secretos** obligatorio previo a cualquier llamada externa (máscara de API keys, contraseñas, tokens Bearer, emails/IPs si la política de organización lo exige).
   - Los niveles de autonomía (`AI-04`) se aplican en el servidor, jamás en el prompt del modelo.

## Consecuencias
- Garantiza consistencia absoluta entre portal web, clientes nativos y futuro servidor MCP (`AI-06`), sin duplicar reglas ni bases de datos.
