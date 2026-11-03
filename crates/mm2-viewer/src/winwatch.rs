//! Windows only: notes in the log (stderr, which the launcher keeps in
//! `last-run.log`) every other window that comes up over the game's, with
//! its class, title, program and place, and when the game loses focus. A
//! tester saw a black box flash over the game every 20 s or so; this says
//! what it is.

/// Starts the watch on the game's window (its `HWND`).
pub fn start(hwnd: isize) {
    let _ = std::thread::Builder::new()
        .name("window watch".into())
        .spawn(move || watch(hwnd));
}

fn watch(game: isize) {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId,
        IsWindowVisible,
    };

    struct Scan {
        game: isize,
        above: Vec<isize>,
        done: bool,
    }
    unsafe extern "system" fn each(hwnd: HWND, data: LPARAM) -> i32 {
        let scan = unsafe { &mut *(data as *mut Scan) };
        if hwnd as isize == scan.game {
            scan.done = true;
            return 0;
        }
        if unsafe { IsWindowVisible(hwnd) } != 0 {
            scan.above.push(hwnd as isize);
        }
        1
    }
    let rect = |h: isize| {
        let mut r = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        unsafe { GetWindowRect(h as HWND, &mut r) };
        r
    };
    let text = |h: isize, class: bool| {
        let mut buf = [0u16; 256];
        let n = unsafe {
            if class {
                GetClassNameW(h as HWND, buf.as_mut_ptr(), buf.len() as i32)
            } else {
                GetWindowTextW(h as HWND, buf.as_mut_ptr(), buf.len() as i32)
            }
        };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    };
    let program = |h: isize| {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(h as HWND, &mut pid) };
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return format!("pid {pid}");
        }
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = unsafe {
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len)
        };
        unsafe { CloseHandle(process) };
        if ok == 0 {
            return format!("pid {pid}");
        }
        format!(
            "{} (pid {pid})",
            String::from_utf16_lossy(&buf[..len as usize])
        )
    };
    let start = Instant::now();
    let mut seen: HashMap<isize, Instant> = HashMap::new();
    eprintln!("window watch: on");
    loop {
        std::thread::sleep(Duration::from_millis(15));
        let mut scan = Scan {
            game,
            above: Vec::new(),
            done: false,
        };
        unsafe { EnumWindows(Some(each), &mut scan as *mut Scan as LPARAM) };
        if !scan.done {
            continue;
        }
        let g = rect(game);
        for h in scan.above {
            let r = rect(h);
            let over = r.left < g.right && r.right > g.left && r.top < g.bottom && r.bottom > g.top;
            if !over || r.right <= r.left || r.bottom <= r.top {
                continue;
            }
            let now = Instant::now();
            let fresh = seen
                .get(&h)
                .is_none_or(|t| now.duration_since(*t) > Duration::from_secs(2));
            seen.insert(h, now);
            if fresh {
                eprintln!(
                    "window watch {:.2}s: over the game: class \"{}\", title \"{}\", {}, at {},{} {}x{} (game {},{} {}x{})",
                    start.elapsed().as_secs_f32(),
                    text(h, true),
                    text(h, false),
                    program(h),
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    g.left,
                    g.top,
                    g.right - g.left,
                    g.bottom - g.top,
                );
            }
        }
    }
}
