use agentdesk_protocol::{FrameEncoding, InputEvent, MouseBtn};
use image::codecs::jpeg::JpegEncoder;
use image::{ColorType, ImageReader};
use std::io::Cursor;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("capture backend error: {0}")]
    Backend(String),
    #[error("codec error: {0}")]
    Codec(String),
    #[error("no frame available yet")]
    WouldBlock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureBackend {
    Dxgi,
    Gdi,
    Stub,
}

impl CaptureBackend {
    pub fn as_str(&self) -> &'static str {
        match self {
            CaptureBackend::Dxgi => "dxgi",
            CaptureBackend::Gdi => "gdi",
            CaptureBackend::Stub => "stub",
        }
    }
}

pub struct RawFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub backend: CaptureBackend,
}

pub trait ScreenCapturer: Send {
    fn capture_rgba(&mut self) -> Result<RawFrame, CaptureError>;
    fn dimensions(&self) -> (u32, u32);
    fn backend(&self) -> CaptureBackend;
}

pub fn encode_frame(
    frame: &RawFrame,
    encoding: FrameEncoding,
    jpeg_quality: u8,
) -> Result<Vec<u8>, CaptureError> {
    match encoding {
        FrameEncoding::Jpeg => {
            let mut rgb = Vec::with_capacity((frame.width * frame.height * 3) as usize);
            for px in frame.rgba.chunks_exact(4) {
                rgb.push(px[0]);
                rgb.push(px[1]);
                rgb.push(px[2]);
            }
            let mut out = Vec::with_capacity((frame.width * frame.height / 8) as usize);
            let mut enc = JpegEncoder::new_with_quality(&mut out, jpeg_quality.clamp(20, 95));
            enc.encode(&rgb, frame.width, frame.height, ColorType::Rgb8.into())
                .map_err(|e| CaptureError::Codec(e.to_string()))?;
            Ok(out)
        }
        FrameEncoding::Lz4Rgba => Ok(lz4_flex::compress_prepend_size(&frame.rgba)),
    }
}

pub fn decode_frame_to_rgba(
    encoding: FrameEncoding,
    payload: &[u8],
) -> Result<(u32, u32, Vec<u8>), CaptureError> {
    match encoding {
        FrameEncoding::Jpeg => {
            let img = ImageReader::new(Cursor::new(payload))
                .with_guessed_format()
                .map_err(|e| CaptureError::Codec(e.to_string()))?
                .decode()
                .map_err(|e| CaptureError::Codec(e.to_string()))?;
            let rgba = img.to_rgba8();
            Ok((rgba.width(), rgba.height(), rgba.into_raw()))
        }
        FrameEncoding::Lz4Rgba => {
            let raw = lz4_flex::decompress_size_prepended(payload)
                .map_err(|e| CaptureError::Codec(e.to_string()))?;
            // Dimension is supplied in VideoFrame header by caller; return 0x0 marker here.
            Ok((0, 0, raw))
        }
    }
}

#[cfg(windows)]
pub mod win {
    use super::*;
    use windows::core::Interface;
    use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
        D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE,
        D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    };
    use windows::Win32::Graphics::Dxgi::Common::{
        DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
    };
    use windows::Win32::Graphics::Dxgi::{
        IDXGIAdapter, IDXGIDevice, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
        DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
    };
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
        SRCCOPY,
    };
    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::StationsAndDesktops::{
        CloseDesktop, OpenInputDesktop, SetThreadDesktop, DESKTOP_ACCESS_FLAGS,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
        MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
        MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
        MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, VIRTUAL_KEY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

    fn attach_input_desktop() {
        unsafe {
            // GENERIC_ALL / READ_CONTROL | WRITE_DAC | DESKTOP_READOBJECTS | DESKTOP_WRITEOBJECTS
            let flags = DESKTOP_ACCESS_FLAGS(0x0001 | 0x0002 | 0x0040 | 0x0080 | 0x0100);
            if let Ok(hdesk) = OpenInputDesktop(Default::default(), false, flags) {
                let _ = SetThreadDesktop(hdesk);
                let _ = CloseDesktop(hdesk);
            }
        }
    }

    struct DxgiState {
        _device: ID3D11Device,
        context: ID3D11DeviceContext,
        duplication: IDXGIOutputDuplication,
        staging: ID3D11Texture2D,
        width: u32,
        height: u32,
        last_rgba: Vec<u8>,
    }

    impl DxgiState {
        fn new() -> Result<Self, CaptureError> {
            unsafe {
                let mut device: Option<ID3D11Device> = None;
                let mut context: Option<ID3D11DeviceContext> = None;
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_HARDWARE,
                    None,
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    Some(&[D3D_FEATURE_LEVEL_11_0]),
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut context),
                )
                .map_err(|e| CaptureError::Backend(format!("D3D11CreateDevice: {e}")))?;

                let device = device.ok_or_else(|| CaptureError::Backend("no D3D11 device".into()))?;
                let context =
                    context.ok_or_else(|| CaptureError::Backend("no D3D11 context".into()))?;

                let dxgi_device: IDXGIDevice = device
                    .cast()
                    .map_err(|e| CaptureError::Backend(format!("IDXGIDevice: {e}")))?;
                let adapter: IDXGIAdapter = dxgi_device
                    .GetAdapter()
                    .map_err(|e| CaptureError::Backend(format!("GetAdapter: {e}")))?;
                let output = adapter
                    .EnumOutputs(0)
                    .map_err(|e| CaptureError::Backend(format!("EnumOutputs(0): {e}")))?;
                let output1: IDXGIOutput1 = output
                    .cast()
                    .map_err(|e| CaptureError::Backend(format!("IDXGIOutput1: {e}")))?;

                let duplication = output1
                    .DuplicateOutput(&device)
                    .map_err(|e| CaptureError::Backend(format!("DuplicateOutput: {e}")))?;

                let desc = duplication.GetDesc();
                let width = desc.ModeDesc.Width;
                let height = desc.ModeDesc.Height;

                let tex_desc = D3D11_TEXTURE2D_DESC {
                    Width: width,
                    Height: height,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MiscFlags: 0,
                };
                let mut staging: Option<ID3D11Texture2D> = None;
                device
                    .CreateTexture2D(&tex_desc, None, Some(&mut staging))
                    .map_err(|e| CaptureError::Backend(format!("CreateTexture2D: {e}")))?;
                let staging =
                    staging.ok_or_else(|| CaptureError::Backend("staging texture null".into()))?;

                Ok(Self {
                    _device: device,
                    context,
                    duplication,
                    staging,
                    width,
                    height,
                    last_rgba: vec![0u8; (width * height * 4) as usize],
                })
            }
        }

        fn capture(&mut self, timeout_ms: u32) -> Result<RawFrame, CaptureError> {
            unsafe {
                let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
                let mut resource: Option<IDXGIResource> = None;
                let hr = self.duplication.AcquireNextFrame(
                    timeout_ms,
                    &mut frame_info,
                    &mut resource,
                );
                if let Err(e) = hr {
                    if e.code() == DXGI_ERROR_WAIT_TIMEOUT {
                        if !self.last_rgba.is_empty() {
                            return Ok(RawFrame {
                                width: self.width,
                                height: self.height,
                                rgba: self.last_rgba.clone(),
                                backend: CaptureBackend::Dxgi,
                            });
                        }
                        return Err(CaptureError::WouldBlock);
                    }
                    if e.code() == DXGI_ERROR_ACCESS_LOST {
                        return Err(CaptureError::Backend("DXGI_ERROR_ACCESS_LOST".into()));
                    }
                    return Err(CaptureError::Backend(format!("AcquireNextFrame: {e}")));
                }

                let res = resource.ok_or_else(|| CaptureError::Backend("null resource".into()))?;
                let tex: ID3D11Texture2D = res
                    .cast()
                    .map_err(|e| CaptureError::Backend(format!("cast texture: {e}")))?;

                self.context.CopyResource(&self.staging, &tex);
                let _ = self.duplication.ReleaseFrame();

                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                self.context
                    .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                    .map_err(|e| CaptureError::Backend(format!("Map staging: {e}")))?;

                let row_pitch = mapped.RowPitch as usize;
                let w = self.width as usize;
                let h = self.height as usize;
                let src = std::slice::from_raw_parts(mapped.pData as *const u8, row_pitch * h);

                if self.last_rgba.len() != w * h * 4 {
                    self.last_rgba.resize(w * h * 4, 0);
                }
                for y in 0..h {
                    let row_src = &src[y * row_pitch..y * row_pitch + w * 4];
                    let row_dst = &mut self.last_rgba[y * w * 4..(y + 1) * w * 4];
                    for (px_s, px_d) in row_src.chunks_exact(4).zip(row_dst.chunks_exact_mut(4)) {
                        px_d[0] = px_s[2]; // R
                        px_d[1] = px_s[1]; // G
                        px_d[2] = px_s[0]; // B
                        px_d[3] = 255;     // A
                    }
                }
                self.context.Unmap(&self.staging, 0);

                Ok(RawFrame {
                    width: self.width,
                    height: self.height,
                    rgba: self.last_rgba.clone(),
                    backend: CaptureBackend::Dxgi,
                })
            }
        }
    }

    fn capture_gdi() -> Result<RawFrame, CaptureError> {
        unsafe {
            let w = GetSystemMetrics(SM_CXSCREEN).max(1) as u32;
            let h = GetSystemMetrics(SM_CYSCREEN).max(1) as u32;
            let hdc_screen = GetDC(None);
            let hdc_mem = CreateCompatibleDC(hdc_screen);
            let hbmp = CreateCompatibleBitmap(hdc_screen, w as i32, h as i32);
            let old = SelectObject(hdc_mem, hbmp);

            let ok = BitBlt(hdc_mem, 0, 0, w as i32, h as i32, hdc_screen, 0, 0, SRCCOPY);
            let mut rgba = vec![0u8; (w * h * 4) as usize];

            if ok.is_ok() {
                let mut bi = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: w as i32,
                        biHeight: -(h as i32),
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                GetDIBits(
                    hdc_mem,
                    hbmp,
                    0,
                    h,
                    Some(rgba.as_mut_ptr() as *mut _),
                    &mut bi,
                    DIB_RGB_COLORS,
                );
                for px in rgba.chunks_exact_mut(4) {
                    px.swap(0, 2); // BGRA -> RGBA
                    px[3] = 255;
                }
            }

            SelectObject(hdc_mem, old);
            let _ = DeleteObject(hbmp);
            let _ = DeleteDC(hdc_mem);
            ReleaseDC(None, hdc_screen);

            if ok.is_err() {
                return Err(CaptureError::Backend("GDI BitBlt failed".into()));
            }
            Ok(RawFrame {
                width: w,
                height: h,
                rgba,
                backend: CaptureBackend::Gdi,
            })
        }
    }

    pub struct WindowsCapturer {
        dxgi: Option<DxgiState>,
        width: u32,
        height: u32,
        backend: CaptureBackend,
    }

    impl WindowsCapturer {
        pub fn new() -> Self {
            attach_input_desktop();
            match DxgiState::new() {
                Ok(dx) => {
                    let (w, h) = (dx.width, dx.height);
                    tracing::info!(width = w, height = h, "DXGI Desktop Duplication initialized");
                    Self {
                        dxgi: Some(dx),
                        width: w,
                        height: h,
                        backend: CaptureBackend::Dxgi,
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "DXGI unavailable, falling back to GDI BitBlt");
                    let (w, h) = unsafe {
                        (
                            GetSystemMetrics(SM_CXSCREEN).max(1) as u32,
                            GetSystemMetrics(SM_CYSCREEN).max(1) as u32,
                        )
                    };
                    Self {
                        dxgi: None,
                        width: w,
                        height: h,
                        backend: CaptureBackend::Gdi,
                    }
                }
            }
        }
    }

    impl ScreenCapturer for WindowsCapturer {
        fn capture_rgba(&mut self) -> Result<RawFrame, CaptureError> {
            attach_input_desktop();
            if let Some(dx) = &mut self.dxgi {
                match dx.capture(50) {
                    Ok(f) => return Ok(f),
                    Err(e) => {
                        tracing::warn!(error = %e, "DXGI capture lost/failed, switching to GDI fallback");
                        self.dxgi = None;
                        self.backend = CaptureBackend::Gdi;
                    }
                }
            }
            capture_gdi()
        }

        fn dimensions(&self) -> (u32, u32) {
            (self.width, self.height)
        }

        fn backend(&self) -> CaptureBackend {
            self.backend
        }
    }

    pub fn inject_input_event(ev: &InputEvent) {
        unsafe {
            let input = match ev {
                InputEvent::MouseMoveNorm { x, y } => {
                    let nx = (x.clamp(0.0, 1.0) * 65535.0) as i32;
                    let ny = (y.clamp(0.0, 1.0) * 65535.0) as i32;
                    INPUT {
                        r#type: INPUT_MOUSE,
                        Anonymous: INPUT_0 {
                            mi: MOUSEINPUT {
                                dx: nx,
                                dy: ny,
                                mouseData: 0,
                                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                                time: 0,
                                dwExtraInfo: 0,
                            },
                        },
                    }
                }
                InputEvent::MouseButton { button, pressed } => {
                    let flags = match (button, pressed) {
                        (MouseBtn::Left, true) => MOUSEEVENTF_LEFTDOWN,
                        (MouseBtn::Left, false) => MOUSEEVENTF_LEFTUP,
                        (MouseBtn::Right, true) => MOUSEEVENTF_RIGHTDOWN,
                        (MouseBtn::Right, false) => MOUSEEVENTF_RIGHTUP,
                        (MouseBtn::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
                        (MouseBtn::Middle, false) => MOUSEEVENTF_MIDDLEUP,
                    };
                    INPUT {
                        r#type: INPUT_MOUSE,
                        Anonymous: INPUT_0 {
                            mi: MOUSEINPUT {
                                dwFlags: flags,
                                ..Default::default()
                            },
                        },
                    }
                }
                InputEvent::MouseWheel { delta_y } => INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            mouseData: *delta_y as u32,
                            dwFlags: MOUSEEVENTF_WHEEL,
                            ..Default::default()
                        },
                    },
                },
                InputEvent::Key { vk, pressed } => INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: VIRTUAL_KEY(*vk),
                            wScan: 0,
                            dwFlags: if *pressed {
                                Default::default()
                            } else {
                                KEYEVENTF_KEYUP
                            },
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                },
            };
            let _ = SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
        }
    }

    pub fn current_process_private_mb() -> f64 {
        unsafe {
            let mut pmc = PROCESS_MEMORY_COUNTERS_EX::default();
            pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
            let ok = GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut pmc as *mut _ as *mut _,
                pmc.cb,
            );
            if ok.is_ok() {
                (pmc.PrivateUsage as f64) / (1024.0 * 1024.0)
            } else {
                0.0
            }
        }
    }
}

#[cfg(not(windows))]
pub mod win {
    use super::*;
    pub struct WindowsCapturer;
    impl WindowsCapturer {
        pub fn new() -> Self {
            Self
        }
    }
    impl ScreenCapturer for WindowsCapturer {
        fn capture_rgba(&mut self) -> Result<RawFrame, CaptureError> {
            Ok(RawFrame {
                width: 640,
                height: 360,
                rgba: vec![32; 640 * 360 * 4],
                backend: CaptureBackend::Stub,
            })
        }
        fn dimensions(&self) -> (u32, u32) {
            (640, 360)
        }
        fn backend(&self) -> CaptureBackend {
            CaptureBackend::Stub
        }
    }
    pub fn inject_input_event(_ev: &InputEvent) {}
    pub fn current_process_private_mb() -> f64 {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_encode_decode_roundtrip() {
        let frame = RawFrame {
            width: 32,
            height: 32,
            rgba: vec![128; 32 * 32 * 4],
            backend: CaptureBackend::Stub,
        };
        let jpg = encode_frame(&frame, FrameEncoding::Jpeg, 80).unwrap();
        assert!(!jpg.is_empty());
        let (w, h, rgba) = decode_frame_to_rgba(FrameEncoding::Jpeg, &jpg).unwrap();
        assert_eq!((w, h), (32, 32));
        assert_eq!(rgba.len(), 32 * 32 * 4);
    }
}
