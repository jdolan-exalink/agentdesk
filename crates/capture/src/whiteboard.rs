use agentdesk_protocol::WhiteboardStroke;

#[cfg(windows)]
mod win_wb {
    use super::*;
    use crate::win::monitor_desktop_rect;
    use agentdesk_protocol::WhiteboardTool;
    use std::sync::mpsc;
    use std::thread::JoinHandle;
    use windows::core::w;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, AC_SRC_ALPHA,
        AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
        GetSystemMetrics, PostMessageW, PostQuitMessage, RegisterClassW, SetTimer, SetWindowPos, ShowWindow,
        TranslateMessage, UpdateLayeredWindow, HWND_TOPMOST, MSG, SM_CXSCREEN, SM_CYSCREEN,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_HIDE, SW_SHOWNOACTIVATE,
        ULW_ALPHA, WM_APP, WM_CLOSE, WM_DESTROY, WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    };

    const WM_WB_SYNC: u32 = WM_APP + 7;

    enum WbCmd {
        AddStroke(WhiteboardStroke),
        Undo,
        Clear,
        SetVisible(bool),
        SetMonitor(usize),
        Close,
    }

    pub struct Overlay {
        hwnd: isize,
        tx: mpsc::Sender<WbCmd>,
        thread: Option<JoinHandle<()>>,
    }

    struct DrawState {
        strokes: Vec<WhiteboardStroke>,
        visible: bool,
        bounds: (i32, i32, i32, i32),
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    fn blend_pixel(buf: &mut [u8], w: i32, h: i32, x: i32, y: i32, r: u8, g: u8, b: u8, a: u8) {
        if x < 0 || y < 0 || x >= w || y >= h || a == 0 {
            return;
        }
        let idx = ((y * w + x) * 4) as usize;
        let af = a as u32;
        let inv = 255 - af;
        // Premultiplied BGRA for AC_SRC_ALPHA
        let pb = ((b as u32 * af) / 255).min(255);
        let pg = ((g as u32 * af) / 255).min(255);
        let pr = ((r as u32 * af) / 255).min(255);

        buf[idx] = (pb + (buf[idx] as u32 * inv) / 255).min(255) as u8;
        buf[idx + 1] = (pg + (buf[idx + 1] as u32 * inv) / 255).min(255) as u8;
        buf[idx + 2] = (pr + (buf[idx + 2] as u32 * inv) / 255).min(255) as u8;
        buf[idx + 3] = (af + (buf[idx + 3] as u32 * inv) / 255).min(255) as u8;
    }

    fn draw_disc(buf: &mut [u8], w: i32, h: i32, cx: f32, cy: f32, rad: f32, rgba: u32) {
        let r = ((rgba >> 24) & 0xFF) as u8;
        let g = ((rgba >> 16) & 0xFF) as u8;
        let b = ((rgba >> 8) & 0xFF) as u8;
        let a = (rgba & 0xFF) as u8;
        let r_ceil = rad.ceil() as i32;
        let x0 = (cx.floor() as i32 - r_ceil).max(0);
        let x1 = (cx.ceil() as i32 + r_ceil).min(w - 1);
        let y0 = (cy.floor() as i32 - r_ceil).max(0);
        let y1 = (cy.ceil() as i32 + r_ceil).min(h - 1);
        let r2 = rad * rad;
        for py in y0..=y1 {
            let dy = py as f32 - cy;
            for px in x0..=x1 {
                let dx = px as f32 - cx;
                if dx * dx + dy * dy <= r2 {
                    blend_pixel(buf, w, h, px, py, r, g, b, a);
                }
            }
        }
    }

    fn draw_segment(
        buf: &mut [u8],
        w: i32,
        h: i32,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        thickness: f32,
        rgba: u32,
    ) {
        let dx = x1 - x0;
        let dy = y1 - y0;
        let dist = (dx * dx + dy * dy).sqrt();
        let rad = (thickness * 0.5).max(1.0);
        let steps = (dist / (rad * 0.45).max(1.0)).ceil().max(1.0) as i32;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            draw_disc(buf, w, h, x0 + dx * t, y0 + dy * t, rad, rgba);
        }
    }

    fn render_overlay(hwnd: HWND, st: &DrawState) {
        let (vx, vy, vw, vh) = st.bounds;
        if vw <= 0 || vh <= 0 {
            return;
        }
        unsafe {
            if !st.visible && st.strokes.is_empty() {
                let _ = ShowWindow(hwnd, SW_HIDE);
                return;
            }
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                vx,
                vy,
                vw,
                vh,
                SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOMOVE | SWP_NOSIZE,
            );

            let mem_dc = CreateCompatibleDC(None);
            if mem_dc.is_invalid() {
                return;
            }
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: vw,
                    biHeight: -vh,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits_ptr = std::ptr::null_mut();
            let Ok(dib) = CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits_ptr, None, 0)
            else {
                let _ = DeleteDC(mem_dc);
                return;
            };
            if bits_ptr.is_null() {
                let _ = DeleteObject(dib);
                let _ = DeleteDC(mem_dc);
                return;
            }
            let old = SelectObject(mem_dc, dib);
            let slice = std::slice::from_raw_parts_mut(bits_ptr as *mut u8, (vw * vh * 4) as usize);
            slice.fill(0);

            for stroke in &st.strokes {
                let col = match stroke.tool {
                    WhiteboardTool::Highlighter => (stroke.color_rgba & 0xFFFF_FF00) | 0x6E,
                    _ => stroke.color_rgba,
                };
                let thick = match stroke.tool {
                    WhiteboardTool::Highlighter => (stroke.thickness * 2.8).max(12.0),
                    _ => stroke.thickness.max(2.5),
                };
                match stroke.tool {
                    WhiteboardTool::Pen | WhiteboardTool::Highlighter => {
                        if stroke.points.len() == 1 {
                            let (nx, ny) = stroke.points[0];
                            draw_disc(slice, vw, vh, nx * vw as f32, ny * vh as f32, thick * 0.5, col);
                        } else {
                            for pair in stroke.points.windows(2) {
                                let (x0, y0) = (pair[0].0 * vw as f32, pair[0].1 * vh as f32);
                                let (x1, y1) = (pair[1].0 * vw as f32, pair[1].1 * vh as f32);
                                draw_segment(slice, vw, vh, x0, y0, x1, y1, thick, col);
                            }
                        }
                    }
                    WhiteboardTool::Arrow => {
                        if let (Some(a), Some(b)) = (stroke.points.first(), stroke.points.last()) {
                            let (x0, y0) = (a.0 * vw as f32, a.1 * vh as f32);
                            let (x1, y1) = (b.0 * vw as f32, b.1 * vh as f32);
                            draw_segment(slice, vw, vh, x0, y0, x1, y1, thick, col);
                            let angle = (y1 - y0).atan2(x1 - x0);
                            let head_len = (thick * 4.5).max(18.0);
                            let spread = 0.52f32;
                            for sgn in [-1.0f32, 1.0f32] {
                                let ha = angle + std::f32::consts::PI - sgn * spread;
                                let hx = x1 + ha.cos() * head_len;
                                let hy = y1 + ha.sin() * head_len;
                                draw_segment(slice, vw, vh, x1, y1, hx, hy, thick, col);
                            }
                        }
                    }
                    WhiteboardTool::Rect => {
                        if let (Some(a), Some(b)) = (stroke.points.first(), stroke.points.last()) {
                            let x0 = a.0.min(b.0) * vw as f32;
                            let y0 = a.1.min(b.1) * vh as f32;
                            let x1 = a.0.max(b.0) * vw as f32;
                            let y1 = a.1.max(b.1) * vh as f32;
                            draw_segment(slice, vw, vh, x0, y0, x1, y0, thick, col);
                            draw_segment(slice, vw, vh, x1, y0, x1, y1, thick, col);
                            draw_segment(slice, vw, vh, x1, y1, x0, y1, thick, col);
                            draw_segment(slice, vw, vh, x0, y1, x0, y0, thick, col);
                        }
                    }
                }
            }

            let dst_pt = POINT { x: vx, y: vy };
            let src_pt = POINT { x: 0, y: 0 };
            let sz = SIZE { cx: vw, cy: vh };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = UpdateLayeredWindow(
                hwnd,
                None,
                Some(&dst_pt),
                Some(&sz),
                mem_dc,
                Some(&src_pt),
                windows::Win32::Foundation::COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            SelectObject(mem_dc, old);
            let _ = DeleteObject(dib);
            let _ = DeleteDC(mem_dc);
        }
    }

    /// Real desktop rectangle of the captured monitor (origin included), so the strokes land on
    /// the screen the viewer is looking at even when it is not the primary one.
    fn monitor_rect(index: usize) -> (i32, i32, i32, i32) {
        if let Some(r) = monitor_desktop_rect(index) {
            return r;
        }
        unsafe {
            let w = GetSystemMetrics(SM_CXSCREEN).max(640);
            let h = GetSystemMetrics(SM_CYSCREEN).max(480);
            (0, 0, w, h)
        }
    }

    impl Overlay {
        pub fn new() -> Self {
            let (tx, rx) = mpsc::channel::<WbCmd>();
            let (hwnd_tx, hwnd_rx) = mpsc::sync_channel::<isize>(1);

            let thread = std::thread::spawn(move || unsafe {
                let hinst = GetModuleHandleW(None).unwrap_or_default();
                let cls_name = w!("AgentDeskWhiteboardOverlay");
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(wnd_proc),
                    hInstance: hinst.into(),
                    lpszClassName: cls_name,
                    ..Default::default()
                };
                let _ = RegisterClassW(&wc);

                let (vx, vy, vw, vh) = monitor_rect(0);
                let ex_style = WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE;
                let Ok(hwnd) = CreateWindowExW(
                    ex_style,
                    cls_name,
                    w!("AgentDesk Pizarra"),
                    WS_POPUP,
                    vx,
                    vy,
                    vw,
                    vh,
                    None,
                    None,
                    hinst,
                    None,
                ) else {
                    let _ = hwnd_tx.send(0);
                    return;
                };
                let _ = hwnd_tx.send(hwnd.0 as isize);

                let mut state = DrawState {
                    strokes: Vec::new(),
                    visible: false,
                    bounds: (vx, vy, vw, vh),
                };

                // Other always-on-top windows (support tools, kiosk apps) can cover the overlay
                // after it was shown; re-assert the top of the z-order once a second.
                let _ = SetTimer(hwnd, 1, 1000, None);

                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    if msg.message == WM_TIMER {
                        if state.visible || !state.strokes.is_empty() {
                            let _ = SetWindowPos(
                                hwnd,
                                HWND_TOPMOST,
                                0,
                                0,
                                0,
                                0,
                                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                            );
                        }
                    } else if msg.message == WM_WB_SYNC {
                        let mut dirty = false;
                        while let Ok(cmd) = rx.try_recv() {
                            match cmd {
                                WbCmd::AddStroke(s) => {
                                    state.strokes.push(s);
                                    dirty = true;
                                }
                                WbCmd::Undo => {
                                    state.strokes.pop();
                                    dirty = true;
                                }
                                WbCmd::Clear => {
                                    state.strokes.clear();
                                    dirty = true;
                                }
                                WbCmd::SetVisible(v) => {
                                    state.visible = v;
                                    dirty = true;
                                }
                                WbCmd::SetMonitor(idx) => {
                                    state.bounds = monitor_rect(idx);
                                    dirty = true;
                                }
                                WbCmd::Close => {
                                    let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                                }
                            }
                        }
                        if dirty {
                            render_overlay(hwnd, &state);
                        }
                    } else {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            });

            let hwnd = hwnd_rx.recv().unwrap_or(0);
            Self {
                hwnd,
                tx,
                thread: Some(thread),
            }
        }

        fn send_cmd(&self, cmd: WbCmd) {
            if self.tx.send(cmd).is_ok() && self.hwnd != 0 {
                unsafe {
                    let _ = PostMessageW(
                        HWND(self.hwnd as *mut _),
                        WM_WB_SYNC,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
        }

        pub fn add_stroke(&self, stroke: WhiteboardStroke) {
            self.send_cmd(WbCmd::AddStroke(stroke));
        }

        pub fn undo(&self) {
            self.send_cmd(WbCmd::Undo);
        }

        pub fn clear(&self) {
            self.send_cmd(WbCmd::Clear);
        }

        pub fn set_visible(&self, visible: bool) {
            self.send_cmd(WbCmd::SetVisible(visible));
        }

        pub fn set_monitor(&self, idx: usize) {
            self.send_cmd(WbCmd::SetMonitor(idx));
        }
    }

    impl Drop for Overlay {
        fn drop(&mut self) {
            self.send_cmd(WbCmd::Close);
            if let Some(handle) = self.thread.take() {
                let _ = handle.join();
            }
        }
    }
}

pub struct WhiteboardOverlay {
    #[cfg(windows)]
    inner: win_wb::Overlay,
}

impl Default for WhiteboardOverlay {
    fn default() -> Self {
        Self::new()
    }
}

impl WhiteboardOverlay {
    pub fn new() -> Self {
        Self {
            #[cfg(windows)]
            inner: win_wb::Overlay::new(),
        }
    }

    pub fn add_stroke(&self, stroke: WhiteboardStroke) {
        #[cfg(windows)]
        self.inner.add_stroke(stroke);
        #[cfg(not(windows))]
        let _ = stroke;
    }

    pub fn undo(&self) {
        #[cfg(windows)]
        self.inner.undo();
    }

    pub fn clear(&self) {
        #[cfg(windows)]
        self.inner.clear();
    }

    pub fn set_visible(&self, visible: bool) {
        #[cfg(windows)]
        self.inner.set_visible(visible);
        #[cfg(not(windows))]
        let _ = visible;
    }

    pub fn set_monitor(&self, idx: usize) {
        #[cfg(windows)]
        self.inner.set_monitor(idx);
        #[cfg(not(windows))]
        let _ = idx;
    }
}
