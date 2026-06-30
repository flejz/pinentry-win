//! Win32 dialog implementation.
//!
//! Custom WNDCLASS — no .rc resource files needed.
//! Dialog state is shared with WndProc via a thread-local (one dialog at a time).

use std::cell::RefCell;
use std::mem::size_of;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
use windows::Win32::UI::WindowsAndMessaging::*;
use zeroize::Zeroizing;

use crate::state::PinentryState;

// ── Control IDs ────────────────────────────────────────────────────────────────
const BTN_OK: i32 = 1;      // IDOK
const BTN_CANCEL: i32 = 2;  // IDCANCEL
const BTN_NOTOK: i32 = 10;
const EDIT_PIN: i32 = 100;
const STATIC_DESC: i32 = 101;
const STATIC_ERROR: i32 = 102;
const STATIC_PROMPT: i32 = 103;

// ── Static / Edit / Button window-style bits (not always re-exported) ──────────
const SS_LEFT_ALIGN: u32  = 0x0000_0000;
const SS_RIGHT_ALIGN: u32 = 0x0000_0002;
const SS_NOPREFIX_BIT: u32 = 0x0000_0080; // SS_NOPREFIX
const ES_PASSWORD_BIT: u32  = 0x0000_0020; // ES_PASSWORD
const ES_AUTOHSCROLL_BIT: u32 = 0x0000_0080; // ES_AUTOHSCROLL
const BS_PUSHBUTTON_STYLE: u32     = 0x0000_0000;
const BS_DEFPUSHBUTTON_STYLE: u32  = 0x0000_0001;

// WM_NEXTDLGCTL — route keyboard focus to a specific child control
const EM_LIMITTEXT_MSG: u32  = 0x00C5; // EM_LIMITTEXT / EM_SETLIMITTEXT
const WM_SETFONT_MSG: u32    = 0x0030; // WM_SETFONT

// ── Window class name (static wide, null-terminated) ──────────────────────────
static CLASS_NAME: &[u16] = &[
    b'P' as u16, b'i' as u16, b'n' as u16, b'W' as u16, b'i' as u16,
    b'n' as u16, 0,
];

// ── Dialog mode ────────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode { GetPin, Confirm, Message }

// ── Thread-local shared with WndProc ──────────────────────────────────────────
struct Tls {
    mode: Mode,
    desc: Vec<u16>,
    error: Vec<u16>,
    prompt: Vec<u16>,
    ok_label: Vec<u16>,
    cancel_label: Vec<u16>,
    notok_label: Vec<u16>,
    has_desc: bool,
    has_error: bool,
    has_notok: bool,
    one_button: bool,
    // written by WndProc
    pin: Zeroizing<Vec<u16>>,
    confirmed: bool,
    canceled: bool,
    // per-monitor DPI (dots per inch); default 96 = 100% scaling
    dpi: u32,
}

impl Default for Tls {
    fn default() -> Self {
        Tls {
            mode: Mode::Message,
            desc: w(""),
            error: w(""),
            prompt: w("PIN:"),
            ok_label: w("OK"),
            cancel_label: w("Cancel"),
            notok_label: w("No"),
            has_desc: false,
            has_error: false,
            has_notok: false,
            one_button: false,
            pin: Zeroizing::new(vec![]),
            confirmed: false,
            canceled: false,
            dpi: 96,
        }
    }
}

thread_local! {
    static TLS: RefCell<Tls> = RefCell::new(Tls::default());
}

// ── Helpers ────────────────────────────────────────────────────────────────────

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// GTK uses '_' as accelerator prefix; Win32 uses '&'. Strip leading '_' from
// button labels so gpg-agent's "_OK" / "_Cancel" render cleanly.
fn w_btn(s: &str) -> Vec<u16> {
    w(s.trim_start_matches('_'))
}

fn wlen(buf: &[u16]) -> usize {
    buf.iter().position(|&c| c == 0).unwrap_or(buf.len())
}

/// Scale a baseline-96-DPI pixel value to the actual monitor DPI.
fn scale(px: i32, dpi: u32) -> i32 {
    (px as i64 * dpi as i64 / 96i64) as i32
}

// ── WndProc ────────────────────────────────────────────────────────────────────

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            on_create(hwnd);
            LRESULT(0)
        }

        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            match id {
                BTN_OK => {
                    on_ok(hwnd);
                    let _ = DestroyWindow(hwnd);
                }
                BTN_CANCEL | BTN_NOTOK => {
                    TLS.with(|c| c.borrow_mut().canceled = true);
                    let _ = DestroyWindow(hwnd);
                }
                _ => {}
            }
            LRESULT(0)
        }

        WM_ACTIVATE => {
            // Re-focus EDIT whenever window is activated (click-away and back).
            // WA_INACTIVE = 0; any non-zero value means becoming active.
            let is_active = (wparam.0 & 0xFFFF) != 0;
            if is_active {
                let is_pin = TLS.with(|c| c.borrow().mode == Mode::GetPin);
                if is_pin {
                    if let Ok(hedit) = GetDlgItem(Some(hwnd), EDIT_PIN) {
                        let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(hedit));
                    }
                }
            }
            LRESULT(0)
        }

        WM_DPICHANGED => {
            let new_dpi = (wparam.0 & 0xFFFF) as u32;
            // lParam = suggested RECT from Windows — use it directly
            let rect = &*(lparam.0 as *const RECT);
            let _ = SetWindowPos(
                hwnd, None,
                rect.left, rect.top,
                rect.right - rect.left, rect.bottom - rect.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            // Update DPI and rebuild all children at new scale
            TLS.with(|c| c.borrow_mut().dpi = new_dpi);
            // Destroy existing children
            for &id in &[STATIC_DESC, STATIC_ERROR, STATIC_PROMPT,
                         EDIT_PIN, BTN_OK, BTN_CANCEL, BTN_NOTOK] {
                if let Ok(h) = GetDlgItem(Some(hwnd), id) {
                    let _ = DestroyWindow(h);
                }
            }
            // Recreate at new DPI (on_create reads from TLS.dpi)
            on_create(hwnd);
            // Restore focus to EDIT
            if TLS.with(|c| c.borrow().mode == Mode::GetPin) {
                if let Ok(h) = GetDlgItem(Some(hwnd), EDIT_PIN) {
                    let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(h));
                }
            }
            LRESULT(0)
        }

        WM_CLOSE => {
            TLS.with(|c| c.borrow_mut().canceled = true);
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }

        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }

        WM_CTLCOLORSTATIC => {
            // Paint error label red; leave all other statics at system defaults.
            let hctl = HWND(lparam.0 as *mut _);
            let hdc  = HDC(wparam.0 as *mut _);
            let is_err = TLS.with(|c| {
                let b = c.borrow();
                b.has_error &&
                    GetDlgItem(Some(hwnd), STATIC_ERROR)
                        .map(|h| h == hctl)
                        .unwrap_or(false)
            });
            if is_err {
                SetTextColor(hdc, COLORREF(0x0000_00FF)); // R=FF G=00 B=00 (0x00BBGGRR)
                SetBkMode(hdc, TRANSPARENT);
                LRESULT(GetStockObject(NULL_BRUSH).0 as isize)
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// ── Send system font to a control ─────────────────────────────────────────────

unsafe fn send_font(ctrl: HWND, font: HGDIOBJ) {
    SendMessageW(ctrl, WM_SETFONT_MSG, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
}

// ── Create child controls on WM_CREATE ────────────────────────────────────────

unsafe fn on_create(hwnd: HWND) {
    let hmod = GetModuleHandleW(None).unwrap_or_default();
    let hinstance: HINSTANCE = hmod.into();

    // Segoe UI 9pt (the Windows message font) — DEFAULT_GUI_FONT is Segoe UI on Win10/11
    let font = GetStockObject(DEFAULT_GUI_FONT);

    // Apply font to the parent too (affects WM_CTLCOLORSTATIC background brush)
    send_font(hwnd, font);

    TLS.with(|cell| {
        let tls = cell.borrow();
        let d = tls.dpi;

        let pad    = scale(16,  d);
        let cw     = scale(440, d);
        let ctrl_w = cw - pad * 2;

        let mut y = pad;

        // ── Description ──────────────────────────────────────────────────
        if tls.has_desc {
            if let Ok(h) = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                PCWSTR(tls.desc.as_ptr()),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_LEFT_ALIGN | SS_NOPREFIX_BIT),
                pad, y, ctrl_w, scale(68, d),
                Some(hwnd),
                Some(HMENU(STATIC_DESC as isize as *mut _)),
                Some(hinstance),
                None,
            ) { send_font(h, font); }
            y += scale(76, d);
        }

        // ── Error (red text via WM_CTLCOLORSTATIC) ───────────────────────
        if tls.has_error {
            if let Ok(h) = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                PCWSTR(tls.error.as_ptr()),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_LEFT_ALIGN | SS_NOPREFIX_BIT),
                pad, y, ctrl_w, scale(20, d),
                Some(hwnd),
                Some(HMENU(STATIC_ERROR as isize as *mut _)),
                Some(hinstance),
                None,
            ) { send_font(h, font); }
            y += scale(28, d);
        }

        // ── Prompt label + password EDIT ─────────────────────────────────
        if tls.mode == Mode::GetPin {
            let label_w   = scale(100, d);
            let x_offset  = scale(108, d);
            let edit_h    = scale(26,  d);

            if let Ok(h) = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                PCWSTR(tls.prompt.as_ptr()),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_RIGHT_ALIGN | SS_NOPREFIX_BIT),
                pad, y + scale(4, d), label_w, scale(20, d),
                Some(hwnd),
                Some(HMENU(STATIC_PROMPT as isize as *mut _)),
                Some(hinstance),
                None,
            ) { send_font(h, font); }

            if let Ok(hedit) = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                w!("EDIT"),
                None,
                WS_CHILD | WS_VISIBLE | WS_TABSTOP
                    | WINDOW_STYLE(ES_PASSWORD_BIT | ES_AUTOHSCROLL_BIT),
                pad + x_offset, y, ctrl_w - x_offset, edit_h,
                Some(hwnd),
                Some(HMENU(EDIT_PIN as isize as *mut _)),
                Some(hinstance),
                None,
            ) {
                send_font(hedit, font);
                SendMessageW(hedit, EM_LIMITTEXT_MSG, Some(WPARAM(2048)), Some(LPARAM(0)));
            }
            y += scale(42, d);
        }

        // ── Buttons (right-aligned) ───────────────────────────────────────
        let btn_h   = scale(28, d);
        let btn_w   = scale(92, d);
        let btn_gap = scale(8,  d);
        let btn_y   = y + scale(12, d);
        let mut btn_x = cw - pad - btn_w;

        // OK — always present, default push button
        if let Ok(h) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("BUTTON"),
            PCWSTR(tls.ok_label.as_ptr()),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_DEFPUSHBUTTON_STYLE),
            btn_x, btn_y, btn_w, btn_h,
            Some(hwnd),
            Some(HMENU(BTN_OK as isize as *mut _)),
            Some(hinstance),
            None,
        ) { send_font(h, font); }
        btn_x -= btn_w + btn_gap;

        if !tls.one_button && tls.mode != Mode::Message {
            // Cancel
            if let Ok(h) = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("BUTTON"),
                PCWSTR(tls.cancel_label.as_ptr()),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON_STYLE),
                btn_x, btn_y, btn_w, btn_h,
                Some(hwnd),
                Some(HMENU(BTN_CANCEL as isize as *mut _)),
                Some(hinstance),
                None,
            ) { send_font(h, font); }
            btn_x -= btn_w + btn_gap;

            // "No" — only present when SETNOTOK was sent
            if tls.has_notok {
                if let Ok(h) = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("BUTTON"),
                    PCWSTR(tls.notok_label.as_ptr()),
                    WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON_STYLE),
                    btn_x, btn_y, btn_w, btn_h,
                    Some(hwnd),
                    Some(HMENU(BTN_NOTOK as isize as *mut _)),
                    Some(hinstance),
                    None,
                ) { send_font(h, font); }
            }
        }
        let _ = btn_x; // suppress unused warning
    });
}

// ── Read PIN and mark confirmed ────────────────────────────────────────────────

unsafe fn on_ok(hwnd: HWND) {
    TLS.with(|cell| {
        let mut tls = cell.borrow_mut();
        tls.confirmed = true;

        if tls.mode == Mode::GetPin {
            if let Ok(hedit) = GetDlgItem(Some(hwnd), EDIT_PIN) {
                let len = GetWindowTextLengthW(hedit) as usize;
                let mut buf = Zeroizing::new(vec![0u16; len + 2]);
                GetWindowTextW(hedit, &mut buf[..]);
                tls.pin = Zeroizing::new(buf[..len].to_vec());
                // Blank the edit control so the secret doesn't linger in the control
                let empty: Vec<u16> = vec![0u16];
                let _ = SetWindowTextW(hedit, PCWSTR(empty.as_ptr()));
            }
        }
    });
}

// ── Dynamic client dimensions ──────────────────────────────────────────────────

fn client_height() -> i32 {
    TLS.with(|cell| {
        let t = cell.borrow();
        let d = t.dpi;
        let mut h = scale(16, d); // top pad
        if t.has_desc  { h += scale(76, d); }
        if t.has_error { h += scale(28, d); }
        if t.mode == Mode::GetPin { h += scale(42, d); }
        h + scale(12, d) + scale(28, d) + scale(16, d) // btn gap + btn h + bottom pad
    })
}

fn client_width() -> i32 {
    TLS.with(|c| scale(440, c.borrow().dpi))
}

// ── Public entry points ────────────────────────────────────────────────────────

pub fn show_getpin(state: &PinentryState) -> Result<Zeroizing<String>, u32> {
    TLS.with(|cell| {
        *cell.borrow_mut() = Tls {
            mode: Mode::GetPin,
            desc: w(state.desc_str().unwrap_or("")),
            error: w(state.error_str().unwrap_or("")),
            prompt: w(state.prompt_str()),
            ok_label: w_btn(state.ok_str()),
            cancel_label: w_btn(state.cancel_str()),
            notok_label: w_btn(state.notok_str()),
            has_desc: state.desc_str().is_some(),
            has_error: state.error_str().is_some(),
            has_notok: false,
            one_button: false,
            ..Tls::default()
        };
    });

    run_dialog(state.title_str()).map_err(|_| crate::error::GPG_ERR_CANCELED)?;

    if TLS.with(|c| c.borrow().canceled) {
        return Err(crate::error::GPG_ERR_CANCELED);
    }

    let pin = TLS.with(|c| {
        let mut t = c.borrow_mut();
        let raw = std::mem::take(&mut *t.pin);
        let s = String::from_utf16_lossy(&raw[..wlen(&raw)]);
        Zeroizing::new(s)
    });
    Ok(pin)
}

pub fn show_confirm(state: &PinentryState) -> Result<bool, u32> {
    let mode = if state.one_button { Mode::Message } else { Mode::Confirm };
    let desc_text = state.desc_str().unwrap_or(if state.one_button { state.prompt_str() } else { "" });

    TLS.with(|cell| {
        *cell.borrow_mut() = Tls {
            mode,
            desc: w(desc_text),
            error: w(state.error_str().unwrap_or("")),
            prompt: w(state.prompt_str()),
            ok_label: w_btn(state.ok_str()),
            cancel_label: w_btn(state.cancel_str()),
            notok_label: w_btn(state.notok_str()),
            has_desc: state.desc_str().is_some() || state.one_button,
            has_error: state.error_str().is_some(),
            has_notok: state.notok_set,
            one_button: state.one_button,
            ..Tls::default()
        };
    });

    run_dialog(state.title_str()).map_err(|_| crate::error::GPG_ERR_CANCELED)?;

    if TLS.with(|c| c.borrow().canceled) {
        Err(crate::error::GPG_ERR_NOT_CONFIRMED)
    } else {
        Ok(true)
    }
}

// ── Core dialog runner ─────────────────────────────────────────────────────────

fn run_dialog(title: &str) -> anyhow::Result<()> {
    unsafe {
        let hmod = GetModuleHandleW(None)?;
        let hinstance: HINSTANCE = hmod.into();

        // Register our window class; ignore "already exists" error.
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH((COLOR_BTNFACE.0 as isize + 1) as *mut _),
            lpszClassName: PCWSTR(CLASS_NAME.as_ptr()),
            ..Default::default()
        };
        RegisterClassExW(&wc);

        // Compute a preliminary centered position at 96 DPI to find which monitor
        // the window will land on, then detect that monitor's actual DPI.
        let style    = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN;
        let ex_style = WS_EX_DLGMODALFRAME | WS_EX_TOPMOST;

        // Preliminary size at default DPI to compute center point.
        let mut pre_rect = RECT { left: 0, top: 0, right: 440, bottom: client_height() };
        let _ = AdjustWindowRectEx(&mut pre_rect, style, false, ex_style);
        let pre_w = pre_rect.right  - pre_rect.left;
        let pre_h = pre_rect.bottom - pre_rect.top;
        let cx = GetSystemMetrics(SM_CXSCREEN);
        let cy = GetSystemMetrics(SM_CYSCREEN);
        let pre_x = (cx - pre_w) / 2;
        let pre_y = (cy - pre_h) / 2;

        // Detect DPI of the monitor that contains the preliminary center point.
        let dpi = {
            let pt = POINT { x: pre_x + pre_w / 2, y: pre_y + pre_h / 2 };
            let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTOPRIMARY);
            let mut dpix = 0u32;
            let mut dpiy = 0u32;
            let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dpix, &mut dpiy);
            if dpix == 0 { 96 } else { dpix }
        };
        TLS.with(|c| c.borrow_mut().dpi = dpi);

        // Recompute exact total window size now that we know the real DPI.
        let mut rect = RECT { left: 0, top: 0, right: client_width(), bottom: client_height() };
        let _ = AdjustWindowRectEx(&mut rect, style, false, ex_style);
        let total_w = rect.right  - rect.left;
        let total_h = rect.bottom - rect.top;

        let x = (cx - total_w) / 2;
        let y = (cy - total_h) / 2;

        let title_wide = w(title);

        let hwnd = CreateWindowExW(
            ex_style,
            PCWSTR(CLASS_NAME.as_ptr()),
            PCWSTR(title_wide.as_ptr()),
            style,
            x, y, total_w, total_h,
            None,             // no owner window
            None,             // no menu bar
            Some(hinstance),
            None,             // no CREATESTRUCT extra data
        )?;

        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = UpdateWindow(hwnd);

        // Set focus to the EDIT control after the window is visible and active.
        let mode = TLS.with(|c| c.borrow().mode);
        if mode == Mode::GetPin {
            if let Ok(hedit) = GetDlgItem(Some(hwnd), EDIT_PIN) {
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(hedit));
            }
        }

        // Message loop. IsDialogMessage handles Tab, Enter, Escape for us.
        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 == 0 || r.0 == -1 {
                break;
            }
            // Ctrl+Backspace: intercept before dispatch — WM_KEYDOWN goes to the
            // focused EDIT child, not the parent WndProc, so we must catch it here.
            if msg.message == WM_KEYDOWN
                && msg.wParam.0 as u16 == 0x08 // VK_BACK
                && (GetKeyState(0x11 /* VK_CONTROL */) as i16) < 0
            {
                if let Ok(hedit) = GetDlgItem(Some(hwnd), EDIT_PIN) {
                    let empty = [0u16];
                    let _ = SetWindowTextW(hedit, PCWSTR(empty.as_ptr()));
                }
                continue; // consume — don't let EDIT handle the backspace too
            }
            if IsDialogMessageW(hwnd, &mut msg).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        Ok(())
    }
}
