use gpui_kit::base::StyledExt;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc as tokio_mpsc, watch};

pub enum PanelEvent {
    Chat { sender: String, text: String },
    Toast(String),
    Close,
}

pub struct FloatingPanelLaunch {
    pub caller_name: String,
    pub allow_control: bool,
    pub connection_id: String,
    pub dark: bool,
    pub logo: Arc<RenderImage>,
    pub stop_tx: watch::Sender<bool>,
    pub stop_rx: watch::Receiver<bool>,
    pub chat_tx: tokio_mpsc::UnboundedSender<String>,
    pub incoming_rx: Receiver<PanelEvent>,
}

#[cfg(windows)]
fn work_area_rect() -> (i32, i32, i32, i32) {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SystemParametersInfoW, SM_CXSCREEN, SM_CYSCREEN, SPI_GETWORKAREA,
    };
    unsafe {
        let mut rect = RECT::default();
        if SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut rect as *mut RECT as *mut _),
            Default::default(),
        )
        .is_ok()
        {
            return (rect.left, rect.top, rect.right, rect.bottom);
        }
        let w = GetSystemMetrics(SM_CXSCREEN).max(800);
        let h = GetSystemMetrics(SM_CYSCREEN).max(600);
        (0, 0, w, h - 40)
    }
}

#[cfg(not(windows))]
fn work_area_rect() -> (i32, i32, i32, i32) {
    (0, 0, 1280, 720)
}

#[cfg(windows)]
fn extract_hwnd(window: &mut Window) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return None;
    };
    Some(h.hwnd.get() as isize)
}

#[cfg(not(windows))]
fn extract_hwnd(_window: &mut Window) -> Option<isize> {
    None
}

#[cfg(windows)]
fn dock_hwnd_bottom_right(hwnd_val: Option<isize>, w: i32, h: i32, flush_edge: bool) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    };
    let Some(val) = hwnd_val else {
        return;
    };
    let (_, _, right, bottom) = work_area_rect();
    let margin_x = if flush_edge { 0 } else { 14 };
    let margin_y = if flush_edge { 96 } else { 16 };
    let x = (right - w - margin_x).max(0);
    let y = (bottom - h - margin_y).max(0);
    unsafe {
        let _ = SetWindowPos(
            HWND(val as *mut _),
            HWND_TOPMOST,
            x,
            y,
            w,
            h,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

#[cfg(not(windows))]
fn dock_hwnd_bottom_right(_hwnd: Option<isize>, _w: i32, _h: i32, _flush_edge: bool) {}

#[derive(Clone)]
struct ChatBubble {
    mine: bool,
    sender: String,
    text: String,
}

pub struct FloatingSessionPanel {
    caller_name: String,
    allow_control: bool,
    connection_id: String,
    dark: bool,
    logo: Arc<RenderImage>,
    stop_tx: watch::Sender<bool>,
    stop_rx: watch::Receiver<bool>,
    chat_tx: tokio_mpsc::UnboundedSender<String>,
    incoming_rx: Receiver<PanelEvent>,
    hwnd: Option<isize>,
    compact: bool,
    chat_open: bool,
    unread: usize,
    messages: Vec<ChatBubble>,
    last_toast: String,
    chat_input: Entity<InputState>,
}

impl FloatingSessionPanel {
    pub fn open(launch: FloatingPanelLaunch, cx: &mut App) {
        let (_, _, right, bottom) = work_area_rect();
        let w = 324.0f32;
        let h = 236.0f32;
        let x = ((right as f32) - w - 14.0).max(0.0);
        let y = ((bottom as f32) - h - 16.0).max(0.0);

        let bounds = Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
        let _ = open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                kind: WindowKind::PopUp,
                titlebar: None,
                is_resizable: false,
                is_minimizable: false,
                ..Default::default()
            },
            cx,
            |window, cx| {
                let hwnd = extract_hwnd(window);
                dock_hwnd_bottom_right(hwnd, 324, 236, false);
                cx.new(|cx| Self::new(launch, hwnd, window, cx))
            },
        );
    }

    fn new(
        launch: FloatingPanelLaunch,
        hwnd: Option<isize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let chat_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Escribí un mensaje al técnico…")
        });
        cx.subscribe_in(&chat_input, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.send_chat(window, cx);
            }
        })
        .detach();

        let weak = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| loop {
                cx.background_executor()
                    .timer(Duration::from_millis(80))
                    .await;
                let keep = cx
                    .update(|window, cx| {
                        let Some(ent) = weak.upgrade() else {
                            return false;
                        };
                        ent.update(cx, |this, cx| this.poll(window, cx))
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            })
            .detach();

        Self {
            caller_name: launch.caller_name,
            allow_control: launch.allow_control,
            connection_id: launch.connection_id,
            dark: launch.dark,
            logo: launch.logo,
            stop_tx: launch.stop_tx,
            stop_rx: launch.stop_rx,
            chat_tx: launch.chat_tx,
            incoming_rx: launch.incoming_rx,
            hwnd,
            compact: false,
            chat_open: false,
            unread: 0,
            messages: Vec::new(),
            last_toast: String::new(),
            chat_input,
        }
    }

    fn sync_window_size(&self) {
        if self.compact {
            dock_hwnd_bottom_right(self.hwnd, 40, 108, true);
        } else if self.chat_open {
            dock_hwnd_bottom_right(self.hwnd, 324, 412, false);
        } else {
            dock_hwnd_bottom_right(self.hwnd, 324, 236, false);
        }
    }

    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if *self.stop_rx.borrow() {
            window.remove_window();
            return false;
        }
        let mut dirty = false;
        while let Ok(ev) = self.incoming_rx.try_recv() {
            dirty = true;
            match ev {
                PanelEvent::Chat { sender, text } => {
                    self.messages.push(ChatBubble {
                        mine: false,
                        sender,
                        text,
                    });
                    if self.compact || !self.chat_open {
                        self.unread += 1;
                    }
                    if self.messages.len() > 80 {
                        self.messages.remove(0);
                    }
                }
                PanelEvent::Toast(msg) => {
                    self.last_toast = msg;
                }
                PanelEvent::Close => {
                    window.remove_window();
                    return false;
                }
            }
        }
        if dirty {
            cx.notify();
        }
        true
    }

    fn send_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let msg = self.chat_input.read(cx).value().trim().to_string();
        if msg.is_empty() {
            return;
        }
        let _ = self.chat_tx.send(msg.clone());
        self.messages.push(ChatBubble {
            mine: true,
            sender: "Yo".into(),
            text: msg,
        });
        self.chat_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Escribí un mensaje al técnico…")
        });
        cx.subscribe_in(&self.chat_input, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.send_chat(window, cx);
            }
        })
        .detach();
        cx.notify();
    }

    fn initials(&self) -> String {
        let parts: Vec<_> = self.caller_name.split_whitespace().collect();
        let s: String = parts
            .first()
            .into_iter()
            .chain(parts.last().filter(|_| parts.len() > 1))
            .filter_map(|p| p.chars().find(|c| c.is_alphanumeric()))
            .collect::<String>()
            .to_uppercase();
        if s.is_empty() {
            "AD".into()
        } else {
            s
        }
    }
}

impl Render for FloatingSessionPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (bg, panel, border, text, muted, accent, header_bg) = if self.dark {
            (0x0B1120u32, 0x131C2E, 0x263449, 0xF1F5F9, 0x94A3B8, 0x2563EB, 0x1D4ED8)
        } else {
            (0xFFFFFFu32, 0xF8FAFC, 0xCBD5E1, 0x0F172A, 0x64748B, 0x1D4ED8, 0x0E2A8A)
        };

        if self.compact {
            return div()
                .id("panel-compact-tab")
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .rounded_l_xl()
                .border_1()
                .border_color(rgb(accent))
                .bg(rgb(0x0E2A8A))
                .text_color(rgb(0xFFFFFF))
                .shadow_xl()
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.compact = false;
                    this.sync_window_size();
                    cx.notify();
                }))
                .child(div().size_2().rounded_full().bg(rgb(0x22C55E)))
                .child(div().text_sm().font_bold().child("«"))
                .when(self.unread > 0, |b| {
                    b.child(
                        div()
                            .px_1()
                            .rounded_full()
                            .bg(rgb(0xEF4444))
                            .text_xs()
                            .font_bold()
                            .child(self.unread.to_string()),
                    )
                });
        }

        div()
            .id("floating-session-panel")
            .size_full()
            .flex()
            .flex_col()
            .rounded_xl()
            .overflow_hidden()
            .border_1()
            .border_color(rgb(border))
            .bg(rgb(bg))
            .text_color(rgb(text))
            .shadow_2xl()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(40.))
                    .px_3()
                    .bg(rgb(header_bg))
                    .text_color(rgb(0xFFFFFF))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                img(ImageSource::Render(self.logo.clone()))
                                    .size(px(20.))
                                    .rounded_sm(),
                            )
                            .child(div().text_sm().font_bold().child("AgentDesk"))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .px_2()
                                    .py(px(1.))
                                    .rounded_full()
                                    .bg(rgb(0x16A34A))
                                    .text_xs()
                                    .font_semibold()
                                    .child("En vivo"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .id("panel-compact-btn")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_xs()
                                    .font_semibold()
                                    .hover(|s| s.bg(rgb(0x1E40AF)))
                                    .child("»")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.compact = true;
                                        this.sync_window_size();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("panel-close-btn")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .bg(rgb(0xDC2626))
                                    .text_xs()
                                    .font_bold()
                                    .hover(|s| s.bg(rgb(0xB91C1C)))
                                    .child("×")
                                    .on_click(cx.listener(|this, _, window, _| {
                                        let _ = this.stop_tx.send(true);
                                        window.remove_window();
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(border))
                    .bg(rgb(panel))
                    .child(
                        div()
                            .id("panel-chat-toggle")
                            .flex()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .border_1()
                            .border_color(rgb(if self.chat_open { accent } else { border }))
                            .bg(rgb(if self.chat_open { accent } else { bg }))
                            .text_color(rgb(if self.chat_open { 0xFFFFFF } else { text }))
                            .text_xs()
                            .font_semibold()
                            .child(if self.unread > 0 {
                                format!("Chat ({})", self.unread)
                            } else {
                                "Chat".into()
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.chat_open = !this.chat_open;
                                if this.chat_open {
                                    this.unread = 0;
                                }
                                this.sync_window_size();
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("panel-minimize-bar")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .border_1()
                            .border_color(rgb(border))
                            .bg(rgb(bg))
                            .text_xs()
                            .child("Compactar »")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.compact = true;
                                this.sync_window_size();
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("panel-cut-btn")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(rgb(0xDC2626))
                            .text_color(rgb(0xFFFFFF))
                            .text_xs()
                            .font_semibold()
                            .child("Cortar")
                            .on_click(cx.listener(|this, _, window, _| {
                                let _ = this.stop_tx.send(true);
                                window.remove_window();
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(rgb(muted))
                            .child(format!("Sesión remota · ID local {}", self.connection_id)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .p_2()
                            .rounded_lg()
                            .border_1()
                            .border_color(rgb(border))
                            .bg(rgb(panel))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(34.))
                                    .rounded_full()
                                    .bg(rgb(accent))
                                    .text_color(rgb(0xFFFFFF))
                                    .text_xs()
                                    .font_bold()
                                    .child(self.initials()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .child(self.caller_name.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(muted))
                                            .child(if self.allow_control {
                                                "Control total · Cifrado E2EE · Fondo optimizado"
                                            } else {
                                                "Solo visualización · Cifrado E2EE"
                                            }),
                                    ),
                            ),
                    )
                    .when(!self.last_toast.is_empty(), |b| {
                        b.child(
                            div()
                                .text_xs()
                                .text_color(rgb(0x16A34A))
                                .child(self.last_toast.clone()),
                        )
                    }),
            )
            .when(self.chat_open, |b| {
                b.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .px_3()
                        .pb_3()
                        .child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scrollbar()
                                .p_2()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(border))
                                .bg(rgb(panel))
                                .flex()
                                .flex_col()
                                .gap_1()
                                .when(self.messages.is_empty(), |m| {
                                    m.child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(muted))
                                            .child("Sin mensajes todavía…"),
                                    )
                                })
                                .children(self.messages.iter().map(|m| {
                                    div()
                                        .p_2()
                                        .rounded_md()
                                        .bg(rgb(if m.mine { accent } else { bg }))
                                        .text_color(rgb(if m.mine { 0xFFFFFF } else { text }))
                                        .text_xs()
                                        .child(format!("{}: {}", m.sender, m.text))
                                })),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(Input::new(&self.chat_input).flex_1())
                                .child(
                                    div()
                                        .id("panel-send-chat")
                                        .px_3()
                                        .py_1()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .bg(rgb(accent))
                                        .text_color(rgb(0xFFFFFF))
                                        .text_xs()
                                        .font_semibold()
                                        .child("Enviar")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.send_chat(window, cx);
                                        })),
                                ),
                        ),
                )
            })
    }
}
