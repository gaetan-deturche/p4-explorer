//! Bringing another program's window to the front: the Unreal Editor's diff
//! window. Windows lets only the foreground program hand the foreground over,
//! and an editor reached by remote execution never is — its new window opened
//! behind Auger. Auger, the program the user just clicked, gives it away.

#[cfg(windows)]
mod win {
    type Hwnd = isize;
    type Bool = i32;

    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(cb: extern "system" fn(Hwnd, isize) -> Bool, lparam: isize) -> Bool;
        fn GetWindowThreadProcessId(hwnd: Hwnd, pid: *mut u32) -> u32;
        fn IsWindowVisible(hwnd: Hwnd) -> Bool;
        fn GetWindowTextLengthW(hwnd: Hwnd) -> i32;
        fn IsIconic(hwnd: Hwnd) -> Bool;
        fn ShowWindow(hwnd: Hwnd, cmd: i32) -> Bool;
        fn SetForegroundWindow(hwnd: Hwnd) -> Bool;
        fn AllowSetForegroundWindow(pid: u32) -> Bool;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: Bool, pid: u32) -> isize;
        fn QueryFullProcessImageNameW(process: isize, flags: u32, buf: *mut u16, size: *mut u32) -> Bool;
        fn CloseHandle(handle: isize) -> Bool;
    }
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const SW_RESTORE: i32 = 9;
    const ASFW_ANY: u32 = u32::MAX;

    fn exe_name(pid: u32) -> String {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h == 0 {
                return String::new();
            }
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
            CloseHandle(h);
            if ok == 0 {
                return String::new();
            }
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            path.rsplit(['\\', '/']).next().unwrap_or("").to_string()
        }
    }

    extern "system" fn collect(hwnd: Hwnd, out: isize) -> Bool {
        unsafe {
            if IsWindowVisible(hwnd) != 0 && GetWindowTextLengthW(hwnd) > 0 {
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, &mut pid);
                (*(out as *mut Vec<(isize, u32)>)).push((hwnd, pid));
            }
        }
        1
    }

    /// Visible, titled top-level windows of every process running `exe`, in
    /// z-order (front first), as (window, process id).
    pub fn windows_of(exe: &str) -> Vec<(isize, u32)> {
        let mut all: Vec<(isize, u32)> = Vec::new();
        unsafe {
            EnumWindows(collect, &mut all as *mut _ as isize);
        }
        let mut names: std::collections::HashMap<u32, bool> = std::collections::HashMap::new();
        all.into_iter()
            .filter(|(_, pid)| *names.entry(*pid).or_insert_with(|| exe_name(*pid).eq_ignore_ascii_case(exe)))
            .collect()
    }

    pub fn raise(hwnd: isize) -> bool {
        unsafe {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            SetForegroundWindow(hwnd) != 0
        }
    }

    /// Let `pid` (or any process, for `None`) take the foreground once.
    pub fn allow(pid: Option<u32>) {
        unsafe {
            AllowSetForegroundWindow(pid.unwrap_or(ASFW_ANY));
        }
    }
}

const UNREAL: &str = "UnrealEditor.exe";

/// The Unreal Editor's windows before a request that may open a new one.
pub struct UnrealWindows(Vec<(isize, u32)>);

/// Note what is open now, and let the editor take the foreground itself too.
pub fn before_unreal_request() -> UnrealWindows {
    #[cfg(windows)]
    {
        win::allow(None);
        UnrealWindows(win::windows_of(UNREAL))
    }
    #[cfg(not(windows))]
    UnrealWindows(Vec::new())
}

/// Bring forward the window the request opened, watching for it a few seconds;
/// if none appeared (the asset opened in a tab of an existing window), the
/// editor's front window — when there is a single editor to choose from.
pub fn raise_unreal_after(before: UnrealWindows) {
    // On its own thread: the diff command returns without waiting for the window.
    #[cfg(windows)]
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let now = win::windows_of(UNREAL);
            if let Some(&(hwnd, _)) = now.iter().find(|w| !before.0.iter().any(|b| b.0 == w.0)) {
                win::raise(hwnd);
                return;
            }
            if std::time::Instant::now() >= deadline {
                let mut pids: Vec<u32> = now.iter().map(|w| w.1).collect();
                pids.sort_unstable();
                pids.dedup();
                if pids.len() == 1 {
                    win::raise(now[0].0);
                }
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });
    #[cfg(not(windows))]
    let _ = before;
}

/// A freshly launched editor: let it take the foreground when its window opens.
pub fn allow_launched(pid: u32) {
    #[cfg(windows)]
    win::allow(Some(pid));
    #[cfg(not(windows))]
    let _ = pid;
}
