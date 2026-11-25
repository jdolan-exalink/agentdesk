use axum::response::Html;

const BASE_CSS: &str = r#"
:root {
  --bg: #0b0e14;
  --surface: #121722;
  --surface-2: #181f2e;
  --border: #242d3f;
  --text: #e6edf7;
  --muted: #8b9bb4;
  --primary: #3b82f6;
  --primary-hover: #2563eb;
  --ok: #22c55e;
  --warn: #f59e0b;
  --err: #ef4444;
  --radius: 8px;
  --font: 'Segoe UI', system-ui, -apple-system, sans-serif;
}
[data-theme="light"] {
  --bg: #f6f8fb;
  --surface: #ffffff;
  --surface-2: #eef2f7;
  --border: #d8e0ec;
  --text: #0f172a;
  --muted: #475569;
}
* { box-sizing: border-box; }
body { margin: 0; font-family: var(--font); background: var(--bg); color: var(--text); font-size: 14px; line-height: 1.5; }
a { color: var(--primary); text-decoration: none; }
a:hover { text-decoration: underline; }
.nav { display: flex; align-items: center; justify-content: space-between; padding: 12px 24px; border-bottom: 1px solid var(--border); background: var(--surface); }
.brand { font-weight: 700; font-size: 16px; display: flex; align-items: center; gap: 8px; color: var(--text); }
.badge { display: inline-flex; align-items: center; gap: 5px; padding: 2px 8px; border-radius: 999px; font-size: 12px; font-weight: 600; background: var(--surface-2); border: 1px solid var(--border); }
.badge.ok { color: var(--ok); border-color: rgba(34,197,94,.35); }
.badge.off { color: var(--muted); }
.badge.warn { color: var(--warn); border-color: rgba(245,158,11,.35); }
.btn { display: inline-flex; align-items: center; gap: 6px; padding: 7px 14px; border-radius: 6px; border: 1px solid var(--border); background: var(--surface-2); color: var(--text); cursor: pointer; font-size: 13px; font-weight: 500; }
.btn:hover { border-color: var(--primary); text-decoration: none; }
.btn.primary { background: var(--primary); border-color: var(--primary); color: #fff; }
.btn.primary:hover { background: var(--primary-hover); }
.btn.danger { background: rgba(239,68,68,.15); border-color: rgba(239,68,68,.4); color: #fca5a5; }
.container { max-width: 1120px; margin: 0 auto; padding: 28px 20px; }
.card { background: var(--surface); border: 1px solid var(--border); border-radius: var(--radius); padding: 18px; }
.grid { display: grid; gap: 14px; }
.grid-3 { grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); }
.grid-4 { grid-template-columns: repeat(auto-fit, minmax(190px, 1fr)); }
input, select, textarea { width: 100%; padding: 8px 10px; border-radius: 6px; border: 1px solid var(--border); background: var(--bg); color: var(--text); font-family: inherit; font-size: 13px; }
label { display: block; font-size: 12px; color: var(--muted); margin-bottom: 4px; font-weight: 600; }
.field { margin-bottom: 12px; }
table { width: 100%; border-collapse: collapse; font-size: 13px; }
th, td { text-align: left; padding: 9px 10px; border-bottom: 1px solid var(--border); }
th { color: var(--muted); font-weight: 600; font-size: 12px; text-transform: uppercase; letter-spacing: .03em; }
tr:hover td { background: rgba(59,130,246,.04); }
.mono { font-family: ui-monospace, SFMono-Regular, Consolas, monospace; font-size: 12px; }
.muted { color: var(--muted); }
.row { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
"#;

fn shell(title: &str, body: &str) -> Html<String> {
    Html(format!(
        r#"<!doctype html>
<html lang="es">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width,initial-scale=1" />
  <title>{title} — AgentDesk</title>
  <style>{BASE_CSS}</style>
</head>
<body>
  <header class="nav">
    <a class="brand" href="/">AgentDesk <span class="badge">Self-Hosted Rust</span></a>
    <nav class="row">
      <a class="btn" href="/downloads">Descargar</a>
      <a class="btn" href="/docs">Docs</a>
      <a class="btn" href="/support">Solicitar asistencia</a>
      <a class="btn" href="/support/status">Estado</a>
      <a class="btn primary" href="/app">Consola</a>
    </nav>
  </header>
  {body}
</body>
</html>"#
    ))
}

pub async fn landing_page() -> Html<String> {
    shell(
        "Inicio",
        r#"<main class="container">
  <section class="card" style="padding:32px;margin-bottom:20px;">
    <span class="badge ok">Motor nativo Rust + E2EE Noise_XX</span>
    <h1 style="margin:12px 0 8px;font-size:30px;">Soporte remoto inteligente, privado y self-hosted</h1>
    <p class="muted" style="max-width:720px;margin-bottom:20px;">
      Alternativa integral a TeamViewer, AnyDesk y RustDesk. Portal web, centro de descargas, asistencia sin fricciones,
      agenda de clientes y visor remoto nativo en Rust desde una única instalación bajo tu control.
    </p>
    <div class="row">
      <a class="btn primary" href="/support">Solicitar asistencia ahora</a>
      <a class="btn" href="/downloads">Centro de descargas</a>
      <a class="btn" href="/login">Iniciar sesión</a>
      <a class="btn" href="/setup">Primer inicio (Setup)</a>
    </div>
  </section>
  <section class="grid grid-3">
    <div class="card">
      <h3 style="margin-top:0;">Cero nube del fabricante</h3>
      <p class="muted">Relay ciego E2EE (Noise_XX) y servidor de control Axum + PostgreSQL alojados en tu propia infraestructura.</p>
    </div>
    <div class="card">
      <h3 style="margin-top:0;">Clientes Agendados y Agentes</h3>
      <p class="muted">Agenda compartida por empresa con estado en vivo de equipos Host y sesiones temporales QuickSupport.</p>
    </div>
    <div class="card">
      <h3 style="margin-top:0;">Asistencia Web en 1 clic</h3>
      <p class="muted">Ticket atómico desde navegador, código público, token de seguimiento y lanzamiento directo vía <code>agentdesk://</code>.</p>
    </div>
  </section>
</main>"#,
    )
}

pub async fn downloads_page() -> Html<String> {
    shell(
        "Descargas",
        r#"<main class="container">
  <h1>Centro de descargas</h1>
  <p class="muted">Binarios compilados en Rust con verificación SHA-256 en tiempo real.</p>
  <div id="list" class="grid grid-3" style="margin-top:16px;">Cargando artefactos...</div>
  <script>
  fetch('/api/v1/downloads').then(r=>r.json()).then(items=>{
    const el=document.getElementById('list');
    el.innerHTML=items.map(i=>`
      <div class="card">
        <div class="row" style="justify-content:space-between;">
          <strong>${i.name}</strong>
          <span class="badge ${i.available?'ok':'warn'}">${i.available?'Disponible':'No compilado'}</span>
        </div>
        <p class="muted">${i.notes}</p>
        <div class="mono muted" style="word-break:break-all;margin:8px 0;">
          ${i.platform} ${i.architecture} · v${i.version} · ${(i.size_bytes/1048576).toFixed(2)} MB<br/>
          SHA-256: ${i.sha256}
        </div>
        ${i.available?`<a class="btn primary" href="${i.download_url}">Descargar ${i.filename}</a>`:`<span class="muted">Ejecutar cargo build --workspace</span>`}
      </div>`).join('');
  });
  </script>
</main>"#,
    )
}

pub async fn support_page() -> Html<String> {
    shell(
        "Solicitar Asistencia",
        r#"<main class="container">
  <div class="grid grid-3" style="grid-template-columns:1.2fr .8fr;">
    <section class="card">
      <h2 style="margin-top:0;">Solicitar asistencia remota</h2>
      <form id="sup-form">
        <div class="field"><label>Nombre</label><input id="name" required placeholder="Juan Pérez"/></div>
        <div class="field"><label>Contacto (email o teléfono)</label><input id="contact" required placeholder="juan@empresa.com"/></div>
        <div class="field"><label>Empresa (opcional)</label><input id="company" placeholder="Acme S.A."/></div>
        <div class="field"><label>Categoría</label>
          <select id="category"><option>Soporte General</option><option>Acceso / Red</option><option>Urgente</option></select>
        </div>
        <div class="field"><label>Descripción del problema</label><textarea id="desc" rows="3" required placeholder="Describe qué ocurre..."></textarea></div>
        <div class="field row">
          <input id="consent" type="checkbox" style="width:auto;" required />
          <label for="consent" style="margin:0;">Acepto el tratamiento de datos y el inicio consentido de la sesión de soporte.</label>
        </div>
        <button class="btn primary" type="submit">Crear solicitud</button>
      </form>
    </section>
    <section class="card" id="status-box">
      <h3 style="margin-top:0;">Estado de solicitud</h3>
      <p class="muted">Completa el formulario para recibir tu código público y token de seguimiento.</p>
      <div id="status-detail"></div>
    </section>
  </div>
  <script>
  let pollTimer=null;
  document.getElementById('sup-form').addEventListener('submit', async (e)=>{
    e.preventDefault();
    const res = await fetch('/api/v1/support/requests',{
      method:'POST',headers:{'Content-Type':'application/json'},
      body:JSON.stringify({
        requester_name: document.getElementById('name').value,
        requester_contact: document.getElementById('contact').value,
        company_name: document.getElementById('company').value,
        category: document.getElementById('category').value,
        description: document.getElementById('desc').value,
        consent_accepted: document.getElementById('consent').checked
      })
    });
    const d = await res.json();
    const box = document.getElementById('status-detail');
    if(!res.ok){ box.innerHTML = `<span class="badge warn">${d.error}</span>`; return; }
    const render=(st)=>`
      <div class="badge ok">${st}</div>
      <p><strong>Código:</strong> <span class="mono">${d.public_code}</span></p>
      <p><strong>Deep link:</strong> <a class="mono" href="${d.deep_link}">${d.deep_link}</a></p>
      <p class="muted">Si no tienes el agente instalado, ejecuta QuickSupport con:</p>
      <pre class="card mono" style="overflow:auto;">agentdesk-host.exe --mode quicksupport --support-code ${d.public_code}</pre>
      <button class="btn danger" id="cancel-btn">Cancelar solicitud</button>`;
    box.innerHTML = render(d.status);
    document.getElementById('cancel-btn').onclick = async ()=>{
      await fetch(`/api/v1/support/requests/${d.public_code}/cancel?token=${encodeURIComponent(d.tracking_token)}`,{method:'POST'});
      box.innerHTML = `<span class="badge warn">cancelled</span>`;
      if(pollTimer) clearInterval(pollTimer);
    };
    if(pollTimer) clearInterval(pollTimer);
    pollTimer = setInterval(async ()=>{
      const r = await fetch(`/api/v1/support/requests/${d.public_code}/status?token=${encodeURIComponent(d.tracking_token)}`);
      if(r.ok){ const t = await r.json(); const b=box.querySelector('.badge'); if(b) b.textContent=t.status; }
    }, 2500);
  });
  </script>
</main>"#,
    )
}

pub async fn setup_page() -> Html<String> {
    shell(
        "Primer Inicio",
        r#"<main class="container" style="max-width:520px;">
  <div class="card">
    <h2 style="margin-top:0;">Asistente de primer inicio</h2>
    <p class="muted">Crea la organización principal y el primer administrador usando el token de bootstrap impreso en la consola del servidor.</p>
    <form id="setup-form">
      <div class="field"><label>Organización</label><input id="org_name" required value="Mi Empresa"/></div>
      <div class="field"><label>Slug</label><input id="org_slug" required value="mi-empresa"/></div>
      <div class="field"><label>Nombre Administrador</label><input id="admin_name" required value="Administrador"/></div>
      <div class="field"><label>Email</label><input id="admin_email" type="email" required value="juan@dolan.com.ar"/></div>
      <div class="field"><label>Contraseña (mín. 8 caracteres)</label><input id="admin_pw" type="password" required/></div>
      <div class="field"><label>Bootstrap Token</label><input id="boot_tok" required placeholder="Token de un solo uso"/></div>
      <div id="err" class="muted" style="color:var(--err);margin-bottom:8px;"></div>
      <button class="btn primary" type="submit">Inicializar servidor</button>
    </form>
  </div>
  <script>
  document.getElementById('setup-form').onsubmit=async(e)=>{
    e.preventDefault();
    const res=await fetch('/api/v1/setup/bootstrap',{
      method:'POST',headers:{'Content-Type':'application/json'},
      body:JSON.stringify({
        organization_name: org_name.value,
        organization_slug: org_slug.value,
        admin_name: admin_name.value,
        admin_email: admin_email.value,
        admin_password: admin_pw.value,
        bootstrap_token: boot_tok.value
      })
    });
    const d=await res.json();
    if(!res.ok){ err.textContent=d.error||'Error'; return; }
    location.href='/app';
  };
  </script>
</main>"#,
    )
}

pub async fn login_page() -> Html<String> {
    shell(
        "Iniciar Sesión",
        r#"<main class="container" style="max-width:440px;">
  <div class="card">
    <h2 style="margin-top:0;">Iniciar sesión en AgentDesk</h2>
    <form id="login-form">
      <div class="field"><label>Email</label><input id="email" type="email" required/></div>
      <div class="field"><label>Contraseña</label><input id="pw" type="password" required/></div>
      <div id="err" style="color:var(--err);margin-bottom:8px;"></div>
      <div class="row" style="justify-content:space-between;">
        <button class="btn primary" type="submit">Entrar</button>
        <a href="/setup">¿Primera vez? Ir a Setup</a>
      </div>
    </form>
  </div>
  <script>
  login_form = document.getElementById('login-form');
  login_form.onsubmit=async(e)=>{
    e.preventDefault();
    const res=await fetch('/api/v1/auth/login',{
      method:'POST',headers:{'Content-Type':'application/json'},
      body:JSON.stringify({email:email.value,password:pw.value})
    });
    const d=await res.json();
    if(!res.ok){ err.textContent=d.error||'Credenciales inválidas'; return; }
    location.href='/app';
  };
  </script>
</main>"#,
    )
}

pub async fn app_dashboard_page() -> Html<String> {
    let tpl = r#"<!doctype html>
<html lang="es">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width,initial-scale=1" />
  <title>Consola — AgentDesk</title>
  <style>__BASE_CSS__
  .layout { display:grid; grid-template-columns:230px 1fr; min-height:100vh; }
  .sidebar { background:var(--surface); border-right:1px solid var(--border); padding:16px; display:flex; flex-direction:column; gap:6px; }
  .nav-item { padding:8px 10px; border-radius:6px; cursor:pointer; color:var(--muted); font-weight:500; }
  .nav-item.active, .nav-item:hover { background:var(--surface-2); color:var(--text); }
  .topbar { display:flex; align-items:center; justify-content:space-between; padding:12px 22px; border-bottom:1px solid var(--border); background:var(--surface); gap:12px; }
  .content { padding:22px; }
  .kpi { font-size:24px; font-weight:700; margin-top:4px; }
  @media(max-width:860px){ .layout { grid-template-columns:1fr; } .sidebar { flex-direction:row; overflow:auto; } }
  </style>
</head>
<body>
<div class="layout">
  <aside class="sidebar">
    <div class="brand" style="margin-bottom:12px;">AgentDesk</div>
    <div class="nav-item active" data-tab="agenda">Clientes Agendados</div>
    <div class="nav-item" data-tab="devices">Equipos con Agente</div>
    <div class="nav-item" data-tab="tickets">Cola de Soporte</div>
    <div class="nav-item" data-tab="audit">Auditoría</div>
    <div style="margin-top:auto;padding-top:16px;border-top:1px solid var(--border);">
      <a class="nav-item" href="/" style="display:block;">Portal público</a>
      <a class="nav-item" href="/downloads" style="display:block;">Descargas</a>
    </div>
  </aside>
  <div>
    <header class="topbar">
      <div class="row" style="flex:1;max-width:420px;">
        <input id="global-search" placeholder="Buscar equipo, cliente o ticket (Ctrl+K)..." />
      </div>
      <div class="row">
        <span id="org-badge" class="badge">...</span>
        <span id="sse-badge" class="badge ok">En vivo</span>
        <button class="btn" id="theme-btn">Tema</button>
        <button class="btn" id="logout-btn">Salir</button>
      </div>
    </header>
    <main class="content">
      <section class="grid grid-4" style="margin-bottom:18px;">
        <div class="card"><div class="muted">Equipos Online</div><div class="kpi" id="k-online">0</div></div>
        <div class="card"><div class="muted">Clientes Agendados</div><div class="kpi" id="k-agenda">0</div></div>
        <div class="card"><div class="muted">Tickets en Cola</div><div class="kpi" id="k-tickets">0</div></div>
        <div class="card"><div class="muted">Sesiones Activas</div><div class="kpi" id="k-sessions">0</div></div>
      </section>

      <section id="tab-agenda" class="card">
        <div class="row" style="justify-content:space-between;margin-bottom:12px;">
          <h2 style="margin:0;">Clientes Agendados</h2>
          <button class="btn primary" id="toggle-add-agenda">+ Agendar cliente</button>
        </div>
        <form id="agenda-form" class="card" style="display:none;margin-bottom:14px;background:var(--surface-2);">
          <div class="grid grid-3">
            <div class="field"><label>Alias / Equipo</label><input id="ag-alias" required placeholder="PC Caja 01"/></div>
            <div class="field"><label>Empresa cliente</label><input id="ag-company" placeholder="Cliente Norte S.A."/></div>
            <div class="field"><label>Contacto</label><input id="ag-contact" placeholder="María Gómez"/></div>
            <div class="field"><label>Grupo</label><input id="ag-group" value="General"/></div>
            <div class="field"><label>Vincular equipo (opcional)</label><select id="ag-device"><option value="">Sin vincular</option></select></div>
            <div class="field"><label>Próxima cita (opcional)</label><input id="ag-sched" type="datetime-local"/></div>
          </div>
          <div class="field"><label>Notas</label><input id="ag-notes" placeholder="Horario mantenimiento, detalles"/></div>
          <button class="btn primary" type="submit">Guardar cliente</button>
        </form>
        <table><thead><tr><th>Fav</th><th>Alias</th><th>Empresa / Contacto</th><th>Grupo</th><th>Equipo vinculado</th><th>Estado</th><th>Acciones</th></tr></thead><tbody id="agenda-body"></tbody></table>
      </section>

      <section id="tab-devices" class="card" style="display:none;">
        <div class="row" style="justify-content:space-between;margin-bottom:12px;">
          <h2 style="margin:0;">Equipos con Agente (Host / QuickSupport)</h2>
          <button class="btn primary" id="new-enroll-btn">+ Generar token de enrolamiento</button>
        </div>
        <div id="enroll-out" class="mono card" style="display:none;margin-bottom:12px;background:var(--surface-2);"></div>
        <table><thead><tr><th>Código</th><th>Hostname</th><th>Tipo</th><th>SO / Res</th><th>RAM</th><th>Estado</th><th>Acciones</th></tr></thead><tbody id="devices-body"></tbody></table>
      </section>

      <section id="tab-tickets" class="card" style="display:none;">
        <h2 style="margin-top:0;">Cola de Asistencia Web</h2>
        <table><thead><tr><th>Ticket</th><th>Solicitante</th><th>Empresa</th><th>Descripción</th><th>Estado</th><th>Acciones</th></tr></thead><tbody id="tickets-body"></tbody></table>
      </section>

      <section id="tab-audit" class="card" style="display:none;">
        <h2 style="margin-top:0;">Auditoría Append-Only</h2>
        <table><thead><tr><th>Hora UTC</th><th>Actor</th><th>Acción</th><th>Recurso</th><th>Resultado</th></tr></thead><tbody id="audit-body"></tbody></table>
      </section>
    </main>
  </div>
</div>
<script>
let state={devices:[],agenda:[],tickets:[],sessions:[],audit:[]};
document.addEventListener('keydown',(e)=>{if((e.ctrlKey||e.metaKey)&&e.key.toLowerCase()==='k'){e.preventDefault();document.getElementById('global-search').focus();}});
document.getElementById('theme-btn').onclick=()=>{document.documentElement.dataset.theme=document.documentElement.dataset.theme==='light'?'':'light';};
document.getElementById('logout-btn').onclick=async()=>{await fetch('/api/v1/auth/logout',{method:'POST'});location.href='/login';};
document.querySelectorAll('.nav-item[data-tab]').forEach(el=>el.onclick=()=>{
  document.querySelectorAll('.nav-item[data-tab]').forEach(n=>n.classList.remove('active'));
  el.classList.add('active');
  ['agenda','devices','tickets','audit'].forEach(t=>document.getElementById('tab-'+t).style.display=(t===el.dataset.tab?'block':'none'));
});
document.getElementById('toggle-add-agenda').onclick=()=>{const f=document.getElementById('agenda-form');f.style.display=f.style.display==='none'?'block':'none';};

async function api(url,opts){const r=await fetch(url,opts);if(r.status===401){location.href='/login';throw new Error('401');}return r.json();}

async function loadAll(){
  const me=await api('/api/v1/auth/me');
  document.getElementById('org-badge').textContent=me.organization_name+' · '+me.display_name;
  const [devices,agenda,tickets,sessions,audit]=await Promise.all([
    api('/api/v1/devices'),api('/api/v1/address-book'),api('/api/v1/tickets'),api('/api/v1/sessions'),api('/api/v1/audit')
  ]);
  state={devices,agenda,tickets,sessions,audit};
  render();
}

function render(){
  const q=(document.getElementById('global-search').value||'').toLowerCase();
  document.getElementById('k-online').textContent=state.devices.filter(d=>d.status==='online').length;
  document.getElementById('k-agenda').textContent=state.agenda.length;
  document.getElementById('k-tickets').textContent=state.tickets.filter(t=>t.status==='queued'||t.status==='assigned').length;
  document.getElementById('k-sessions').textContent=state.sessions.filter(s=>s.status==='active'||s.status==='pending').length;

  const sel=document.getElementById('ag-device');
  sel.innerHTML='<option value="">Sin vincular</option>'+state.devices.map(d=>`<option value="${d.id}">${d.hostname} (${d.public_code})</option>`).join('');

  document.getElementById('agenda-body').innerHTML=state.agenda.filter(a=>!q||JSON.stringify(a).toLowerCase().includes(q)).map(a=>`
    <tr>
      <td>${a.is_favorite?'★':'☆'}</td>
      <td><strong>${a.alias}</strong><div class="muted">${a.notes||''}</div></td>
      <td>${a.client_company||'-'}<div class="muted">${a.contact_name||''}</div></td>
      <td><span class="badge">${a.group_name}</span></td>
      <td class="mono">${a.device_hostname?`${a.device_hostname} (${a.device_public_code})`:'-'}</td>
      <td>${a.device_status?`<span class="badge ${a.device_status==='online'?'ok':'off'}">${a.device_status}</span>`:'<span class="muted">Sin equipo</span>'}</td>
      <td class="row">
        ${a.device_id?`<a class="btn primary" href="agentdesk://connect/${a.device_id}">Conectar</a>`:''}
        <button class="btn danger" onclick="delAgenda('${a.id}')">Eliminar</button>
      </td>
    </tr>`).join('');

  document.getElementById('devices-body').innerHTML=state.devices.filter(d=>!q||JSON.stringify(d).toLowerCase().includes(q)).map(d=>`
    <tr>
      <td class="mono">${d.public_code}</td>
      <td><strong>${d.hostname}</strong></td>
      <td><span class="badge">${d.kind}</span></td>
      <td>${d.os_name} (${d.screen_width}x${d.screen_height})</td>
      <td>${d.private_mem_mb.toFixed(1)} MB</td>
      <td><span class="badge ${d.status==='online'?'ok':'off'}">${d.status}</span></td>
      <td><a class="btn primary" href="agentdesk://connect/${d.id}">Conectar</a></td>
    </tr>`).join('');

  document.getElementById('tickets-body').innerHTML=state.tickets.filter(t=>!q||JSON.stringify(t).toLowerCase().includes(q)).map(t=>`
    <tr>
      <td class="mono">${t.public_code}</td>
      <td>${t.requester_name}<div class="muted">${t.requester_contact}</div></td>
      <td>${t.company_name||'-'}</td>
      <td>${t.description}</td>
      <td><span class="badge ${t.status==='queued'?'warn':'ok'}">${t.status}</span></td>
      <td class="row">
        ${t.status==='queued'?`<button class="btn primary" onclick="claimTicket('${t.id}')">Reclamar</button>`:''}
        ${t.device_id?`<a class="btn primary" href="agentdesk://connect/${t.device_id}">Conectar</a>`:''}
        ${t.status!=='closed'?`<button class="btn" onclick="closeTicket('${t.id}')">Cerrar</button>`:''}
      </td>
    </tr>`).join('');

  document.getElementById('audit-body').innerHTML=state.audit.map(e=>`
    <tr><td class="mono">${e.occurred_at}</td><td>${e.actor_type}</td><td class="mono">${e.action}</td><td class="mono">${e.resource_type}:${e.resource_id}</td><td>${e.outcome}</td></tr>`).join('');
}

document.getElementById('global-search').oninput=render;
document.getElementById('agenda-form').onsubmit=async(e)=>{
  e.preventDefault();
  const sched=document.getElementById('ag-sched').value;
  await api('/api/v1/address-book',{
    method:'POST',headers:{'Content-Type':'application/json'},
    body:JSON.stringify({
      device_id: document.getElementById('ag-device').value||null,
      alias: document.getElementById('ag-alias').value,
      client_company: document.getElementById('ag-company').value,
      contact_name: document.getElementById('ag-contact').value,
      contact_email: '',
      group_name: document.getElementById('ag-group').value,
      is_favorite: true,
      notes: document.getElementById('ag-notes').value,
      scheduled_next_at: sched?new Date(sched).toISOString():null
    })
  });
  e.target.reset(); e.target.style.display='none'; loadAll();
};
async function delAgenda(id){await api('/api/v1/address-book/'+id,{method:'DELETE'});loadAll();}
async function claimTicket(id){await api('/api/v1/tickets/'+id+'/claim',{method:'POST'});loadAll();}
async function closeTicket(id){await api('/api/v1/tickets/'+id+'/close',{method:'POST'});loadAll();}
document.getElementById('new-enroll-btn').onclick=async()=>{
  const r=await api('/api/v1/enrollments',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({label:'Enrolamiento Host',max_uses:25,expires_hours:72})});
  const out=document.getElementById('enroll-out');
  out.style.display='block';
  out.textContent='Ejecuta en el endpoint:\nagentdesk-host.exe --server '+location.origin+' --enroll-token '+r.enrollment_token;
};
loadAll();
const es=new EventSource('/api/v1/events');
['device.online','device.offline','device.enrolled','support.created','ticket.assigned','session.started'].forEach(ev=>es.addEventListener(ev,()=>loadAll()));
</script>
</body>
</html>"#;
    Html(tpl.replace("__BASE_CSS__", BASE_CSS))
}

pub async fn docs_page() -> Html<String> {
    shell(
        "Documentación",
        r#"<main class="container">
  <h1>Documentación y Ayuda — AgentDesk</h1>
  <p class="muted">Guías rápidas de instalación, acceso desatendido, seguridad E2EE y Copiloto IA.</p>
  <div id="docs-list" class="grid" style="margin-top:16px;">Cargando documentación...</div>
  <script>
  fetch('/api/v1/docs').then(r=>r.json()).then(pages=>{
    document.getElementById('docs-list').innerHTML=pages.map(p=>`
      <article class="card">
        <h2 style="margin-top:0;">${p.title}</h2>
        <pre style="white-space:pre-wrap;font-family:inherit;margin:0;">${p.body_markdown}</pre>
      </article>`).join('');
  });
  </script>
</main>"#,
    )
}

pub async fn status_page() -> Html<String> {
    shell(
        "Estado del Servicio",
        r#"<main class="container" style="max-width:760px;">
  <div class="card">
    <h1 style="margin-top:0;">Estado Operativo de AgentDesk</h1>
    <p class="muted">Estado agregado del plano de control, relay E2EE y base de datos (sin exponer topología interna ni clientes).</p>
    <div id="st-badge" class="badge ok">Verificando...</div>
    <div id="st-meta" class="mono muted" style="margin-top:12px;"></div>
  </div>
  <script>
  fetch('/health/ready').then(r=>r.json()).then(d=>{
    const b=document.getElementById('st-badge');
    b.textContent=d.status==='ready'?'● TODOS LOS SERVICIOS OPERATIVOS':'● DEGRADADO';
    document.getElementById('st-meta').textContent='Servicio: '+d.service+' · Public URL: '+(d.public_url||'local');
  }).catch(()=>{
    const b=document.getElementById('st-badge');
    b.className='badge warn';
    b.textContent='● SIN CONEXIÓN';
  });
  </script>
</main>"#,
    )
}
