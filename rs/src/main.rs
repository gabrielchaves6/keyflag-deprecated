#![windows_subsystem = "windows"]
// KeyboardFlag — keep Windows on a single keyboard layout, enforced globally.
//
// Windows' keyboard handling mixes layouts per-window, re-adds them, and exposes a
// dozen switch shortcuts (Win+Space, Alt+Shift, Ctrl+Shift). KeyboardFlag picks ONE
// layout — United States-International or Portuguese (Brazilian ABNT2) — and makes the
// whole session stay on it: whenever any app or the OS drifts to a different layout, it
// snaps the active window back. The only UI is a tray icon (its glyph shows the current
// mode: blue "US" / green "BR") with a right-click menu to switch modes or quit.
//
// Modeled after DeskFlag (same tray / message-loop / registry patterns), minus the
// Direct2D taskbar pill — here the tray icon is drawn with plain GDI.

use core::ffi::c_void;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::CreateMutexW;
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
const ID_EXIT: usize = 11;

// KLIDs (verified present on Windows 10/11 by default; the layout DLLs ship with the OS).
const KLID_US_INTL: &str = "00020409"; // United States-International (kbdusx.dll)
const KLID_ABNT2: &str = "00010416"; // Portuguese (Brazilian ABNT2) (kbdbr.dll)

const REG_SUBKEY: &str = "Software\\KeyboardFlag";

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    UsIntl,
    Abnt2,
}

// ---------- Globals (single-threaded; touched only from the message thread) ----------
static mut MODE: Mode = Mode::UsIntl;
static mut HKL_US: isize = 0;
static mut HKL_ABNT: isize = 0;
static mut TRAY_ICON: isize = 0; // current HICON shown in the tray (rebuilt on mode change)

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// UTF-16 without a NUL terminator (for DrawTextW).
fn wn(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
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
        let bytes =
            core::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2);
        let _ = RegSetValueExW(hkey, PCWSTR(w("Mode").as_ptr()), 0, REG_SZ, Some(bytes));
        let _ = RegCloseKey(hkey);
    }
}

// ---------- Layout enforcement ----------
fn hkl(v: isize) -> HKL {
    HKL(v as *mut c_void)
}

unsafe fn target_hkl() -> isize {
    match MODE {
        Mode::UsIntl => HKL_US,
        Mode::Abnt2 => HKL_ABNT,
    }
}

// Snap the foreground window back to the chosen layout if it has drifted. Cheap (a couple
// of syscalls) and a no-op when already correct, so it's safe to call on a tight timer and
// on every foreground change.
unsafe fn enforce() {
    let target = target_hkl();
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
        // Ask the focused window to switch its input language. This is the same mechanism
        // the language bar uses; it works cross-process for a loaded layout.
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

    let target = target_hkl();
    // Default input language for newly-created threads/processes.
    let mut h = target;
    let _ = SystemParametersInfoW(
        SPI_SETDEFAULTINPUTLANG,
        0,
        Some(&mut h as *mut _ as *mut c_void),
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
    );
    // Activate for our own thread too.
    let _ = ActivateKeyboardLayout(hkl(target), ACTIVATE_KEYBOARD_LAYOUT_FLAGS(0));
    // Flip every open window now, then make sure the focused one is correct.
    let _ = EnumWindows(Some(broadcast_proc), LPARAM(target));
    enforce();

    refresh_tray(hwnd);
}

// ---------- Tray icon (GDI-drawn glyph: "US" on blue / "BR" on green) ----------
fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

unsafe fn make_icon(mode: Mode) -> HICON {
    let sz = 32i32;
    let (bg, label) = match mode {
        Mode::UsIntl => (rgb(37, 99, 235), "US"),
        Mode::Abnt2 => (rgb(22, 163, 74), "BR"),
    };

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

    let font = CreateFontW(
        -22,
        0,
        0,
        0,
        FW_BOLD.0 as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET.0 as u32,
        OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        (DEFAULT_PITCH.0 | (FF_DONTCARE.0 << 4) as u8) as u32,
        PCWSTR(w("Segoe UI").as_ptr()),
    );
    let old_font = SelectObject(mem_dc, font);
    SetBkMode(mem_dc, TRANSPARENT);
    SetTextColor(mem_dc, rgb(255, 255, 255));
    let mut txt = wn(label);
    let mut tr = full;
    DrawTextW(
        mem_dc,
        &mut txt,
        &mut tr,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
    );
    SelectObject(mem_dc, old_font);
    let _ = DeleteObject(font);

    // GDI never writes the alpha channel, so force the whole 32bpp bitmap opaque — otherwise
    // the icon comes out fully transparent (invisible) in the tray.
    let p = bits as *mut u8;
    for i in 0..(sz * sz) as usize {
        *p.add(i * 4 + 3) = 255;
    }

    SelectObject(mem_dc, old);
    // All-zero AND mask: alpha (forced to 255) governs opacity for a 32bpp icon.
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

unsafe fn tray_base(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW::default();
    nid.cbSize = core::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid
}

unsafe fn tooltip() -> &'static str {
    match MODE {
        Mode::UsIntl => "KeyboardFlag — US International",
        Mode::Abnt2 => "KeyboardFlag — Português ABNT2",
    }
}

unsafe fn add_tray(hwnd: HWND) {
    let icon = make_icon(MODE);
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

// Rebuild the icon + tooltip for the current mode.
unsafe fn refresh_tray(hwnd: HWND) {
    let icon = make_icon(MODE);
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

unsafe fn show_tray_menu(hwnd: HWND) {
    let menu = CreatePopupMenu().unwrap_or_default();
    let _ = AppendMenuW(menu, MF_STRING, ID_US, PCWSTR(w("US International").as_ptr()));
    let _ = AppendMenuW(menu, MF_STRING, ID_ABNT, PCWSTR(w("Português ABNT2").as_ptr()));
    // Dot the active mode.
    let active = if MODE == Mode::UsIntl { ID_US } else { ID_ABNT } as u32;
    let _ = CheckMenuRadioItem(menu, ID_US as u32, ID_ABNT as u32, active, MF_BYCOMMAND.0);
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, ID_ABOUT, PCWSTR(w("About KeyboardFlag").as_ptr()));
    let _ = AppendMenuW(menu, MF_STRING, ID_EXIT, PCWSTR(w("Exit").as_ptr()));

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD, pt.x, pt.y, 0, hwnd, None);
    let _ = DestroyMenu(menu);
    match cmd.0 as usize {
        ID_US => apply_mode(hwnd, Mode::UsIntl),
        ID_ABNT => apply_mode(hwnd, Mode::Abnt2),
        ID_ABOUT => {
            let body = w(concat!(
                "KeyboardFlag v",
                env!("CARGO_PKG_VERSION"),
                "\n\nKeeps Windows on a single keyboard layout\n(US International or Português ABNT2).\n\nPick a layout from this tray menu — it stays put."
            ));
            let title = w("KeyboardFlag");
            MessageBoxW(hwnd, PCWSTR(body.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONINFORMATION);
        }
        ID_EXIT => {
            remove_tray(hwnd);
            PostQuitMessage(0);
        }
        _ => {}
    }
}

// ---------- Window proc ----------
extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
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
        let name = w("KeyboardFlag_singleton_mutex");
        let _mutex = CreateMutexW(None, TRUE, PCWSTR(name.as_ptr()));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }

        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        // Load both target layouts into the session so either can be activated/forced.
        HKL_US = LoadKeyboardLayoutW(PCWSTR(w(KLID_US_INTL).as_ptr()), KLF_ACTIVATE)
            .map(|h| h.0 as isize)
            .unwrap_or(0);
        HKL_ABNT = LoadKeyboardLayoutW(PCWSTR(w(KLID_ABNT2).as_ptr()), KLF_ACTIVATE)
            .map(|h| h.0 as isize)
            .unwrap_or(0);

        MODE = read_mode();

        let hinstance = GetModuleHandleW(None)?;
        let class_name = w("KeyboardFlagWnd");
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
            PCWSTR(w("KeyboardFlag").as_ptr()),
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
