//! What Windows can tell about the foreground window and the user's
//! presence: program, title, idle time, fullscreen, "something keeps the
//! screen on". Read-only; nothing here changes the system.

use crate::sys::{from_wide, wide};
use std::ffi::c_void;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::System::Power::CallNtPowerInformation;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    IsWindowVisible,
};

/// The window in front.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Foreground {
    pub hwnd: isize,
    pub pid: u32,
    /// Full path of the program, when it could be read.
    pub path: String,
    /// Lower-case file name (`code.exe`).
    pub exe: String,
}

fn hwnd(raw: isize) -> HWND {
    HWND(raw as *mut c_void)
}

/// Full path of a process image. Works for elevated processes too
/// (limited-information access).
pub fn process_path(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    // SAFETY: the handle is closed before returning; the buffer outlives the call.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(handle);
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// `C:\Apps\Code.exe` -> `code.exe`
pub fn file_name_lower(path: &str) -> String {
    path.rsplit(['\\', '/'])
        .next()
        .unwrap_or(path)
        .to_lowercase()
}

fn window_pid(window: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: plain query with a valid out pointer.
    unsafe {
        GetWindowThreadProcessId(window, Some(&mut pid));
    }
    pid
}

struct ChildSearch {
    host_pid: u32,
    found: u32,
}

unsafe extern "system" fn find_other_pid(child: HWND, lparam: LPARAM) -> BOOL {
    let search = &mut *(lparam.0 as *mut ChildSearch);
    let pid = window_pid(child);
    if pid != 0 && pid != search.host_pid {
        search.found = pid;
        return BOOL(0);
    }
    BOOL(1)
}

/// The foreground window and its program. Store apps are hosted by a frame
/// process; the real program is the owner of one of its child windows.
pub fn foreground() -> Option<Foreground> {
    // SAFETY: read-only window queries.
    let window = unsafe { GetForegroundWindow() };
    if window.0.is_null() {
        return None;
    }
    describe(window.0 as isize)
}

/// Program information for any top-level window.
pub fn describe(raw: isize) -> Option<Foreground> {
    let window = hwnd(raw);
    let mut pid = window_pid(window);
    if pid == 0 {
        return None;
    }
    let mut path = process_path(pid).unwrap_or_default();
    if file_name_lower(&path) == "applicationframehost.exe" {
        let mut search = ChildSearch {
            host_pid: pid,
            found: 0,
        };
        // SAFETY: the callback only runs during this call and `search` outlives it.
        unsafe {
            let _ = EnumChildWindows(
                window,
                Some(find_other_pid),
                LPARAM(&mut search as *mut ChildSearch as isize),
            );
        }
        if search.found != 0 {
            if let Some(real) = process_path(search.found) {
                pid = search.found;
                path = real;
            }
        }
    }
    let exe = file_name_lower(&path);
    Some(Foreground {
        hwnd: raw,
        pid,
        path,
        exe,
    })
}

/// The title of a window (empty when it has none or it cannot be read).
pub fn window_title(raw: isize) -> String {
    let mut buf = [0u16; 512];
    // SAFETY: the buffer is valid for its length.
    let n = unsafe { GetWindowTextW(hwnd(raw), &mut buf) };
    if n <= 0 {
        return String::new();
    }
    from_wide(&buf[..n as usize]).trim().to_string()
}

/// Seconds since the last keyboard or mouse input in this session.
pub fn idle_seconds() -> u32 {
    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: valid structure with its size set.
    unsafe {
        if !GetLastInputInfo(&mut info).as_bool() {
            return 0;
        }
        GetTickCount().wrapping_sub(info.dwTime) / 1000
    }
}

/// Whether a fullscreen program, a presentation or a fullscreen game is in front.
pub fn fullscreen_in_front() -> bool {
    // SAFETY: simple query.
    match unsafe { SHQueryUserNotificationState() } {
        Ok(state) => {
            state == QUNS_BUSY
                || state == QUNS_RUNNING_D3D_FULL_SCREEN
                || state == QUNS_PRESENTATION_MODE
        }
        Err(_) => false,
    }
}

/// Whether some program asked Windows to keep the display on (video players
/// and call apps do this while playing).
pub fn display_required() -> bool {
    const SYSTEM_EXECUTION_STATE: i32 = 16;
    const ES_DISPLAY_REQUIRED: u32 = 0x2;
    let mut state = 0u32;
    // SAFETY: the output buffer is a u32, as this information level requires.
    let status = unsafe {
        CallNtPowerInformation(
            windows::Win32::System::Power::POWER_INFORMATION_LEVEL(SYSTEM_EXECUTION_STATE),
            None,
            0,
            Some(&mut state as *mut u32 as *mut c_void),
            std::mem::size_of::<u32>() as u32,
        )
    };
    status.0 == 0 && state & ES_DISPLAY_REQUIRED != 0
}

/// The friendly name of a program from its version information
/// (`FileDescription`), if it has one.
pub fn file_description(path: &str) -> Option<String> {
    let wpath = wide(path);
    // SAFETY: buffers are sized by the API's own answer and outlive the calls.
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(wpath.as_ptr()), None);
        if size == 0 || size > 4 * 1024 * 1024 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(
            PCWSTR(wpath.as_ptr()),
            0,
            size,
            data.as_mut_ptr() as *mut c_void,
        )
        .ok()?;
        // The languages the file has strings for.
        let mut ptr: *mut c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let key = wide("\\VarFileInfo\\Translation");
        let mut langs: Vec<(u16, u16)> = Vec::new();
        if VerQueryValueW(
            data.as_ptr() as *const c_void,
            PCWSTR(key.as_ptr()),
            &mut ptr,
            &mut len,
        )
        .as_bool()
            && !ptr.is_null()
        {
            let pairs = std::slice::from_raw_parts(ptr as *const u16, len as usize / 2);
            for pair in pairs.as_chunks::<2>().0 {
                langs.push((pair[0], pair[1]));
            }
        }
        langs.push((0x0409, 1200));
        langs.push((0x0409, 1252));
        langs.push((0, 1200));
        for (lang, codepage) in langs {
            let key = wide(&format!(
                "\\StringFileInfo\\{lang:04x}{codepage:04x}\\FileDescription"
            ));
            let mut ptr: *mut c_void = std::ptr::null_mut();
            let mut len = 0u32;
            if VerQueryValueW(
                data.as_ptr() as *const c_void,
                PCWSTR(key.as_ptr()),
                &mut ptr,
                &mut len,
            )
            .as_bool()
                && !ptr.is_null()
                && len > 0
            {
                let text = std::slice::from_raw_parts(ptr as *const u16, len as usize);
                let name = from_wide(text).trim().to_string();
                if !name.is_empty() {
                    return Some(name);
                }
            }
        }
        None
    }
}

/// A presentable name for a program without version information:
/// `my-tool.exe` -> `My-tool`.
pub fn name_from_exe(exe: &str) -> String {
    let stem = exe.rsplit_once('.').map(|(s, _)| s).unwrap_or(exe);
    let mut chars = stem.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "Unknown".to_string(),
    }
}

unsafe extern "system" fn collect_visible(window: HWND, lparam: LPARAM) -> BOOL {
    let list = &mut *(lparam.0 as *mut Vec<isize>);
    if IsWindowVisible(window).as_bool() {
        list.push(window.0 as isize);
    }
    BOOL(1)
}

/// Every visible top-level window (used by `--probe`).
pub fn visible_windows() -> Vec<isize> {
    let mut list: Vec<isize> = Vec::new();
    // SAFETY: the callback only runs during this call and `list` outlives it.
    unsafe {
        let _ = EnumWindows(
            Some(collect_visible),
            LPARAM(&mut list as *mut Vec<isize> as isize),
        );
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(file_name_lower("C:\\Apps\\Code.EXE"), "code.exe");
        assert_eq!(file_name_lower("tool.exe"), "tool.exe");
        assert_eq!(name_from_exe("my-tool.exe"), "My-tool");
        assert_eq!(name_from_exe(""), "Unknown");
    }

    #[test]
    fn own_process_and_system_queries() {
        // Our own process: the path must be this test binary.
        let path = process_path(std::process::id()).expect("own path");
        assert!(path.to_lowercase().ends_with(".exe"));
        assert!(process_path(0).is_none());
        // These must not fail or panic whatever the desktop is doing; their
        // values are not printed (they describe the real PC).
        let _ = idle_seconds();
        let _ = fullscreen_in_front();
        let _ = display_required();
        let _ = foreground();
        assert_eq!(window_title(0), "");
        // A system program with version information.
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:/Windows".into());
        let name = file_description(&format!("{root}/System32/notepad.exe"))
            .or_else(|| file_description(&format!("{root}/explorer.exe")));
        assert!(name.is_some_and(|n| !n.is_empty()));
        assert!(file_description("C:/missing/nothing.exe").is_none());
    }
}
