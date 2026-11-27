use crate::theme::{centered_text_edit, placeholder_text, primary_button, styled_card, Palette};
use crate::{AgentDeskApp, NavTab, UiEvent};
use agentdesk_protocol::{
    AiChatRequest, AiChatResponse, AiExecuteActionRequest, AiSettingsDto, CalendarEventDto,
    CreateCalendarEventRequest, CreateUserRequest, DeviceGroupDto, MfaSetupResponse,
    MfaVerifyRequest, RmmCommandDto, RunCommandRequest, RunDiagnosticRequest,
    UpdateAiSettingsRequest, UserDto,
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

pub fn render_percent_trend(
    ui: &mut egui::Ui,
    p: &Palette,
    label: &str,
    current: f64,
    samples: &[f64],
) {
    styled_card(p).show(ui, |ui| {
        ui.set_min_width(132.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).small().color(p.text_muted));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.strong(format!("{:.0}%", current.clamp(0.0, 100.0)));
            });
        });

        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width().max(96.0), 38.0),
            egui::Sense::hover(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4, p.input_bg);
        let plot = rect.shrink(4.0);
        for level in [0.25_f32, 0.5, 0.75] {
            let y = plot.bottom() - plot.height() * level;
            painter.line_segment(
                [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
                egui::Stroke::new(0.5_f32, p.card_border),
            );
        }

        if samples.is_empty() {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Sin muestras previas",
                egui::FontId::proportional(10.0),
                p.text_muted,
            );
        } else {
            let denominator = samples.len().saturating_sub(1).max(1) as f32;
            let points: Vec<_> = samples
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    let x = plot.left() + plot.width() * index as f32 / denominator;
                    let y = plot.bottom() - plot.height() * value.clamp(0.0, 100.0) as f32 / 100.0;
                    egui::pos2(x, y)
                })
                .collect();
            if points.len() > 1 {
                painter.add(egui::Shape::line(
                    points.clone(),
                    egui::Stroke::new(1.8_f32, p.accent),
                ));
            }
            if let Some(last) = points.last() {
                painter.circle_filled(*last, 2.5, p.accent);
            }
        }
        ui.label(
            RichText::new(format!("{} muestras guardadas", samples.len()))
                .small()
                .color(p.text_muted),
        );
    });
}

impl AgentDeskApp {
    pub fn load_extra_tabs_data(&self) {
        let Some(auth) = &self.auth else { return };
        let http = self.http.clone();
        let tx = self.ui_tx.clone();
        let base = self.server_url.trim_end_matches('/').to_string();
        let token = auth.token.clone();

        self.rt.spawn(async move {
            if let Ok(r) = http
                .get(format!("{base}/api/v1/ai/settings"))
                .bearer_auth(&token)
                .send()
                .await
            {
                if let Ok(s) = r.json::<AiSettingsDto>().await {
                    let _ = tx.send(UiEvent::AiSettingsLoaded(s));
                }
            }
            if let Ok(r) = http
                .get(format!("{base}/api/v1/calendar"))
                .bearer_auth(&token)
                .send()
                .await
            {
                if let Ok(ev) = r.json::<Vec<CalendarEventDto>>().await {
                    let _ = tx.send(UiEvent::CalendarLoaded(ev));
                }
            }
            if let Ok(r) = http
                .get(format!("{base}/api/v1/users"))
                .bearer_auth(&token)
                .send()
                .await
            {
                if let Ok(u) = r.json::<Vec<UserDto>>().await {
                    let _ = tx.send(UiEvent::UsersLoaded(u));
                }
            }
            if let Ok(r) = http
                .get(format!("{base}/api/v1/groups"))
                .bearer_auth(&token)
                .send()
                .await
            {
                if let Ok(g) = r.json::<Vec<DeviceGroupDto>>().await {
                    let _ = tx.send(UiEvent::GroupsLoaded(g));
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
                RichText::new(
                    "Inspecciona máquinas, analiza capturas y ejecuta acciones bajo aprobación.",
                )
                .small()
                .color(p.text_muted),
            );
        });
        ui.separator();

        if let Some(ticket_id) = self.ai_ticket_context {
            let ticket = self
                .tickets
                .iter()
                .find(|ticket| ticket.id == ticket_id)
                .cloned();
            styled_card(&p).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.strong("Contexto de solicitud");
                    if let Some(ticket) = &ticket {
                        ui.label(RichText::new(&ticket.public_code).monospace().color(p.accent));
                        if !ticket.company_name.is_empty() {
                            ui.label(&ticket.company_name);
                        }
                        if !ticket.category.is_empty() {
                            ui.label(RichText::new(&ticket.category).small().color(p.text_muted));
                        }
                    } else {
                        ui.colored_label(p.warn, "El ticket ya no está en la lista cargada.");
                    }
                    if ui.small_button("Quitar contexto").clicked() {
                        self.ai_ticket_context = None;
                        self.ai_resolution_draft = None;
                    }
                    if ticket.is_some() && ui.button("Preparar borrador de resolución").clicked() {
                        self.ai_prompt = "Redacta un borrador breve de resolución para este ticket. Separa diagnóstico, acciones realizadas y pendientes. No inventes datos ni afirmes acciones que no figuren en el contexto; indica lo que no se pudo verificar.".into();
                    }
                });
                if let Some(ticket) = ticket {
                    ui.label(RichText::new(ticket.description).color(p.text_primary));
                    if let Some(device_id) = ticket.device_id {
                        if let Some(device) = self.devices.iter().find(|device| device.id == device_id) {
                            ui.label(RichText::new(format!("Equipo asociado: {} ({})", device.hostname, device.public_code)).small().color(p.text_muted));
                        } else {
                            ui.colored_label(p.warn, "El ticket tiene un equipo asociado que no está disponible para esta cuenta.");
                        }
                    }
                }
            });
        }

        egui::CollapsingHeader::new(
            "⚙ Configuración de Proveedor IA (ChatGPT OAuth PKCE / OpenAI / Ollama)",
        )
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
                    ui.add(centered_text_edit(&mut self.ai_model).desired_width(170.0));
                    ui.label("Base URL:");
                    ui.add(centered_text_edit(&mut self.ai_base_url).desired_width(260.0));
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("API Key / Token manual:");
                    ui.add(
                        centered_text_edit(&mut self.ai_api_key)
                            .password(true)
                            .hint_text(placeholder_text(
                                &p,
                                "sk-... (opcional si usas OAuth o Ollama)",
                            ))
                            .desired_width(300.0),
                    );
                    ui.checkbox(
                        &mut self.ai_redact_pii,
                        "Redactar secretos/PII antes de enviar",
                    );
                });

                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add(primary_button(&p, "Guardar configuración IA"))
                        .clicked()
                    {
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
                            reasoning_effort: "high".into(),
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
                                        let _ = tx.send(UiEvent::Toast(
                                            "Configuración IA guardada".into(),
                                        ));
                                    }
                                }
                                Ok(r) => {
                                    let _ = tx.send(UiEvent::Toast(format!(
                                        "Error IA settings: {}",
                                        r.status()
                                    )));
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
                                    if let Some(u) = v.get("authorize_url").and_then(|s| s.as_str())
                                    {
                                        open_url_in_browser(u);
                                        let _ = tx.send(UiEvent::AiOAuthUrl(u.to_string()));
                                    }
                                }
                            }
                        });
                    }

                    if let Some(s) = &self.ai_settings {
                        let badge =
                            if s.oauth_connected || s.has_api_key || s.provider == "ollama_local" {
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
                        ui.add(centered_text_edit(&mut self.ai_oauth_url).desired_width(420.0));
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
                let ticket_device = self.ai_ticket_context
                    .and_then(|ticket_id| self.tickets.iter().find(|ticket| ticket.id == ticket_id))
                    .and_then(|ticket| ticket.device_id);
                if ticket_device.is_some() {
                    ui.label(format!("Equipo del ticket: {}", dev_labels[self.ai_device_idx]));
                } else {
                    egui::ComboBox::from_label("Máquina a inspeccionar")
                        .selected_text(dev_labels[self.ai_device_idx].clone())
                        .show_ui(ui, |ui| {
                            for (i, lbl) in dev_labels.iter().enumerate() {
                                ui.selectable_value(&mut self.ai_device_idx, i, lbl);
                            }
                        });
                }
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

            let mut apply_resolution_draft: Option<(Uuid, String)> = None;
            if let Some((ticket_id, draft)) = self.ai_resolution_draft.as_mut() {
                if self.ai_ticket_context == Some(*ticket_id) {
                    ui.separator();
                    styled_card(&p).show(ui, |ui| {
                        ui.strong("Borrador para revisar");
                        ui.label(RichText::new("La IA solo propone el texto. Editalo y confirmá el resultado antes de cerrar el ticket.").small().color(p.text_muted));
                        ui.add(egui::TextEdit::multiline(draft).desired_rows(4).desired_width(f32::INFINITY));
                        if ui.button("Llevar al formulario de resolución").clicked() {
                            apply_resolution_draft = Some((*ticket_id, draft.clone()));
                        }
                    });
                }
            }
            if let Some((ticket_id, draft)) = apply_resolution_draft {
                if let Some(ticket) = self.tickets.iter().find(|ticket| ticket.id == ticket_id) {
                    self.ticket_resolution_summary = draft;
                    self.ticket_resolution_outcome = "resolved".into();
                    self.ticket_resolution_error.clear();
                    self.ticket_close_confirm = Some((ticket_id, ticket.public_code.clone()));
                    self.tab = NavTab::Tickets;
                }
            }

            if !self.ai_proposed_actions.is_empty() {
                ui.separator();
                ui.strong("Acciones propuestas por la IA (requieren tu aprobación):");
                let selected_dev_id = if self.ai_device_idx > 0 {
                    self.devices.get(self.ai_device_idx - 1).map(|d| d.id)
                } else {
                    None
                };
                for act in self.ai_proposed_actions.clone() {
                    styled_card(&p).show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            let risk_color = match act.risk_level.to_lowercase().as_str() {
                                "critical" | "high" => p.danger,
                                "medium" => p.warn,
                                _ => p.ok,
                            };
                            ui.colored_label(risk_color, act.risk_level.to_uppercase());
                            ui.strong(&act.title);
                        });
                        ui.label(
                            RichText::new(&act.rationale)
                                .small()
                                .color(p.text_muted),
                        );
                        let mut command = act.command.clone();
                        ui.add(
                            egui::TextEdit::multiline(&mut command)
                                .font(egui::FontId::monospace(13.0))
                                .desired_rows(2)
                                .interactive(false),
                        );
                        if let Some(did) = selected_dev_id {
                            if ui
                                .add(primary_button(&p, "Aprobar y ejecutar en el equipo elegido"))
                                .clicked()
                            {
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
                        } else {
                            ui.colored_label(
                                p.warn,
                                "Elegí explícitamente un equipo antes de ejecutar esta acción.",
                            );
                        }
                    });
                }
            }

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add(
                    centered_text_edit(&mut self.ai_prompt)
                        .hint_text(placeholder_text(&p, "Ej: Revisa el uso de CPU/RAM, servicios caídos y dime qué está fallando..."))
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
                    let ticket_id = self.ai_ticket_context;
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
                                ticket_id,
                                prompt,
                                include_diagnostics: inc_diag,
                                include_screenshot: inc_shot,
                                chat_session_id: None,
                                model: None,
                                reasoning_effort: None,
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
                                    chat_session_id: None,
                                    reasoning_effort: "high".into(),
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
                                    chat_session_id: None,
                                    reasoning_effort: "high".into(),
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
            ui.label(
                "No hay dispositivos registrados aún. Abre agentdesk-host o actualiza la lista.",
            );
            return;
        }
        if self.rmm_device_idx >= self.devices.len() {
            self.rmm_device_idx = 0;
        }

        styled_card(&p).show(ui, |ui| {
            let selected = self.devices[self.rmm_device_idx].clone();
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_label("Equipo")
                    .selected_text(format!(
                        "{} ({}) [{}]",
                        selected.hostname, selected.public_code, selected.status
                    ))
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
                    ("resources", "⚙ Recursos HW (CPU/RAM/Disco/GPU)"),
                ] {
                    if ui.button(label).clicked() {
                        let auth = self.auth.as_ref().unwrap();
                        let http = self.http.clone();
                        let tx = self.ui_tx.clone();
                        let base = self.server_url.trim_end_matches('/').to_string();
                        let token = auth.token.clone();
                        let did = selected.id;
                        let k = kind.to_string();
                        self.rmm_diag_output =
                            format!("Consultando {kind} en {}...", selected.hostname);
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
                                    let _ = tx.send(UiEvent::RmmDiagnosticResult(
                                        serde_json::json!({ "error": txt }),
                                    ));
                                }
                                Err(e) => {
                                    let _ = tx.send(UiEvent::RmmDiagnosticResult(
                                        serde_json::json!({ "error": e.to_string() }),
                                    ));
                                }
                            }
                        });
                    }
                }
            });

            if let Some(hw) = self.rmm_hw_snapshot.clone() {
                ui.add_space(6.0);
                render_hardware_resources_ui(ui, &p, &hw);
            }
            if !self.rmm_diag_output.is_empty() {
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .max_height(190.0)
                    .show(ui, |ui| {
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
                    centered_text_edit(&mut self.rmm_command)
                        .hint_text(placeholder_text(
                            &p,
                            "Ej: Get-Service AgentDesk* | Format-Table -AutoSize",
                        ))
                        .desired_width(480.0),
                );
                if ui.add(primary_button(&p, "Ejecutar")).clicked()
                    && !self.rmm_command.trim().is_empty()
                {
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
                egui::ScrollArea::vertical()
                    .max_height(210.0)
                    .show(ui, |ui| {
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
            if ui
                .add(primary_button(&p, "⬇ Exportar Sesiones CSV"))
                .clicked()
            {
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
                            let _ = tx.send(UiEvent::Toast(format!(
                                "CSV guardado en {}",
                                dest.display()
                            )));
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
                ui.add(centered_text_edit(&mut self.cal_title).desired_width(180.0));
                ui.label("Cliente:");
                ui.add(centered_text_edit(&mut self.cal_client).desired_width(150.0));
                ui.label("En horas:");
                ui.add(centered_text_edit(&mut self.cal_hours_from_now).desired_width(50.0));
                ui.label("Duración (min):");
                ui.add(centered_text_edit(&mut self.cal_duration_mins).desired_width(55.0));
            });
            ui.horizontal(|ui| {
                ui.label("Notas:");
                ui.add(centered_text_edit(&mut self.cal_notes).desired_width(360.0));
                if ui.add(primary_button(&p, "+ Agendar Cita")).clicked()
                    && !self.cal_title.trim().is_empty()
                {
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
                                if let Ok(ev) = http
                                    .get(format!("{base}/api/v1/calendar"))
                                    .bearer_auth(&token)
                                    .send()
                                    .await
                                {
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
                        ui.label(format!(
                            "· {} · Técnico: {}",
                            ev.client_name, ev.technician_name
                        ));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Eliminar").clicked() {
                                let auth = self.auth.as_ref().unwrap();
                                let http = self.http.clone();
                                let tx = self.ui_tx.clone();
                                let base = self.server_url.trim_end_matches('/').to_string();
                                let token = auth.token.clone();
                                let eid: Uuid = ev.id;
                                self.rt.spawn(async move {
                                    let _ = http
                                        .delete(format!("{base}/api/v1/calendar/{eid}"))
                                        .bearer_auth(&token)
                                        .send()
                                        .await;
                                    if let Ok(r) = http
                                        .get(format!("{base}/api/v1/calendar"))
                                        .bearer_auth(&token)
                                        .send()
                                        .await
                                    {
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
        ui.heading("Usuarios y seguridad");
        ui.separator();

        styled_card(&p).show(ui, |ui| {
            ui.strong("Verificación en dos pasos");
            ui.horizontal(|ui| {
                if ui
                    .add(primary_button(&p, "Generar secreto MFA TOTP"))
                    .clicked()
                {
                    let auth = self.auth.as_ref().unwrap();
                    let http = self.http.clone();
                    let tx = self.ui_tx.clone();
                    let base = self.server_url.trim_end_matches('/').to_string();
                    let token = auth.token.clone();
                    self.rt.spawn(async move {
                        if let Ok(r) = http
                            .post(format!("{base}/api/v1/auth/mfa/setup"))
                            .bearer_auth(&token)
                            .send()
                            .await
                        {
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
                    ui.add(centered_text_edit(&mut self.mfa_code_input).desired_width(110.0));
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
                                    let _ = tx.send(UiEvent::Toast(
                                        "MFA TOTP activado correctamente".into(),
                                    ));
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
            ui.strong("Nuevo usuario");
            ui.horizontal_wrapped(|ui| {
                ui.label("Email:");
                ui.add(centered_text_edit(&mut self.new_user_email).desired_width(170.0));
                ui.label("Nombre:");
                ui.add(centered_text_edit(&mut self.new_user_name).desired_width(140.0));
                ui.label("Contraseña:");
                ui.add(
                    centered_text_edit(&mut self.new_user_pw)
                        .password(true)
                        .desired_width(110.0),
                );
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
                                if let Ok(ur) = http
                                    .get(format!("{base}/api/v1/users"))
                                    .bearer_auth(&token)
                                    .send()
                                    .await
                                {
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
                        if u.mfa_enabled {
                            "● MFA activo"
                        } else {
                            "○ Sin MFA"
                        },
                    );
                });
            });
            ui.add_space(4.0);
        }
    }
}

/// Busca el bloque de recursos HW (`cpu`/`ram`) dentro de una respuesta de diagnóstico.
pub fn extract_resources(v: &serde_json::Value) -> Option<serde_json::Value> {
    let candidates = [
        Some(v),
        v.get("data"),
        v.get("data").and_then(|d| d.get("resources")),
        v.get("resources"),
        v.get("result"),
        v.get("result").and_then(|d| d.get("resources")),
    ];
    for c in candidates.into_iter().flatten() {
        if c.get("cpu").is_some() && c.get("ram").is_some() {
            return Some(c.clone());
        }
    }
    None
}

fn load_color(p: &Palette, pct: f32) -> egui::Color32 {
    if pct >= 90.0 {
        p.danger
    } else if pct >= 70.0 {
        p.warn
    } else {
        p.ok
    }
}

fn gauge(ui: &mut egui::Ui, p: &Palette, label: &str, pct: f32, detail: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).strong().color(p.text_primary));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(format!("{pct:.0}%"))
                    .small()
                    .strong()
                    .color(load_color(p, pct)),
            );
        });
    });
    ui.add(egui::Label::new(RichText::new(detail).small().color(p.text_muted)).truncate());
    let frac = (pct / 100.0).clamp(0.0, 1.0);
    ui.add(
        egui::ProgressBar::new(frac)
            .fill(load_color(p, pct))
            .text("")
            .desired_height(7.0)
            .desired_width(f32::INFINITY),
    );
    ui.add_space(7.0);
}

pub fn render_hardware_resources_ui(ui: &mut egui::Ui, p: &Palette, hw: &serde_json::Value) {
    let f = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
    let s = |v: &serde_json::Value, k: &str| {
        v.get(k).and_then(|x| x.as_str()).unwrap_or("-").to_string()
    };

    let host = s(hw, "hostname");
    let os = s(hw, "os_caption");
    let up = f(hw, "uptime_hours");
    ui.label(
        RichText::new(format!("{host} · {os} · uptime {up:.1} h"))
            .small()
            .color(p.text_muted),
    );
    ui.add_space(4.0);

    if let Some(cpu) = hw.get("cpu") {
        gauge(
            ui,
            p,
            "Procesador",
            f(cpu, "load_pct") as f32,
            &format!(
                "{} · {} núcleos / {} hilos",
                s(cpu, "name"),
                f(cpu, "cores") as i64,
                f(cpu, "logical_processors") as i64
            ),
        );
    }
    if let Some(ram) = hw.get("ram") {
        gauge(
            ui,
            p,
            "Memoria RAM",
            f(ram, "load_pct") as f32,
            &format!(
                "{:.1} GB usados de {:.1} GB (libres {:.1} GB)",
                f(ram, "used_gb"),
                f(ram, "total_gb"),
                f(ram, "free_gb")
            ),
        );
    }
    if let Some(disks) = hw.get("disks").and_then(|d| d.as_array()) {
        for d in disks {
            let status = s(d, "status");
            gauge(
                ui,
                p,
                &format!("Disco {} ({})", s(d, "drive"), s(d, "label")),
                f(d, "used_pct") as f32,
                &format!(
                    "{:.0} GB libres de {:.0} GB · estado: {status}",
                    f(d, "free_gb"),
                    f(d, "total_gb")
                ),
            );
        }
    }
    if let Some(gpus) = hw.get("gpus").and_then(|d| d.as_array()) {
        for g in gpus {
            let vram = g
                .get("vram_gb")
                .and_then(|x| x.as_f64())
                .map(|v| format!("{v:.1} GB VRAM"))
                .unwrap_or_else(|| "VRAM n/d".into());
            let detail = format!("{} · driver {} · {}", vram, s(g, "driver"), s(g, "status"));
            match g.get("load_pct").and_then(|x| x.as_f64()) {
                Some(load) => gauge(
                    ui,
                    p,
                    &format!("GPU {}", s(g, "name")),
                    load as f32,
                    &detail,
                ),
                None => {
                    ui.label(
                        RichText::new(format!("GPU {}", s(g, "name")))
                            .strong()
                            .color(p.text_primary),
                    );
                    ui.label(
                        RichText::new(format!("{detail} · carga n/d"))
                            .small()
                            .color(p.text_muted),
                    );
                    ui.add_space(4.0);
                }
            }
        }
    }
    if let Some(users) = hw.get("windows_users").and_then(|d| d.as_array()) {
        let names: Vec<String> = users
            .iter()
            .map(|u| {
                let en = u.get("enabled").and_then(|x| x.as_bool()).unwrap_or(true);
                format!("{}{}", s(u, "name"), if en { "" } else { " (deshab.)" })
            })
            .collect();
        ui.label(
            RichText::new(format!("Usuarios Windows: {}", names.join(", ")))
                .small()
                .color(p.text_muted),
        );
    }
    if let Some(sess) = hw.get("windows_sessions").and_then(|d| d.as_array()) {
        for se in sess {
            ui.label(
                RichText::new(format!(
                    "Sesión #{} · {} · {}",
                    s(se, "id"),
                    s(se, "username"),
                    s(se, "state")
                ))
                .small()
                .color(p.text_muted),
            );
        }
    }
}

impl AgentDeskApp {
    /// Lanza una llamada autenticada a la API; al terminar muestra un toast y refresca datos.
    pub fn fire_api(
        &self,
        method: reqwest::Method,
        path: String,
        body: Option<serde_json::Value>,
        ok_msg: String,
    ) {
        let Some(auth) = &self.auth else { return };
        let http = self.http.clone();
        let tx = self.ui_tx.clone();
        let base = self.server_url.trim_end_matches('/').to_string();
        let token = auth.token.clone();
        self.rt.spawn(async move {
            let mut rb = http
                .request(method, format!("{base}{path}"))
                .bearer_auth(&token);
            if let Some(b) = body {
                rb = rb.json(&b);
            }
            match rb.send().await {
                Ok(r) if r.status().is_success() => {
                    let _ = tx.send(UiEvent::Toast(ok_msg));
                    let _ = tx.send(UiEvent::RefreshRequested);
                }
                Ok(r) => {
                    let txt = r.text().await.unwrap_or_default();
                    let msg = serde_json::from_str::<serde_json::Value>(&txt)
                        .ok()
                        .and_then(|v| {
                            v.get("error")
                                .and_then(|e| e.as_str())
                                .map(|s| s.to_string())
                        })
                        .unwrap_or(txt);
                    let _ = tx.send(UiEvent::Toast(format!("Error: {msg}")));
                }
                Err(e) => {
                    let _ = tx.send(UiEvent::Toast(format!("Error de red: {e}")));
                }
            }
        });
    }

    pub fn ui_groups(&mut self, ui: &mut egui::Ui) {
        if self.auth.is_none() {
            self.ui_account_login_card(ui, "Grupos y Compartición");
            return;
        }
        let p = self.palette;
        ui.heading("Grupos de equipos y compartición");
        ui.label(
            RichText::new("Creá grupos, compartilos con otros usuarios registrados y registrá equipos por ID de conexión o por email.")
                .small()
                .color(p.text_muted),
        );
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            styled_card(&p).show(ui, |ui| {
                ui.strong("Crear grupo");
                ui.horizontal_wrapped(|ui| {
                    ui.label("Nombre:");
                    ui.add(centered_text_edit(&mut self.new_group_name).desired_width(180.0));
                    ui.label("Descripción:");
                    ui.add(centered_text_edit(&mut self.new_group_desc).desired_width(240.0));
                    if ui.add(primary_button(&p, "+ Crear grupo")).clicked()
                        && !self.new_group_name.trim().is_empty()
                    {
                        self.fire_api(
                            reqwest::Method::POST,
                            "/api/v1/groups".into(),
                            Some(serde_json::json!({
                                "name": self.new_group_name.trim(),
                                "description": self.new_group_desc.trim()
                            })),
                            "Grupo creado".into(),
                        );
                        self.new_group_name.clear();
                        self.new_group_desc.clear();
                    }
                });
            });
            ui.add_space(8.0);

            styled_card(&p).show(ui, |ui| {
                ui.strong("Registrar / asignar equipo por ID o email");
                ui.horizontal_wrapped(|ui| {
                    ui.label("ID de conexión:");
                    ui.add(
                        centered_text_edit(&mut self.assign_conn_id)
                            .hint_text(placeholder_text(&p, "123 456 789"))
                            .desired_width(130.0),
                    );
                    ui.label("Email usuario:");
                    ui.add(
                        centered_text_edit(&mut self.assign_email)
                            .hint_text(placeholder_text(&p, "tecnico@empresa.com (opcional)"))
                            .desired_width(220.0),
                    );
                    ui.label("Alias:");
                    ui.add(centered_text_edit(&mut self.assign_alias).desired_width(130.0));
                });
                let names: Vec<String> = std::iter::once("Sin grupo".to_string())
                    .chain(self.groups.iter().map(|g| g.name.clone()))
                    .collect();
                ui.horizontal_wrapped(|ui| {
                    egui::ComboBox::from_id_salt("assign_group")
                        .selected_text(names.get(self.assign_group_idx).cloned().unwrap_or_default())
                        .show_ui(ui, |ui| {
                            for (i, n) in names.iter().enumerate() {
                                ui.selectable_value(&mut self.assign_group_idx, i, n);
                            }
                        });
                    if ui.add(primary_button(&p, "Registrar equipo")).clicked()
                        && !self.assign_conn_id.trim().is_empty()
                    {
                        let gid = if self.assign_group_idx > 0 {
                            self.groups.get(self.assign_group_idx - 1).map(|g| g.id)
                        } else {
                            None
                        };
                        self.fire_api(
                            reqwest::Method::POST,
                            "/api/v1/devices/assign".into(),
                            Some(serde_json::json!({
                                "connection_id": self.assign_conn_id.trim(),
                                "owner_email": if self.assign_email.trim().is_empty() { None } else { Some(self.assign_email.trim().to_string()) },
                                "group_id": gid,
                                "alias": if self.assign_alias.trim().is_empty() { None } else { Some(self.assign_alias.trim().to_string()) },
                            })),
                            "Equipo registrado/asignado".into(),
                        );
                        self.assign_conn_id.clear();
                        self.assign_email.clear();
                        self.assign_alias.clear();
                    }
                });
            });
            ui.add_space(8.0);

            let groups = self.groups.clone();
            if groups.is_empty() {
                ui.label(RichText::new("Todavía no hay grupos.").color(p.text_muted));
            }
            for g in groups {
                styled_card(&p).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(format!("📁 {}", g.name));
                        ui.label(
                            RichText::new(format!(
                                "{} equipos · propietario {} · tu permiso: {}",
                                g.device_count, g.owner_email, g.my_permission
                            ))
                            .small()
                            .color(p.text_muted),
                        );
                        if g.my_permission == "full_control"
                            && ui.add(crate::theme::danger_button(&p, "Eliminar grupo")).clicked()
                        {
                            self.fire_api(
                                reqwest::Method::DELETE,
                                format!("/api/v1/groups/{}", g.id),
                                None,
                                "Grupo eliminado".into(),
                            );
                        }
                    });
                    if !g.description.is_empty() {
                        ui.label(RichText::new(&g.description).small().color(p.text_muted));
                    }
                    for sh in &g.shares {
                        ui.horizontal(|ui| {
                            ui.label(format!("🤝 {} — {}", sh.user_email, sh.permission));
                            if g.my_permission == "full_control" && ui.small_button("× quitar").clicked() {
                                self.fire_api(
                                    reqwest::Method::DELETE,
                                    format!("/api/v1/groups/{}/shares/{}", g.id, sh.user_id),
                                    None,
                                    "Compartición revocada".into(),
                                );
                            }
                        });
                    }
                    if g.my_permission == "full_control" {
                        let perm = self
                            .share_perm_map
                            .entry(g.id)
                            .or_insert_with(|| "read_only".to_string())
                            .clone();
                        let mut email = self.share_email_map.get(&g.id).cloned().unwrap_or_default();
                        let mut new_perm = perm.clone();
                        let mut do_share = false;
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Compartir con:");
                            ui.add(
                                centered_text_edit(&mut email)
                                    .hint_text(placeholder_text(&p, "email@empresa.com"))
                                    .desired_width(210.0),
                            );
                            egui::ComboBox::from_id_salt(format!("perm_{}", g.id))
                                .selected_text(new_perm.clone())
                                .show_ui(ui, |ui| {
                                    for pm in ["read_only", "read_write", "full_control"] {
                                        ui.selectable_value(&mut new_perm, pm.to_string(), pm);
                                    }
                                });
                            if ui.button("Compartir").clicked() && !email.trim().is_empty() {
                                do_share = true;
                            }
                        });
                        self.share_perm_map.insert(g.id, new_perm.clone());
                        if do_share {
                            self.fire_api(
                                reqwest::Method::POST,
                                format!("/api/v1/groups/{}/shares", g.id),
                                Some(serde_json::json!({ "user_email": email.trim(), "permission": new_perm })),
                                "Grupo compartido".into(),
                            );
                            email.clear();
                        }
                        self.share_email_map.insert(g.id, email);
                    }
                });
                ui.add_space(6.0);
            }
        });
    }
}
