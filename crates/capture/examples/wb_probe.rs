//! Manual probe: shows the whiteboard overlay with a stroke and injects a mouse move.
use agentdesk_capture::{win::inject_input_event, WhiteboardOverlay};
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
    inject_input_event(&InputEvent::MouseMoveNorm { x: 0.25, y: 0.25 });
    std::thread::sleep(std::time::Duration::from_millis(200));
    #[cfg(windows)]
    unsafe {
        let mut p = windows::Win32::Foundation::POINT::default();
        let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p);
        println!("cursor after inject: {},{}", p.x, p.y);
    }
    let secs: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(6);
    println!("overlay visible for {secs}s");
    std::thread::sleep(std::time::Duration::from_secs(secs));
}
