#![windows_subsystem = "windows"]
// DEPRECATED. This executable is no longer maintained: DeskFlag absorbed the enforcer in v0.2.46
// (13 August 2026), the way it absorbed DeskSwipe before it. The living copy of everything below
// is rs/src/keyboard.rs in gabrielchaves6/desk_flag, on a thread of its own, with the US/BR badge
// as an element on the taskbar card rather than a tray icon of its own — and with an off switch
// that puts HKCU\Keyboard Layout\Preload back, which this one never did. Do not run both: two
// enforcers take turns correcting each other several times a second. See README.md.
//
// KeyFlag — keep Windows on a single keyboard layout, enforced globally.
//
// Windows' keyboard handling mixes layouts per-window, re-adds them, and exposes a dozen
// switch shortcuts (Win+Space, Alt+Shift, Ctrl+Shift). KeyFlag picks ONE layout — United
// States-International or Portuguese (Brazilian ABNT2) — and makes the whole session stay on
// it: whenever any app or the OS drifts to a different layout, it snaps the active window
// back. The only UI is a tray icon (its glyph shows the current mode: blue "US" / green
// "BR") with a right-click menu to switch modes, view About, check for updates, or quit.
//
// Sibling of DeskFlag (same tray / message-loop / registry / About / auto-update patterns),
// minus the Direct2D taskbar pill — here the tray badge is drawn with plain GDI.

use core::ffi::c_void;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::Urlmon::URLDownloadToFileW;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::{AttachThreadInput, CreateMutexW, GetCurrentThreadId};
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// ---------- Constants ----------
const WM_TRAY: u32 = WM_APP + 1;
const TIMER_ID: usize = 1;

const ID_US: usize = 1;
const ID_ABNT: usize = 2;
const ID_ABOUT: usize = 12;
const ID_UPDATE: usize = 13;
const ID_EXIT: usize = 11;

// KLIDs (present on Windows 10/11 by default; the layout DLLs ship with the OS).
const KLID_US_INTL: &str = "00020409"; // United States-International (kbdusx.dll)
const KLID_ABNT2: &str = "00010416"; // Portuguese (Brazilian ABNT2) (kbdbr.dll)

const REG_SUBKEY: &str = "Software\\KeyFlag";
const ABOUT_URL: &str = "https://github.com/gabrielchaves6/keyflag";
// Public releases repo (this repo is public, so releases/latest is readable unauthenticated).
const UPDATE_REPO: &str = "gabrielchaves6/keyflag";

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    UsIntl,
    Abnt2,
}

// ---------- Globals (single-threaded; touched only from the message thread) ----------
// "TaskbarCreated": broadcast by the shell every time it recreates the notification area
// (explorer restart/crash). Its id is assigned at runtime, so it can't be a match arm constant.
static mut WM_TASKBARCREATED: u32 = 0;

static mut MODE: Mode = Mode::UsIntl;
static mut HKL_TARGET: isize = 0; // HKL of the currently-enforced layout (the only one kept loaded)
static mut TRAY_ICON: isize = 0; // current tray HICON (US/BR badge, rebuilt on mode change)

static mut ABOUT_HWND: Option<HWND> = None;
static mut ABOUT_ICON: isize = 0; // brand logo (embedded keyflag.ico, or GDI fallback)
static mut ABOUT_LINK: RECT = RECT { left: 0, top: 0, right: 0, bottom: 0 };
static mut ABOUT_CLOSE: RECT = RECT { left: 0, top: 0, right: 0, bottom: 0 };
static mut ABOUT_CLOSE_HOT: bool = false;
static mut ACTIVE_WORK: RECT = RECT { left: 0, top: 0, right: 0, bottom: 0 };

static mut DLG_CLOSE: RECT = RECT { left: 0, top: 0, right: 0, bottom: 0 };
static mut DLG_CLOSE_HOT: bool = false;
static mut DLG_HEADING: String = String::new();
static mut DLG_BODY: String = String::new();
static mut DLG_PRIMARY: String = String::new();
static mut DLG_SECONDARY: String = String::new();
static mut DLG_BTN_PRIMARY: RECT = RECT { left: 0, top: 0, right: 0, bottom: 0 };
static mut DLG_BTN_SECONDARY: RECT = RECT { left: 0, top: 0, right: 0, bottom: 0 };
static mut DLG_RESULT: i32 = 0;
static mut DLG_CLASS_REGISTERED: bool = false;

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// UTF-16 without a NUL terminator (for DrawTextW / GetTextExtentPoint32W).
fn wn(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

// ---------- Persisted choice ----------
fn read_mode() -> Mode {
    unsafe {
        let sk = w(REG_SUBKEY);
        let v = w("Mode");
        let mut buf = [0u16; 16];
        let mut size = (buf.len() * 2) as u32;
        let rc = RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(sk.as_ptr()),
            PCWSTR(v.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        );
        if rc != ERROR_SUCCESS {
            return Mode::UsIntl;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        if String::from_utf16_lossy(&buf[..len]) == "ABNT" {
            Mode::Abnt2
        } else {
            Mode::UsIntl
        }
    }
}

fn write_mode(mode: Mode) {
    unsafe {
        let sk = w(REG_SUBKEY);
        let mut hkey = HKEY::default();
        let rc = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(sk.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut hkey,
            None,
        );
        if rc != ERROR_SUCCESS {
            return;
        }
        let val = match mode {
            Mode::UsIntl => "US",
            Mode::Abnt2 => "ABNT",
        };
        let data = w(val);
        let bytes = core::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2);
        let _ = RegSetValueExW(hkey, PCWSTR(w("Mode").as_ptr()), 0, REG_SZ, Some(bytes));
        let _ = RegCloseKey(hkey);
    }
}

// ---------- Layout enforcement ----------
fn hkl(v: isize) -> HKL {
    HKL(v as *mut c_void)
}

fn klid_for(mode: Mode) -> &'static str {
    match mode {
        Mode::UsIntl => KLID_US_INTL,
        Mode::Abnt2 => KLID_ABNT2,
    }
}

// Load (or re-load) a layout into the session and make it active; returns its HKL (0 on
// failure). Idempotent — calling it for an already-loaded layout just returns its handle.
unsafe fn load_layout(klid: &str) -> isize {
    LoadKeyboardLayoutW(PCWSTR(w(klid).as_ptr()), KLF_ACTIVATE)
        .map(|h| h.0 as isize)
        .unwrap_or(0)
}

// Unload every loaded layout except `keep`, collapsing the session's input-method list to a
// single entry. With only one input method, Windows hides its own tray language indicator, so
// KeyFlag's US/BR badge is the only one left. Called on mode apply (startup + toggle), NOT on
// the timer — so it doesn't continuously fight apps (RDP/VMs/IMEs) that load their own layout.
unsafe fn unload_others(keep: isize) {
    if keep == 0 {
        return; // never unload everything — we'd be left with no layout at all
    }
    let mut list = [HKL::default(); 32];
    let n = GetKeyboardLayoutList(Some(&mut list));
    for h in list.iter().take(n.max(0) as usize) {
        if !h.0.is_null() && h.0 as isize != keep {
            let _ = UnloadKeyboardLayout(*h);
        }
    }
}

// Persist a single input method in the user profile (HKCU\Keyboard Layout\Preload) so the
// indicator stays gone across logons — otherwise Windows reloads the old 2-method list at
// sign-in and the indicator flickers back until KeyFlag starts and re-collapses it.
unsafe fn set_preload_single(mode: Mode) {
    let sk = w("Keyboard Layout\\Preload");
    let mut hkey = HKEY::default();
    let rc = RegCreateKeyExW(
        HKEY_CURRENT_USER,
        PCWSTR(sk.as_ptr()),
        0,
        PCWSTR::null(),
        REG_OPTION_NON_VOLATILE,
        KEY_SET_VALUE,
        None,
        &mut hkey,
        None,
    );
    if rc != ERROR_SUCCESS {
        return;
    }
    // "1" = the chosen layout; drop any other numbered entries (the second method, etc.).
    let data = w(klid_for(mode));
    let bytes = core::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2);
    let _ = RegSetValueExW(hkey, PCWSTR(w("1").as_ptr()), 0, REG_SZ, Some(bytes));
    for i in 2..=9 {
        let name = w(&i.to_string());
        let _ = RegDeleteValueW(hkey, PCWSTR(name.as_ptr()));
    }
    let _ = RegCloseKey(hkey);
}

// Snap the foreground window back to the chosen layout if it has drifted. Cheap (a couple of
// syscalls) and a no-op when already correct, so it's safe to call on a tight timer and on
// every foreground change.
unsafe fn enforce() {
    let target = HKL_TARGET;
    if target == 0 {
        return;
    }
    let fg = GetForegroundWindow();
    if fg.0.is_null() {
        return;
    }
    let tid = GetWindowThreadProcessId(fg, None);
    if tid == 0 {
        return;
    }
    let cur = GetKeyboardLayout(tid);
    if cur.0 as isize != target {
        // Ask the focused window to switch its input language — the same mechanism the
        // language bar uses; works cross-process for a loaded layout.
        let _ = PostMessageW(fg, WM_INPUTLANGCHANGEREQUEST, WPARAM(0), LPARAM(target));
    }
}

// EnumWindows callback: push the chosen layout onto every top-level window, so already-open
// apps flip immediately on a mode switch instead of only when next focused.
unsafe extern "system" fn broadcast_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let _ = PostMessageW(hwnd, WM_INPUTLANGCHANGEREQUEST, WPARAM(0), LPARAM(lparam.0));
    TRUE
}

// WinEvent hook: fires (out of our process) whenever the foreground window changes.
unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _id_thread: u32,
    _time: u32,
) {
    enforce();
}

unsafe fn apply_mode(hwnd: HWND, mode: Mode) {
    MODE = mode;
    write_mode(mode);

    let target = load_layout(klid_for(mode));
    HKL_TARGET = target;
    if target != 0 {
        // Default input language for newly-created threads/processes.
        let mut h = target;
        let _ = SystemParametersInfoW(
            SPI_SETDEFAULTINPUTLANG,
            0,
            Some(&mut h as *mut _ as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        let _ = ActivateKeyboardLayout(hkl(target), ACTIVATE_KEYBOARD_LAYOUT_FLAGS(0));
        // Flip every open window now, then make sure the focused one is correct.
        let _ = EnumWindows(Some(broadcast_proc), LPARAM(target));
        enforce();
        // Drop the other layout so the session has a single input method (hides the OS
        // indicator), and persist that single method for future logons.
        unload_others(target);
        set_preload_single(mode);
    }

    refresh_tray(hwnd);
}

// ---------- GDI-drawn icons (tray badge "US"/"BR"; brand fallback "K") ----------
unsafe fn make_text_icon(bg: COLORREF, label: &str) -> HICON {
    let sz = 32i32;
    let screen_dc = GetDC(None);
    let mem_dc = CreateCompatibleDC(screen_dc);
    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: core::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: sz,
            biHeight: -sz, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut c_void = core::ptr::null_mut();
    let dib =
        CreateDIBSection(mem_dc, &mut bmi, DIB_RGB_COLORS, &mut bits, None, 0).unwrap_or_default();
    let old = SelectObject(mem_dc, dib);

    let full = RECT { left: 0, top: 0, right: sz, bottom: sz };
    let brush = CreateSolidBrush(bg);
    FillRect(mem_dc, &full, brush);
    let _ = DeleteObject(brush);

    let font = make_font(22, 700, false);
    let old_font = SelectObject(mem_dc, font);
    SetBkMode(mem_dc, TRANSPARENT);
    let _ = SetTextColor(mem_dc, rgb(255, 255, 255));
    let mut txt = wn(label);
    let mut tr = full;
    DrawTextW(mem_dc, &mut txt, &mut tr, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    SelectObject(mem_dc, old_font);
    let _ = DeleteObject(font);

    // GDI never writes the alpha channel, so force the whole 32bpp bitmap opaque — otherwise
    // the icon comes out fully transparent (invisible).
    let p = bits as *mut u8;
    for i in 0..(sz * sz) as usize {
        *p.add(i * 4 + 3) = 255;
    }

    SelectObject(mem_dc, old);
    let mask_bits = vec![0u8; (sz * sz) as usize];
    let mask = CreateBitmap(sz, sz, 1, 1, Some(mask_bits.as_ptr() as *const _));
    let ii = ICONINFO {
        fIcon: TRUE,
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: dib,
    };
    let hicon = CreateIconIndirect(&ii).unwrap_or_default();
    let _ = DeleteObject(mask);
    let _ = DeleteObject(dib);
    let _ = DeleteDC(mem_dc);
    ReleaseDC(None, screen_dc);
    hicon
}

unsafe fn mode_icon(mode: Mode) -> HICON {
    match mode {
        Mode::UsIntl => make_text_icon(rgb(37, 99, 235), "US"),
        Mode::Abnt2 => make_text_icon(rgb(22, 163, 74), "BR"),
    }
}

// Brand icon (taskbar / About / installer): the embedded keyflag.ico (MSVC build) if present,
// else a keyflag.ico next to the exe, else a GDI-drawn indigo "K" fallback.
unsafe fn load_app_icon() -> HICON {
    let hinst: HINSTANCE = GetModuleHandleW(None).map(|h| h.into()).unwrap_or_default();
    if let Ok(h) = LoadIconW(hinst, PCWSTR(1 as *const u16)) {
        if !h.is_invalid() {
            return h;
        }
    }
    if let Some(h) = icon_from_file() {
        return h;
    }
    make_text_icon(rgb(79, 70, 229), "K")
}

unsafe fn icon_from_file() -> Option<HICON> {
    let mut buf = [0u16; 260];
    let n = GetModuleFileNameW(None, &mut buf);
    if n == 0 {
        return None;
    }
    let exe = String::from_utf16_lossy(&buf[..n as usize]);
    let ico = std::path::Path::new(&exe).parent()?.join("keyflag.ico");
    if !ico.exists() {
        return None;
    }
    let icow = w(&ico.to_string_lossy());
    let h = LoadImageW(None, PCWSTR(icow.as_ptr()), IMAGE_ICON, 0, 0, LR_LOADFROMFILE | LR_DEFAULTSIZE).ok()?;
    Some(HICON(h.0))
}

// ---------- Tray ----------
unsafe fn tray_base(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW::default();
    nid.cbSize = core::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid
}

unsafe fn tooltip() -> &'static str {
    match MODE {
        Mode::UsIntl => "KeyFlag — US International",
        Mode::Abnt2 => "KeyFlag — Português ABNT2",
    }
}

// Register the tray icon. Also called again on every TaskbarCreated, so it has to be
// re-entrant: drop the previous badge instead of leaking one HICON per shell restart.
unsafe fn add_tray(hwnd: HWND) {
    let icon = mode_icon(MODE);
    if TRAY_ICON != 0 {
        let _ = DestroyIcon(HICON(TRAY_ICON as *mut c_void));
    }
    TRAY_ICON = icon.0 as isize;
    let mut nid = tray_base(hwnd);
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = icon;
    let tip = w(tooltip());
    let n = tip.len().min(127);
    nid.szTip[..n].copy_from_slice(&tip[..n]);
    let _ = Shell_NotifyIconW(NIM_ADD, &nid);
}

unsafe fn refresh_tray(hwnd: HWND) {
    let icon = mode_icon(MODE);
    let mut nid = tray_base(hwnd);
    nid.uFlags = NIF_ICON | NIF_TIP;
    nid.hIcon = icon;
    let tip = w(tooltip());
    let n = tip.len().min(127);
    nid.szTip[..n].copy_from_slice(&tip[..n]);
    let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    if TRAY_ICON != 0 {
        let _ = DestroyIcon(HICON(TRAY_ICON as *mut c_void));
    }
    TRAY_ICON = icon.0 as isize;
}

unsafe fn remove_tray(hwnd: HWND) {
    let nid = tray_base(hwnd);
    let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
}

// SetForegroundWindow refuses when the caller doesn't own the foreground — and with a custom
// shell (Seelen UI) the click that opens our menu is delivered by an always-on-top flyout that
// keeps it. TrackPopupMenu on a window that never became foreground shows a menu that closes
// on the first mouse move. Borrowing the foreground thread's input queue lifts the restriction.
unsafe fn force_foreground(hwnd: HWND) {
    if SetForegroundWindow(hwnd).as_bool() {
        return;
    }
    let fg = GetForegroundWindow();
    if fg.0.is_null() {
        return;
    }
    let fg_tid = GetWindowThreadProcessId(fg, None);
    let our_tid = GetCurrentThreadId();
    if fg_tid == 0 || fg_tid == our_tid {
        return;
    }
    let _ = AttachThreadInput(fg_tid, our_tid, TRUE);
    let _ = SetForegroundWindow(hwnd);
    let _ = AttachThreadInput(fg_tid, our_tid, FALSE);
}

unsafe fn show_tray_menu(hwnd: HWND) {
    let menu = CreatePopupMenu().unwrap_or_default();
    let _ = AppendMenuW(menu, MF_STRING, ID_US, PCWSTR(w("US International").as_ptr()));
    let _ = AppendMenuW(menu, MF_STRING, ID_ABNT, PCWSTR(w("Português ABNT2").as_ptr()));
    let active = if MODE == Mode::UsIntl { ID_US } else { ID_ABNT } as u32;
    let _ = CheckMenuRadioItem(menu, ID_US as u32, ID_ABNT as u32, active, MF_BYCOMMAND.0);
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, ID_ABOUT, PCWSTR(w("About KeyFlag").as_ptr()));
    let _ = AppendMenuW(menu, MF_STRING, ID_UPDATE, PCWSTR(w("Check for updates…").as_ptr()));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, ID_EXIT, PCWSTR(w("Exit").as_ptr()));

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    force_foreground(hwnd);
    let cmd = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD, pt.x, pt.y, 0, hwnd, None);
    // Without this the menu doesn't dismiss on the next click outside it (KB135788): the
    // owner has to receive one more message after TrackPopupMenu returns.
    let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
    let _ = DestroyMenu(menu);
    match cmd.0 as usize {
        ID_US => apply_mode(hwnd, Mode::UsIntl),
        ID_ABNT => apply_mode(hwnd, Mode::Abnt2),
        ID_ABOUT => {
            set_active_work();
            show_about();
        }
        ID_UPDATE => {
            set_active_work();
            check_for_updates(hwnd);
        }
        ID_EXIT => {
            remove_tray(hwnd);
            PostQuitMessage(0);
        }
        _ => {}
    }
}

unsafe fn set_active_work() {
    let mut work = RECT::default();
    let _ = SystemParametersInfoW(
        SPI_GETWORKAREA,
        0,
        Some(&mut work as *mut _ as *mut c_void),
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
    );
    ACTIVE_WORK = work;
}

// ===================== Auto-update (pull from the public releases repo) =====================
fn ver_tuple(s: &str) -> (u32, u32, u32) {
    let s = s.trim().trim_start_matches('v');
    let mut p = s.split(|c| c == '.' || c == '-' || c == '+');
    let n = |o: Option<&str>| o.and_then(|x| x.trim().parse::<u32>().ok()).unwrap_or(0);
    (n(p.next()), n(p.next()), n(p.next()))
}

fn json_string(body: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let after = &body[body.find(&pat)? + pat.len()..];
    let after = &after[after.find(':')? + 1..];
    let after = &after[after.find('"')? + 1..];
    Some(after[..after.find('"')?].to_string())
}

fn find_exe_asset(body: &str) -> Option<String> {
    let key = "\"browser_download_url\"";
    let mut start = 0;
    while let Some(i) = body[start..].find(key) {
        let abs = start + i;
        if let Some(u) = json_string(&body[abs..], "browser_download_url") {
            if u.to_lowercase().ends_with(".exe") {
                return Some(u);
            }
        }
        start = abs + key.len();
    }
    None
}

unsafe fn url_download(url: &str, dest: &std::path::Path) -> bool {
    let url_w = w(url);
    let dest_w = w(&dest.to_string_lossy());
    URLDownloadToFileW(None, PCWSTR(url_w.as_ptr()), PCWSTR(dest_w.as_ptr()), 0, None).is_ok()
}

unsafe fn check_for_updates(hwnd: HWND) {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let api = format!("https://api.github.com/repos/{UPDATE_REPO}/releases/latest?_={nonce}");
    let tmp = std::env::temp_dir();
    let json_path = tmp.join("keyflag_release.json");
    if !url_download(&api, &json_path) {
        show_dialog("Sem conexão", "Não foi possível verificar atualizações. Verifique sua conexão com a internet e tente novamente.", "OK", "");
        return;
    }
    let body = std::fs::read_to_string(&json_path).unwrap_or_default();
    let _ = std::fs::remove_file(&json_path);

    let latest = json_string(&body, "tag_name").unwrap_or_default();
    let cur = env!("CARGO_PKG_VERSION");
    if latest.is_empty() {
        show_dialog("Não foi possível verificar", "Não foi possível ler a versão mais recente do servidor.", "OK", "");
        return;
    }
    if ver_tuple(&latest) <= ver_tuple(cur) {
        show_dialog("Tudo atualizado", &format!("Você já está na versão mais recente ({latest})."), "OK", "");
        return;
    }
    let Some(url) = find_exe_asset(&body) else {
        show_dialog("Atualização disponível", &format!("A versão {latest} está disponível, mas o instalador não foi encontrado no release."), "OK", "");
        return;
    };

    let prompt = format!("A versão {latest} está disponível (você tem v{cur}).\n\nAtualizar agora? O KeyFlag será reiniciado automaticamente — sem assistente de instalação.");
    if !show_dialog("Atualização disponível", &prompt, "Atualizar agora", "Depois") {
        return;
    }
    let setup = tmp.join("KeyFlag-Setup.exe");
    if !url_download(&url, &setup) {
        show_dialog("Falha no download", "Não foi possível baixar o instalador. Tente novamente mais tarde.", "OK", "");
        return;
    }
    // Run the installer silently and quit so it can replace the running KeyFlag.exe. Setup
    // kills this instance (KillRunning), installs over it, then its [Run] entry relaunches the
    // new build — so the lone consent click above is the whole update, no wizard.
    let setup_w = w(&setup.to_string_lossy());
    let args_w = w("/VERYSILENT /SUPPRESSMSGBOXES /NORESTART");
    ShellExecuteW(hwnd, PCWSTR(w("open").as_ptr()), PCWSTR(setup_w.as_ptr()), PCWSTR(args_w.as_ptr()), PCWSTR::null(), SW_SHOWNORMAL);
    remove_tray(hwnd);
    PostQuitMessage(0);
}

// ===================== Borderless window chrome (custom close + drag) =====================
unsafe fn make_font(height: i32, weight: i32, underline: bool) -> HFONT {
    CreateFontW(
        -height,
        0,
        0,
        0,
        weight,
        0,
        if underline { 1 } else { 0 },
        0,
        DEFAULT_CHARSET.0 as u32,
        OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        (DEFAULT_PITCH.0 | (FF_DONTCARE.0 << 4) as u8) as u32,
        PCWSTR(w("Segoe UI").as_ptr()),
    )
}

const CLOSE_BTN_W: i32 = 46;
const CLOSE_BTN_H: i32 = 36;
const DRAG_STRIP_H: i32 = 92;

fn in_rect(r: RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn close_btn_rect(client_right: i32) -> RECT {
    RECT { left: client_right - CLOSE_BTN_W, top: 0, right: client_right, bottom: CLOSE_BTN_H }
}

unsafe fn paint_close_btn(hdc: HDC, client_right: i32, hot: bool) -> RECT {
    let rc = close_btn_rect(client_right);
    if hot {
        let hb = CreateSolidBrush(rgb(196, 43, 43));
        FillRect(hdc, &rc, hb);
        let _ = DeleteObject(hb);
    }
    let cx = (rc.left + rc.right) / 2;
    let cy = (rc.top + rc.bottom) / 2;
    let s = 5;
    let pen = CreatePen(PS_SOLID, 1, if hot { rgb(255, 255, 255) } else { rgb(180, 186, 198) });
    let old = SelectObject(hdc, pen);
    let _ = MoveToEx(hdc, cx - s, cy - s, None);
    let _ = LineTo(hdc, cx + s + 1, cy + s + 1);
    let _ = MoveToEx(hdc, cx + s, cy - s, None);
    let _ = LineTo(hdc, cx - s - 1, cy + s + 1);
    SelectObject(hdc, old);
    let _ = DeleteObject(pen);
    rc
}

unsafe fn setup_chrome(hwnd: HWND) {
    let _ = SendMessageW(hwnd, WM_SETICON, WPARAM(0), LPARAM(ABOUT_ICON)); // ICON_SMALL
    let _ = SendMessageW(hwnd, WM_SETICON, WPARAM(1), LPARAM(ABOUT_ICON)); // ICON_BIG
    let round: i32 = 2; // DWMWCP_ROUND
    let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(33), &round as *const _ as *const _, 4);
}

unsafe fn begin_drag_if_top(hwnd: HWND, x: i32, y: i32, client_right: i32) -> bool {
    if y < DRAG_STRIP_H && !in_rect(close_btn_rect(client_right), x, y) {
        let _ = ReleaseCapture();
        SendMessageW(hwnd, WM_NCLBUTTONDOWN, WPARAM(HTCAPTION as usize), LPARAM(0));
        return true;
    }
    false
}

// ===================== About window =====================
extern "system" fn about_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);

                let bg = CreateSolidBrush(rgb(24, 27, 35));
                FillRect(hdc, &rc, bg);
                let _ = DeleteObject(bg);

                let pad = 28;
                if ABOUT_ICON != 0 {
                    let _ = DrawIconEx(hdc, pad, 34, HICON(ABOUT_ICON as *mut _), 56, 56, 0, None, DI_NORMAL);
                }

                SetBkMode(hdc, TRANSPARENT);
                let title_font = make_font(27, 600, false);
                let body_font = make_font(17, 400, false);
                let link_font = make_font(17, 400, true);

                let text_x = pad + 56 + 18;
                let old = SelectObject(hdc, title_font);
                let _ = SetTextColor(hdc, rgb(250, 250, 252));
                let mut r = RECT { left: text_x, top: 36, right: rc.right - pad, bottom: 68 };
                let mut t = wn("KeyFlag");
                DrawTextW(hdc, &mut t, &mut r, DT_LEFT | DT_SINGLELINE);

                SelectObject(hdc, body_font);
                let _ = SetTextColor(hdc, rgb(150, 156, 170));
                let mut r2 = RECT { left: text_x, top: 70, right: rc.right - pad, bottom: 94 };
                let mut t2 = wn(&format!("Version {}", env!("CARGO_PKG_VERSION")));
                DrawTextW(hdc, &mut t2, &mut r2, DT_LEFT | DT_SINGLELINE);

                let _ = SetTextColor(hdc, rgb(196, 202, 214));
                let mut r3 = RECT { left: pad, top: 116, right: rc.right - pad, bottom: 144 };
                let mut t3 = wn("Single keyboard-layout enforcer");
                DrawTextW(hdc, &mut t3, &mut r3, DT_LEFT | DT_SINGLELINE);

                let _ = SetTextColor(hdc, rgb(150, 156, 170));
                let mut r4 = RECT { left: pad, top: 146, right: rc.right - pad, bottom: 174 };
                let mut t4 = wn("Locks Windows to US International or ABNT2.");
                DrawTextW(hdc, &mut t4, &mut r4, DT_LEFT | DT_SINGLELINE);

                SelectObject(hdc, link_font);
                let _ = SetTextColor(hdc, rgb(90, 150, 245));
                let link_top = rc.bottom - 40;
                let mut r5 = RECT { left: pad, top: link_top, right: rc.right - pad, bottom: rc.bottom - 14 };
                let mut t5 = wn("github.com/gabrielchaves6/keyflag");
                DrawTextW(hdc, &mut t5, &mut r5, DT_LEFT | DT_SINGLELINE);
                let mut sz = SIZE::default();
                let _ = GetTextExtentPoint32W(hdc, &t5, &mut sz);
                ABOUT_LINK = RECT { left: pad, top: link_top, right: pad + sz.cx, bottom: link_top + sz.cy };

                SelectObject(hdc, old);
                let _ = DeleteObject(title_font);
                let _ = DeleteObject(body_font);
                let _ = DeleteObject(link_font);
                ABOUT_CLOSE = paint_close_btn(hdc, rc.right, ABOUT_CLOSE_HOT);
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let x = (lparam.0 & 0xffff) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                begin_drag_if_top(hwnd, x, y, rc.right);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let x = (lparam.0 & 0xffff) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                if in_rect(ABOUT_CLOSE, x, y) {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                } else if in_rect(ABOUT_LINK, x, y) {
                    let _ = ShellExecuteW(
                        None,
                        PCWSTR(w("open").as_ptr()),
                        PCWSTR(w(ABOUT_URL).as_ptr()),
                        PCWSTR::null(),
                        PCWSTR::null(),
                        SW_SHOWNORMAL,
                    );
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let x = (lparam.0 & 0xffff) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                let hot = in_rect(ABOUT_CLOSE, x, y);
                if hot != ABOUT_CLOSE_HOT {
                    ABOUT_CLOSE_HOT = hot;
                    let _ = InvalidateRect(hwnd, Some(&ABOUT_CLOSE), FALSE);
                }
                LRESULT(0)
            }
            WM_SETCURSOR => {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = ScreenToClient(hwnd, &mut pt);
                if in_rect(ABOUT_LINK, pt.x, pt.y) || in_rect(ABOUT_CLOSE, pt.x, pt.y) {
                    if let Ok(hand) = LoadCursorW(None, IDC_HAND) {
                        SetCursor(hand);
                    }
                    return LRESULT(1);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_KEYDOWN if VIRTUAL_KEY(wparam.0 as u16) == VK_ESCAPE => {
                let _ = ShowWindow(hwnd, SW_HIDE);
                LRESULT(0)
            }
            WM_GETICON => LRESULT(ABOUT_ICON),
            WM_CLOSE => {
                let _ = ShowWindow(hwnd, SW_HIDE);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

unsafe fn show_about() {
    let hwnd = match ABOUT_HWND {
        Some(h) => h,
        None => return,
    };
    let w_px = 440;
    let h_px = 260;
    let work = ACTIVE_WORK;
    let (cx, cy) = if work.right > work.left {
        (work.left + (work.right - work.left) / 2, work.top + (work.bottom - work.top) / 2)
    } else {
        (GetSystemMetrics(SM_CXSCREEN) / 2, GetSystemMetrics(SM_CYSCREEN) / 2)
    };
    let _ = SetWindowPos(hwnd, HWND_TOPMOST, cx - w_px / 2, cy - h_px / 2, w_px, h_px, SWP_SHOWWINDOW);
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);
    let _ = InvalidateRect(hwnd, None, TRUE);
}

// ===================== Styled message dialog (About look) =====================
const DLG_W: i32 = 430;
const DLG_H: i32 = 248; // minimum dialog height (grown to fit the body)
const DLG_BTN_W: i32 = 116; // minimum button width
const DLG_BTN_H: i32 = 34;
const DLG_BTN_PAD_X: i32 = 18; // horizontal text padding inside a button
const DLG_BODY_GAP: i32 = 28; // vertical gap between the body text and the buttons

// The font used for button labels — shared by measuring and painting so widths match.
unsafe fn button_font() -> HFONT {
    make_font(17, 600, false)
}

// Width a button needs to fit `label`: text extent plus padding, never below the minimum.
unsafe fn button_width(hdc: HDC, label: &str) -> i32 {
    let font = button_font();
    let of = SelectObject(hdc, font);
    let mut t = wn(label);
    let mut r = RECT::default();
    DrawTextW(hdc, &mut t, &mut r, DT_CALCRECT | DT_SINGLELINE);
    SelectObject(hdc, of);
    let _ = DeleteObject(font);
    (r.right - r.left + 2 * DLG_BTN_PAD_X).max(DLG_BTN_W)
}

// Paint one flat, slightly-rounded button `width` wide and return its rect (for hit-testing).
unsafe fn paint_button(hdc: HDC, x: i32, y: i32, width: i32, label: &str, accent: bool) -> RECT {
    let rc = RECT { left: x, top: y, right: x + width, bottom: y + DLG_BTN_H };
    let fill = CreateSolidBrush(if accent { rgb(56, 118, 240) } else { rgb(48, 52, 62) });
    let pen = CreatePen(PS_SOLID, 1, if accent { rgb(56, 118, 240) } else { rgb(74, 80, 92) });
    let old_b = SelectObject(hdc, fill);
    let old_p = SelectObject(hdc, pen);
    let _ = RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, 12, 12);
    SelectObject(hdc, old_b);
    SelectObject(hdc, old_p);
    let _ = DeleteObject(fill);
    let _ = DeleteObject(pen);

    let font = make_font(17, 600, false);
    let of = SelectObject(hdc, font);
    SetBkMode(hdc, TRANSPARENT);
    let _ = SetTextColor(hdc, if accent { rgb(255, 255, 255) } else { rgb(214, 219, 228) });
    let mut t = wn(label);
    let mut r = rc;
    DrawTextW(hdc, &mut t, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    SelectObject(hdc, of);
    let _ = DeleteObject(font);
    rc
}

extern "system" fn dialog_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);

                let bg = CreateSolidBrush(rgb(24, 27, 35));
                FillRect(hdc, &rc, bg);
                let _ = DeleteObject(bg);

                let pad = 28;
                if ABOUT_ICON != 0 {
                    let _ = DrawIconEx(hdc, pad, 30, HICON(ABOUT_ICON as *mut _), 48, 48, 0, None, DI_NORMAL);
                }

                SetBkMode(hdc, TRANSPARENT);
                let title_font = make_font(24, 600, false);
                let body_font = make_font(17, 400, false);

                let text_x = pad + 48 + 16;
                let old = SelectObject(hdc, title_font);
                let _ = SetTextColor(hdc, rgb(250, 250, 252));
                let mut r = RECT { left: text_x, top: 38, right: rc.right - pad, bottom: 74 };
                let mut t = wn(&DLG_HEADING);
                DrawTextW(hdc, &mut t, &mut r, DT_LEFT | DT_SINGLELINE);

                SelectObject(hdc, body_font);
                let _ = SetTextColor(hdc, rgb(196, 202, 214));
                let mut r2 = RECT { left: pad, top: 96, right: rc.right - pad, bottom: rc.bottom - 64 };
                let mut t2 = wn(&DLG_BODY);
                DrawTextW(hdc, &mut t2, &mut r2, DT_LEFT | DT_WORDBREAK);
                SelectObject(hdc, old);
                let _ = DeleteObject(title_font);
                let _ = DeleteObject(body_font);

                // Buttons, bottom-right. Each is sized to its label so a longer label like
                // "Atualizar agora" never overflows the button.
                let by = rc.bottom - 20 - DLG_BTN_H;
                let pw = button_width(hdc, &DLG_PRIMARY);
                let px = rc.right - pad - pw;
                DLG_BTN_PRIMARY = paint_button(hdc, px, by, pw, &DLG_PRIMARY, true);
                if !DLG_SECONDARY.is_empty() {
                    let sw = button_width(hdc, &DLG_SECONDARY);
                    let sx = px - 12 - sw;
                    DLG_BTN_SECONDARY = paint_button(hdc, sx, by, sw, &DLG_SECONDARY, false);
                } else {
                    DLG_BTN_SECONDARY = RECT::default();
                }

                DLG_CLOSE = paint_close_btn(hdc, rc.right, DLG_CLOSE_HOT);
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let x = (lparam.0 & 0xffff) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                begin_drag_if_top(hwnd, x, y, rc.right);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let x = (lparam.0 & 0xffff) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                if in_rect(DLG_CLOSE, x, y) {
                    DLG_RESULT = 2;
                    let _ = DestroyWindow(hwnd);
                } else if in_rect(DLG_BTN_PRIMARY, x, y) {
                    DLG_RESULT = 1;
                    let _ = DestroyWindow(hwnd);
                } else if !DLG_SECONDARY.is_empty() && in_rect(DLG_BTN_SECONDARY, x, y) {
                    DLG_RESULT = 2;
                    let _ = DestroyWindow(hwnd);
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let x = (lparam.0 & 0xffff) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                let hot = in_rect(DLG_CLOSE, x, y);
                if hot != DLG_CLOSE_HOT {
                    DLG_CLOSE_HOT = hot;
                    let _ = InvalidateRect(hwnd, Some(&DLG_CLOSE), FALSE);
                }
                LRESULT(0)
            }
            WM_SETCURSOR => {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = ScreenToClient(hwnd, &mut pt);
                if in_rect(DLG_BTN_PRIMARY, pt.x, pt.y)
                    || in_rect(DLG_CLOSE, pt.x, pt.y)
                    || (!DLG_SECONDARY.is_empty() && in_rect(DLG_BTN_SECONDARY, pt.x, pt.y))
                {
                    if let Ok(hand) = LoadCursorW(None, IDC_HAND) {
                        SetCursor(hand);
                    }
                    return LRESULT(1);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_KEYDOWN => {
                match VIRTUAL_KEY(wparam.0 as u16) {
                    VK_RETURN => {
                        DLG_RESULT = 1;
                        let _ = DestroyWindow(hwnd);
                    }
                    VK_ESCAPE => {
                        DLG_RESULT = 2;
                        let _ = DestroyWindow(hwnd);
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_GETICON => LRESULT(ABOUT_ICON),
            WM_CLOSE => {
                if DLG_RESULT == 0 {
                    DLG_RESULT = 2;
                }
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

// Show a modal styled dialog and return true iff the primary button was chosen. `secondary`
// "" means a single (OK-style) button. Runs a nested message loop until the dialog closes.
unsafe fn show_dialog(heading: &str, body: &str, primary: &str, secondary: &str) -> bool {
    let hinstance: HINSTANCE = GetModuleHandleW(None).map(|h| h.into()).unwrap_or_default();
    let class = w("KeyFlagDialog");
    if !DLG_CLASS_REGISTERED {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(dialog_wndproc),
            hInstance: hinstance,
            lpszClassName: PCWSTR(class.as_ptr()),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: HICON(ABOUT_ICON as *mut _),
            ..Default::default()
        };
        RegisterClassW(&wc);
        DLG_CLASS_REGISTERED = true;
    }

    DLG_HEADING = heading.to_string();
    DLG_BODY = body.to_string();
    DLG_PRIMARY = primary.to_string();
    DLG_SECONDARY = secondary.to_string();
    DLG_RESULT = 0;

    // Grow the dialog vertically to fit the word-wrapped body, keeping a fixed gap above the
    // buttons — so a long prompt no longer collides with them. Floors at DLG_H.
    let dlg_h = {
        let screen = GetDC(None);
        let font = make_font(17, 400, false);
        let of = SelectObject(screen, font);
        let mut t = wn(body);
        let mut r = RECT { left: 0, top: 0, right: DLG_W - 2 * 28, bottom: 0 };
        DrawTextW(screen, &mut t, &mut r, DT_LEFT | DT_WORDBREAK | DT_CALCRECT);
        let body_h = r.bottom - r.top;
        SelectObject(screen, of);
        let _ = DeleteObject(font);
        ReleaseDC(None, screen);
        (96 + body_h + DLG_BODY_GAP + DLG_BTN_H + 20).max(DLG_H)
    };

    let work = ACTIVE_WORK;
    let (cx, cy) = if work.right > work.left {
        (work.left + (work.right - work.left) / 2, work.top + (work.bottom - work.top) / 2)
    } else {
        (GetSystemMetrics(SM_CXSCREEN) / 2, GetSystemMetrics(SM_CYSCREEN) / 2)
    };
    let hwnd = match CreateWindowExW(
        WINDOW_EX_STYLE(0),
        PCWSTR(class.as_ptr()),
        PCWSTR(w("KeyFlag").as_ptr()),
        WS_POPUP,
        cx - DLG_W / 2,
        cy - dlg_h / 2,
        DLG_W,
        dlg_h,
        None,
        None,
        hinstance,
        None,
    ) {
        Ok(h) => h,
        Err(_) => return false,
    };
    setup_chrome(hwnd);
    DLG_CLOSE_HOT = false;

    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);

    let mut m = MSG::default();
    loop {
        if DLG_RESULT != 0 {
            break;
        }
        if !GetMessageW(&mut m, None, 0, 0).as_bool() {
            break;
        }
        let _ = TranslateMessage(&m);
        DispatchMessageW(&m);
    }
    if IsWindow(hwnd).as_bool() {
        let _ = DestroyWindow(hwnd);
    }
    DLG_RESULT == 1
}

// ---------- Window proc (hidden message window) ----------
extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        // The shell dropped every icon when it recreated the tray — put ours back, or KeyFlag
        // keeps enforcing the layout with no visible badge and no way to reach its menu.
        if msg != 0 && msg == WM_TASKBARCREATED {
            add_tray(hwnd);
            return LRESULT(0);
        }
        match msg {
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            WM_TIMER => {
                enforce();
                LRESULT(0)
            }
            WM_TRAY => {
                let m = (lparam.0 as u32) & 0xffff;
                if m == WM_LBUTTONUP || m == WM_RBUTTONUP || m == WM_CONTEXTMENU {
                    show_tray_menu(hwnd);
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn main() -> Result<()> {
    unsafe {
        // Single instance: two enforcers would fight over the layout.
        let name = w("KeyFlag_singleton_mutex");
        let _mutex = CreateMutexW(None, TRUE, PCWSTR(name.as_ptr()));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }

        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        // Layouts are loaded on demand by apply_mode (called below), which also collapses the
        // session to a single input method so the OS keyboard indicator stays hidden.
        MODE = read_mode();

        let app_icon = load_app_icon();
        ABOUT_ICON = app_icon.0 as isize;

        let hinstance = GetModuleHandleW(None)?;
        let class_name = w("KeyFlagWnd");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance.into(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        RegisterClassW(&wc);

        // Hidden message window: receives the tray callback and the enforcement timer.
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(w("KeyFlag").as_ptr()),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            hinstance,
            None,
        )?;

        // About window: a real, interactive borderless window with the KeyFlag icon.
        let about_class = w("KeyFlagAbout");
        let wc_about = WNDCLASSW {
            lpfnWndProc: Some(about_wndproc),
            hInstance: hinstance.into(),
            lpszClassName: PCWSTR(about_class.as_ptr()),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hIcon: app_icon,
            ..Default::default()
        };
        RegisterClassW(&wc_about);
        let about_hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(about_class.as_ptr()),
            PCWSTR(w("About KeyFlag").as_ptr()),
            WS_POPUP,
            0,
            0,
            440,
            260,
            None,
            None,
            hinstance,
            None,
        )?;
        setup_chrome(about_hwnd);
        ABOUT_HWND = Some(about_hwnd);

        // Learn the shell's "recreate your icon" broadcast before adding the icon, so a shell
        // restart racing our startup can't slip through. ChangeWindowMessageFilterEx keeps UIPI
        // from dropping the broadcast if the shell ever runs at a different integrity level.
        WM_TASKBARCREATED = RegisterWindowMessageW(PCWSTR(w("TaskbarCreated").as_ptr()));
        if WM_TASKBARCREATED != 0 {
            let _ = ChangeWindowMessageFilterEx(hwnd, WM_TASKBARCREATED, MSGFLT_ALLOW, None);
        }

        add_tray(hwnd);
        // Apply the persisted mode immediately on startup.
        apply_mode(hwnd, MODE);

        // Snap back on every foreground change (the main trigger), plus a backup poll for
        // drifts that happen without a foreground change (e.g. a shortcut in the same window).
        let _hook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            HMODULE::default(),
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        SetTimer(hwnd, TIMER_ID, 350, None);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}
