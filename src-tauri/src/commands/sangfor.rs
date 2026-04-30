//! Sangfor VDI client automation.
//!
//! Two commands: launch-or-focus the client, and inject username/password
//! into its login dialog via Win32 messaging. Windows-only; other platforms
//! receive a "not supported" error.

use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LaunchResult {
    /// True if we spawned a new process; false if an existing one was focused.
    pub launched: bool,
    /// Human-readable status (locale-neutral English; UI layer translates).
    pub message: String,
}

const SANGFOR_EXE_PATH: &str =
    r"C:\Program Files (x86)\Sangfor\VDI\SangforCSClient\SangforCSClient.exe";
const SANGFOR_PROCESS_NAME: &str = "SangforCSClient.exe";

#[tauri::command]
#[specta::specta]
pub fn launch_sangfor_client() -> Result<LaunchResult, String> {
    #[cfg(target_os = "windows")]
    {
        platform::launch_sangfor_client_impl()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err("Sangfor client automation is only supported on Windows".into())
    }
}

#[tauri::command]
#[specta::specta]
pub fn inject_sangfor_credentials(username: String, password: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        platform::inject_sangfor_credentials_impl(&username, &password)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (username, password);
        Err("Sangfor client automation is only supported on Windows".into())
    }
}

#[tauri::command]
#[specta::specta]
pub fn click_sangfor_login() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        platform::click_sangfor_login_impl()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err("Sangfor client automation is only supported on Windows".into())
    }
}

/// End-to-end Sangfor login: launch, wait for dialog, inject credentials,
/// click "登录". Used by the MCP server; also safe to call from Tauri
/// command handlers. Uses tokio::time::sleep so it cooperates with the
/// async runtime instead of blocking a worker thread.
pub async fn run_sangfor_full_flow(username: &str, password: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        platform::run_full_flow_impl(username, password).await
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (username, password);
        Err("Sangfor client automation is only supported on Windows".into())
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::{LaunchResult, SANGFOR_EXE_PATH, SANGFOR_PROCESS_NAME};
    use std::cell::RefCell;
    use std::path::Path;
    use windows::core::PCWSTR;
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
        EnumChildWindows, EnumWindows, GetClassNameW, GetWindowLongW, GetWindowRect,
        GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
        SendMessageW, SetForegroundWindow, ShowWindow, GWL_STYLE, HWND_TOP, SW_RESTORE,
        WM_GETTEXT, WM_GETTEXTLENGTH,
    };

    // Edit control style bits (diagnostics / filtering).
    const ES_PASSWORD: i32 = 0x0020;
    const ES_READONLY: i32 = 0x0800;
    // Edit control messages used to clear the field before typing.
    const EM_SETSEL: u32 = 0x00B1;
    const WM_CLEAR: u32 = 0x0303;
    // Button click notification (works cross-process; no focus required).
    const BM_CLICK: u32 = 0x00F5;

    pub(super) fn launch_sangfor_client_impl() -> Result<LaunchResult, String> {
        if let Some(pid) = find_process_by_name(SANGFOR_PROCESS_NAME) {
            // Already running: find its top-level window and bring it to front.
            if let Some(hwnd) = find_window_by_pid(pid) {
                let _ = unsafe { ShowWindow(hwnd, SW_RESTORE) };
                let _ = unsafe { SetForegroundWindow(hwnd) };
                return Ok(LaunchResult {
                    launched: false,
                    message: "Focused existing Sangfor client window".into(),
                });
            }
            // Process exists but no visible window yet — treat as already launched.
            return Ok(LaunchResult {
                launched: false,
                message: "Sangfor client already running (no window yet)".into(),
            });
        }

        if !Path::new(SANGFOR_EXE_PATH).exists() {
            return Err(format!(
                "Sangfor client not found at {SANGFOR_EXE_PATH}"
            ));
        }

        // Detach: don't wait for child, don't inherit stdio.
        std::process::Command::new(SANGFOR_EXE_PATH)
            .spawn()
            .map_err(|e| format!("Failed to launch Sangfor client: {e}"))?;

        Ok(LaunchResult {
            launched: true,
            message: "Launched Sangfor client".into(),
        })
    }

    pub(super) fn inject_sangfor_credentials_impl(
        username: &str,
        password: &str,
    ) -> Result<(), String> {
        log::info!(
            "[sangfor] inject requested (username_len={}, password_len={})",
            username.len(),
            password.len()
        );

        let top = find_sangfor_login_window().ok_or_else(|| {
            log::warn!("[sangfor] no matching top-level window found");
            "Sangfor login window not found — is the client open?".to_string()
        })?;
        log_window(top, "target login window");

        let edits = collect_edit_children(top);
        log::info!("[sangfor] found {} Edit descendants", edits.len());
        for (i, &e) in edits.iter().enumerate() {
            log_edit(i, e);
        }

        // The login dialog hosts multiple tabs (account / cert / USB-KEY),
        // and every tab's Edit controls coexist in the child list — only
        // the currently-selected tab's Edits are actually visible. We pick
        // targets from the visible, non-readonly, reasonably-sized Edits.
        let visible_inputs: Vec<(usize, HWND, EditInfo)> = edits
            .iter()
            .enumerate()
            .map(|(i, &h)| (i, h, edit_info(h)))
            .filter(|(_, _, info)| {
                info.visible && !info.readonly && info.height >= 18 && info.width >= 80
            })
            .collect();
        log::info!(
            "[sangfor] {} candidate input Edits after visibility/size filter",
            visible_inputs.len()
        );
        for (i, h, info) in &visible_inputs {
            log::info!(
                "[sangfor]   candidate orig_idx={i} hwnd={:#x} password={}",
                h.0 as usize,
                info.password
            );
        }

        let username_target = visible_inputs
            .iter()
            .find(|(_, _, info)| !info.password)
            .map(|&(_, h, _)| h)
            .ok_or_else(|| {
                "Could not locate a visible username Edit on the current tab".to_string()
            })?;
        let password_target = visible_inputs
            .iter()
            .find(|(_, _, info)| info.password)
            .map(|&(_, h, _)| h)
            .ok_or_else(|| {
                "Could not locate a visible password Edit on the current tab".to_string()
            })?;

        log::info!(
            "[sangfor] selected username hwnd={:#x}, password hwnd={:#x}",
            username_target.0 as usize,
            password_target.0 as usize
        );

        // Bring the Sangfor dialog to the foreground so SendInput reaches it.
        unsafe {
            let _ = ShowWindow(top, SW_RESTORE);
            let fg_ok = SetForegroundWindow(top).as_bool();
            log::info!("[sangfor] SetForegroundWindow top -> {fg_ok}");
        }
        // Small settle so the window-manager switch completes before we SetFocus.
        std::thread::sleep(std::time::Duration::from_millis(80));

        type_into_edit(top, username_target, username, "username")?;
        type_into_edit(top, password_target, password, "password")?;
        log::info!("[sangfor] inject finished");
        Ok(())
    }

    pub(super) fn click_sangfor_login_impl() -> Result<(), String> {
        log::info!("[sangfor] click_login requested");
        let top = find_sangfor_login_window()
            .ok_or_else(|| "Sangfor login window not found — is the client open?".to_string())?;
        log_window(top, "target login window");

        let buttons = collect_button_children(top);
        log::info!("[sangfor] found {} Button descendants", buttons.len());
        let mut candidates: Vec<(HWND, String, bool)> = Vec::new();
        for (i, &b) in buttons.iter().enumerate() {
            let visible = unsafe { IsWindowVisible(b).as_bool() };
            let text = get_window_text(b);
            log::info!(
                "[sangfor]   button[{i}] hwnd={:#x} visible={visible} text={text:?}",
                b.0 as usize
            );
            if visible {
                candidates.push((b, text, visible));
            }
        }

        // Prefer exact match "登录"; exclude "自动登录" which contains "登录".
        let target = candidates
            .iter()
            .find(|(_, text, _)| text.trim() == "登录")
            .or_else(|| {
                candidates
                    .iter()
                    .find(|(_, text, _)| text.contains("登录") && !text.contains("自动"))
            })
            .map(|&(h, _, _)| h)
            .ok_or_else(|| {
                "Could not locate a visible \"登录\" button on the current tab".to_string()
            })?;

        log::info!("[sangfor] clicking login button hwnd={:#x}", target.0 as usize);
        unsafe {
            SendMessageW(target, BM_CLICK, WPARAM(0), LPARAM(0));
        }
        Ok(())
    }

    /// Orchestrated flow: launch → retry-wait for dialog → inject → click.
    /// Mirrors the retry whitelist used by the frontend's run-all button.
    pub(super) async fn run_full_flow_impl(
        username: &str,
        password: &str,
    ) -> Result<(), String> {
        log::info!("[sangfor] run_full_flow start");
        launch_sangfor_client_impl()?;

        const MAX_ATTEMPTS: u32 = 120;
        const DELAY_MS: u64 = 500;
        let mut last_err = String::new();
        for attempt in 1..=MAX_ATTEMPTS {
            match inject_sangfor_credentials_impl(username, password) {
                Ok(()) => {
                    log::info!("[sangfor] inject ok on attempt {attempt}");
                    last_err.clear();
                    break;
                }
                Err(e) => {
                    last_err = e.clone();
                    if !is_transient_sangfor_error(&e) {
                        log::warn!(
                            "[sangfor] non-transient error on attempt {attempt}: {e}"
                        );
                        return Err(e);
                    }
                    if attempt == MAX_ATTEMPTS {
                        log::warn!(
                            "[sangfor] retry budget exhausted ({MAX_ATTEMPTS} × {DELAY_MS}ms)"
                        );
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
        click_sangfor_login_impl()?;
        log::info!("[sangfor] run_full_flow done");
        Ok(())
    }

    fn is_transient_sangfor_error(err: &str) -> bool {
        err.contains("login window not found")
            || err.contains("Expected at least")
            || err.contains("Could not locate a visible")
    }

    fn collect_button_children(parent: HWND) -> Vec<HWND> {
        ENUM_STATE.with(|s| s.borrow_mut().collected.clear());
        unsafe {
            let _ = EnumChildWindows(parent, Some(enum_collect_buttons_proc), LPARAM(0));
        }
        ENUM_STATE.with(|s| s.borrow().collected.clone())
    }

    unsafe extern "system" fn enum_collect_buttons_proc(hwnd: HWND, _lparam: LPARAM) -> BOOL {
        if get_class_name(hwnd).eq_ignore_ascii_case("Button") {
            ENUM_STATE.with(|s| s.borrow_mut().collected.push(hwnd));
        }
        BOOL(1)
    }

    #[derive(Clone, Copy)]
    struct EditInfo {
        visible: bool,
        readonly: bool,
        password: bool,
        width: i32,
        height: i32,
    }

    fn edit_info(hwnd: HWND) -> EditInfo {
        let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) };
        let visible = unsafe { IsWindowVisible(hwnd).as_bool() };
        let (width, height) = get_window_rect(hwnd)
            .map(|r| (r.right - r.left, r.bottom - r.top))
            .unwrap_or((0, 0));
        EditInfo {
            visible,
            readonly: (style & ES_READONLY) != 0,
            password: (style & ES_PASSWORD) != 0,
            width,
            height,
        }
    }

    // ---- process enumeration --------------------------------------------

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

    // ---- window lookup --------------------------------------------------

    thread_local! {
        static ENUM_STATE: RefCell<EnumState> = RefCell::new(EnumState::default());
    }

    #[derive(Default)]
    struct EnumState {
        target_pid: u32,
        found_hwnd: Option<HWND>,
        collected: Vec<HWND>,
    }

    /// Find the first visible top-level window owned by `pid`.
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
            return BOOL(0); // stop
        }
        BOOL(1) // continue
    }

    /// Find the top-level login dialog. Logs every visible `#32770` dialog
    /// seen during enumeration so we can diagnose mismatches against the
    /// actual Sangfor client build in use.
    fn find_sangfor_login_window() -> Option<HWND> {
        ENUM_STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.found_hwnd = None;
            s.collected.clear();
        });
        unsafe {
            let _ = EnumWindows(Some(enum_find_sangfor_proc), LPARAM(0));
        }

        let candidates: Vec<HWND> = ENUM_STATE.with(|s| s.borrow().collected.clone());
        log::info!(
            "[sangfor] enumerated {} visible #32770 dialogs",
            candidates.len()
        );
        let mut best: Option<HWND> = None;
        for (i, &hwnd) in candidates.iter().enumerate() {
            let title = get_window_text(hwnd);
            let matches = title.contains("接入客户端")
                || title.contains("SANGFOR")
                || title.contains("深信服")
                || title.contains("aDesk");
            log::info!(
                "[sangfor]   candidate[{i}] hwnd={:#x} matches={matches} title={title:?}",
                hwnd.0 as usize
            );
            if matches && best.is_none() {
                best = Some(hwnd);
            }
        }
        // If nothing matched by title, fall back to the first visible #32770
        // — better to try something than fail silently.
        if best.is_none() && !candidates.is_empty() {
            log::warn!(
                "[sangfor] no title match; falling back to first #32770 candidate"
            );
            best = candidates.first().copied();
        }
        best
    }

    unsafe extern "system" fn enum_find_sangfor_proc(hwnd: HWND, _lparam: LPARAM) -> BOOL {
        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        if get_class_name(hwnd) != "#32770" {
            return BOOL(1);
        }
        ENUM_STATE.with(|s| s.borrow_mut().collected.push(hwnd));
        BOOL(1) // keep collecting all candidates
    }

    /// Collect Edit descendants of `parent` in enumeration order (Z-order top-down).
    fn collect_edit_children(parent: HWND) -> Vec<HWND> {
        ENUM_STATE.with(|s| s.borrow_mut().collected.clear());
        unsafe {
            let _ = EnumChildWindows(parent, Some(enum_collect_edits_proc), LPARAM(0));
        }
        ENUM_STATE.with(|s| s.borrow().collected.clone())
    }

    unsafe extern "system" fn enum_collect_edits_proc(hwnd: HWND, _lparam: LPARAM) -> BOOL {
        let class = get_class_name(hwnd);
        if class.eq_ignore_ascii_case("Edit") {
            ENUM_STATE.with(|s| s.borrow_mut().collected.push(hwnd));
        }
        BOOL(1)
    }

    // ---- text I/O on Edit controls --------------------------------------

    /// Focus the target Edit (cross-process via AttachThreadInput), clear
    /// any existing content, then SendInput each UTF-16 code unit as a
    /// KEYEVENTF_UNICODE synthetic keypress. This drives the control the
    /// same way a real keyboard would — required for secure password
    /// Edits that reject WM_SETTEXT / WM_CHAR from other processes.
    fn type_into_edit(top: HWND, edit: HWND, text: &str, label: &str) -> Result<(), String> {
        let focused = focus_edit_cross_process(top, edit);
        log::info!(
            "[sangfor] type_into[{label}] hwnd={:#x} focused={focused}",
            edit.0 as usize
        );

        // Clear existing content. EM_SETSEL(0,-1) + WM_CLEAR is focus-independent
        // and works on Edit controls across processes.
        unsafe {
            SendMessageW(edit, EM_SETSEL, WPARAM(0), LPARAM(-1));
            SendMessageW(edit, WM_CLEAR, WPARAM(0), LPARAM(0));
        }

        let units: Vec<u16> = text.encode_utf16().collect();
        if units.is_empty() {
            log::warn!("[sangfor] type_into[{label}] text is empty, nothing to send");
            return Ok(());
        }

        // Build one INPUT array with key-down+key-up for each code unit.
        let mut inputs: Vec<INPUT> = Vec::with_capacity(units.len() * 2);
        for &cu in &units {
            inputs.push(unicode_key(cu, false));
            inputs.push(unicode_key(cu, true));
        }
        let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        let expected = inputs.len() as u32;
        log::info!(
            "[sangfor] type_into[{label}] SendInput sent={sent}/{expected} code_units={}",
            units.len()
        );
        if sent != expected {
            return Err(format!(
                "SendInput partial for {label}: {sent}/{expected} events accepted"
            ));
        }

        // SendInput is asynchronous: it enqueues events and returns before
        // the target has processed them. Without draining, a subsequent
        // SetFocus to the next field races the still-pending keystrokes,
        // causing characters to leak into the wrong control. For Edits we
        // can read back and busy-wait until the text length matches (only
        // reliable on non-password fields — password Edits return empty).
        let drain_budget_ms: u64 = 100 + units.len() as u64 * 10;
        let drain_start = std::time::Instant::now();
        loop {
            let current_len = read_edit_text(edit).encode_utf16().count();
            let done = current_len >= units.len();
            let elapsed = drain_start.elapsed().as_millis() as u64;
            if done || elapsed > drain_budget_ms {
                log::info!(
                    "[sangfor] type_into[{label}] drain: current_len={current_len} expected={} elapsed={elapsed}ms budget={drain_budget_ms}ms done={done}",
                    units.len()
                );
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // Password fields never report a length, so give them an unconditional
        // settle window equal to the per-character budget before we hand
        // focus elsewhere.
        let edit_is_password = (unsafe { GetWindowLongW(edit, GWL_STYLE) } & ES_PASSWORD) != 0;
        if edit_is_password {
            let settle_ms = 100 + units.len() as u64 * 8;
            log::info!(
                "[sangfor] type_into[{label}] password field: settling {settle_ms}ms"
            );
            std::thread::sleep(std::time::Duration::from_millis(settle_ms));
        }
        Ok(())
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

    /// Cross-process SetFocus: attach our thread's input queue to the target
    /// window's thread, call SetFocus, then detach. Required because SetFocus
    /// only works within a single input queue.
    fn focus_edit_cross_process(top: HWND, edit: HWND) -> bool {
        unsafe {
            let our_tid = GetCurrentThreadId();
            let mut _pid = 0u32;
            let target_tid = GetWindowThreadProcessId(top, Some(&mut _pid));
            if target_tid == 0 {
                log::warn!("[sangfor] GetWindowThreadProcessId returned 0");
                return false;
            }
            let attached = if target_tid != our_tid {
                AttachThreadInput(our_tid, target_tid, true).as_bool()
            } else {
                true
            };
            let set_ok = SetFocus(edit).is_ok();
            if target_tid != our_tid && attached {
                let _ = AttachThreadInput(our_tid, target_tid, false);
            }
            log::debug!(
                "[sangfor] focus_edit: our_tid={our_tid} target_tid={target_tid} attached={attached} setfocus_ok={set_ok}"
            );
            set_ok
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

    // ---- small Win32 helpers --------------------------------------------

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

    fn wide_to_string(wide: &[u16]) -> String {
        let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
        String::from_utf16_lossy(&wide[..end])
    }

    fn get_window_rect(hwnd: HWND) -> Option<RECT> {
        let mut rect = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut rect).ok()? };
        Some(rect)
    }

    fn log_window(hwnd: HWND, role: &str) {
        let class = get_class_name(hwnd);
        let title = get_window_text(hwnd);
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        let rect = get_window_rect(hwnd);
        log::info!(
            "[sangfor] {role}: hwnd={:#x} pid={} class={class:?} title={title:?} rect={:?}",
            hwnd.0 as usize,
            pid,
            rect.map(|r| (r.left, r.top, r.right - r.left, r.bottom - r.top))
        );
    }

    fn log_edit(index: usize, hwnd: HWND) {
        let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) };
        let is_password = (style & ES_PASSWORD) != 0;
        let is_readonly = (style & ES_READONLY) != 0;
        let text = read_edit_text(hwnd);
        let rect = get_window_rect(hwnd);
        // Don't log the actual contents of password fields even if read_edit_text
        // returned something — just the length.
        let text_field = if is_password {
            format!("<password-style, len={}>", text.len())
        } else {
            format!("{text:?}")
        };
        log::info!(
            "[sangfor]   edit[{index}] hwnd={:#x} style={:#010x} password={is_password} readonly={is_readonly} rect={:?} text={text_field}",
            hwnd.0 as usize,
            style,
            rect.map(|r| (r.left, r.top, r.right - r.left, r.bottom - r.top))
        );
    }

    // Suppress unused-import warnings for items only used via their traits/types in other configs.
    #[allow(dead_code)]
    fn _refs() {
        let _ = (HWND_TOP, PCWSTR::null());
    }
}
