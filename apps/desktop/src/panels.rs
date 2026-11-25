use crate::theme::{primary_button, styled_card};
use crate::{AgentDeskApp, NavTab, UiEvent};
use agentdesk_protocol::{
    AiChatRequest, AiChatResponse, AiExecuteActionRequest, AiSettingsDto, CalendarEventDto,
    CreateCalendarEventRequest, CreateUserRequest, MfaSetupResponse, MfaVerifyRequest, RmmCommandDto,
    RunCommandRequest, RunDiagnosticRequest, UpdateAiSettingsRequest, UserDto,
};
use chrono::Utc;
use eframe::egui::{self, RichText};
use std::process::Command;
use uuid::Uuid;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

pub fn egui_key_to_vk(key: egui::Key) -> Option<u16> {
    use egui::Key;
    Some(match key {
        Key::Enter => 0x0D,
        Key::Tab => 0x09,
        Key::Backspace => 0x08,
        Key::Escape => 0x1B,
        Key::Space => 0x20,
        Key::Delete => 0x2E,
        Key::Insert => 0x2D,
        Key::Home => 0x24,
        Key::End => 0x23,
        Key::PageUp => 0x21,
        Key::PageDown => 0x22,
        Key::ArrowLeft => 0x25,
        Key::ArrowUp => 0x26,
        Key::ArrowRight => 0x27,
        Key::ArrowDown => 0x28,
        Key::F1 => 0x70,
        Key::F2 => 0x71,
        Key::F3 => 0x72,
        Key::F4 => 0x73,
        Key::F5 => 0x74,
        Key::F6 => 0x75,
        Key::F7 => 0x76,
        Key::F8 => 0x77,
        Key::F9 => 0x78,
        Key::F10 => 0x79,
        Key::F11 => 0x7A,
        Key::F12 => 0x7B,
        Key::A => 0x41,
        Key::B => 0x42,
        Key::C => 0x43,
        Key::D => 0x44,
        Key::E => 0x45,
        Key::F => 0x46,
        Key::G => 0x47,
        Key::H => 0x48,
        Key::I => 0x49,
        Key::J => 0x4A,
        Key::K => 0x4B,
        Key::L => 0x4C,
        Key::M => 0x4D,
        Key::N => 0x4E,
        Key::O => 0x4F,
        Key::P => 0x50,
        Key::Q => 0x51,
        Key::R => 0x52,
        Key::S => 0x53,
        Key::T => 0x54,
        Key::U => 0x55,
        Key::V => 0x56,
        Key::W => 0x57,
        Key::X => 0x58,
        Key::Y => 0x59,
        Key::Z => 0x5A,
        Key::Num0 => 0x30,
        Key::Num1 => 0x31,
        Key::Num2 => 0x32,
        Key::Num3 => 0x33,
        Key::Num4 => 0x34,
        Key::Num5 => 0x35,
        Key::Num6 => 0x36,
        Key::Num7 => 0x37,
        Key::Num8 => 0x38,
        Key::Num9 => 0x39,
        _ => return None,
    })
}

fn open_url_in_browser(url: &str) {
    #[cfg(windows)]
    {
        let _ = Command::new("cmd")
            .args(["/C", "start", "", url])
            .creation_flags(0x08000000)
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("xdg-open").arg(url).spawn();
    }
}

impl AgentDeskApp {
    pub fn load_extra_tabs_data(&self) {
        let Some(auth) = &self.auth else { return };
        let http = self.http.clone();
        let tx = self.ui_tx.clone();
        let base = self.server_url.trim_end_matches('/').to_string();
        let token = auth.token.clone();

        self.rt.spawn(async move {
            if let Ok(r) = http.get(format!("{base}/api/v1/ai/settings")).bearer_auth(&token).send().await {
                if let Ok(s) = r.json::<AiSettingsDto>().await {
                    let _ = tx.send(UiEvent::AiSettingsLoaded(s));
                }
            }
            if let Ok(r) = http.get(format!("{base}/api/v1/calendar")).bearer_auth(&token).send().await {
                if let Ok(ev) = r.json::<Vec<CalendarEventDto>>().await {
                    let _ = tx.send(UiEvent::CalendarLoaded(ev));
                }
            }
            if let Ok(r) = http.get(format!("{base}/api/v1/users")).bearer_auth(&token).send().await {
                if let Ok(u) = r.json::<Vec<UserDto>>().await {
                    let _ = tx.send(UiEvent::UsersLoaded(u));
                }
            }
        });
    }

    pub fn ui_ai_copilot(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.auth.is_none() {
            self.ui_account_login_card(ui, "Copiloto IA (ChatGPT OAuth)");
            return;
        }
        let p = self.palette;
        ui.horizontal(|ui| {
            ui.heading("Copiloto de Soporte IA");
            ui.label(
                RichText::new("Inspecciona máquinas, analiza capturas y ejecuta acciones bajo aprobación.")
                    .small()
                    .color(p.text_muted),
            );
        });
        ui.separator();

        egui::CollapsingHeader::new("⚙ Configuración de Proveedor IA (ChatGPT OAuth PKCE / OpenAI / Ollama)")
            .default_open(self.ai_settings.is_none())
            .show(ui, |ui| {
                styled_card(&p).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Proveedor:");
                        for (val, label) in [
                            ("chat_gpt_o_auth", "ChatGPT OAuth"),
                            ("openai_api", "OpenAI API"),
                            ("ollama_local", "Ollama Local"),
                            ("disabled", "Deshabilitado"),
                        ] {
                            ui.selectable_value(&mut self.ai_provider, val.to_string(), label);
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Modelo:");
                        ui.add(egui::TextEdit::singleline(&mut self.ai_model).desired_width(170.0));
                        ui.label("Base URL:");
                        ui.add(egui::TextEdit::singleline(&mut self.ai_base_url).desired_width(260.0));
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label("API Key / Token manual:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.ai_api_key)
                                .password(true)
                                .hint_text("sk-... (opcional si usas OAuth o Ollama)")
                                .desired_width(300.0),
                        );
                        ui.checkbox(&mut self.ai_redact_pii, "Redactar secretos/PII antes de enviar");
                    });

                    ui.horizontal_wrapped(|ui| {
                        if ui.add(primary_button(&p, "Guardar configuración IA")).clicked() {
                            let auth = self.auth.as_ref().unwrap();
                            let http = self.http.clone();
                            let tx = self.ui_tx.clone();
                            let base = self.server_url.trim_end_matches('/').to_string();
                            let token = auth.token.clone();
                            let payload = UpdateAiSettingsRequest {
                                provider: self.ai_provider.clone(),
                                model: self.ai_model.clone(),
                                base_url: self.ai_base_url.clone(),
                                api_key: if self.ai_api_key.trim().is_empty() {
                                    None
                                } else {
                                    Some(self.ai_api_key.clone())
                                },
                                oauth_client_id: Some(self.ai_oauth_client_id.clone()),
                                oauth_auth_url: None,
                                oauth_token_url: None,
                                oauth_manual_token: if self.ai_oauth_manual_token.trim().is_empty() {
                                    None
                                } else {
                                    Some(self.ai_oauth_manual_token.clone())
                                },
                                autonomy_level: self.ai_autonomy.clone(),
                                redact_pii: self.ai_redact_pii,
                            };
                            self.rt.spawn(async move {
                                match http
                                    .put(format!("{base}/api/v1/ai/settings"))
                                    .bearer_auth(&token)
                                    .json(&payload)
                                    .send()
                                    .await
                                {
                                    Ok(r) if r.status().is_success() => {
                                        if let Ok(s) = r.json::<AiSettingsDto>().await {
                                            let _ = tx.send(UiEvent::AiSettingsLoaded(s));
                                            let _ = tx.send(UiEvent::Toast("Configuración IA guardada".into()));
                                        }
                                    }
                                    Ok(r) => {
                                        let _ = tx.send(UiEvent::Toast(format!("Error IA settings: {}", r.status())));
                                    }
                                    Err(e) => {
                                        let _ = tx.send(UiEvent::Toast(format!("Error red: {e}")));
                                    }
                                }
                            });
                        }

                        if ui.button("🔐 Conectar con ChatGPT (OAuth PKCE)").clicked() {
                            let auth = self.auth.as_ref().unwrap();
                            let http = self.http.clone();
                            let tx = self.ui_tx.clone();
                            let base = self.server_url.trim_end_matches('/').to_string();
                            let token = auth.token.clone();
                            self.rt.spawn(async move {
                                if let Ok(r) = http
                                    .get(format!("{base}/api/v1/ai/oauth/start"))
                                    .bearer_auth(&token)
                                    .send()
                                    .await
                                {
                                    if let Ok(v) = r.json::<serde_json::Value>().await {
                                        if let Some(u) = v.get("authorize_url").and_then(|s| s.as_str()) {
                                            open_url_in_browser(u);
                                            let _ = tx.send(UiEvent::AiOAuthUrl(u.to_string()));
                                        }
                                    }
                                }
                            });
                        }

                        if let Some(s) = &self.ai_settings {
                            let badge = if s.oauth_connected || s.has_api_key || s.provider == "ollama_local" {
                                ("● Listo", p.ok)
                            } else {
                                ("○ Falta credencial OAuth/API", p.warn)
                            };
                            ui.colored_label(badge.1, badge.0);
                        }
                    });

                    if !self.ai_oauth_url.is_empty() {
                        ui.horizontal(|ui| {
                            ui.label("URL OAuth generada:");
                            ui.add(egui::TextEdit::singleline(&mut self.ai_oauth_url).desired_width(420.0));
                            if ui.button("Copiar").clicked() {
                                ctx.copy_text(self.ai_oauth_url.clone());
                            }
                        });
                    }
                });
            });

        ui.add_space(6.0);
        styled_card(&p).show(ui, |ui| {
            let mut dev_labels: Vec<String> = vec!["(Sin máquina específica)".into()];
            for d in &self.devices {
                dev_labels.push(format!("{} ({}) [{}]", d.hostname, d.public_code, d.status));
            }
            if self.ai_device_idx >= dev_labels.len() {
                self.ai_device_idx = 0;
            }
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_label("Máquina a inspeccionar")
                    .selected_text(dev_labels[self.ai_device_idx].clone())
                    .show_ui(ui, |ui| {
                        for (i, lbl) in dev_labels.iter().enumerate() {
                            ui.selectable_value(&mut self.ai_device_idx, i, lbl);
                        }
                    });
                ui.checkbox(&mut self.ai_include_diag, "📊 Adjuntar diagnóstico en vivo");
                ui.checkbox(&mut self.ai_include_shot, "📸 Adjuntar captura de pantalla");
            });

            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for (who, text) in &self.ai_messages {
                        ui.label(RichText::new(who).strong().color(if who == "IA" { p.accent } else { p.ok }));
                        ui.label(text);
                        ui.add_space(4.0);
                    }
                });

            if !self.ai_proposed_actions.is_empty() {
                ui.separator();
                ui.strong("Acciones propuestas por la IA (requieren tu aprobación):");
                let selected_dev_id = if self.ai_device_idx > 0 {
                    self.devices.get(self.ai_device_idx - 1).map(|d| d.id)
                } else {
                    self.devices.first().map(|d| d.id)
                };
                for act in self.ai_proposed_actions.clone() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("[{}] {} — {}", act.risk_level, act.title, act.command));
                        if let Some(did) = selected_dev_id {
                            if ui.add(primary_button(&p, "✅ Aprobar y ejecutar")).clicked() {
                                let auth = self.auth.as_ref().unwrap();
                                let http = self.http.clone();
                                let tx = self.ui_tx.clone();
                                let base = self.server_url.trim_end_matches('/').to_string();
                                let token = auth.token.clone();
                                let payload = AiExecuteActionRequest {
                                    device_id: did,
                                    shell: act.shell.clone(),
                                    command: act.command.clone(),
                                    rationale: act.rationale.clone(),
                                };
                                self.rt.spawn(async move {
                                    match http
                                        .post(format!("{base}/api/v1/ai/execute-action"))
                                        .bearer_auth(&token)
                                        .json(&payload)
                                        .send()
                                        .await
                                    {
                                        Ok(r) if r.status().is_success() => {
                                            if let Ok(res) = r.json::<RmmCommandDto>().await {
                                                let _ = tx.send(UiEvent::AiActionExecuted(res));
                                            }
                                        }
                                        Ok(r) => {
                                            let _ = tx.send(UiEvent::Toast(format!("Error ejecutando acción: {}", r.status())));
                                        }
                                        Err(e) => {
                                            let _ = tx.send(UiEvent::Toast(format!("Error red: {e}")));
                                        }
                                    }
                                });
                            }
                        }
                    });
                }
            }

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.ai_prompt)
                        .hint_text("Ej: Revisa el uso de CPU/RAM, servicios caídos y dime qué está fallando...")
                        .desired_width(540.0),
                );
                let send_btn = ui.add_enabled(
                    !self.ai_busy && !self.ai_prompt.trim().is_empty(),
                    primary_button(&p, if self.ai_busy { "Consultando..." } else { "Consultar IA" }),
                );
                if send_btn.clicked() {
                    let did = if self.ai_device_idx > 0 {
                        self.devices.get(self.ai_device_idx - 1).map(|d| d.id)
                    } else {
                        None
                    };
                    let prompt = self.ai_prompt.clone();
                    self.ai_messages.push(("Técnico".into(), prompt.clone()));
                    self.ai_prompt.clear();
                    self.ai_busy = true;

                    let auth = self.auth.as_ref().unwrap();
                    let http = self.http.clone();
                    let tx = self.ui_tx.clone();
                    let base = self.server_url.trim_end_matches('/').to_string();
                    let token = auth.token.clone();
                    let inc_diag = self.ai_include_diag;
                    let inc_shot = self.ai_include_shot;
                    self.rt.spawn(async move {
                        let resp = http
                            .post(format!("{base}/api/v1/ai/chat"))
                            .bearer_auth(&token)
                            .json(&AiChatRequest {
                                device_id: did,
                                ticket_id: None,
                                prompt,
                                include_diagnostics: inc_diag,
                                include_screenshot: inc_shot,
                            })
                            .send()
                            .await;
                        match resp {
                            Ok(r) if r.status().is_success() => {
                                if let Ok(chat_res) = r.json::<AiChatResponse>().await {
                                    let _ = tx.send(UiEvent::AiChatResult(chat_res));
                                }
                            }
                            Ok(r) => {
                                let txt = r.text().await.unwrap_or_default();
                                let _ = tx.send(UiEvent::AiChatResult(AiChatResponse {
                                    reply: format!("Error del copiloto: {txt}"),
                                    proposed_actions: vec![],
                                    provider_used: "error".into(),
                                    model_used: String::new(),
                                    redacted_context_applied: true,
                                    diagnostics_snapshot: None,
                                }));
                            }
                            Err(e) => {
                                let _ = tx.send(UiEvent::AiChatResult(AiChatResponse {
                                    reply: format!("Error de red: {e}"),
                                    proposed_actions: vec![],
                                    provider_used: "error".into(),
                                    model_used: String::new(),
                                    redacted_context_applied: true,
                                    diagnostics_snapshot: None,
                                }));
                            }
                        }
                    });
                }
            });
        });
    }

    pub fn ui_rmm_terminal(&mut self, ui: &mut egui::Ui) {
        if self.auth.is_none() {
            self.ui_account_login_card(ui, "RMM y Terminal Remota");
            return;
        }
        let p = self.palette;
        ui.heading("RMM — Diagnóstico en Vivo y Terminal Remota");
        ui.separator();

        if self.devices.is_empty() {
            ui.label("No hay dispositivos registrados aún. Abre agentdesk-host o actualiza la lista.");
            return;
        }
        if self.rmm_device_idx >= self.devices.len() {
            self.rmm_device_idx = 0;
        }

        styled_card(&p).show(ui, |ui| {
            let selected = self.devices[self.rmm_device_idx].clone();
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_label("Equipo")
                    .selected_text(format!("{} ({}) [{}]", selected.hostname, selected.public_code, selected.status))
                    .show_ui(ui, |ui| {
                        for (i, d) in self.devices.iter().enumerate() {
                            ui.selectable_value(
                                &mut self.rmm_device_idx,
                                i,
                                format!("{} ({}) [{}]", d.hostname, d.public_code, d.status),
                            );
                        }
                    });

                for (kind, label) in [
                    ("metrics", "📊 Métricas"),
                    ("inventory", "🖥 Inventario"),
                    ("services", "⚙ Servicios"),
                    ("events", "📋 Eventos Windows"),
                ] {
                    if ui.button(label).clicked() {
                        let auth = self.auth.as_ref().unwrap();
                        let http = self.http.clone();
                        let tx = self.ui_tx.clone();
                        let base = self.server_url.trim_end_matches('/').to_string();
                        let token = auth.token.clone();
                        let did = selected.id;
                        let k = kind.to_string();
                        self.rmm_diag_output = format!("Consultando {kind} en {}...", selected.hostname);
                        self.rt.spawn(async move {
                            match http
                                .post(format!("{base}/api/v1/devices/{did}/diagnostics"))
                                .bearer_auth(&token)
                                .json(&RunDiagnosticRequest { kind: k })
                                .send()
                                .await
                            {
                                Ok(r) if r.status().is_success() => {
                                    if let Ok(v) = r.json::<serde_json::Value>().await {
                                        let _ = tx.send(UiEvent::RmmDiagnosticResult(v));
                                    }
                                }
                                Ok(r) => {
                                    let txt = r.text().await.unwrap_or_default();
                                    let _ = tx.send(UiEvent::RmmDiagnosticResult(serde_json::json!({ "error": txt })));
                                }
                                Err(e) => {
                                    let _ = tx.send(UiEvent::RmmDiagnosticResult(serde_json::json!({ "error": e.to_string() })));
                                }
                            }
                        });
                    }
                }
            });

            if !self.rmm_diag_output.is_empty() {
                ui.add_space(6.0);
                egui::ScrollArea::vertical().max_height(190.0).show(ui, |ui| {
                    ui.monospace(&self.rmm_diag_output);
                });
            }
        });

        ui.add_space(10.0);
        styled_card(&p).show(ui, |ui| {
            let selected = self.devices[self.rmm_device_idx].clone();
            ui.strong(format!("Terminal Remota Auditable — {}", selected.hostname));
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.rmm_shell, "powershell".into(), "PowerShell");
                ui.selectable_value(&mut self.rmm_shell, "cmd".into(), "CMD");
                ui.add(
                    egui::TextEdit::singleline(&mut self.rmm_command)
                        .hint_text("Ej: Get-Service AgentDesk* | Format-Table -AutoSize")
                        .desired_width(480.0),
                );
                if ui.add(primary_button(&p, "Ejecutar")).clicked() && !self.rmm_command.trim().is_empty() {
                    let auth = self.auth.as_ref().unwrap();
                    let http = self.http.clone();
                    let tx = self.ui_tx.clone();
                    let base = self.server_url.trim_end_matches('/').to_string();
                    let token = auth.token.clone();
                    let did = selected.id;
                    let shell = self.rmm_shell.clone();
                    let cmd_str = self.rmm_command.clone();
                    self.rmm_cmd_output = format!("Ejecutando en {}...", selected.hostname);
                    self.rt.spawn(async move {
                        match http
                            .post(format!("{base}/api/v1/devices/{did}/command"))
                            .bearer_auth(&token)
                            .json(&RunCommandRequest {
                                shell,
                                command: cmd_str,
                                timeout_secs: 20,
                            })
                            .send()
                            .await
                        {
                            Ok(r) if r.status().is_success() => {
                                if let Ok(res) = r.json::<RmmCommandDto>().await {
                                    let _ = tx.send(UiEvent::RmmCommandResult(res));
                                }
                            }
                            Ok(r) => {
                                let txt = r.text().await.unwrap_or_default();
                                let _ = tx.send(UiEvent::Toast(format!("Error terminal: {txt}")));
                            }
                            Err(e) => {
                                let _ = tx.send(UiEvent::Toast(format!("Error red: {e}")));
                            }
                        }
                    });
                }
            });

            if !self.rmm_cmd_output.is_empty() {
                ui.add_space(6.0);
                egui::ScrollArea::vertical().max_height(210.0).show(ui, |ui| {
                    ui.monospace(&self.rmm_cmd_output);
                });
            }
        });
    }

    pub fn ui_calendar_reports(&mut self, ui: &mut egui::Ui) {
        if self.auth.is_none() {
            self.ui_account_login_card(ui, "Calendario y Reportes");
            return;
        }
        let p = self.palette;
        ui.horizontal(|ui| {
            ui.heading("Calendario Técnico y Reportes CSV");
            if ui.add(primary_button(&p, "⬇ Exportar Sesiones CSV")).clicked() {
                let auth = self.auth.as_ref().unwrap();
                let http = self.http.clone();
                let tx = self.ui_tx.clone();
                let base = self.server_url.trim_end_matches('/').to_string();
                let token = auth.token.clone();
                self.rt.spawn(async move {
                    if let Ok(r) = http
                        .get(format!("{base}/api/v1/reports/sessions.csv"))
                        .bearer_auth(&token)
                        .send()
                        .await
                    {
                        if let Ok(csv) = r.text().await {
                            let dl_base = std::env::var("USERPROFILE")
                                .map(std::path::PathBuf::from)
                                .unwrap_or_else(|_| std::env::temp_dir())
                                .join("Downloads")
                                .join("AgentDesk");
                            let _ = std::fs::create_dir_all(&dl_base);
                            let dest = dl_base.join("agentdesk-sessions.csv");
                            let _ = std::fs::write(&dest, csv);
                            let _ = tx.send(UiEvent::Toast(format!("CSV guardado en {}", dest.display())));
                        }
                    }
                });
            }
            if ui.button("↻ Actualizar").clicked() {
                self.load_extra_tabs_data();
            }
        });
        ui.separator();

        styled_card(&p).show(ui, |ui| {
            ui.strong("Agendar nueva cita técnica (con detección automática de conflictos)");
            ui.horizontal_wrapped(|ui| {
                ui.label("Título:");
                ui.add(egui::TextEdit::singleline(&mut self.cal_title).desired_width(180.0));
                ui.label("Cliente:");
                ui.add(egui::TextEdit::singleline(&mut self.cal_client).desired_width(150.0));
                ui.label("En horas:");
                ui.add(egui::TextEdit::singleline(&mut self.cal_hours_from_now).desired_width(50.0));
                ui.label("Duración (min):");
                ui.add(egui::TextEdit::singleline(&mut self.cal_duration_mins).desired_width(55.0));
            });
            ui.horizontal(|ui| {
                ui.label("Notas:");
                ui.add(egui::TextEdit::singleline(&mut self.cal_notes).desired_width(360.0));
                if ui.add(primary_button(&p, "+ Agendar Cita")).clicked() && !self.cal_title.trim().is_empty() {
                    let h: i64 = self.cal_hours_from_now.trim().parse().unwrap_or(2);
                    let mins: i64 = self.cal_duration_mins.trim().parse().unwrap_or(60);
                    let starts = Utc::now() + chrono::Duration::hours(h.max(0));
                    let ends = starts + chrono::Duration::minutes(mins.max(15));
                    let auth = self.auth.as_ref().unwrap();
                    let http = self.http.clone();
                    let tx = self.ui_tx.clone();
                    let base = self.server_url.trim_end_matches('/').to_string();
                    let token = auth.token.clone();
                    let payload = CreateCalendarEventRequest {
                        device_id: self.devices.first().map(|d| d.id),
                        title: self.cal_title.clone(),
                        client_name: self.cal_client.clone(),
                        starts_at: starts,
                        ends_at: ends,
                        notes: self.cal_notes.clone(),
                    };
                    self.cal_title.clear();
                    self.cal_notes.clear();
                    self.rt.spawn(async move {
                        match http
                            .post(format!("{base}/api/v1/calendar"))
                            .bearer_auth(&token)
                            .json(&payload)
                            .send()
                            .await
                        {
                            Ok(r) if r.status().is_success() => {
                                if let Ok(ev) = http.get(format!("{base}/api/v1/calendar")).bearer_auth(&token).send().await {
                                    if let Ok(list) = ev.json::<Vec<CalendarEventDto>>().await {
                                        let _ = tx.send(UiEvent::CalendarLoaded(list));
                                    }
                                }
                                let _ = tx.send(UiEvent::Toast("Cita agendada".into()));
                            }
                            Ok(r) => {
                                let txt = r.text().await.unwrap_or_default();
                                let _ = tx.send(UiEvent::Toast(format!("Error calendario: {txt}")));
                            }
                            Err(e) => {
                                let _ = tx.send(UiEvent::Toast(format!("Error red: {e}")));
                            }
                        }
                    });
                }
            });
        });

        ui.add_space(8.0);
        let events = self.calendar_events.clone();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for ev in events {
                styled_card(&p).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.strong(&ev.title);
                        ui.label(format!("· {} · Técnico: {}", ev.client_name, ev.technician_name));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Eliminar").clicked() {
                                let auth = self.auth.as_ref().unwrap();
                                let http = self.http.clone();
                                let tx = self.ui_tx.clone();
                                let base = self.server_url.trim_end_matches('/').to_string();
                                let token = auth.token.clone();
                                let eid: Uuid = ev.id;
                                self.rt.spawn(async move {
                                    let _ = http.delete(format!("{base}/api/v1/calendar/{eid}")).bearer_auth(&token).send().await;
                                    if let Ok(r) = http.get(format!("{base}/api/v1/calendar")).bearer_auth(&token).send().await {
                                        if let Ok(list) = r.json::<Vec<CalendarEventDto>>().await {
                                            let _ = tx.send(UiEvent::CalendarLoaded(list));
                                        }
                                    }
                                });
                            }
                            ui.monospace(format!(
                                "{} → {}",
                                ev.starts_at.format("%d/%m %H:%M"),
                                ev.ends_at.format("%H:%M UTC")
                            ));
                        });
                    });
                    if !ev.notes.is_empty() {
                        ui.label(RichText::new(&ev.notes).small().color(p.text_muted));
                    }
                });
                ui.add_space(6.0);
            }
        });
    }

    pub fn ui_users_mfa(&mut self, ui: &mut egui::Ui) {
        if self.auth.is_none() {
            self.ui_account_login_card(ui, "Usuarios RBAC y MFA TOTP");
            return;
        }
        let p = self.palette;
        ui.heading("Usuarios (7 Roles RBAC) y MFA TOTP");
        ui.separator();

        styled_card(&p).show(ui, |ui| {
            ui.strong("Seguridad de mi cuenta — MFA TOTP (Google Authenticator / Authy)");
            ui.horizontal(|ui| {
                if ui.add(primary_button(&p, "Generar secreto MFA TOTP")).clicked() {
                    let auth = self.auth.as_ref().unwrap();
                    let http = self.http.clone();
                    let tx = self.ui_tx.clone();
                    let base = self.server_url.trim_end_matches('/').to_string();
                    let token = auth.token.clone();
                    self.rt.spawn(async move {
                        if let Ok(r) = http.post(format!("{base}/api/v1/auth/mfa/setup")).bearer_auth(&token).send().await {
                            if let Ok(ms) = r.json::<MfaSetupResponse>().await {
                                let _ = tx.send(UiEvent::MfaSetupResult(ms));
                            }
                        }
                    });
                }
            });
            if let Some(mfa) = &self.mfa_setup {
                ui.monospace(format!("Secreto Base32: {}", mfa.secret_base32));
                ui.small(&mfa.otpauth_uri);
                ui.horizontal(|ui| {
                    ui.label("Código 6 dígitos:");
                    ui.add(egui::TextEdit::singleline(&mut self.mfa_code_input).desired_width(110.0));
                    if ui.button("Verificar y activar MFA").clicked() {
                        let auth = self.auth.as_ref().unwrap();
                        let http = self.http.clone();
                        let tx = self.ui_tx.clone();
                        let base = self.server_url.trim_end_matches('/').to_string();
                        let token = auth.token.clone();
                        let code = self.mfa_code_input.clone();
                        self.rt.spawn(async move {
                            match http
                                .post(format!("{base}/api/v1/auth/mfa/verify"))
                                .bearer_auth(&token)
                                .json(&MfaVerifyRequest { code })
                                .send()
                                .await
                            {
                                Ok(r) if r.status().is_success() => {
                                    let _ = tx.send(UiEvent::Toast("MFA TOTP activado correctamente".into()));
                                }
                                _ => {
                                    let _ = tx.send(UiEvent::Toast("Código TOTP inválido".into()));
                                }
                            }
                        });
                    }
                });
            }
        });

        ui.add_space(10.0);
        styled_card(&p).show(ui, |ui| {
            ui.strong("Alta rápida de usuario en la organización");
            ui.horizontal_wrapped(|ui| {
                ui.label("Email:");
                ui.add(egui::TextEdit::singleline(&mut self.new_user_email).desired_width(170.0));
                ui.label("Nombre:");
                ui.add(egui::TextEdit::singleline(&mut self.new_user_name).desired_width(140.0));
                ui.label("Pass:");
                ui.add(egui::TextEdit::singleline(&mut self.new_user_pw).password(true).desired_width(110.0));
                egui::ComboBox::from_label("Rol")
                    .selected_text(self.new_user_role.clone())
                    .show_ui(ui, |ui| {
                        for r in [
                            "instance_admin",
                            "org_admin",
                            "supervisor",
                            "technician",
                            "limited_technician",
                            "auditor",
                            "client",
                        ] {
                            ui.selectable_value(&mut self.new_user_role, r.to_string(), r);
                        }
                    });
                if ui.add(primary_button(&p, "Crear usuario")).clicked() {
                    let auth = self.auth.as_ref().unwrap();
                    let http = self.http.clone();
                    let tx = self.ui_tx.clone();
                    let base = self.server_url.trim_end_matches('/').to_string();
                    let token = auth.token.clone();
                    let payload = CreateUserRequest {
                        email: self.new_user_email.clone(),
                        display_name: self.new_user_name.clone(),
                        password: self.new_user_pw.clone(),
                        role: self.new_user_role.clone(),
                    };
                    self.rt.spawn(async move {
                        match http
                            .post(format!("{base}/api/v1/users"))
                            .bearer_auth(&token)
                            .json(&payload)
                            .send()
                            .await
                        {
                            Ok(r) if r.status().is_success() => {
                                if let Ok(ur) = http.get(format!("{base}/api/v1/users")).bearer_auth(&token).send().await {
                                    if let Ok(list) = ur.json::<Vec<UserDto>>().await {
                                        let _ = tx.send(UiEvent::UsersLoaded(list));
                                    }
                                }
                                let _ = tx.send(UiEvent::Toast("Usuario creado".into()));
                            }
                            Ok(r) => {
                                let txt = r.text().await.unwrap_or_default();
                                let _ = tx.send(UiEvent::Toast(format!("Error usuario: {txt}")));
                            }
                            Err(e) => {
                                let _ = tx.send(UiEvent::Toast(format!("Error red: {e}")));
                            }
                        }
                    });
                }
            });
        });

        ui.add_space(8.0);
        for u in &self.users {
            styled_card(&p).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(&u.display_name);
                    ui.monospace(&u.email);
                    ui.label(format!("Rol: {}", u.role));
                    ui.colored_label(
                        if u.mfa_enabled { p.ok } else { p.text_muted },
                        if u.mfa_enabled { "● MFA activo" } else { "○ Sin MFA" },
                    );
                });
            });
            ui.add_space(4.0);
        }
    }
}
