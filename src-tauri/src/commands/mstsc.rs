//! Windows Remote Desktop (mstsc.exe) automation.
//!
//! Mirrors the Sangfor module's three-step flow: launch-or-focus, inject
//! an IP/hostname into the "Computer" field, and click "Connect". Shares
//! Win32 plumbing with the sangfor module in spirit; kept standalone for
//! now to minimise blast radius — we can refactor to a shared win32
//! module once a third consumer appears.

use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LaunchResult {
    pub launched: bool,
    pub message: String,
}

const MSTSC_PROCESS_NAME: &str = "mstsc.exe";

#[tauri::command]
#[specta::specta]
pub fn launch_mstsc() -> Result<LaunchResult, String> {
    #[cfg(target_os = "windows")]
    {
        platform::launch_mstsc_impl()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err("mstsc automation is only supported on Windows".into())
    }
}

#[tauri::command]
#[specta::specta]
pub fn inject_mstsc_ip(ip: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        platform::inject_mstsc_ip_impl(&ip)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = ip;
        Err("mstsc automation is only supported on Windows".into())
    }
}

#[tauri::command]
#[specta::specta]
pub fn click_mstsc_connect() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        platform::click_mstsc_connect_impl()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err("mstsc automation is only supported on Windows".into())
    }
}

/// End-to-end mstsc connect: launch, wait for dialog, inject IP, click Connect.
pub async fn run_mstsc_full_flow(ip: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        platform::run_full_flow_impl(ip).await
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = ip;
        Err("mstsc automation is only supported on Windows".into())
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::{LaunchResult, MSTSC_PROCESS_NAME};
    use std::cell::RefCell;
    use std::path::PathBuf;
    use windows::Win32::Foundation::{BOOL, CloseHandle, HWND, LPARAM, RECT, WPARAM};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, SetFocus, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
        KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, EnumWindows, GetClassNameW, GetWindowRect, GetWindowTextLengthW,
        GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, SendMessageW,
        SetForegroundWindow, ShowWindow, SW_RESTORE, WM_GETTEXT, WM_GETTEXTLENGTH,
    };

    const EM_SETSEL: u32 = 0x00B1;
    const WM_CLEAR: u32 = 0x0303;
    const BM_CLICK: u32 = 0x00F5;

    pub(super) fn launch_mstsc_impl() -> Result<LaunchResult, String> {
        if let Some(pid) = find_process_by_name(MSTSC_PROCESS_NAME) {
            if let Some(hwnd) = find_window_by_pid(pid) {
                unsafe {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                    let _ = SetForegroundWindow(hwnd);
                }
                return Ok(LaunchResult {
                    launched: false,
                    message: "Focused existing mstsc window".into(),
                });
            }
            return Ok(LaunchResult {
                launched: false,
                message: "mstsc already running (no window yet)".into(),
            });
        }
        // mstsc.exe lives in System32 on 64-bit Windows (WOW redirected for
        // 32-bit processes). Command::new("mstsc") picks it up via PATH.
        std::process::Command::new("mstsc.exe")
            .spawn()
            .map_err(|e| format!("Failed to launch mstsc: {e}"))?;
        // Best-effort: record resolved path so the log makes it obvious which binary ran.
        let resolved = PathBuf::from(r"C:\Windows\System32\mstsc.exe");
        log::info!(
            "[mstsc] spawned {}",
            resolved.display()
        );
        Ok(LaunchResult {
            launched: true,
            message: "Launched mstsc".into(),
        })
    }

    pub(super) fn inject_mstsc_ip_impl(ip: &str) -> Result<(), String> {
        log::info!("[mstsc] inject_ip requested (ip_len={})", ip.len());
        let top = find_mstsc_window()
            .ok_or_else(|| "mstsc window not found — is Remote Desktop open?".to_string())?;
        log_window(top, "target mstsc window");

        // The Computer field is a ComboBox; its internal Edit is what
        // actually receives keystrokes. Enumerate ComboBox children first,
        // pick the first visible one, then drill into its Edit child.
        let combos = collect_children_by_class(top, "ComboBox");
        log::info!("[mstsc] found {} ComboBox descendants", combos.len());
        let combo = combos
            .iter()
            .copied()
            .find(|h| unsafe { IsWindowVisible(*h).as_bool() })
            .ok_or_else(|| "No visible ComboBox found in mstsc dialog".to_string())?;
        log_window(combo, "computer combobox");

        // Drill into inner Edit.
        let edits = collect_children_by_class(combo, "Edit");
        let edit = edits
            .into_iter()
            .find(|h| unsafe { IsWindowVisible(*h).as_bool() })
            .ok_or_else(|| "No inner Edit found inside Computer ComboBox".to_string())?;
        log_window(edit, "computer edit");

        // Focus + clear + SendInput Unicode keys (same recipe as sangfor).
        unsafe {
            let _ = ShowWindow(top, SW_RESTORE);
            let fg_ok = SetForegroundWindow(top).as_bool();
            log::info!("[mstsc] SetForegroundWindow top -> {fg_ok}");
        }
        std::thread::sleep(std::time::Duration::from_millis(80));

        let focused = focus_control_cross_process(top, edit);
        log::info!(
            "[mstsc] focus edit hwnd={:#x} focused={focused}",
            edit.0 as usize
        );
        unsafe {
            SendMessageW(edit, EM_SETSEL, WPARAM(0), LPARAM(-1));
            SendMessageW(edit, WM_CLEAR, WPARAM(0), LPARAM(0));
        }

        let units: Vec<u16> = ip.encode_utf16().collect();
        if units.is_empty() {
            return Err("IP string is empty".into());
        }
        let mut inputs: Vec<INPUT> = Vec::with_capacity(units.len() * 2);
        for &cu in &units {
            inputs.push(unicode_key(cu, false));
            inputs.push(unicode_key(cu, true));
        }
        let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        let expected = inputs.len() as u32;
        log::info!("[mstsc] SendInput sent={sent}/{expected}");
        if sent != expected {
            return Err(format!(
                "SendInput partial for mstsc ip: {sent}/{expected} events accepted"
            ));
        }

        // Drain: wait until the inner Edit reports the expected length.
        let drain_budget_ms: u64 = 150 + units.len() as u64 * 10;
        let drain_start = std::time::Instant::now();
        loop {
            let current_len = read_edit_text(edit).encode_utf16().count();
            let elapsed = drain_start.elapsed().as_millis() as u64;
            let done = current_len >= units.len();
            if done || elapsed > drain_budget_ms {
                log::info!(
                    "[mstsc] drain: current_len={current_len} expected={} elapsed={elapsed}ms budget={drain_budget_ms}ms done={done}",
                    units.len()
                );
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Ok(())
    }

    pub(super) fn click_mstsc_connect_impl() -> Result<(), String> {
        log::info!("[mstsc] click_connect requested");
        let top = find_mstsc_window()
            .ok_or_else(|| "mstsc window not found — is Remote Desktop open?".to_string())?;
        log_window(top, "target mstsc window");

        let buttons = collect_children_by_class(top, "Button");
        log::info!("[mstsc] found {} Button descendants", buttons.len());
        let mut visible: Vec<(HWND, String)> = Vec::new();
        for (i, &b) in buttons.iter().enumerate() {
            let vis = unsafe { IsWindowVisible(b).as_bool() };
            let text = get_window_text(b);
            log::info!(
                "[mstsc]   button[{i}] hwnd={:#x} visible={vis} text={text:?}",
                b.0 as usize
            );
            if vis {
                visible.push((b, text));
            }
        }
        // Match "连接" or "Connect"; strip common decorations like "(&N)".
        let target = visible
            .iter()
            .find(|(_, t)| {
                let s = t.replace('&', "");
                s == "连接" || s == "连接(N)" || s == "Connect" || s.eq_ignore_ascii_case("connect")
            })
            .or_else(|| {
                visible
                    .iter()
                    .find(|(_, t)| t.contains("连接") || t.to_ascii_lowercase().contains("connect"))
            })
            .map(|(h, _)| *h)
            .ok_or_else(|| {
                "Could not locate a visible Connect button in mstsc dialog".to_string()
            })?;
        log::info!("[mstsc] clicking connect hwnd={:#x}", target.0 as usize);
        unsafe {
            SendMessageW(target, BM_CLICK, WPARAM(0), LPARAM(0));
        }
        Ok(())
    }

    pub(super) async fn run_full_flow_impl(ip: &str) -> Result<(), String> {
        log::info!("[mstsc] run_full_flow start");
        launch_mstsc_impl()?;

        const MAX_ATTEMPTS: u32 = 30;
        const DELAY_MS: u64 = 300;
        let mut last_err = String::new();
        for attempt in 1..=MAX_ATTEMPTS {
            match inject_mstsc_ip_impl(ip) {
                Ok(()) => {
                    log::info!("[mstsc] inject ok on attempt {attempt}");
                    last_err.clear();
                    break;
                }
                Err(e) => {
                    last_err = e.clone();
                    if !is_transient_mstsc_error(&e) {
                        return Err(e);
                    }
                    if attempt == MAX_ATTEMPTS {
                        return Err(format!(
                            "Window/UI did not become ready after {}s: {e}",
                            (MAX_ATTEMPTS as u64 * DELAY_MS) / 1000
                        ));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(DELAY_MS)).await;
                }
            }
        }
        if !last_err.is_empty() {
            return Err(last_err);
        }

        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        click_mstsc_connect_impl()?;
        log::info!("[mstsc] run_full_flow done");
        Ok(())
    }

    fn is_transient_mstsc_error(err: &str) -> bool {
        err.contains("mstsc window not found")
            || err.contains("No visible ComboBox")
            || err.contains("No inner Edit")
    }

    // ---- shared primitives (parallel to sangfor module) ------------------

    thread_local! {
        static ENUM_STATE: RefCell<EnumState> = RefCell::new(EnumState::default());
    }

    #[derive(Default)]
    struct EnumState {
        target_pid: u32,
        target_class: String,
        found_hwnd: Option<HWND>,
        collected: Vec<HWND>,
    }

    fn find_process_by_name(name: &str) -> Option<u32> {
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            let mut found = None;
            if Process32FirstW(snapshot, &mut entry).is_ok() {
                loop {
                    let exe = wide_to_string(&entry.szExeFile);
                    if exe.eq_ignore_ascii_case(name) {
                        found = Some(entry.th32ProcessID);
                        break;
                    }
                    if Process32NextW(snapshot, &mut entry).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snapshot);
            found
        }
    }

    fn find_window_by_pid(pid: u32) -> Option<HWND> {
        ENUM_STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.target_pid = pid;
            s.found_hwnd = None;
        });
        unsafe {
            let _ = EnumWindows(Some(enum_find_by_pid_proc), LPARAM(0));
        }
        ENUM_STATE.with(|s| s.borrow().found_hwnd)
    }

    unsafe extern "system" fn enum_find_by_pid_proc(hwnd: HWND, _lparam: LPARAM) -> BOOL {
        let target = ENUM_STATE.with(|s| s.borrow().target_pid);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == target && IsWindowVisible(hwnd).as_bool() {
            ENUM_STATE.with(|s| s.borrow_mut().found_hwnd = Some(hwnd));
            return BOOL(0);
        }
        BOOL(1)
    }

    fn find_mstsc_window() -> Option<HWND> {
        ENUM_STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.collected.clear();
        });
        unsafe {
            let _ = EnumWindows(Some(enum_find_mstsc_proc), LPARAM(0));
        }
        let candidates: Vec<HWND> = ENUM_STATE.with(|s| s.borrow().collected.clone());
        log::info!(
            "[mstsc] enumerated {} visible #32770 dialogs",
            candidates.len()
        );
        let mut best: Option<HWND> = None;
        for (i, &hwnd) in candidates.iter().enumerate() {
            let title = get_window_text(hwnd);
            let matches = title.contains("远程桌面连接")
                || title.contains("Remote Desktop Connection")
                || title.contains("远程桌面");
            log::info!(
                "[mstsc]   candidate[{i}] hwnd={:#x} matches={matches} title={title:?}",
                hwnd.0 as usize
            );
            if matches && best.is_none() {
                best = Some(hwnd);
            }
        }
        best
    }

    unsafe extern "system" fn enum_find_mstsc_proc(hwnd: HWND, _lparam: LPARAM) -> BOOL {
        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        if get_class_name(hwnd) != "#32770" {
            return BOOL(1);
        }
        ENUM_STATE.with(|s| s.borrow_mut().collected.push(hwnd));
        BOOL(1)
    }

    fn collect_children_by_class(parent: HWND, class: &str) -> Vec<HWND> {
        ENUM_STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.collected.clear();
            s.target_class = class.to_string();
        });
        unsafe {
            let _ = EnumChildWindows(parent, Some(enum_collect_by_class_proc), LPARAM(0));
        }
        ENUM_STATE.with(|s| s.borrow().collected.clone())
    }

    unsafe extern "system" fn enum_collect_by_class_proc(hwnd: HWND, _lparam: LPARAM) -> BOOL {
        let class_name = get_class_name(hwnd);
        let want = ENUM_STATE.with(|s| s.borrow().target_class.clone());
        if class_name.eq_ignore_ascii_case(&want) {
            ENUM_STATE.with(|s| s.borrow_mut().collected.push(hwnd));
        }
        BOOL(1)
    }

    fn focus_control_cross_process(top: HWND, target: HWND) -> bool {
        unsafe {
            let our_tid = GetCurrentThreadId();
            let mut _pid = 0u32;
            let target_tid = GetWindowThreadProcessId(top, Some(&mut _pid));
            if target_tid == 0 {
                return false;
            }
            let attached = if target_tid != our_tid {
                AttachThreadInput(our_tid, target_tid, true).as_bool()
            } else {
                true
            };
            let ok = SetFocus(target).is_ok();
            if target_tid != our_tid && attached {
                let _ = AttachThreadInput(our_tid, target_tid, false);
            }
            ok
        }
    }

    fn unicode_key(code_unit: u16, key_up: bool) -> INPUT {
        let mut flags: KEYBD_EVENT_FLAGS = KEYEVENTF_UNICODE;
        if key_up {
            flags |= KEYEVENTF_KEYUP;
        }
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0),
                    wScan: code_unit,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn read_edit_text(hwnd: HWND) -> String {
        unsafe {
            let len = SendMessageW(hwnd, WM_GETTEXTLENGTH, WPARAM(0), LPARAM(0)).0;
            if len <= 0 {
                return String::new();
            }
            let cap = (len as usize) + 1;
            let mut buf = vec![0u16; cap];
            SendMessageW(
                hwnd,
                WM_GETTEXT,
                WPARAM(cap),
                LPARAM(buf.as_mut_ptr() as isize),
            );
            wide_to_string(&buf)
        }
    }

    fn get_class_name(hwnd: HWND) -> String {
        let mut buf = [0u16; 256];
        let len = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
        String::from_utf16_lossy(&buf[..len])
    }

    fn get_window_text(hwnd: HWND) -> String {
        unsafe {
            let len = GetWindowTextLengthW(hwnd);
            if len <= 0 {
                return String::new();
            }
            let mut buf = vec![0u16; (len as usize) + 1];
            let written = GetWindowTextW(hwnd, &mut buf) as usize;
            String::from_utf16_lossy(&buf[..written])
        }
    }

    fn get_window_rect(hwnd: HWND) -> Option<RECT> {
        let mut r = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut r).ok()? };
        Some(r)
    }

    fn wide_to_string(wide: &[u16]) -> String {
        let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
        String::from_utf16_lossy(&wide[..end])
    }

    fn log_window(hwnd: HWND, role: &str) {
        let class = get_class_name(hwnd);
        let title = get_window_text(hwnd);
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        let rect = get_window_rect(hwnd);
        log::info!(
            "[mstsc] {role}: hwnd={:#x} pid={} class={class:?} title={title:?} rect={:?}",
            hwnd.0 as usize,
            pid,
            rect.map(|r| (r.left, r.top, r.right - r.left, r.bottom - r.top))
        );
    }

}
