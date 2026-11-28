//! Manual probe: shows the whiteboard overlay with a stroke and injects mouse moves through
//! the same path the hosts use. Moves the real mouse. `cargo run -p agentdesk-capture --example wb_probe`
use agentdesk_capture::{RemoteInputInjector, WhiteboardOverlay};
use agentdesk_protocol::{InputEvent, WhiteboardStroke, WhiteboardTool};

fn main() {
    let wb = WhiteboardOverlay::new();
    wb.set_visible(true);
    wb.add_stroke(WhiteboardStroke {
        id: 1,
        tool: WhiteboardTool::Pen,
        color_rgba: 0xFF0000FF,
        thickness: 12.0,
        points: vec![(0.1, 0.1), (0.5, 0.5), (0.9, 0.1)],
    });
    let mut inj = RemoteInputInjector::new();
    println!("startup notice: {:?}", inj.startup_notice());
    for (x, y) in [(0.25f32, 0.25f32), (0.75, 0.5), (0.5, 0.75)] {
        // Run on a fresh thread each time, like tokio workers do.
        let notice = std::thread::scope(|s| s.spawn(|| inj.inject(&InputEvent::MouseMoveNorm { x, y })).join().unwrap());
        let mut p = windows::Win32::Foundation::POINT::default();
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p);
        }
        println!("move ({x},{y}) -> cursor {},{} notice={notice:?}", p.x, p.y);
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    let secs: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(3);
    std::thread::sleep(std::time::Duration::from_secs(secs));
}
