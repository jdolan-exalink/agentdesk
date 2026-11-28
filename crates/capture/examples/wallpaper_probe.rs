//! Checks the wallpaper blackout: prints the wallpaper path before/during/after.
use agentdesk_capture::win::WallpaperBlackoutGuard;
use windows::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETDESKWALLPAPER};

fn wp() -> String {
    let mut b = vec![0u16; 520];
    unsafe {
        let _ = SystemParametersInfoW(SPI_GETDESKWALLPAPER, b.len() as u32, Some(b.as_mut_ptr() as *mut _), Default::default());
    }
    let n = b.iter().position(|&c| c == 0).unwrap_or(0);
    String::from_utf16_lossy(&b[..n])
}

fn main() {
    println!("before: {:?}", wp());
    let g = WallpaperBlackoutGuard::activate();
    println!("during: {:?}", wp());
    if std::env::args().any(|a| a == "crash") {
        std::process::exit(0);
    }
    std::thread::sleep(std::time::Duration::from_secs(2));
    drop(g);
    println!("after:  {:?}", wp());
}
