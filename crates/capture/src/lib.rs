pub mod audio;
pub mod h264;
pub mod hostdiag;
pub mod printing;
pub mod stream;
pub mod whiteboard;

pub use audio::{spawn_audio_capture_worker, AudioPlayer};
pub use h264::{MfH264Decoder, MfH264Encoder};
pub use printing::{enumerate_printers, print_file_to_printer};
pub use stream::{
    h264_annexb_has_idr, FrameAdmission,
    spawn_capture_worker, AbrController, HostStreamEncoder, StreamConfig, ViewerFrameCanvas,
};
pub use whiteboard::WhiteboardOverlay;

use agentdesk_protocol::{CursorShape, FrameEncoding, InputEvent, MonitorInfo, MouseBtn};
use jpeg_encoder::{ColorType as JpegColorType, Encoder as JpegEncoder, SamplingFactor};
use thiserror::Error;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

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
        FrameEncoding::Jpeg | FrameEncoding::HybridTiles => {
            let mut out = Vec::with_capacity((frame.width * frame.height / 10) as usize);
            let mut enc = JpegEncoder::new(&mut out, jpeg_quality.clamp(20, 95));
            enc.set_sampling_factor(SamplingFactor::F_2_2);
            enc.encode(
                &frame.rgba,
                frame.width.min(u16::MAX as u32) as u16,
                frame.height.min(u16::MAX as u32) as u16,
                JpegColorType::Rgba,
            )
            .map_err(|e| CaptureError::Codec(e.to_string()))?;
            Ok(out)
        }
        FrameEncoding::Lz4Rgba => Ok(lz4_flex::compress_prepend_size(&frame.rgba)),
        FrameEncoding::H264AnnexB => Err(CaptureError::Codec(
            "H264AnnexB requires MfH264Encoder".into(),
        )),
    }
}

pub fn decode_frame_to_bgra(
    encoding: FrameEncoding,
    payload: &[u8],
) -> Result<(u32, u32, Vec<u8>), CaptureError> {
    match encoding {
        FrameEncoding::Jpeg => {
            let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::BGRA);
            let mut decoder = JpegDecoder::new_with_options(payload, options);
            let bgra = decoder
                .decode()
                .map_err(|e| CaptureError::Codec(e.to_string()))?;
            let info = decoder
                .info()
                .ok_or_else(|| CaptureError::Codec("missing JPEG dimensions".into()))?;
            Ok((info.width as u32, info.height as u32, bgra))
        }
        FrameEncoding::Lz4Rgba => {
            let mut raw = lz4_flex::decompress_size_prepended(payload)
                .map_err(|e| CaptureError::Codec(e.to_string()))?;
            for px in raw.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
            Ok((0, 0, raw))
        }
        FrameEncoding::HybridTiles | FrameEncoding::H264AnnexB => {
            Err(CaptureError::Codec("Frame encoding requires ViewerFrameCanvas".into()))
        }
    }
}

pub fn decode_frame_to_rgba(
    encoding: FrameEncoding,
    payload: &[u8],
) -> Result<(u32, u32, Vec<u8>), CaptureError> {
    match encoding {
        FrameEncoding::Jpeg => {
            let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
            let mut decoder = JpegDecoder::new_with_options(payload, options);
            let rgba = decoder
                .decode()
                .map_err(|e| CaptureError::Codec(e.to_string()))?;
            let info = decoder
                .info()
                .ok_or_else(|| CaptureError::Codec("missing JPEG dimensions".into()))?;
            Ok((info.width as u32, info.height as u32, rgba))
        }
        FrameEncoding::Lz4Rgba => {
            let raw = lz4_flex::decompress_size_prepended(payload)
                .map_err(|e| CaptureError::Codec(e.to_string()))?;
            // Dimension is supplied in VideoFrame header by caller; return 0x0 marker here.
            Ok((0, 0, raw))
        }
        FrameEncoding::HybridTiles | FrameEncoding::H264AnnexB => {
            Err(CaptureError::Codec("Frame encoding requires ViewerFrameCanvas".into()))
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
        GetDIBits, GetSysColor, ReleaseDC, SelectObject, SetSysColors, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, COLOR_DESKTOP, DIB_RGB_COLORS, SRCCOPY,
    };
    use windows::Win32::Foundation::{COLORREF, HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber,
        OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::StationsAndDesktops::{
        CloseDesktop, OpenInputDesktop, SetThreadDesktop, DESKTOP_ACCESS_FLAGS,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
        KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
        MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, VIRTUAL_KEY,
    };
    use windows::Win32::UI::Shell::{DragQueryFileW, DROPFILES, HDROP};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetCursorInfo, GetCursorPos, GetSystemMetrics, LoadCursorW, SystemParametersInfoW, CURSORINFO,
        CURSOR_SHOWING, IDC_ARROW, IDC_CROSS, IDC_HAND, IDC_IBEAM, IDC_SIZEALL, IDC_SIZENS,
        IDC_SIZEWE, IDC_WAIT, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN,
        SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SPIF_SENDCHANGE, SPI_GETDESKWALLPAPER,
        SPI_SETDESKWALLPAPER,
    };

    const CF_UNICODETEXT_U32: u32 = 13;
    const CF_HDROP_U32: u32 = 15;

    pub fn enumerate_monitors() -> Vec<MonitorInfo> {
        let mut out = Vec::new();
        unsafe {
            let mut device: Option<ID3D11Device> = None;
            if D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                None,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )
            .is_ok()
            {
                if let Some(dev) = device {
                    if let Ok(dxgi_dev) = dev.cast::<IDXGIDevice>() {
                        if let Ok(adapter) = dxgi_dev.GetAdapter() {
                            let mut idx = 0u32;
                            while let Ok(output) = adapter.EnumOutputs(idx) {
                                if let Ok(desc) = output.GetDesc() {
                                    let raw_name: Vec<u16> = desc
                                        .DeviceName
                                        .iter()
                                        .copied()
                                        .take_while(|&c| c != 0)
                                        .collect();
                                    let name = String::from_utf16_lossy(&raw_name);
                                    let w = (desc.DesktopCoordinates.right
                                        - desc.DesktopCoordinates.left)
                                        .max(1) as u32;
                                    let h = (desc.DesktopCoordinates.bottom
                                        - desc.DesktopCoordinates.top)
                                        .max(1) as u32;
                                    let primary = desc.DesktopCoordinates.left == 0
                                        && desc.DesktopCoordinates.top == 0;
                                    out.push(MonitorInfo {
                                        index: idx as usize,
                                        name: if name.is_empty() {
                                            format!("Monitor {}", idx + 1)
                                        } else {
                                            name
                                        },
                                        width: w,
                                        height: h,
                                        is_primary: primary,
                                    });
                                }
                                idx += 1;
                            }
                        }
                    }
                }
            }
            if out.is_empty() {
                let w = GetSystemMetrics(SM_CXSCREEN).max(1) as u32;
                let h = GetSystemMetrics(SM_CYSCREEN).max(1) as u32;
                out.push(MonitorInfo {
                    index: 0,
                    name: "Monitor Principal".into(),
                    width: w,
                    height: h,
                    is_primary: true,
                });
            }
        }
        out
    }

    /// Moves the calling thread to the input desktop only when it differs from the thread's
    /// current one. The handle needs DESKTOP_JOURNALPLAYBACK (all desktop rights, 0x1FF):
    /// with fewer rights SetThreadDesktop succeeds but every SendInput is silently dropped.
    fn follow_input_desktop_for_injection() {
        let env = crate::hostdiag::host_environment();
        match env.input_desktop.as_deref() {
            Some(name) if !name.eq_ignore_ascii_case(&env.thread_desktop) => unsafe {
                if let Ok(hdesk) = OpenInputDesktop(Default::default(), false, DESKTOP_ACCESS_FLAGS(0x01FF)) {
                    if SetThreadDesktop(hdesk).is_ok() {
                        crate::hostdiag::log(&format!(
                            "input thread moved from desktop {} to {name}",
                            env.thread_desktop
                        ));
                    } else {
                        let _ = CloseDesktop(hdesk);
                    }
                }
            },
            _ => {}
        }
    }

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
        origin_x: i32,
        origin_y: i32,
        last_rgba: Vec<u8>,
    }

    impl DxgiState {
        fn new_for_monitor(monitor_idx: usize) -> Result<Self, CaptureError> {
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
                    .EnumOutputs(monitor_idx as u32)
                    .or_else(|_| adapter.EnumOutputs(0))
                    .map_err(|e| CaptureError::Backend(format!("EnumOutputs({monitor_idx}): {e}")))?;
                let out_desc = output.GetDesc().ok();
                let origin_x = out_desc
                    .as_ref()
                    .map(|d| d.DesktopCoordinates.left)
                    .unwrap_or(0);
                let origin_y = out_desc
                    .as_ref()
                    .map(|d| d.DesktopCoordinates.top)
                    .unwrap_or(0);
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
                    origin_x,
                    origin_y,
                    last_rgba: vec![0u8; (width * height * 4) as usize],
                })
            }
        }

        /// Fast path for Level 1/2 streaming: returns `Ok(None)` immediately on timeout
        /// or cursor-only updates without touching pixel memory, and copies BGRA rows
        /// directly into `out_bgra` without any RGBA channel swap.
        fn capture_bgra_if_changed(
            &mut self,
            timeout_ms: u32,
            out_bgra: &mut Vec<u8>,
        ) -> Result<Option<(u32, u32)>, CaptureError> {
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
                        return Ok(None);
                    }
                    if e.code() == DXGI_ERROR_ACCESS_LOST {
                        return Err(CaptureError::Backend("DXGI_ERROR_ACCESS_LOST".into()));
                    }
                    return Err(CaptureError::Backend(format!("AcquireNextFrame: {e}")));
                }

                if frame_info.LastPresentTime == 0 && frame_info.TotalMetadataBufferSize == 0 {
                    let _ = self.duplication.ReleaseFrame();
                    return Ok(None);
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
                let row_bytes = w * 4;
                let src = std::slice::from_raw_parts(mapped.pData as *const u8, row_pitch * h);

                out_bgra.resize(row_bytes * h, 0);
                if row_pitch == row_bytes {
                    out_bgra.copy_from_slice(&src[..row_bytes * h]);
                } else {
                    for y in 0..h {
                        out_bgra[y * row_bytes..(y + 1) * row_bytes]
                            .copy_from_slice(&src[y * row_pitch..y * row_pitch + row_bytes]);
                    }
                }
                self.context.Unmap(&self.staging, 0);

                Ok(Some((self.width, self.height)))
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

    fn capture_gdi_bgra_into(out_bgra: &mut Vec<u8>) -> Result<(u32, u32), CaptureError> {
        unsafe {
            let w = GetSystemMetrics(SM_CXSCREEN).max(1) as u32;
            let h = GetSystemMetrics(SM_CYSCREEN).max(1) as u32;
            let hdc_screen = GetDC(None);
            let hdc_mem = CreateCompatibleDC(hdc_screen);
            let hbmp = CreateCompatibleBitmap(hdc_screen, w as i32, h as i32);
            let old = SelectObject(hdc_mem, hbmp);

            let ok = BitBlt(hdc_mem, 0, 0, w as i32, h as i32, hdc_screen, 0, 0, SRCCOPY);
            out_bgra.resize((w * h * 4) as usize, 0);

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
                    Some(out_bgra.as_mut_ptr() as *mut _),
                    &mut bi,
                    DIB_RGB_COLORS,
                );
                for px in out_bgra.chunks_exact_mut(4) {
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
            Ok((w, h))
        }
    }

    pub struct WindowsCapturer {
        dxgi: Option<DxgiState>,
        monitor_idx: usize,
        width: u32,
        height: u32,
        origin_x: i32,
        origin_y: i32,
        backend: CaptureBackend,
    }

    impl WindowsCapturer {
        pub fn new() -> Self {
            Self::new_for_monitor(0)
        }

        pub fn new_for_monitor(monitor_idx: usize) -> Self {
            attach_input_desktop();
            match DxgiState::new_for_monitor(monitor_idx) {
                Ok(dx) => {
                    let (w, h, ox, oy) = (dx.width, dx.height, dx.origin_x, dx.origin_y);
                    tracing::info!(width = w, height = h, monitor_idx, "DXGI Desktop Duplication initialized");
                    Self {
                        dxgi: Some(dx),
                        monitor_idx,
                        width: w,
                        height: h,
                        origin_x: ox,
                        origin_y: oy,
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
                        monitor_idx,
                        width: w,
                        height: h,
                        origin_x: 0,
                        origin_y: 0,
                        backend: CaptureBackend::Gdi,
                    }
                }
            }
        }

        pub fn switch_monitor(&mut self, monitor_idx: usize) -> bool {
            if let Ok(dx) = DxgiState::new_for_monitor(monitor_idx) {
                self.monitor_idx = monitor_idx;
                self.width = dx.width;
                self.height = dx.height;
                self.origin_x = dx.origin_x;
                self.origin_y = dx.origin_y;
                self.dxgi = Some(dx);
                self.backend = CaptureBackend::Dxgi;
                true
            } else {
                false
            }
        }

        /// Level 1/2 fast capture: avoids pixel copying on unchanged DXGI frames and writes
        /// native BGRA bytes directly into `out_bgra` without RGBA channel swapping.
        pub fn capture_bgra_if_changed(
            &mut self,
            timeout_ms: u32,
            out_bgra: &mut Vec<u8>,
        ) -> Result<Option<(u32, u32)>, CaptureError> {
            attach_input_desktop();
            if let Some(dx) = &mut self.dxgi {
                match dx.capture_bgra_if_changed(timeout_ms, out_bgra) {
                    Ok(res) => return Ok(res),
                    Err(e) => {
                        tracing::warn!(error = %e, "DXGI fast capture lost, retrying re-init");
                        if let Ok(mut fresh) = DxgiState::new_for_monitor(self.monitor_idx) {
                            self.width = fresh.width;
                            self.height = fresh.height;
                            self.origin_x = fresh.origin_x;
                            self.origin_y = fresh.origin_y;
                            let res = fresh.capture_bgra_if_changed(timeout_ms, out_bgra);
                            self.dxgi = Some(fresh);
                            if let Ok(opt) = res {
                                return Ok(opt);
                            }
                        }
                        self.dxgi = None;
                        self.backend = CaptureBackend::Gdi;
                    }
                }
            }
            capture_gdi_bgra_into(out_bgra).map(Some)
        }

        /// Reads the OS cursor position and standard system shape normalized to the active monitor.
        pub fn capture_cursor_state(&self) -> Option<(f32, f32, bool, CursorShape)> {
            unsafe {
                let mut ci = CURSORINFO {
                    cbSize: std::mem::size_of::<CURSORINFO>() as u32,
                    ..Default::default()
                };
                if GetCursorInfo(&mut ci).is_err() {
                    return None;
                }
                let visible = (ci.flags.0 & CURSOR_SHOWING.0) != 0;
                let w = self.width.max(1) as f32;
                let h = self.height.max(1) as f32;
                let x_norm = ((ci.ptScreenPos.x - self.origin_x) as f32 / w).clamp(0.0, 1.0);
                let y_norm = ((ci.ptScreenPos.y - self.origin_y) as f32 / h).clamp(0.0, 1.0);

                let shape = if !visible {
                    CursorShape::Hidden
                } else if LoadCursorW(None, IDC_IBEAM).ok() == Some(ci.hCursor) {
                    CursorShape::IBeam
                } else if LoadCursorW(None, IDC_HAND).ok() == Some(ci.hCursor) {
                    CursorShape::Hand
                } else if LoadCursorW(None, IDC_WAIT).ok() == Some(ci.hCursor) {
                    CursorShape::Wait
                } else if LoadCursorW(None, IDC_SIZENS).ok() == Some(ci.hCursor) {
                    CursorShape::SizeNS
                } else if LoadCursorW(None, IDC_SIZEWE).ok() == Some(ci.hCursor) {
                    CursorShape::SizeWE
                } else if LoadCursorW(None, IDC_SIZEALL).ok() == Some(ci.hCursor) {
                    CursorShape::SizeAll
                } else if LoadCursorW(None, IDC_CROSS).ok() == Some(ci.hCursor) {
                    CursorShape::Cross
                } else if LoadCursorW(None, IDC_ARROW).ok() == Some(ci.hCursor) {
                    CursorShape::Arrow
                } else {
                    CursorShape::Arrow
                };

                Some((x_norm, y_norm, visible, shape))
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

    /// Desktop rectangle `(left, top, width, height)` of the DXGI output `index`, in the same
    /// order and coordinate space the capturer uses (`EnumOutputs` on the default adapter).
    pub fn monitor_desktop_rect(index: usize) -> Option<(i32, i32, i32, i32)> {
        unsafe {
            let mut device: Option<ID3D11Device> = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                None,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )
            .ok()?;
            let adapter = device?.cast::<IDXGIDevice>().ok()?.GetAdapter().ok()?;
            // Same fallback as the capturer: an out-of-range index captures output 0.
            let output = adapter
                .EnumOutputs(index as u32)
                .or_else(|_| adapter.EnumOutputs(0))
                .ok()?;
            let r = output.GetDesc().ok()?.DesktopCoordinates;
            Some((r.left, r.top, (r.right - r.left).max(1), (r.bottom - r.top).max(1)))
        }
    }

    /// Monitor rectangle that normalized mouse coordinates from the viewer refer to.
    static INPUT_TARGET: std::sync::Mutex<Option<(i32, i32, i32, i32)>> =
        std::sync::Mutex::new(None);

    /// Points remote mouse input at the monitor the viewer is looking at. The viewer sends
    /// coordinates normalized to the captured monitor, not to the whole virtual desktop.
    pub fn set_input_target_monitor(index: usize) {
        let rect = monitor_desktop_rect(index);
        if let Ok(mut t) = INPUT_TARGET.lock() {
            *t = rect;
        }
    }

    fn input_target_rect() -> (i32, i32, i32, i32) {
        if let Ok(mut t) = INPUT_TARGET.lock() {
            if t.is_none() {
                *t = monitor_desktop_rect(0);
            }
            if let Some(r) = *t {
                return r;
            }
        }
        unsafe {
            (
                0,
                0,
                GetSystemMetrics(SM_CXSCREEN).max(1),
                GetSystemMetrics(SM_CYSCREEN).max(1),
            )
        }
    }

    /// Maps a viewer-normalized point to absolute screen pixels on the target monitor.
    fn norm_to_screen(x: f32, y: f32) -> (i32, i32) {
        let (left, top, w, h) = input_target_rect();
        let px = left + (x.clamp(0.0, 1.0) * (w - 1) as f32).round() as i32;
        let py = top + (y.clamp(0.0, 1.0) * (h - 1) as f32).round() as i32;
        (px, py)
    }

    /// True when the OS cursor sits on `target` (a few px of tolerance). Used after injecting a
    /// move to detect that Windows dropped it (UIPI: an elevated window is in front, or the
    /// secure desktop / lock screen is active).
    pub fn cursor_is_at(target: (i32, i32)) -> bool {
        unsafe {
            let mut p = windows::Win32::Foundation::POINT::default();
            if GetCursorPos(&mut p).is_err() {
                return true;
            }
            (p.x - target.0).abs() <= 3 && (p.y - target.1).abs() <= 3
        }
    }

    /// Injects a viewer event. For mouse moves returns `Some(screen_px)` with the target point
    /// so the caller can verify it with [`cursor_is_at`]; `None` for everything else.
    pub fn inject_input_event_checked(ev: &InputEvent) -> Option<(i32, i32)> {
        inject_input_event(ev);
        match ev {
            InputEvent::MouseMoveNorm { x, y } => Some(norm_to_screen(*x, *y)),
            _ => None,
        }
    }

    pub fn inject_input_event(ev: &InputEvent) {
        // SendInput only reaches the desktop that currently receives input. The injecting
        // thread (a tokio worker, no windows of its own) follows it, so input keeps working
        // after a lock/unlock or UAC prompt switched desktops. Re-checked at most twice a second.
        thread_local! {
            static LAST_ATTACH: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
        }
        LAST_ATTACH.with(|c| {
            if c.get().is_none_or(|t| t.elapsed() > std::time::Duration::from_millis(500)) {
                follow_input_desktop_for_injection();
                c.set(Some(std::time::Instant::now()));
            }
        });
        unsafe {
            match ev {
                InputEvent::UnicodeChar { ch } => {
                    let mut buf = [0u16; 2];
                    for code in ch.encode_utf16(&mut buf).iter().copied() {
                        let down = INPUT {
                            r#type: INPUT_KEYBOARD,
                            Anonymous: INPUT_0 {
                                ki: KEYBDINPUT {
                                    wVk: VIRTUAL_KEY(0),
                                    wScan: code,
                                    dwFlags: KEYEVENTF_UNICODE,
                                    time: 0,
                                    dwExtraInfo: 0,
                                },
                            },
                        };
                        let up = INPUT {
                            r#type: INPUT_KEYBOARD,
                            Anonymous: INPUT_0 {
                                ki: KEYBDINPUT {
                                    wVk: VIRTUAL_KEY(0),
                                    wScan: code,
                                    dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                                    time: 0,
                                    dwExtraInfo: 0,
                                },
                            },
                        };
                        let _ = SendInput(&[down, up], std::mem::size_of::<INPUT>() as i32);
                    }
                    return;
                }
                _ => {}
            }
            let input = match ev {
                InputEvent::MouseMoveNorm { x, y } => {
                    // MOUSEEVENTF_VIRTUALDESK spans every monitor: convert the monitor-relative
                    // point to virtual-desktop units, or a multi-monitor host gets the click
                    // somewhere else than where the viewer pointed.
                    let (px, py) = norm_to_screen(*x, *y);
                    let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
                    let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
                    let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(2);
                    let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(2);
                    let nx = (((px - vx) as i64 * 65535 + (vw as i64 - 1) / 2) / (vw as i64 - 1))
                        .clamp(0, 65535) as i32;
                    let ny = (((py - vy) as i64 * 65535 + (vh as i64 - 1) / 2) / (vh as i64 - 1))
                        .clamp(0, 65535) as i32;
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
                InputEvent::UnicodeChar { .. } => unreachable!(),
            };
            let _ = SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
        }
    }

    pub fn inject_special_combo(combo: &str) {
        let key_evt = |vk: u16, pressed: bool| {
            inject_input_event(&InputEvent::Key { vk, pressed });
        };
        match combo {
            "Win" => {
                key_evt(0x5B, true);
                key_evt(0x5B, false);
            }
            "Alt+Tab" => {
                key_evt(0x12, true);
                key_evt(0x09, true);
                key_evt(0x09, false);
                key_evt(0x12, false);
            }
            "Ctrl+Shift+Esc" | "Ctrl+Alt+Del" => {
                key_evt(0x11, true);
                key_evt(0x10, true);
                key_evt(0x1B, true);
                key_evt(0x1B, false);
                key_evt(0x10, false);
                key_evt(0x11, false);
            }
            _ => {}
        }
    }

    pub fn read_clipboard_text() -> Option<String> {
        unsafe {
            if OpenClipboard(None).is_err() {
                return None;
            }
            let result = (|| {
                let handle = GetClipboardData(CF_UNICODETEXT_U32).ok()?;
                let hmem = HGLOBAL(handle.0);
                let ptr = GlobalLock(hmem) as *const u16;
                if ptr.is_null() {
                    return None;
                }
                let mut len = 0usize;
                while *ptr.add(len) != 0 && len < 65536 {
                    len += 1;
                }
                let slice = std::slice::from_raw_parts(ptr, len);
                let text = String::from_utf16_lossy(slice);
                let _ = GlobalUnlock(hmem);
                Some(text)
            })();
            let _ = CloseClipboard();
            result
        }
    }

    pub fn write_clipboard_text(text: &str) -> bool {
        unsafe {
            let mut wide: Vec<u16> = text.encode_utf16().collect();
            wide.push(0);
            let byte_len = wide.len() * std::mem::size_of::<u16>();
            let Ok(hmem) = GlobalAlloc(GMEM_MOVEABLE, byte_len) else {
                return false;
            };
            let ptr = GlobalLock(hmem) as *mut u16;
            if ptr.is_null() {
                return false;
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
            let _ = GlobalUnlock(hmem);

            if OpenClipboard(None).is_err() {
                return false;
            }
            let _ = EmptyClipboard();
            let ok = SetClipboardData(CF_UNICODETEXT_U32, HANDLE(hmem.0)).is_ok();
            let _ = CloseClipboard();
            ok
        }
    }

    pub fn clipboard_sequence_number() -> u32 {
        unsafe { GetClipboardSequenceNumber() }
    }

    pub fn read_clipboard_files() -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        unsafe {
            if OpenClipboard(None).is_err() {
                return out;
            }
            if let Ok(handle) = GetClipboardData(CF_HDROP_U32) {
                let hdrop = HDROP(handle.0);
                let count = DragQueryFileW(hdrop, u32::MAX, None);
                let mut total_bytes = 0u64;
                for i in 0..count.min(32) {
                    let needed = DragQueryFileW(hdrop, i, None);
                    if needed > 0 && needed < 1024 {
                        let mut buf = vec![0u16; needed as usize + 1];
                        let copied = DragQueryFileW(hdrop, i, Some(&mut buf));
                        if copied > 0 {
                            let p = std::path::PathBuf::from(String::from_utf16_lossy(
                                &buf[..copied as usize],
                            ));
                            if let Ok(meta) = std::fs::metadata(&p) {
                                if meta.is_file()
                                    && total_bytes.saturating_add(meta.len()) <= 64 * 1024 * 1024
                                {
                                    total_bytes += meta.len();
                                    out.push(p);
                                }
                            }
                        }
                    }
                }
            }
            let _ = CloseClipboard();
        }
        out
    }

    pub fn write_clipboard_files(paths: &[std::path::PathBuf]) -> bool {
        if paths.is_empty() {
            return false;
        }
        let mut wide_list: Vec<u16> = Vec::new();
        for p in paths {
            wide_list.extend(p.to_string_lossy().encode_utf16());
            wide_list.push(0);
        }
        wide_list.push(0);

        let header_size = std::mem::size_of::<DROPFILES>();
        let total_bytes = header_size + wide_list.len() * std::mem::size_of::<u16>();
        unsafe {
            let Ok(hmem) = GlobalAlloc(GMEM_MOVEABLE, total_bytes) else {
                return false;
            };
            let ptr = GlobalLock(hmem) as *mut u8;
            if ptr.is_null() {
                return false;
            }
            std::ptr::write_bytes(ptr, 0, total_bytes);
            let df = DROPFILES {
                pFiles: header_size as u32,
                pt: Default::default(),
                fNC: false.into(),
                fWide: true.into(),
            };
            std::ptr::copy_nonoverlapping(&df as *const DROPFILES as *const u8, ptr, header_size);
            std::ptr::copy_nonoverlapping(
                wide_list.as_ptr() as *const u8,
                ptr.add(header_size),
                wide_list.len() * 2,
            );
            let _ = GlobalUnlock(hmem);

            if OpenClipboard(None).is_err() {
                return false;
            }
            let _ = EmptyClipboard();
            let ok = SetClipboardData(CF_HDROP_U32, HANDLE(hmem.0)).is_ok();
            let _ = CloseClipboard();
            ok
        }
    }

    /// Temporarily switches the Windows desktop background to solid black during an active
    /// remote session for lower entropy/bandwidth and faster encoding, restoring on Drop.
    pub struct WallpaperBlackoutGuard {
        original_wallpaper: Vec<u16>,
        original_color: u32,
        active: bool,
    }

    impl WallpaperBlackoutGuard {
        /// Changes the wallpaper on a helper thread. `SPI_SETDESKWALLPAPER` + `SetSysColors`
        /// broadcast WM_SETTINGCHANGE synchronously to every top-level window and can block for
        /// seconds when any app is slow to answer; doing it inline stalled the session start
        /// (the viewer sat on "Negociando túnel E2EE"). The original wallpaper is restored when
        /// the returned handle is dropped.
        pub fn activate_in_background() -> crate::BackgroundWallpaperGuard {
            crate::BackgroundWallpaperGuard::start()
        }

        pub fn activate() -> Self {
            unsafe {
                let mut path_buf = vec![0u16; 520];
                let got_wp = SystemParametersInfoW(
                    SPI_GETDESKWALLPAPER,
                    path_buf.len() as u32,
                    Some(path_buf.as_mut_ptr() as *mut _),
                    Default::default(),
                )
                .is_ok();
                let orig_color = GetSysColor(COLOR_DESKTOP);

                let empty: [u16; 1] = [0];
                let _ = SystemParametersInfoW(
                    SPI_SETDESKWALLPAPER,
                    0,
                    Some(empty.as_ptr() as *mut _),
                    SPIF_SENDCHANGE,
                );
                let elem = [COLOR_DESKTOP.0];
                let black = [COLORREF(0x0000_0000)];
                let _ = SetSysColors(1, elem.as_ptr(), black.as_ptr());

                if !got_wp {
                    path_buf.clear();
                }
                Self {
                    original_wallpaper: path_buf,
                    original_color: orig_color,
                    active: true,
                }
            }
        }
    }

    impl Drop for WallpaperBlackoutGuard {
        fn drop(&mut self) {
            if !self.active {
                return;
            }
            unsafe {
                let elem = [COLOR_DESKTOP.0];
                let cols = [COLORREF(self.original_color)];
                let _ = SetSysColors(1, elem.as_ptr(), cols.as_ptr());
                if !self.original_wallpaper.is_empty() {
                    let _ = SystemParametersInfoW(
                        SPI_SETDESKWALLPAPER,
                        0,
                        Some(self.original_wallpaper.as_ptr() as *mut _),
                        SPIF_SENDCHANGE,
                    );
                }
            }
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
        pub fn new_for_monitor(_idx: usize) -> Self {
            Self
        }
        pub fn switch_monitor(&mut self, _idx: usize) -> bool {
            false
        }
        pub fn capture_bgra_if_changed(
            &mut self,
            _timeout_ms: u32,
            out_bgra: &mut Vec<u8>,
        ) -> Result<Option<(u32, u32)>, CaptureError> {
            out_bgra.resize(640 * 360 * 4, 32);
            Ok(Some((640, 360)))
        }
        pub fn capture_cursor_state(&self) -> Option<(f32, f32, bool, CursorShape)> {
            None
        }
    }
    pub fn enumerate_monitors() -> Vec<MonitorInfo> {
        vec![MonitorInfo {
            index: 0,
            name: "Stub Monitor".into(),
            width: 640,
            height: 360,
            is_primary: true,
        }]
    }
    pub fn inject_special_combo(_combo: &str) {}
    pub fn read_clipboard_text() -> Option<String> {
        None
    }
    pub fn write_clipboard_text(_text: &str) -> bool {
        false
    }
    pub fn clipboard_sequence_number() -> u32 {
        0
    }
    pub fn read_clipboard_files() -> Vec<std::path::PathBuf> {
        Vec::new()
    }
    pub fn write_clipboard_files(_paths: &[std::path::PathBuf]) -> bool {
        false
    }
    pub struct WallpaperBlackoutGuard;
    impl WallpaperBlackoutGuard {
        pub fn activate() -> Self {
            Self
        }
        pub fn activate_in_background() -> crate::BackgroundWallpaperGuard {
            crate::BackgroundWallpaperGuard::start()
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
    pub fn inject_input_event_checked(_ev: &InputEvent) -> Option<(i32, i32)> {
        None
    }
    pub fn cursor_is_at(_target: (i32, i32)) -> bool {
        true
    }
    pub fn set_input_target_monitor(_index: usize) {}
    pub fn monitor_desktop_rect(_index: usize) -> Option<(i32, i32, i32, i32)> {
        None
    }
    pub fn current_process_private_mb() -> f64 {
        0.0
    }
}

/// Injects viewer input on the host and notices when Windows silently drops it (UIPI blocks
/// injected input while an elevated window is in front, and the lock screen / UAC prompt runs
/// on the secure desktop). Everything is also logged to the host session log.
pub struct RemoteInputInjector {
    misses: u32,
    last_notice: Option<std::time::Instant>,
    env: hostdiag::HostEnvironment,
    stats_since: std::time::Instant,
    st_events: u64,
    st_moves: u64,
    st_moves_ok: u64,
}

impl Default for RemoteInputInjector {
    fn default() -> Self {
        Self::new()
    }
}

impl RemoteInputInjector {
    pub fn new() -> Self {
        win::set_input_target_monitor(0);
        let env = hostdiag::host_environment();
        hostdiag::log(&format!(
            "session start v{} | {} | target_rect={:?}",
            env!("CARGO_PKG_VERSION"),
            env.summary(),
            win::monitor_desktop_rect(0)
        ));
        Self {
            misses: 0,
            last_notice: None,
            env,
            stats_since: std::time::Instant::now(),
            st_events: 0,
            st_moves: 0,
            st_moves_ok: 0,
        }
    }

    /// Problems detected at session start that will stop remote control, as one line for the
    /// viewer (`None` when the host looks healthy).
    pub fn startup_notice(&self) -> Option<String> {
        let p = self.env.problems();
        if p.is_empty() {
            None
        } else {
            Some(format!("\u{26A0} Control remoto: {}.", p.join("; ")))
        }
    }

    /// The viewer switched monitors: normalized coordinates now refer to `index`.
    pub fn set_monitor(&self, index: usize) {
        win::set_input_target_monitor(index);
        hostdiag::log(&format!(
            "select monitor {index} -> rect {:?}",
            win::monitor_desktop_rect(index)
        ));
    }

    fn flush_stats(&mut self) {
        if self.stats_since.elapsed() < std::time::Duration::from_secs(5) || self.st_events == 0 {
            return;
        }
        let env = hostdiag::host_environment();
        hostdiag::log(&format!(
            "input: {} events, {} moves, {} landed | {}",
            self.st_events,
            self.st_moves,
            self.st_moves_ok,
            env.summary()
        ));
        self.st_events = 0;
        self.st_moves = 0;
        self.st_moves_ok = 0;
        self.stats_since = std::time::Instant::now();
    }

    pub fn inject(&mut self, ev: &agentdesk_protocol::InputEvent) -> Option<String> {
        self.st_events += 1;
        let Some(target) = win::inject_input_event_checked(ev) else {
            self.flush_stats();
            return None;
        };
        self.st_moves += 1;
        if win::cursor_is_at(target) {
            self.st_moves_ok += 1;
            self.misses = 0;
            self.flush_stats();
            return None;
        }
        self.misses += 1;
        self.flush_stats();
        let recently = self
            .last_notice
            .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(30));
        if self.misses >= 4 && !recently {
            self.last_notice = Some(std::time::Instant::now());
            self.misses = 0;
            let env = hostdiag::host_environment();
            hostdiag::log(&format!("input dropped by Windows at {target:?} | {}", env.summary()));
            let mut extra = env.problems().join("; ");
            if extra.is_empty() {
                extra = "probablemente una ventana de administrador en primer plano".into();
            }
            return Some(format!(
                "\u{26A0} Windows está bloqueando el mouse y el teclado remotos ({extra}). \
                 Cerrala desde el equipo o ejecutá AgentDesk como administrador allí."
            ));
        }
        None
    }
}

/// Owns a helper thread that holds a [`WallpaperBlackoutGuard`]; dropping it restores the
/// wallpaper on that same thread, so neither activation nor restore blocks the caller.
pub struct BackgroundWallpaperGuard {
    _release: std::sync::mpsc::Sender<()>,
}

impl BackgroundWallpaperGuard {
    pub fn start() -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let guard = win::WallpaperBlackoutGuard::activate();
            // Returns Err once the owner drops the sender.
            let _ = rx.recv();
            drop(guard);
        });
        Self { _release: tx }
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
