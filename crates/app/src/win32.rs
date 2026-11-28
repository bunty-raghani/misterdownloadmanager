//! The only Windows FFI in Mister Download Manager (D-008): IDM's dialog frame for our dialog windows, and the
//! shell's own small file-type icons (the ones Explorer and IDM show — Windows' artwork, not IDM's),
//! and the drop target's OLE `IDropTarget` (`ole`).
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::mem::{size_of, zeroed};

/// Stable identity of the opened object; paths alone do not prove ownership.
/// Volume, file index and creation time: file indexes are reused (FAT and exFAT derive them
/// from the directory entry, ReFS does not promise uniqueness), a creation time tells a file
/// that took the name later from the one recorded. Without all three, no identity.
pub fn torrent_file_identity(file: &std::fs::File) -> std::io::Result<[u64; 5]> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_BASIC_INFO, FILE_ID_INFO, FileBasicInfo, FileIdInfo, GetFileInformationByHandleEx,
    };
    // SAFETY: every field of this output structure admits zero.
    let mut id: FILE_ID_INFO = unsafe { zeroed() };
    // SAFETY: every field of this output structure admits zero.
    let mut basic: FILE_BASIC_INFO = unsafe { zeroed() };
    // SAFETY: the file owns a live handle and info is a writable output structure.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&mut id as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the live handle and correctly sized output are valid for FileBasicInfo.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileBasicInfo,
            (&mut basic as *mut FILE_BASIC_INFO).cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    if id.FileId.Identifier == [0; 16] || basic.CreationTime <= 0 || !keeps_file_ids(file) {
        return Err(std::io::ErrorKind::Unsupported.into());
    }
    Ok([
        id.VolumeSerialNumber,
        u64::from_le_bytes(id.FileId.Identifier[..8].try_into().unwrap()),
        u64::from_le_bytes(id.FileId.Identifier[8..].try_into().unwrap()),
        basic.CreationTime as u64,
        0,
    ])
}

/// Whether the file's volume is NTFS or ReFS, where a file's index is its own and a recreated
/// file gets a new one. FAT and exFAT make the index from the entry's place in its folder and
/// restore a creation time within about 15 seconds ("tunnelling"), so a file saved under the
/// same name just after another was deleted looks the same: there, no identity is given, and
/// the files are never deleted by MDM and a start asks before writing to them.
fn keeps_file_ids(file: &std::fs::File) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationByHandleW;
    let mut name = [0u16; 32];
    // SAFETY: the file owns a live handle; `name` is a writable buffer of the length given, and
    // the other outputs are not asked for.
    let ok = unsafe {
        GetVolumeInformationByHandleW(
            file.as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        )
    };
    if ok == 0 {
        return false;
    }
    let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
    let name = String::from_utf16_lossy(&name[..end]);
    name.eq_ignore_ascii_case("NTFS") || name.eq_ignore_ascii_case("ReFS")
}

pub fn torrent_file_identity_at(path: &std::path::Path) -> std::io::Result<[u64; 5]> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
    };
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    torrent_file_identity(&file)
}

/// Open (or, with `create`, make with `create_new`) the file at `path` for the torrent engine
/// without following a link at its end: `FILE_FLAG_OPEN_REPARSE_POINT` opens a symbolic link
/// itself, not what it points to, and one is refused. A folder is refused too.
pub fn open_torrent_file(path: &std::path::Path, create: bool) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
    };
    let mut options = std::fs::OpenOptions::new();
    options
        .read(true)
        .write(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    if create {
        options.create_new(true);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    let attributes = metadata.file_attributes();
    if attributes & FILE_ATTRIBUTE_DIRECTORY != 0
        || (attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 && metadata.file_type().is_symlink())
    {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    Ok(file)
}

/// Where the opened object really is: the system's final path of the handle, links resolved.
pub fn torrent_file_path(file: &std::fs::File) -> std::io::Result<std::path::PathBuf> {
    use std::os::windows::{ffi::OsStringExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
    let handle = file.as_raw_handle();
    let mut buffer = vec![0u16; 1024];
    loop {
        // SAFETY: the live handle and a buffer of the length given.
        let n = unsafe {
            GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), buffer.len() as u32, 0)
        } as usize;
        if n == 0 {
            return Err(std::io::Error::last_os_error());
        }
        if n < buffer.len() {
            buffer.truncate(n);
            return Ok(std::ffi::OsString::from_wide(&buffer).into());
        }
        buffer.resize(n + 1, 0);
    }
}

/// Delete the verified object through its handle, even if the path is replaced. The handle is
/// opened sharing read, write and delete, so a program that has the file open (the torrent
/// engine on its way out, an antivirus scanning a file just made, the indexer) does not stop it,
/// and the name goes at once, as `std::fs::remove_file` does; where the system doesn't know that
/// way (before Windows 10 1709) the older one is used. A refusal that is only a moment's
/// contention is tried again a few times.
pub fn remove_owned_torrent_file(
    path: &std::path::Path,
    expected: [u64; 5],
) -> std::io::Result<bool> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
        FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO, FILE_DISPOSITION_INFO_EX,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FileDispositionInfo, FileDispositionInfoEx, SetFileInformationByHandle,
    };
    let mut last = None;
    for attempt in 0..4 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(100 * attempt));
        }
        let file = match std::fs::OpenOptions::new()
            .access_mode(DELETE | FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
        {
            Ok(file) => file,
            // Gone already, or a refusal that may pass: look again.
            Err(e) if matches!(e.raw_os_error(), Some(5 | 32)) => {
                last = Some(e);
                continue;
            }
            Err(e) => return Err(e),
        };
        if torrent_file_identity(&file)? != expected {
            return Ok(false);
        }
        let handle = file.as_raw_handle();
        let ex = FILE_DISPOSITION_INFO_EX {
            Flags: FILE_DISPOSITION_FLAG_DELETE
                | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
                | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
        };
        // SAFETY: the live handle permits deletion; the structure and its size are the ones
        // the information class names.
        if unsafe {
            SetFileInformationByHandle(
                handle,
                FileDispositionInfoEx,
                (&ex as *const FILE_DISPOSITION_INFO_EX).cast(),
                size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
            )
        } != 0
        {
            return Ok(true);
        }
        let first = std::io::Error::last_os_error();
        let info = FILE_DISPOSITION_INFO { DeleteFile: true };
        // SAFETY: as above, for the older structure.
        if unsafe {
            SetFileInformationByHandle(
                handle,
                FileDispositionInfo,
                (&info as *const FILE_DISPOSITION_INFO).cast(),
                size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        } != 0
        {
            return Ok(true);
        }
        let second = std::io::Error::last_os_error();
        if matches!(second.raw_os_error(), Some(5 | 32))
            || matches!(first.raw_os_error(), Some(5 | 32))
        {
            last = Some(second);
            continue;
        }
        return Err(second);
    }
    Err(last.unwrap_or_else(|| std::io::ErrorKind::PermissionDenied.into()))
}

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, ReleaseDC,
};
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::Shell::{
    DefSubclassProc, SHFILEINFOW, SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW, SetWindowSubclass,
};
#[cfg(target_pointer_width = "64")]
use windows_sys::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, EnumWindows, GWL_EXSTYLE, GWL_STYLE, GetIconInfo, GetWindowLongPtrW,
    GetWindowThreadProcessId, ICONINFO, InternalGetWindowText, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOZORDER, SetWindowPos, WM_NCCALCSIZE, WM_NCHITTEST, WS_EX_DLGMODALFRAME,
    WS_THICKFRAME,
};

/// 32-bit Windows: `SetWindowLongPtrW` is `SetWindowLongW` there, whose value is 32 bits (all a
/// pointer is on it); taking a pointer-sized value as the 64-bit one does, the calls below are the
/// same on both.
#[cfg(target_pointer_width = "32")]
#[allow(non_snake_case)]
unsafe fn SetWindowLongPtrW(
    hwnd: HWND,
    index: windows_sys::Win32::UI::WindowsAndMessaging::WINDOW_LONG_PTR_INDEX,
    value: isize,
) -> isize {
    // SAFETY: the caller's call, as the 64-bit function's.
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SetWindowLongW(hwnd, index, value as i32)
            as isize
    }
}

struct Find {
    pid: u32,
    title: Vec<u16>,
    /// Where each window's title is read: as long as the title looked for, one more unit and the
    /// NUL, so a longer title reads longer, never the same. A fixed 256 units cut every title to
    /// 255, and a window named after a long file was never found (#540).
    buf: Vec<u16>,
    found: HWND,
}

unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
    // SAFETY: lparam is the &mut Find passed to EnumWindows below, alive for the call.
    let f = unsafe { &mut *(lparam as *mut Find) };
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid != f.pid {
        return 1;
    }
    // The title as the window keeps it, read without a message: GetWindowTextW sends WM_GETTEXT
    // to a window of this process, and one whose thread is busy (on a stalled disk: the file
    // icons' COM window) held the window's thread until it answered (#583).
    // SAFETY: InternalGetWindowText writes at most buf.len() units, the NUL included.
    let n = unsafe { InternalGetWindowText(hwnd, f.buf.as_mut_ptr(), f.buf.len() as i32) };
    if n > 0 && f.buf[..n as usize] == f.title[..] {
        f.found = hwnd;
        return 0;
    }
    1
}

/// The window of this thread that holds the mouse capture (a button is down in it), if any.
fn own_capture() -> Option<HWND> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetCapture;
    // SAFETY: a plain query; GetCapture only sees this thread's windows.
    let h = unsafe { GetCapture() };
    (!h.is_null()).then_some(h)
}

/// The main window of this run, once it exists ([`set_main_window`]). 0: not known yet.
static MAIN_WINDOW: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Whether the main window is in front: no visible window of another program covers any part
/// of it from above. A tray click asks (#520): the click itself takes the focus first, and with
/// the icon under the taskbar's ^ the focus went to the flyout well before, so focus alone
/// can't tell. The taskbar and its flyout, always on top, don't count, nor do hidden, minimized
/// or cloaked windows (another virtual desktop, a suspended app) or this program's own (its
/// dialogs over it). None: the main window isn't known yet.
pub fn main_window_in_front() -> Option<bool> {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GW_HWNDPREV, GetWindow, GetWindowRect, IsIconic, IsWindowVisible, WS_EX_TOPMOST,
    };
    let main = MAIN_WINDOW.load(std::sync::atomic::Ordering::Relaxed) as HWND;
    if main.is_null() {
        return None;
    }
    let rect = |h: HWND| {
        let mut r = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: a read into a local; a gone window fails harmlessly.
        (unsafe { GetWindowRect(h, &mut r) } != 0 && r.right > r.left && r.bottom > r.top)
            .then_some(r)
    };
    let m = rect(main)?;
    // SAFETY: plain queries about windows, walking the z-order up from ours.
    let me = unsafe { GetCurrentProcessId() };
    let mut h = unsafe { GetWindow(main, GW_HWNDPREV) };
    // A bound, in case windows come and go under the walk.
    for _ in 0..4096 {
        if h.is_null() {
            break;
        }
        let mut pid = 0;
        // SAFETY: as above.
        let (visible, topmost) = unsafe {
            GetWindowThreadProcessId(h, &mut pid);
            (
                IsWindowVisible(h) != 0 && IsIconic(h) == 0,
                GetWindowLongPtrW(h, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST != 0,
            )
        };
        if visible && !topmost && pid != me {
            let mut cloaked: u32 = 0;
            // SAFETY: a u32 read into a local of that size.
            let known = unsafe {
                DwmGetWindowAttribute(
                    h,
                    DWMWA_CLOAKED as _,
                    (&mut cloaked as *mut u32).cast(),
                    std::mem::size_of::<u32>() as u32,
                )
            } >= 0;
            if !(known && cloaked != 0)
                && let Some(r) = rect(h)
                && r.left < m.right
                && m.left < r.right
                && r.top < m.bottom
                && m.top < r.bottom
            {
                return Some(false);
            }
        }
        // SAFETY: as above.
        h = unsafe { GetWindow(h, GW_HWNDPREV) };
    }
    Some(true)
}

/// Remember the window the app was made for: the one that stays. A way of drawing that fails
/// (Direct3D with no usable adapter, renderer.rs) leaves its own window behind under the same
/// title until the event loop next runs, and a dialog owned by that window goes down with it
/// (#337).
pub fn set_main_window(cc: &eframe::CreationContext<'_>) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if let Ok(RawWindowHandle::Win32(h)) = cc.window_handle().map(|h| h.as_raw()) {
        MAIN_WINDOW.store(h.hwnd.get(), std::sync::atomic::Ordering::Relaxed);
    }
}

// ---- The session's end (shut down, restart, sign out; an installer closing MDM) ---------------

/// Why the session ends, as WM_QUERYENDSESSION and WM_ENDSESSION say (their lParam).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionEnd {
    /// The computer shuts down or restarts.
    ShutDown,
    /// The user signs out.
    SignOut,
    /// A program closes MDM through the Restart Manager (an installer replacing its files).
    Closed,
}

impl SessionEnd {
    fn of(lparam: LPARAM) -> SessionEnd {
        use windows_sys::Win32::UI::WindowsAndMessaging::{ENDSESSION_CLOSEAPP, ENDSESSION_LOGOFF};
        let flags = lparam as u32;
        if flags & ENDSESSION_CLOSEAPP != 0 {
            SessionEnd::Closed
        } else if flags & ENDSESSION_LOGOFF != 0 {
            SessionEnd::SignOut
        } else {
            SessionEnd::ShutDown
        }
    }
}

/// The subclass that hears the session's end on the main window.
const SESSION_SUBCLASS: usize = 0x5e5;

/// What the session's end runs ([`on_session_end`]).
type OnSessionEnd = Box<dyn FnOnce(SessionEnd)>;

thread_local! {
    /// Taken when the session's end comes.
    static SESSION_END: std::cell::RefCell<Option<OnSessionEnd>> =
        const { std::cell::RefCell::new(None) };
}

/// At the session's end, run `end` on the window's thread before Windows is answered: Windows may
/// end the process as soon as WM_ENDSESSION returns, so whatever must be saved is saved in it.
/// Heard on the main window ([`set_main_window`]; call this after it, on its thread): Windows
/// asks every top-level window, hidden in the tray or not. winit answers neither message itself.
pub fn on_session_end(end: impl FnOnce(SessionEnd) + 'static) {
    let hwnd = MAIN_WINDOW.load(std::sync::atomic::Ordering::Relaxed);
    if hwnd == 0 {
        tracing::warn!("the session's end isn't heard: no main window");
        return;
    }
    // SAFETY: the main window, made on this thread (eframe's creator runs on it).
    unsafe { watch_session_end(hwnd as HWND, Box::new(end)) };
}

/// # Safety
/// `hwnd` is a window of this thread.
unsafe fn watch_session_end(hwnd: HWND, end: OnSessionEnd) {
    SESSION_END.with(|s| *s.borrow_mut() = Some(end));
    // SAFETY: (the caller's) our own window; the proc lives as long as the program.
    unsafe { SetWindowSubclass(hwnd, Some(session_proc), SESSION_SUBCLASS, 0) };
}

/// MDM never holds the session up: nothing it does then needs the user (a download stops where
/// it is and resumes at the next start), so it answers yes at once and does its saving when the
/// end is certain (WM_ENDSESSION), not when another program could still cancel it.
unsafe extern "system" fn session_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{WM_ENDSESSION, WM_QUERYENDSESSION};
    match msg {
        WM_QUERYENDSESSION => {
            let why = SessionEnd::of(lparam);
            tracing::info!("Windows asks whether the session may end ({why:?}): yes");
            return 1;
        }
        WM_ENDSESSION if wparam != 0 => {
            let why = SessionEnd::of(lparam);
            tracing::info!("the session ends ({why:?})");
            let end = SESSION_END.with(|s| s.try_borrow_mut().ok().and_then(|mut s| s.take()));
            if let Some(end) = end {
                end(why);
            }
            return 0;
        }
        WM_ENDSESSION => {
            tracing::info!("the session goes on (another program kept it, or it was cancelled)");
            return 0;
        }
        _ => {}
    }
    // SAFETY: the subclass's own arguments, passed on.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod session_end_tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, ENDSESSION_CLOSEAPP, ENDSESSION_LOGOFF,
        RegisterClassW, SendMessageW, WM_ENDSESSION, WM_QUERYENDSESSION, WNDCLASSW,
        WS_OVERLAPPEDWINDOW,
    };

    #[test]
    fn the_reason_comes_from_lparam() {
        assert_eq!(SessionEnd::of(0), SessionEnd::ShutDown);
        assert_eq!(
            SessionEnd::of(ENDSESSION_LOGOFF as LPARAM),
            SessionEnd::SignOut
        );
        assert_eq!(
            SessionEnd::of(ENDSESSION_CLOSEAPP as LPARAM),
            SessionEnd::Closed
        );
        // A critical end (the battery running out) is still a shut down.
        assert_eq!(SessionEnd::of(0x4000_0000), SessionEnd::ShutDown);
    }

    /// A real hidden top-level window, sent what Windows sends: asked, it says yes at once
    /// without running the end; a session that goes on runs nothing; the end runs it, once,
    /// before WM_ENDSESSION returns.
    #[test]
    fn a_window_answers_the_session_s_end_and_saves_before_it_returns() {
        use std::cell::RefCell;
        use std::rc::Rc;
        let ran: Rc<RefCell<Vec<SessionEnd>>> = Rc::default();
        let class: Vec<u16> = "mdm-session-end-test\0".encode_utf16().collect();
        let title: Vec<u16> = "mdm session end test\0".encode_utf16().collect();
        // SAFETY: a class with the system's own procedure and a hidden window of it on this
        // thread, destroyed below; NUL-terminated strings; messages sent to it.
        unsafe {
            let mut wc: WNDCLASSW = zeroed();
            wc.lpfnWndProc = Some(DefWindowProcW);
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                -20000,
                -20000,
                200,
                100,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!hwnd.is_null());
            let seen = ran.clone();
            watch_session_end(hwnd, Box::new(move |why| seen.borrow_mut().push(why)));
            let logoff = ENDSESSION_LOGOFF as LPARAM;
            assert_eq!(SendMessageW(hwnd, WM_QUERYENDSESSION, 0, logoff), 1);
            assert!(
                ran.borrow().is_empty(),
                "nothing is done while it may be cancelled"
            );
            assert_eq!(SendMessageW(hwnd, WM_ENDSESSION, 0, logoff), 0);
            assert!(
                ran.borrow().is_empty(),
                "a session that goes on ends nothing"
            );
            SendMessageW(hwnd, WM_ENDSESSION, 1, logoff);
            assert_eq!(*ran.borrow(), [SessionEnd::SignOut]);
            SendMessageW(hwnd, WM_ENDSESSION, 1, logoff);
            assert_eq!(ran.borrow().len(), 1, "once");
            DestroyWindow(hwnd);
        }
    }
}

/// The system's double-click time (Control Panel → Mouse): a tray icon's double click is two
/// clicks within it (#1365).
pub fn double_click_time() -> std::time::Duration {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
    // SAFETY: a plain query with no arguments.
    std::time::Duration::from_millis(u64::from(unsafe { GetDoubleClickTime() }))
}

/// This process's top-level window with exactly `title`; the main window by its handle.
pub(crate) fn find_own_window(title: &str) -> Option<HWND> {
    let known = MAIN_WINDOW.load(std::sync::atomic::Ordering::Relaxed);
    own_window(
        title,
        (title == crate::MAIN_TITLE && known != 0).then_some(known as HWND),
    )
}

/// `known` while that window exists, else the top-level window of this process titled `title`.
fn own_window(title: &str, known: Option<HWND>) -> Option<HWND> {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsWindow;
    // SAFETY: a plain query; any value is accepted as the handle.
    if let Some(h) = known.filter(|h| unsafe { IsWindow(*h) } != 0) {
        return Some(h);
    }
    // As the window was given it (dialogs.rs, progress.rs): without isolates (#919).
    let title: Vec<u16> = crate::i18n::os_text(title).encode_utf16().collect();
    let mut f = Find {
        pid: unsafe { GetCurrentProcessId() },
        buf: vec![0; title.len() + 2],
        title,
        found: std::ptr::null_mut(),
    };
    unsafe { EnumWindows(Some(enum_cb), &mut f as *mut Find as LPARAM) };
    (!f.found.is_null()).then_some(f.found)
}

/// The address a connection to `dst` would leave from (PAC's `myIpAddress`), asked of the
/// routing table (GetBestRoute2). No socket is opened: a UDP socket, the usual way to ask, is a
/// listening socket to Windows' firewall, which would ask the user about MDM. Installed as the
/// engine's lookup at start (`mdm_engine::net::set_source_lookup`); unix.rs has none (the
/// engine's UDP way raises no prompt there).
pub fn source_address(dst: std::net::IpAddr) -> Option<std::net::IpAddr> {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetBestRoute2, MIB_IPFORWARD_ROW2};
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, SOCKADDR_INET};
    // SAFETY: plain-data structures, zero is a valid value for each; the union is written and
    // read by its family; GetBestRoute2 only reads `dest` and writes `row` and `src`.
    unsafe {
        let mut dest: SOCKADDR_INET = zeroed();
        match dst {
            IpAddr::V4(a) => {
                dest.Ipv4.sin_family = AF_INET;
                dest.Ipv4.sin_addr.S_un.S_addr = u32::from_ne_bytes(a.octets());
            }
            IpAddr::V6(a) => {
                dest.Ipv6.sin6_family = AF_INET6;
                dest.Ipv6.sin6_addr.u.Byte = a.octets();
            }
        }
        let mut row: MIB_IPFORWARD_ROW2 = zeroed();
        let mut src: SOCKADDR_INET = zeroed();
        let r = GetBestRoute2(
            std::ptr::null(),
            0,
            std::ptr::null(),
            &dest,
            0,
            &mut row,
            &mut src,
        );
        if r != 0 {
            return None;
        }
        match src.si_family {
            AF_INET => Some(IpAddr::V4(Ipv4Addr::from(
                src.Ipv4.sin_addr.S_un.S_addr.to_ne_bytes(),
            ))),
            AF_INET6 => Some(IpAddr::V6(Ipv6Addr::from(src.Ipv6.sin6_addr.u.Byte))),
            _ => None,
        }
    }
}

/// Off-screen test runs (MDM_OFFSCREEN=1): every top-level window of ours gets WS_EX_NOACTIVATE,
/// so Windows never makes it the foreground window, whatever asks (a click, a dialog coming to
/// the front, an accessibility client): the user's keystrokes stay in their own window while
/// tests run. UI Automation still works on them (Select, Invoke, Toggle need no activation).
/// Cheap: only our own windows are touched, once each.
pub fn keep_from_foreground() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowThreadProcessId, IsWindowVisible, WS_EX_NOACTIVATE,
    };
    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
        let pid = lparam as u32;
        let mut owner = 0u32;
        // SAFETY: plain queries and a style change on a window of this process.
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut owner);
            if owner == pid && IsWindowVisible(hwnd) != 0 {
                let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
                if ex & WS_EX_NOACTIVATE == 0 {
                    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex | WS_EX_NOACTIVATE) as isize);
                }
            }
        }
        1
    }
    // SAFETY: the callback only reads its pid from lparam.
    unsafe {
        EnumWindows(Some(cb), GetCurrentProcessId() as LPARAM);
    }
}

/// Our window titled `title`, just shown (dialogs::keep_theme), in front of the other programs'
/// windows if Windows didn't make it the foreground window (#208). Windows gives a program the
/// foreground only when it may take it (the user's click is in it, or the browser's native host
/// passed its right on, `AllowSetForegroundWindow`, which the user's next input elsewhere ends).
/// Refused, the window is placed under the program in front, just above our own windows, and
/// stays there: `HWND_TOP` and `BringWindowToTop` don't move it either. So a window opened at the
/// browser's request (Download File Info, a progress window) could open behind the browser. A
/// round trip through topmost puts it above every window that isn't topmost, as Download
/// complete is (#77), without taking the keyboard from the program being typed in (a click on
/// it does). None while it isn't shown yet (ask again in the next frame); else whether it was
/// lifted.
pub fn come_forward(title: &str) -> Option<bool> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, IsIconic, IsWindowVisible, WS_EX_TOPMOST,
    };
    let hwnd = find_own_window(title)?;
    // SAFETY: plain queries about our own window.
    let (visible, foreground, topmost, minimized) = unsafe {
        (
            IsWindowVisible(hwnd) != 0,
            GetForegroundWindow() == hwnd,
            GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST != 0,
            IsIconic(hwnd) != 0,
        )
    };
    if !visible {
        return None;
    }
    let lift = lifted_when_shown(foreground, topmost, minimized);
    if lift {
        // SAFETY: our own window, on its thread.
        unsafe { above_others(hwnd) };
    }
    Some(lift)
}

/// Whether the window in front (the one the keyboard goes to) is the main window or one of the
/// dialogs it owns: the user is working in the main window. Not another program's window, nor
/// a window of ours that stands on its own (a progress window, a dialog opened over the browser).
pub fn main_in_front() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GW_OWNER, GetForegroundWindow, GetWindow};
    let Some(main) = find_own_window(crate::MAIN_TITLE) else {
        return false;
    };
    // SAFETY: plain queries; any handle, or none, is accepted.
    let mut hwnd = unsafe { GetForegroundWindow() };
    // A dialog's question is owned by the main window too: a short chain.
    for _ in 0..4 {
        if hwnd.is_null() {
            return false;
        }
        if hwnd == main {
            return true;
        }
        // SAFETY: as above.
        hwnd = unsafe { GetWindow(hwnd, GW_OWNER) };
    }
    false
}

/// Whether our window titled `title` exists and is not on the screen: hidden and not minimized
/// (a window whose owner is minimized counts: Windows hides it with its owner).
pub fn window_hidden(title: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible};
    // SAFETY: plain queries about our own window.
    find_own_window(title).is_some_and(|h| unsafe { IsWindowVisible(h) == 0 && IsIconic(h) == 0 })
}

/// Our window titled `title` shown (it was hidden behind winit's back), not activated, and above
/// the other programs' windows ([`come_forward`]) unless off-screen.
pub fn show_hidden(title: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, ShowWindow};
    if let Some(h) = find_own_window(title) {
        // SAFETY: our own window, on its thread.
        unsafe { ShowWindow(h, SW_SHOWNOACTIVATE) };
        if !crate::offscreen() {
            come_forward(title);
        }
    }
}

/// Whether a window just shown goes above the others ([`come_forward`]): it isn't the foreground
/// window, isn't minimized, and isn't topmost already (a question, a notice: lifting it would
/// end its topmost).
fn lifted_when_shown(foreground: bool, topmost: bool, minimized: bool) -> bool {
    !foreground && !topmost && !minimized
}

/// Window `hwnd` above every window that isn't topmost, not activated, nothing else changed (its
/// size, place, frame and styles; its owner stays where it is).
///
/// # Safety
/// `hwnd` is a window of this thread.
unsafe fn above_others(hwnd: HWND) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOOWNERZORDER, SWP_NOSIZE,
    };
    let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER;
    // SAFETY: (the caller's) our own window; z-order changes only.
    unsafe {
        SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, flags);
        SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, flags);
    }
}

/// Give our window titled `title` IDM's modal-dialog frame (WS_EX_DLGMODALFRAME, no sizing
/// frame: 3 px borders, 26 px caption) and resize it so the client stays `client` points, at
/// `scale` pixels per point (the display's scaling times any zoom), with the frame measured for
/// the window's own DPI. `light` dialogs get a light caption. `owned`: the main window owns it
/// (see there).
/// Returns true once applied (call again each frame until it is).
pub fn dialog_frame(title: &str, client: [f32; 2], light: bool, scale: f32, owned: bool) -> bool {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
    let Some(hwnd) = find_own_window(title) else {
        return false;
    };
    unsafe {
        // IDM's light dialogs keep a light caption even in dark mode.
        let dark: i32 = (!light).into();
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &dark as *const i32 as *const c_void,
            size_of::<i32>() as u32,
        );
        // Owned by the main window, as IDM's dialogs are: no taskbar button, kept above it. Only
        // when asked for (`owned`): Windows brings an owned window's owner forward with it each
        // time it is activated (by MDM, or by a click on it), so a dialog over another program
        // (the browser's duplicate question) brought the main window up from behind it. One on
        // its own has its own taskbar button.
        if owned && let Some(main) = find_own_window(crate::MAIN_TITLE) {
            use windows_sys::Win32::UI::WindowsAndMessaging::{GWLP_HWNDPARENT, WS_EX_APPWINDOW};
            SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, main as isize);
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & !WS_EX_APPWINDOW;
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex as isize);
        }
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & !WS_THICKFRAME;
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 | WS_EX_DLGMODALFRAME;
        SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex as isize);
        size_to_client(hwnd, style, ex, client_px(client, scale));
    }
    true
}

/// A client of `client` points at `scale` pixels per point, in whole device pixels.
fn client_px(client: [f32; 2], scale: f32) -> [i32; 2] {
    [
        (client[0] * scale).round() as i32,
        (client[1] * scale).round() as i32,
    ]
}

/// Size our window `hwnd` so its client is `client` device pixels: the window its styles
/// (`style`, `ex`) put round that client at the window's own DPI, then its client measured and
/// the window made larger or smaller by what it came back short or over (#1495). A dialog is as
/// large as its fixed layout: a client a few pixels short of it showed both scroll bars over a
/// window whose content was all there, its buttons cut at the bottom. The frame of a window
/// whose title bar is ours was reckoned, not measured, on Windows 10 in 0.5.2: 2 px a side
/// counted, the system's sizing frame less 6 px drawn, which is 3 px at 125 %, 5 at 150 %.
/// Measured, the client is what was asked for, whatever puts the frame round it (a version of
/// Windows, a scale, a tool that changes the window borders). Not while minimized or maximized,
/// whose client isn't the window's own size.
///
/// # Safety
/// `hwnd` is a window of this thread.
unsafe fn size_to_client(hwnd: HWND, style: u32, ex: u32, client: [i32; 2]) {
    use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClientRect, GetWindowRect, IsIconic, IsZoomed,
    };
    // SAFETY: (the caller's) our own window, on its thread; the rectangles are locals written
    // during the calls.
    unsafe {
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: client[0],
            bottom: client[1],
        };
        AdjustWindowRectExForDpi(&mut rc, style, 0, ex, GetDpiForWindow(hwnd).max(96));
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            rc.right - rc.left,
            rc.bottom - rc.top,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        if IsIconic(hwnd) != 0 || IsZoomed(hwnd) != 0 {
            return;
        }
        let (mut w, mut c): (RECT, RECT) = (zeroed(), zeroed());
        if GetWindowRect(hwnd, &mut w) == 0 || GetClientRect(hwnd, &mut c) == 0 {
            return;
        }
        let got = [c.right - c.left, c.bottom - c.top];
        let Some(outer) = outer_for_client([w.right - w.left, w.bottom - w.top], got, client)
        else {
            return;
        };
        tracing::info!(
            "window client {got:?} where {client:?} was asked for: the window made {outer:?}"
        );
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            outer[0],
            outer[1],
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// The size for a window of `outer` pixels whose client came back `got` to have the client
/// `want`: its frame kept, whatever it measures. None when the client is right already.
fn outer_for_client(outer: [i32; 2], got: [i32; 2], want: [i32; 2]) -> Option<[i32; 2]> {
    (got != want).then(|| [outer[0] + want[0] - got[0], outer[1] + want[1] - got[1]])
}

/// Hit-test results (winuser.h).
const HTCLIENT: LRESULT = 1;
const HTCAPTION: LRESULT = 2;
const HTSYSMENU: LRESULT = 3;

/// The system menu's "Hide to the notification area" (#63): a command id of ours (below
/// 0xF000, its low 4 bits clear, as WM_SYSCOMMAND wants).
const SC_HIDE_TO_TRAY: usize = 0x0E10;

/// Where a drawn title bar's parts are, in client pixels (egui publishes them each frame,
/// `set_caption_hits`): the caption strip's height, the icon (the system menu's place) and the
/// caption buttons, each [left, top, right, bottom].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CaptionHits {
    pub strip_h: i32,
    pub icon: [i32; 4],
    pub buttons: Vec<[i32; 4]>,
    /// Which of `buttons` is Maximize, answered as the system's (a resizable window, #45).
    pub max_button: Option<usize>,
}

thread_local! {
    /// Each framed window's caption parts, by HWND. Read in its subclass proc with try_borrow:
    /// WM_NCHITTEST can arrive while egui's frame holds the map (re-entered messages).
    static CAPTION_HITS: std::cell::RefCell<std::collections::HashMap<isize, CaptionHits>> =
        Default::default();
    /// Windows whose system menu's "Hide to the notification area" was chosen.
    static HIDE_ASKED: std::cell::RefCell<std::collections::HashSet<isize>> = Default::default();
}

/// Publish where window `hwnd`'s drawn title bar's parts are (client pixels), for its hit test.
pub fn set_caption_hits(hwnd: isize, hits: CaptionHits) {
    CAPTION_HITS.with(|m| {
        if let Ok(mut m) = m.try_borrow_mut() {
            m.insert(hwnd, hits);
        }
    });
}

/// The window is gone: forget its caption parts.
pub fn forget_caption(hwnd: isize) {
    CAPTION_HITS.with(|m| {
        if let Ok(mut m) = m.try_borrow_mut() {
            m.remove(&hwnd);
        }
    });
    HIDE_ASKED.with(|s| {
        if let Ok(mut s) = s.try_borrow_mut() {
            s.remove(&hwnd);
        }
    });
    for set in [&ANIMATING, &FADING] {
        set.with(|s| {
            if let Ok(mut s) = s.try_borrow_mut() {
                s.remove(&hwnd);
            }
        });
    }
}

/// Was "Hide to the notification area" chosen in window `hwnd`'s system menu (once)?
pub fn take_hide_asked(hwnd: isize) -> bool {
    HIDE_ASKED.with(|s| s.try_borrow_mut().is_ok_and(|mut s| s.remove(&hwnd)))
}

fn inside(r: &[i32; 4], x: i32, y: i32) -> bool {
    x >= r[0] && x < r[2] && y >= r[1] && y < r[3]
}

/// What client point `(x, y)` is: our caption buttons are the client's (egui hovers and clicks
/// them), the icon is the system menu, the rest of the caption strip is the caption (the system
/// moves, snaps and shakes the window from it, double-clicks and right-clicks it), below it the
/// client's.
fn frame_hit(x: i32, y: i32, hits: &CaptionHits) -> LRESULT {
    if hits.buttons.iter().any(|b| inside(b, x, y)) {
        HTCLIENT
    } else if inside(&hits.icon, x, y) {
        HTSYSMENU
    } else if y >= 0 && y < hits.strip_h {
        HTCAPTION
    } else {
        HTCLIENT
    }
}

/// The Maximize button of a resizable window whose title bar is ours: under the pointer, or
/// pressed. Windows hit-tests it as the system's (HTMAXBUTTON), so Windows 11 shows its Snap
/// Layouts over it; the clicks are then the frame's, and so is its hover, which egui can't see:
/// the title bar reads it from here ([`max_button_state`]).
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct MaxButton {
    pub hot: bool,
    pub pressed: bool,
}

thread_local! {
    static MAX_BUTTON: std::cell::RefCell<std::collections::HashMap<isize, MaxButton>> =
        Default::default();
}

/// Window `hwnd`'s Maximize button's state (see [`MaxButton`]).
pub fn max_button_state(hwnd: isize) -> MaxButton {
    MAX_BUTTON.with(|m| {
        m.try_borrow()
            .ok()
            .and_then(|m| m.get(&hwnd).copied())
            .unwrap_or_default()
    })
}

fn set_max_button(hwnd: HWND, state: MaxButton) {
    use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
    let changed = MAX_BUTTON.with(|m| {
        m.try_borrow_mut()
            .is_ok_and(|mut m| m.insert(hwnd as isize, state) != Some(state))
    });
    if changed {
        // SAFETY: our own window; a repaint is asked for (winit draws a frame).
        unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
    }
}

/// Give the resizable window titled `title` its title bar drawn by us (`on`), or the system's
/// back. Returns the window when done.
pub fn drawn_frame(title: &str, on: bool) -> Option<isize> {
    let hwnd = find_own_window(title)?;
    // Without a caption style on every version, as the progress window and the dialogs (#802):
    // on Windows 10 the caption style hidden by our WM_NCCALCSIZE put the window's picture a
    // title bar lower than its input there too, a black band above the bar and every click
    // landing above what it showed (#620).
    // SAFETY: our own window, on its thread.
    unsafe {
        if on {
            caption_less(hwnd, Drawn::Sizable);
        } else {
            caption_back(hwnd);
        }
    }
    if !on {
        forget_caption(hwnd as isize);
    }
    Some(hwnd as isize)
}

/// Give our window titled `title`, whose title bar is drawn by us (winui::caption, #63), its fixed
/// frame without a caption style ([`caption_less`], #208, #620), the client staying `client`
/// points (the drawn title bar included) at `scale` pixels per point. With `hide_item` (the
/// translated "Hide to the notification area"), the system menu gets it, Alt+Space reaching it
/// from the keyboard; None takes it out. Call while the window is still hidden, and again when
/// the size, the scale or the item changes. Returns the window, once applied.
pub fn thin_frame(
    title: &str,
    client: [f32; 2],
    scale: f32,
    hide_item: Option<&str>,
) -> Option<isize> {
    thin_frame_on(find_own_window(title)? as isize, client, scale, hide_item)
}

/// Reapply a frame to the bound window, even when another download has the same title.
pub fn thin_frame_on(
    window: isize,
    client: [f32; 2],
    scale: f32,
    hide_item: Option<&str>,
) -> Option<isize> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, DeleteMenu, GetSystemMenu, IsWindow, MF_BYCOMMAND, MF_STRING,
    };
    let hwnd = window as HWND;
    let mut pid = 0;
    // SAFETY: queries accepting any handle; pid is a writable local.
    if unsafe { IsWindow(hwnd) } == 0
        || unsafe { GetWindowThreadProcessId(hwnd, &mut pid) } == 0
        || pid != unsafe { GetCurrentProcessId() }
    {
        return None;
    }
    // Windows 11 draws its 1 px border round a window without a caption style; Windows 10 draws
    // none, so there it has the system's own (WS_BORDER).
    let kind = if windows_11() {
        Drawn::Fixed
    } else {
        Drawn::Edged
    };
    // SAFETY: our own window, on its thread; the menu text and the rectangle are locals read
    // during the calls.
    unsafe {
        caption_less(hwnd, kind);
        let menu = GetSystemMenu(hwnd, 0);
        if !menu.is_null() {
            DeleteMenu(menu, SC_HIDE_TO_TRAY as u32, MF_BYCOMMAND);
            if let Some(text) = hide_item {
                let w: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
                AppendMenuW(menu, MF_STRING, SC_HIDE_TO_TRAY, w.as_ptr());
            }
        }
        // The window is the client and the frame its styles describe round it, for its own DPI:
        // none on Windows 11, the 1 px border on Windows 10 (without the caption style a
        // minimized window has for its animation, [`animation_caption`]).
        let style = drawn_style(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, kind);
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        size_to_client(hwnd, style, ex, client_px(client, scale));
    }
    Some(hwnd as isize)
}

/// Windows 11 (build 22000 on). Every window whose title bar is ours and that keeps its size
/// (the progress window, a style theme's dialogs) is framed without a caption style on every
/// version ([`thin_frame`]): with the caption style hidden by our WM_NCCALCSIZE, a Windows 11
/// Insider build (#208) and Windows 10 22H2 (#620) showed the window's picture where the style's
/// client would have been, a black band a title bar tall above it, every click landing above what
/// it showed. The resizable main window with a style theme is framed so too ([`drawn_frame`],
/// #802). Windows 11 colours the sizing frame's strip above its title bar ([`caption_colour`]);
/// Windows 10 can't (DWMWA_CAPTION_COLOR is Windows 11's), so there the strip, 7 px at 100 %, keeps
/// the system's title bar colour (white, the accent colour, or black in dark mode).
fn windows_11() -> bool {
    use windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        reg_string_in(
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "CurrentBuild",
        )
        .and_then(|b| b.parse::<u32>().ok())
        .is_some_and(|b| b >= 22000)
    })
}

/// How a window whose title bar is ours is framed without a caption style ([`caption_less`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Drawn {
    /// A fixed size (the progress window, a dialog with a style theme's title bar): a plain
    /// popup, all client, as the drop target and the menus are (#210, #231). Windows 11 draws
    /// its border, over the client's edge.
    Fixed,
    /// The same with the system's 1 px border round the client (WS_BORDER): Windows 10, which
    /// draws no border round a window without a caption style (#620).
    Edged,
    /// Resizable (the main window with a style theme): a popup with the system's sizing frame,
    /// for resizing on every edge, Aero Snap and Snap Layouts, the shadow, and maximizing.
    Sizable,
}

/// The subclass of a window framed by [`caption_less`]: its id, and its data for each kind.
const DRAWN_SUBCLASS: usize = 0x1d8;

impl Drawn {
    fn data(self) -> usize {
        match self {
            Drawn::Fixed => 0,
            Drawn::Sizable => 1,
            Drawn::Edged => 2,
        }
    }

    fn of(data: usize) -> Drawn {
        match data {
            1 => Drawn::Sizable,
            2 => Drawn::Edged,
            _ => Drawn::Fixed,
        }
    }
}

/// The style of a window whose title bar is ours, framed without a caption style (#208): a
/// popup with no caption, border or dialog frame, with the system's sizing frame when it is
/// resizable; its system menu and its Minimize and Maximize kept (the taskbar's and Alt+Space's
/// commands). With a caption style hidden by our WM_NCCALCSIZE, a Windows 11 Insider build
/// (22635) placed the window's picture where the style's client would have been: a black band a
/// title bar tall across the top of the progress window, as it did with the menus (#210) and the
/// drop target (#231). Here what the styles describe is what the window is: the system's own
/// WM_NCCALCSIZE frames it, no other reading can differ.
pub(crate) fn drawn_style(style: u32, kind: Drawn) -> u32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{WS_BORDER, WS_CAPTION, WS_POPUP};
    // WS_CAPTION is WS_BORDER | WS_DLGFRAME: both go, and the border comes back where wanted.
    let plain = (style & !(WS_CAPTION | WS_THICKFRAME)) | WS_POPUP;
    match kind {
        Drawn::Fixed => plain,
        Drawn::Edged => plain | WS_BORDER,
        Drawn::Sizable => plain | WS_THICKFRAME,
    }
}

/// Its extended styles: without the edges that add to a frame (IDM's dialog frame among them).
/// The rest stays: a taskbar button where it had one, never-activate in an off-screen test copy.
pub(crate) fn drawn_ex_style(ex: u32) -> u32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{WS_EX_CLIENTEDGE, WS_EX_STATICEDGE};
    ex & !(WS_EX_DLGMODALFRAME | WS_EX_CLIENTEDGE | WS_EX_STATICEDGE)
}

/// Frame our window `hwnd`, whose title bar is ours, without a caption style ([`drawn_style`]).
/// Its subclass keeps the styles when winit sets its own again (on every change of its window
/// flags) and hit-tests the drawn title bar. Again on the same window: the styles applied again.
///
/// # Safety
/// `hwnd` is a window of this thread.
unsafe fn caption_less(hwnd: HWND, kind: Drawn) {
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOSIZE;
    // Register before the subclass can receive the shell's first button notification.
    taskbar_button_created_message();
    // SAFETY: (the caller's) our own window; the proc lives as long as the program; the DWM
    // attribute is read from a local during the call.
    unsafe {
        // The subclass first: it keeps the styles below when winit sets its own again.
        SetWindowSubclass(hwnd, Some(caption_less_proc), DRAWN_SUBCLASS, kind.data());
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, drawn_ex_style(ex) as isize);
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        SetWindowLongPtrW(hwnd, GWL_STYLE, drawn_style(style, kind) as isize);
        // Windows 11 rounds a window with a caption style by itself, not a popup: asked for.
        let corner = DWMWCP_ROUND;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &corner as *const _ as *const c_void,
            size_of::<i32>() as u32,
        );
        // The frame measured again (WM_NCCALCSIZE), by the system.
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// The system's title bar back on a window framed by [`caption_less`] (the main window, back to
/// a theme without a title bar of its own).
///
/// # Safety
/// `hwnd` is a window of this thread.
unsafe fn caption_back(hwnd: HWND) {
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_CAPTION_COLOR, DWMWA_COLOR_DEFAULT, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DEFAULT,
        DwmSetWindowAttribute,
    };
    use windows_sys::Win32::UI::Shell::RemoveWindowSubclass;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SWP_NOSIZE, WS_CAPTION, WS_POPUP};
    // SAFETY: (the caller's) our own window; DWM attributes read from locals during the calls.
    unsafe {
        RemoveWindowSubclass(hwnd, Some(caption_less_proc), DRAWN_SUBCLASS);
        dwm_frame_off(hwnd, false);
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        SetWindowLongPtrW(hwnd, GWL_STYLE, ((style & !WS_POPUP) | WS_CAPTION) as isize);
        let corner = DWMWCP_DEFAULT;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &corner as *const _ as *const c_void,
            size_of::<i32>() as u32,
        );
        let colour = DWMWA_COLOR_DEFAULT;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR as u32,
            &colour as *const u32 as *const c_void,
            size_of::<u32>() as u32,
        );
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    CAPTION_COLOUR.with(|m| {
        if let Ok(mut m) = m.try_borrow_mut() {
            m.remove(&(hwnd as isize));
        }
    });
    for set in [&ANIMATING, &FADING] {
        set.with(|s| {
            if let Ok(mut s) = s.try_borrow_mut() {
                s.remove(&(hwnd as isize));
            }
        });
    }
}

thread_local! {
    /// The windows framed by [`caption_less`] that have the caption style for the length of a
    /// minimize, maximize or restore ([`animation_caption`]), by HWND.
    static ANIMATING: std::cell::RefCell<std::collections::HashSet<isize>> = Default::default();
    /// Those of them that have it for the moment they are hidden ([`fade_caption`]).
    static FADING: std::cell::RefCell<std::collections::HashSet<isize>> = Default::default();
}

fn animating(hwnd: HWND) -> bool {
    ANIMATING.with(|s| s.try_borrow().is_ok_and(|s| s.contains(&(hwnd as isize))))
}

/// The message a window framed by [`caption_less`] posts itself when it changed size or place
/// with the caption style of its animation still on: it is taken off once the window is out of
/// the taskbar ([`animation_caption`]).
fn settled_message() -> u32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW;
    static ID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *ID.get_or_init(|| {
        let name: Vec<u16> = "MDM.caption-less.settled\0".encode_utf16().collect();
        // SAFETY: a NUL-terminated string read during the call.
        unsafe { RegisterWindowMessageW(name.as_ptr()) }
    })
}

/// The caption style on (`on`) or off a window framed by [`caption_less`], for the length of the
/// system's minimize, maximize or restore (#874). Windows plays those animations only for a
/// window with a caption style; one without just pops in and out. The style is given as the
/// system's command starts and taken back as soon as the window is in its new state (minimized:
/// once it is back), and WM_NCCALCSIZE keeps the frame of the window without it the whole time
/// (the client filling the window inside its frame), so its picture never moves: a caption style
/// hidden for good is what put the black band above the title bar on Windows 10 (#620, #802).
///
/// # Safety
/// `hwnd` is a window of this thread framed by [`caption_less`] as `kind`.
unsafe fn animation_caption(hwnd: HWND, kind: Drawn, on: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsZoomed, SWP_NOSIZE, WS_CAPTION};
    if animating(hwnd) == on {
        return;
    }
    // First: the subclass's WM_STYLECHANGING keeps the caption style only while it is listed.
    ANIMATING.with(|s| {
        if let Ok(mut s) = s.try_borrow_mut() {
            if on {
                s.insert(hwnd as isize);
            } else {
                s.remove(&(hwnd as isize));
            }
        }
    });
    // SAFETY: (the caller's) our own window, on its thread.
    unsafe {
        if on {
            dwm_frame_off(hwnd, true);
        }
        let style = drawn_style(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, kind);
        let style = if on { style | WS_CAPTION } else { style };
        SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);
        // The frame measured again: the same, by our WM_NCCALCSIZE with the caption style on, by
        // the system's without it. Maximized with the caption style, the window was placed by
        // the system for a caption's frame, a pixel wider than its own on every side: put back
        // where it is maximized without it, its client the work area.
        let maximized = (!on && kind == Drawn::Sizable && IsZoomed(hwnd) != 0)
            .then(|| maximized_to_work_area(hwnd))
            .flatten();
        let (r, place) = match maximized {
            Some((r, _)) => (r, 0),
            None => (zeroed(), SWP_NOMOVE | SWP_NOSIZE),
        };
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_FRAMECHANGED | SWP_NOZORDER | SWP_NOACTIVATE | place,
        );
        if !on {
            dwm_frame_off(hwnd, false);
        }
    }
}

/// A set of windows by HWND, kept by the window thread ([`ANIMATING`], [`FADING`]).
type WindowSet = std::thread::LocalKey<std::cell::RefCell<std::collections::HashSet<isize>>>;

fn fading(hwnd: HWND) -> bool {
    FADING.with(|s| s.try_borrow().is_ok_and(|s| s.contains(&(hwnd as isize))))
}

/// The caption style on (`on`) or off a window framed by [`caption_less`] for the moment it is
/// hidden (the main window or a progress window into the tray): Windows fades a window out only
/// when it has a caption style as it goes (#874). Unlike [`animation_caption`], the style alone
/// changes: nothing is measured again, so the window's frame and picture stay as they are. Not
/// while it has the caption of a minimize, nor taken off one minimized meanwhile (its restore
/// takes it off). (Shown again, a window appears at once, with Classic too: Windows fades in
/// none of MDM's windows.)
///
/// # Safety
/// `hwnd` is a window of this thread framed by [`caption_less`] as `kind`.
unsafe fn fade_caption(hwnd: HWND, kind: Drawn, on: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, WS_CAPTION};
    if on == fading(hwnd) || (on && animating(hwnd)) {
        return;
    }
    // First: the subclass's WM_STYLECHANGING keeps the caption style only while it is listed.
    let list = |set: &'static WindowSet| {
        set.with(|s| {
            if let Ok(mut s) = s.try_borrow_mut() {
                if on {
                    s.insert(hwnd as isize);
                } else {
                    s.remove(&(hwnd as isize));
                }
            }
        })
    };
    list(&FADING);
    // SAFETY: (the caller's) our own window, on its thread.
    unsafe {
        if !on && IsIconic(hwnd) != 0 {
            return;
        }
        list(&ANIMATING);
        if on {
            dwm_frame_off(hwnd, true);
        }
        let style = drawn_style(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, kind);
        let style = if on { style | WS_CAPTION } else { style };
        SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);
        if !on {
            dwm_frame_off(hwnd, false);
        }
    }
}

/// DWM's own frame off (`caption`) or back on a window framed by [`caption_less`] for as long as it
/// has the caption style of an animation or a fade, on Windows 10: there DWM draws the caption
/// style's title bar over the window's for those frames, a white (or accent-coloured) band above
/// MDM's own title bar (#874). With DWM's frame off, a thin edge shows instead; the window is the
/// same before and after. Windows 11 draws no such band.
///
/// # Safety
/// `hwnd` is a window of this thread.
unsafe fn dwm_frame_off(hwnd: HWND, caption: bool) {
    use windows_sys::Win32::Graphics::Dwm::{
        DWMNCRP_DISABLED, DWMNCRP_USEWINDOWSTYLE, DWMWA_NCRENDERING_POLICY, DwmSetWindowAttribute,
    };
    if windows_11() {
        return;
    }
    let policy = if caption {
        DWMNCRP_DISABLED
    } else {
        DWMNCRP_USEWINDOWSTYLE
    };
    // SAFETY: (the caller's) our own window; the policy is read from a local during the call.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_NCRENDERING_POLICY as u32,
            &policy as *const _ as *const c_void,
            size_of::<i32>() as u32,
        );
    }
}

/// The frame (left, top, right, bottom) of a window framed by [`caption_less`] as `kind`, in its
/// own DPI's pixels, without the caption style of its animation ([`animation_caption`]).
///
/// # Safety
/// `hwnd` is a window of this thread.
unsafe fn drawn_frame_edges(hwnd: HWND, kind: Drawn) -> [i32; 4] {
    use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
    // SAFETY: (the caller's) our own window; the rectangle is a local.
    unsafe {
        let style = drawn_style(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, kind);
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let mut f: RECT = zeroed();
        AdjustWindowRectExForDpi(&mut f, style, 0, ex, GetDpiForWindow(hwnd).max(96));
        [-f.left, -f.top, f.right, f.bottom]
    }
}

/// Minimize our window `hwnd` as its title bar's own Minimize does: by the system's command
/// (WM_SYSCOMMAND), which a window framed by [`caption_less`] animates (#874), where winit
/// minimizes it at once.
pub fn minimize(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, SC_MINIMIZE, WM_SYSCOMMAND};
    // SAFETY: a message posted to our own window, a system command it handles.
    unsafe { PostMessageW(hwnd as HWND, WM_SYSCOMMAND, SC_MINIMIZE as WPARAM, 0) };
}

/// Maximize our window `hwnd`, or restore it when it is maximized, as its title bar's own
/// Maximize does (see [`minimize`]).
pub fn toggle_maximized(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IsZoomed, PostMessageW, SC_MAXIMIZE, SC_RESTORE, WM_SYSCOMMAND,
    };
    // SAFETY: a query about our own window, and a system command posted to it.
    unsafe {
        let cmd = if IsZoomed(hwnd as HWND) != 0 {
            SC_RESTORE
        } else {
            SC_MAXIMIZE
        };
        PostMessageW(hwnd as HWND, WM_SYSCOMMAND, cmd as WPARAM, 0);
    }
}

thread_local! {
    /// The colour last given to each resizable window's frame above its drawn title bar.
    static CAPTION_COLOUR: std::cell::RefCell<std::collections::HashMap<isize, u32>> =
        Default::default();
}

/// The colour of the sizing frame's strip above the main window's drawn title bar (`rgb`, the
/// top of the bar), so the two read as one bar (Windows 11 paints the frame of a window without
/// a caption style in its caption colour; Windows 10 in its own, which no app can set). Cheap:
/// DWM is asked only when it changes.
pub fn caption_colour(hwnd: isize, rgb: [u8; 3]) {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_CAPTION_COLOR, DwmSetWindowAttribute};
    if !windows_11() {
        return;
    }
    let colour = u32::from_le_bytes([rgb[0], rgb[1], rgb[2], 0]);
    let changed = CAPTION_COLOUR.with(|m| {
        m.try_borrow_mut()
            .is_ok_and(|mut m| m.insert(hwnd, colour) != Some(colour))
    });
    if changed {
        // SAFETY: our own window; the colour is read from a local during the call.
        unsafe {
            DwmSetWindowAttribute(
                hwnd as HWND,
                DWMWA_CAPTION_COLOR as u32,
                &colour as *const u32 as *const c_void,
                size_of::<u32>() as u32,
            );
        }
    }
}

/// The window menu (the system menu) of a window framed by [`caption_less`], opened at screen
/// point `(x, y)` as the system opens it from a caption: right-click on the title bar, a click on
/// its icon. Its items are enabled as the window's state allows; the command chosen is carried
/// out as the system's (WM_SYSCOMMAND). Alt+Space opens it through the system already.
fn window_menu(hwnd: HWND, x: i32, y: i32, sizable: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnableMenuItem, GetSystemMenu, IsIconic, IsZoomed, MF_BYCOMMAND, MF_ENABLED, MF_GRAYED,
        PostMessageW, SC_CLOSE, SC_MAXIMIZE, SC_MINIMIZE, SC_MOVE, SC_RESTORE, SC_SIZE,
        SetMenuDefaultItem, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_SYSCOMMAND,
        WS_MAXIMIZEBOX, WS_MINIMIZEBOX,
    };
    // SAFETY: our own window and its system menu, on its thread.
    unsafe {
        let menu = GetSystemMenu(hwnd, 0);
        if menu.is_null() {
            return;
        }
        let (zoomed, iconic) = (IsZoomed(hwnd) != 0, IsIconic(hwnd) != 0);
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        for (cmd, on) in [
            (SC_RESTORE, zoomed || iconic),
            (SC_MOVE, !zoomed),
            (SC_SIZE, sizable && !zoomed && !iconic),
            (SC_MINIMIZE, style & WS_MINIMIZEBOX != 0 && !iconic),
            (
                SC_MAXIMIZE,
                sizable && style & WS_MAXIMIZEBOX != 0 && !zoomed,
            ),
            (SC_CLOSE, true),
        ] {
            let flag = if on { MF_ENABLED } else { MF_GRAYED };
            EnableMenuItem(menu, cmd, MF_BYCOMMAND | flag);
        }
        SetMenuDefaultItem(menu, SC_CLOSE, 0);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            x,
            y,
            0,
            hwnd,
            std::ptr::null(),
        );
        if cmd != 0 {
            PostMessageW(hwnd, WM_SYSCOMMAND, cmd as WPARAM, 0);
        }
    }
}

/// A window framed by [`caption_less`]: it keeps its styles, the system frames it
/// (WM_NCCALCSIZE as its styles say), and its drawn title bar is hit-tested as a caption: the
/// buttons are the client's (egui), the icon the system menu's, the rest the caption (moved,
/// snapped, shaken, double-clicked by the system). A resizable one's Maximize answers as the
/// system's (Snap Layouts) and it maximizes to the work area, as a window with a caption does.
unsafe extern "system" fn caption_less_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, HTMAXBUTTON, IsIconic, IsZoomed, NCCALCSIZE_PARAMS, PostMessageW, SC_CLOSE,
        SC_MAXIMIZE, SC_MINIMIZE, SC_RESTORE, STYLESTRUCT, SWP_HIDEWINDOW, SendMessageW, WINDOWPOS,
        WM_GETMINMAXINFO, WM_NCACTIVATE, WM_NCDESTROY, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN,
        WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE, WM_NCRBUTTONUP, WM_STYLECHANGING,
        WM_SYSCOMMAND, WM_WINDOWPOSCHANGED, WM_WINDOWPOSCHANGING, WS_CAPTION, WS_MINIMIZE,
    };
    const HT_MAX: LRESULT = HTMAXBUTTON as LRESULT;
    let kind = Drawn::of(data);
    let sizable = kind == Drawn::Sizable;
    let at = |lp: LPARAM| {
        (
            (lp & 0xffff) as i16 as i32,
            ((lp >> 16) & 0xffff) as i16 as i32,
        )
    };
    match msg {
        m if m != 0 && m == taskbar_button_created_message() => {
            // COM can dispatch this while a progress update is in flight. Record it only:
            // the next watchdog-covered taskbar step resends even an unchanged fraction.
            TASKBAR_BUTTONS.with_borrow_mut(|(generation, windows)| {
                *generation = generation.wrapping_add(1);
                windows.insert(hwnd as isize, *generation);
            });
            return 0;
        }
        WM_NCDESTROY => {
            TASKBAR_BUTTONS.with_borrow_mut(|(_, windows)| {
                windows.remove(&(hwnd as isize));
            });
            forget_caption(hwnd as isize);
        }
        WM_STYLECHANGING if lparam != 0 => {
            // SAFETY: for WM_STYLECHANGING, lParam points to the STYLESTRUCT of the change,
            // whose styleNew the window may alter (Win32 contract).
            let s = unsafe { &mut *(lparam as *mut STYLESTRUCT) };
            match wparam as i32 {
                GWL_STYLE => {
                    // The caption style stays only for the length of an animation.
                    let caption = if animating(hwnd) { WS_CAPTION } else { 0 };
                    s.styleNew = drawn_style(s.styleNew, kind) | caption;
                }
                GWL_EXSTYLE => s.styleNew = drawn_ex_style(s.styleNew),
                _ => {}
            }
            return 0;
        }
        // With the caption style of an animation (#874): the frame of the window without it,
        // the client filling the window inside its sizing frame as before, so its picture
        // doesn't move. Minimized, the system's.
        // SAFETY: our own window.
        WM_NCCALCSIZE
            if lparam != 0
                && animating(hwnd)
                && unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32 & WS_MINIMIZE == 0 =>
        {
            // SAFETY: with wParam TRUE, lParam points to NCCALCSIZE_PARAMS, whose first
            // rectangle is the window's and becomes its client's; with FALSE, to that rectangle
            // alone (Win32 contract).
            let r = unsafe {
                if wparam != 0 {
                    &mut (*(lparam as *mut NCCALCSIZE_PARAMS)).rgrc[0]
                } else {
                    &mut *(lparam as *mut RECT)
                }
            };
            let [l, t, rt, b] = unsafe { drawn_frame_edges(hwnd, kind) };
            r.left += l;
            r.top += t;
            r.right -= rt;
            r.bottom -= b;
            return 0;
        }
        // The system's frame for these styles, nothing of ours or winit's: what any reading of
        // the styles finds.
        WM_NCCALCSIZE => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        // Minimize, maximize and restore (the title bar's buttons, the taskbar, Win+Up and
        // Win+Down, the window menu all send these) animated: the caption style for their
        // length (#874), taken back once the window is in its new state, or once it is back
        // when it went to the taskbar.
        WM_SYSCOMMAND
            if matches!(
                (wparam & 0xFFF0) as u32,
                SC_MINIMIZE | SC_MAXIMIZE | SC_RESTORE
            ) =>
        {
            // SAFETY: our own window, framed by caption_less as `kind`, on its thread.
            unsafe {
                animation_caption(hwnd, kind, true);
                let res = DefSubclassProc(hwnd, msg, wparam, lparam);
                if IsIconic(hwnd) == 0 {
                    animation_caption(hwnd, kind, false);
                }
                return res;
            }
        }
        // Hidden (into the tray): the caption style for that moment, so it fades out as a window
        // with a title bar does (#874), taken back once it is hidden.
        WM_WINDOWPOSCHANGING | WM_WINDOWPOSCHANGED if lparam != 0 => {
            // SAFETY: for both, lParam points to the WINDOWPOS of the change (Win32 contract),
            // read here.
            let flags = unsafe { (*(lparam as *const WINDOWPOS)).flags };
            if flags & SWP_HIDEWINDOW != 0 {
                // SAFETY: our own window, framed by caption_less as `kind`, on its thread.
                unsafe { fade_caption(hwnd, kind, msg == WM_WINDOWPOSCHANGING) };
            }
            if msg == WM_WINDOWPOSCHANGED && animating(hwnd) {
                // As below.
                // SAFETY: a message posted to our own window.
                unsafe { PostMessageW(hwnd, settled_message(), 0, 0) };
            }
        }
        // Back from the taskbar some other way (winit restoring it, a program's ShowWindow):
        // the caption style goes as soon as the change is over.
        WM_WINDOWPOSCHANGED if animating(hwnd) => {
            // SAFETY: a message posted to our own window.
            unsafe { PostMessageW(hwnd, settled_message(), 0, 0) };
        }
        m if m == settled_message() => {
            // SAFETY: our own window, framed by caption_less as `kind`, on its thread.
            unsafe {
                if IsIconic(hwnd) == 0 {
                    animation_caption(hwnd, kind, false);
                }
            }
            return 0;
        }
        // Activation changes repaint the non-client area: with lParam -1 the system skips that.
        WM_NCACTIVATE => return unsafe { DefSubclassProc(hwnd, msg, wparam, -1) },
        WM_SYSCOMMAND if (wparam & 0xFFF0) == SC_HIDE_TO_TRAY => {
            HIDE_ASKED.with(|s| {
                if let Ok(mut s) = s.try_borrow_mut() {
                    s.insert(hwnd as isize);
                }
            });
            return 0;
        }
        // The window menu from the title bar, which the system opens only from a caption.
        WM_NCRBUTTONUP if wparam as LRESULT == HTCAPTION || wparam as LRESULT == HTSYSMENU => {
            let (x, y) = at(lparam);
            window_menu(hwnd, x, y, sizable);
            return 0;
        }
        WM_NCLBUTTONDOWN if wparam as LRESULT == HTSYSMENU => {
            // Under the icon, as the system opens it from its own.
            let hits = CAPTION_HITS.with(|m| {
                m.try_borrow()
                    .ok()
                    .and_then(|m| m.get(&(hwnd as isize)).map(|h| h.icon))
            });
            let mut pt = match hits {
                Some(icon) => POINT {
                    x: icon[0],
                    y: icon[3],
                },
                None => POINT { x: 0, y: 0 },
            };
            // SAFETY: our own window; the point is a local.
            unsafe { windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut pt) };
            window_menu(hwnd, pt.x, pt.y, sizable);
            return 0;
        }
        WM_NCLBUTTONDBLCLK if wparam as LRESULT == HTSYSMENU => {
            // SAFETY: our own window, a system command it handles.
            unsafe { PostMessageW(hwnd, WM_SYSCOMMAND, SC_CLOSE as WPARAM, 0) };
            return 0;
        }
        // Maximize: pressed and released on it toggles the window (the system's own button would
        // draw itself over ours).
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if sizable && wparam as LRESULT == HT_MAX => {
            set_max_button(
                hwnd,
                MaxButton {
                    hot: true,
                    pressed: true,
                },
            );
            return 0;
        }
        WM_NCLBUTTONUP if sizable && wparam as LRESULT == HT_MAX => {
            let was = max_button_state(hwnd as isize);
            set_max_button(
                hwnd,
                MaxButton {
                    hot: true,
                    pressed: false,
                },
            );
            if was.pressed {
                // SAFETY: our own window, a system command it handles.
                let zoomed = unsafe { IsZoomed(hwnd) } != 0;
                let cmd = if zoomed { SC_RESTORE } else { SC_MAXIMIZE };
                unsafe { SendMessageW(hwnd, WM_SYSCOMMAND, cmd as WPARAM, 0) };
            }
            return 0;
        }
        WM_NCMOUSEMOVE if sizable => {
            let on = wparam as LRESULT == HT_MAX;
            let was = max_button_state(hwnd as isize);
            set_max_button(
                hwnd,
                MaxButton {
                    hot: on,
                    pressed: was.pressed && on,
                },
            );
            if on {
                let mut t = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE | TME_NONCLIENT,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                // SAFETY: a plain call with a filled struct, our own window.
                unsafe { TrackMouseEvent(&mut t) };
            }
        }
        WM_NCMOUSELEAVE if sizable => set_max_button(hwnd, MaxButton::default()),
        WM_GETMINMAXINFO if sizable && lparam != 0 => {
            let res = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
            // SAFETY: for WM_GETMINMAXINFO, lParam points to the MINMAXINFO to fill.
            let info = unsafe {
                &mut *(lparam as *mut windows_sys::Win32::UI::WindowsAndMessaging::MINMAXINFO)
            };
            if let Some((r, m)) = maximized_to_work_area(hwnd) {
                info.ptMaxPosition = POINT {
                    x: r.left - m.x,
                    y: r.top - m.y,
                };
                info.ptMaxSize = POINT {
                    x: r.right - r.left,
                    y: r.bottom - r.top,
                };
            }
            return res;
        }
        _ => {}
    }
    let res = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
    if msg == WM_NCHITTEST && res == HTCLIENT {
        let (x, y) = at(lparam);
        let mut pt = POINT { x, y };
        // SAFETY: our own window; the point is a local.
        unsafe { ScreenToClient(hwnd, &mut pt) };
        let hit = CAPTION_HITS.with(|m| {
            m.try_borrow().ok().and_then(|m| {
                m.get(&(hwnd as isize)).map(|h| {
                    if sizable
                        && h.max_button
                            .and_then(|i| h.buttons.get(i))
                            .is_some_and(|b| inside(b, pt.x, pt.y))
                    {
                        HT_MAX
                    } else {
                        frame_hit(pt.x, pt.y, h)
                    }
                })
            })
        });
        return hit.unwrap_or(HTCLIENT);
    }
    res
}

/// Where window `hwnd` is when maximized, on the screen: its monitor's work area (not over the
/// taskbar), with its sizing frame (without the caption style it has while it maximizes, #874)
/// beyond the work area's edges, as a window with a caption style is maximized (Windows maximizes
/// one without a caption over the whole monitor). And its monitor's top left, from which
/// WM_GETMINMAXINFO counts.
fn maximized_to_work_area(hwnd: HWND) -> Option<(RECT, windows_sys::Win32::Foundation::POINT)> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
    };
    // SAFETY: plain queries about our own window and its monitor, into locals.
    unsafe {
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        if mon.is_null() || GetMonitorInfoW(mon, &mut mi) == 0 {
            return None;
        }
        let (w, m) = (mi.rcWork, mi.rcMonitor);
        let [l, t, r, b] = drawn_frame_edges(hwnd, Drawn::Sizable);
        Some((
            RECT {
                left: w.left - l,
                top: w.top - t,
                right: w.right + r,
                bottom: w.bottom + b,
            },
            POINT {
                x: m.left,
                y: m.top,
            },
        ))
    }
}

#[cfg(test)]
mod frame_tests {
    use super::*;

    #[test]
    fn the_drawn_title_bar_hits_as_caption_icon_and_buttons() {
        // 548 × 32 points at 1.5 px per point, 4 buttons of 46 at the end, the icon at the start.
        let h = CaptionHits {
            strip_h: 48,
            icon: [0, 0, 48, 48],
            buttons: (0..4)
                .map(|i| {
                    let x1 = 822 - i * 69;
                    [x1 - 69, 0, x1, 48]
                })
                .collect(),
            max_button: None,
        };
        assert_eq!(frame_hit(10, 10, &h), HTSYSMENU);
        assert_eq!(frame_hit(300, 10, &h), HTCAPTION);
        assert_eq!(frame_hit(821, 10, &h), HTCLIENT);
        assert_eq!(frame_hit(822 - 4 * 69, 10, &h), HTCLIENT);
        assert_eq!(frame_hit(822 - 4 * 69 - 1, 10, &h), HTCAPTION);
        // Below the bar: the client's.
        assert_eq!(frame_hit(300, 48, &h), HTCLIENT);
        // Nothing published yet: everything the client's (frame_hit isn't asked then).
        assert_eq!(frame_hit(300, 10, &CaptionHits::default()), HTCLIENT);
    }
}

#[cfg(test)]
mod caption_less_tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AdjustWindowRectEx, CreateWindowExW, DestroyWindow, DispatchMessageW, GetClientRect,
        GetWindowRect, MINMAXINFO, MSG, PM_REMOVE, PeekMessageW, SWP_NOSIZE, SendMessageW,
        WM_GETMINMAXINFO, WS_BORDER, WS_CAPTION, WS_EX_APPWINDOW, WS_POPUP,
    };

    /// The styles winit gives the progress window (a caption, a sizing frame trimmed by us, a
    /// system menu, Minimize; 0x14CE0000 less WS_VISIBLE) and the main window (Maximize too), and
    /// their extended styles (a window edge, an app window: a taskbar button).
    const WINIT_PROGRESS: u32 = 0x04CE_0000;
    const WINIT_MAIN: u32 = 0x04CF_0000;
    const WINIT_EX: u32 = 0x0004_0100;

    /// The frame (left, top, right, bottom) the styles alone put round a client: what a reader
    /// of the styles, rather than of the window's WM_NCCALCSIZE, takes for it.
    fn styles_frame(style: u32, ex: u32) -> [i32; 4] {
        let mut rc: RECT = unsafe { zeroed() };
        // SAFETY: plain computation on a local rectangle.
        unsafe { AdjustWindowRectEx(&mut rc, style, 0, ex) };
        [-rc.left, -rc.top, rc.right, rc.bottom]
    }

    #[test]
    fn a_drawn_title_bar_s_window_has_no_caption_style() {
        // The caption style the band came from (#208): 31 px above the client, by the styles.
        assert!(styles_frame(WINIT_PROGRESS, WINIT_EX)[1] > 20);
        let fixed = drawn_style(WINIT_PROGRESS, Drawn::Fixed);
        assert_eq!(fixed & WS_CAPTION, 0);
        assert_eq!(fixed & WS_THICKFRAME, 0);
        assert_ne!(fixed & WS_POPUP, 0);
        // The system menu and Minimize stay (Alt+Space, the taskbar's commands).
        assert_eq!(fixed & 0x000A_0000, 0x000A_0000);
        assert_eq!(styles_frame(fixed, drawn_ex_style(WINIT_EX)), [0, 0, 0, 0]);
        // Windows 10's (#620): the same with the system's 1 px border, the same on every side.
        let edged = drawn_style(WINIT_PROGRESS, Drawn::Edged);
        assert_eq!(edged & WS_CAPTION, WS_BORDER);
        assert_eq!(edged & WS_THICKFRAME, 0);
        assert_ne!(edged & WS_POPUP, 0);
        assert_eq!(edged & 0x000A_0000, 0x000A_0000);
        assert_eq!(styles_frame(edged, drawn_ex_style(WINIT_EX)), [1, 1, 1, 1]);
        // Resizable: the sizing frame, the same on every side (none taller at the top).
        let sizable = drawn_style(WINIT_MAIN, Drawn::Sizable);
        assert_eq!(sizable & WS_CAPTION, 0);
        assert_ne!(sizable & WS_THICKFRAME, 0);
        assert_eq!(sizable & 0x000B_0000, 0x000B_0000);
        let [l, t, r, b] = styles_frame(sizable, WINIT_EX);
        assert!(l > 0 && l == t && t == r && r == b, "{l} {t} {r} {b}");
        // Applied again (winit setting its own styles once more), nothing changes.
        assert_eq!(drawn_style(fixed, Drawn::Fixed), fixed);
        assert_eq!(drawn_style(edged, Drawn::Edged), edged);
        for kind in [Drawn::Fixed, Drawn::Edged, Drawn::Sizable] {
            assert_eq!(Drawn::of(kind.data()), kind);
        }
        assert_eq!(drawn_style(sizable, Drawn::Sizable), sizable);
        // IDM's dialog frame goes; the taskbar button stays.
        let ex = drawn_ex_style(WINIT_EX | WS_EX_DLGMODALFRAME);
        assert_eq!(ex & WS_EX_DLGMODALFRAME, 0);
        assert_ne!(ex & WS_EX_APPWINDOW, 0);
    }

    /// A real (hidden, off-screen) window with winit's styles, framed by caption_less: its client
    /// is where its styles say, it keeps its styles when they are set again (winit does on every
    /// change of its flags), its edges size it (resizable) or don't (fixed), and maximized it
    /// fills its monitor's work area, not the whole monitor.
    #[test]
    fn caption_less_frames_a_real_window_as_its_styles_say() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{HTBOTTOMRIGHT, HTLEFT, HTTOP};
        for (n, (style, kind)) in [
            (WINIT_PROGRESS, Drawn::Fixed),
            (WINIT_PROGRESS, Drawn::Edged),
            (WINIT_MAIN, Drawn::Sizable),
        ]
        .into_iter()
        .enumerate()
        {
            let title = format!("mdm caption-less test {n} {}", std::process::id());
            let wide: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
            // A class of its own whose procedure is the system's (a STATIC control answers
            // every hit test as transparent).
            let class: Vec<u16> = "mdm-caption-less-test\0".encode_utf16().collect();
            // SAFETY: a class registered with the system's own procedure, and a hidden window
            // of it on this thread, destroyed below; the strings are NUL-terminated; plain
            // queries and messages to it.
            unsafe {
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    DefWindowProcW, RegisterClassW, WNDCLASSW,
                };
                let mut wc: WNDCLASSW = zeroed();
                wc.lpfnWndProc = Some(DefWindowProcW);
                wc.lpszClassName = class.as_ptr();
                // Registered once for both windows (the second call fails: already there).
                RegisterClassW(&wc);
                let hwnd = CreateWindowExW(
                    WINIT_EX,
                    class.as_ptr(),
                    wide.as_ptr(),
                    style,
                    -20000,
                    -20000,
                    548,
                    444,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                assert!(!hwnd.is_null());
                caption_less(hwnd, kind);
                let st = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
                let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
                assert_eq!(st, drawn_style(style, kind));
                // winit setting its caption style again: the subclass keeps ours.
                SetWindowLongPtrW(hwnd, GWL_STYLE, (st | WS_CAPTION) as isize);
                assert_eq!(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, st);
                // The client is where the styles alone put it: no reading of them can differ.
                let (mut w, mut c): (RECT, RECT) = (zeroed(), zeroed());
                GetWindowRect(hwnd, &mut w);
                GetClientRect(hwnd, &mut c);
                let mut at = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
                windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut at);
                let [l, t, r, b] = styles_frame(st, ex);
                assert_eq!([at.x - w.left, at.y - w.top], [l, t]);
                assert_eq!(
                    [c.right, c.bottom],
                    [w.right - w.left - l - r, w.bottom - w.top - t - b]
                );
                // Its edges: the system's sizing edges where it is resizable, else the client's
                // (the progress window keeps its size).
                let hit = |x: i32, y: i32| {
                    SendMessageW(
                        hwnd,
                        WM_NCHITTEST,
                        0,
                        ((y as u16 as u32) << 16 | x as u16 as u32) as LPARAM,
                    )
                };
                let mid = (w.top + w.bottom) / 2;
                let sizable = kind == Drawn::Sizable;
                assert_eq!(
                    hit(w.left + 1, mid),
                    if sizable { HTLEFT as LRESULT } else { HTCLIENT }
                );
                assert_eq!(
                    hit((w.left + w.right) / 2, w.top + 1),
                    if sizable { HTTOP as LRESULT } else { HTCLIENT }
                );
                assert_eq!(
                    hit(w.right - 2, w.bottom - 2),
                    if sizable {
                        HTBOTTOMRIGHT as LRESULT
                    } else {
                        HTCLIENT
                    }
                );
                // Resized, the client follows as the styles say.
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    0,
                    0,
                    700,
                    500,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
                GetClientRect(hwnd, &mut c);
                assert_eq!([c.right, c.bottom], [700 - l - r, 500 - t - b]);
                // Minimizing, maximizing or restoring (#874): the caption style for the length of
                // the animation, and the window framed as without it, its client in place.
                animation_caption(hwnd, kind, true);
                let on = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
                assert_eq!(on, st | WS_CAPTION);
                // winit setting its own styles again: it stays while the animation lasts.
                SetWindowLongPtrW(hwnd, GWL_STYLE, st as isize);
                assert_eq!(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, on);
                let framed = |hwnd| {
                    let (mut w, mut c): (RECT, RECT) = (zeroed(), zeroed());
                    GetWindowRect(hwnd, &mut w);
                    GetClientRect(hwnd, &mut c);
                    let mut at = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
                    windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut at);
                    [
                        at.x - w.left,
                        at.y - w.top,
                        w.right - w.left - c.right,
                        w.bottom - w.top - c.bottom,
                    ]
                };
                assert_eq!(framed(hwnd), [l, t, l + r, t + b]);
                // Maximized (asked, not done: that would show it): its monitor's work area, with
                // the caption style of the animation on as without it.
                if sizable {
                    let mut info: MINMAXINFO = zeroed();
                    SendMessageW(
                        hwnd,
                        WM_GETMINMAXINFO,
                        0,
                        &mut info as *mut MINMAXINFO as LPARAM,
                    );
                    // The work area with the sizing frame beyond its edges, from the monitor's
                    // top left.
                    use windows_sys::Win32::Graphics::Gdi::{
                        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
                    };
                    let mut mi: MONITORINFO = zeroed();
                    mi.cbSize = size_of::<MONITORINFO>() as u32;
                    GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mi);
                    let (wa, m) = (mi.rcWork, mi.rcMonitor);
                    assert_eq!(
                        [info.ptMaxPosition.x, info.ptMaxPosition.y],
                        [wa.left - m.left - l, wa.top - m.top - t]
                    );
                    assert_eq!(
                        [info.ptMaxSize.x, info.ptMaxSize.y],
                        [wa.right - wa.left + l + r, wa.bottom - wa.top + t + b]
                    );
                }
                // Moved or sized with it on (a window back from the taskbar): it goes once the
                // window has settled, by the message the window posts itself; nothing else moves.
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    -19990,
                    -19990,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
                assert_eq!(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, on);
                assert_eq!(framed(hwnd), [l, t, l + r, t + b]);
                let mut msg: MSG = zeroed();
                while PeekMessageW(&mut msg, hwnd, 0, 0, PM_REMOVE) != 0 {
                    DispatchMessageW(&msg);
                }
                assert_eq!(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, st);
                assert!(!animating(hwnd));
                assert_eq!(framed(hwnd), [l, t, l + r, t + b]);
                // Hidden (#874: into the tray, so Windows fades it out): the caption style from
                // the change's start, kept if winit sets its styles meanwhile, the frame
                // untouched, gone once it is hidden. Shown, or moved, nothing changes. (The
                // messages of the change, sent as the system sends them: the test window itself
                // never shows.)
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    SWP_HIDEWINDOW, SWP_SHOWWINDOW, WINDOWPOS, WM_WINDOWPOSCHANGED,
                    WM_WINDOWPOSCHANGING,
                };
                for change in [SWP_HIDEWINDOW, SWP_SHOWWINDOW, 0] {
                    let mut pos: WINDOWPOS = zeroed();
                    pos.hwnd = hwnd;
                    pos.flags = change | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE;
                    let lp = &mut pos as *mut WINDOWPOS as LPARAM;
                    SendMessageW(hwnd, WM_WINDOWPOSCHANGING, 0, lp);
                    let during = if change == SWP_HIDEWINDOW {
                        st | WS_CAPTION
                    } else {
                        st
                    };
                    assert_eq!(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, during);
                    SetWindowLongPtrW(hwnd, GWL_STYLE, st as isize);
                    assert_eq!(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, during);
                    assert_eq!(framed(hwnd), [l, t, l + r, t + b]);
                    SendMessageW(hwnd, WM_WINDOWPOSCHANGED, 0, lp);
                    assert_eq!(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, st);
                    assert!(!animating(hwnd) && !fading(hwnd));
                    assert_eq!(framed(hwnd), [l, t, l + r, t + b]);
                }
                // Hidden while it sits minimized with the caption of its animation: it keeps
                // it (it goes once the window is back, as above).
                animation_caption(hwnd, kind, true);
                let mut pos: WINDOWPOS = zeroed();
                pos.hwnd = hwnd;
                pos.flags = SWP_HIDEWINDOW | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER;
                let lp = &mut pos as *mut WINDOWPOS as LPARAM;
                SendMessageW(hwnd, WM_WINDOWPOSCHANGING, 0, lp);
                SendMessageW(hwnd, WM_WINDOWPOSCHANGED, 0, lp);
                assert_eq!(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, st | WS_CAPTION);
                assert!(animating(hwnd));
                animation_caption(hwnd, kind, false);
                assert_eq!(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, st);
                DestroyWindow(hwnd);
            }
        }
    }
}

#[cfg(test)]
mod size_to_client_tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowRect,
        NCCALCSIZE_PARAMS, RegisterClassW, WNDCLASSW, WS_BORDER, WS_POPUP,
    };

    /// The size a window must be for the client asked for: its frame added back, whatever it is.
    #[test]
    fn the_window_grows_or_shrinks_by_what_its_client_is_off() {
        // #1495: Keys at 125 % on Windows 10 in 0.5.2, a 500 × 240 px client asked for in a
        // window of 505 × 243 whose frame was 3 px a side, not 2.5: 499 × 240 came back.
        assert_eq!(
            outer_for_client([505, 243], [499, 240], [500, 240]),
            Some([506, 243])
        );
        // Over: smaller.
        assert_eq!(
            outer_for_client([512, 250], [504, 246], [500, 240]),
            Some([508, 244])
        );
        assert_eq!(outer_for_client([502, 242], [500, 240], [500, 240]), None);
        assert_eq!(client_px([400.0, 192.0], 1.25), [500, 240]);
        assert_eq!(client_px([420.0, 268.0], 1.15), [483, 308]);
    }

    /// A window procedure framing the client as Windows 10 framed a window whose title bar is
    /// ours in 0.5.2 at 125 %: 3 px left, right and bottom (the 9 px sizing frame less a fixed
    /// 6), none at the top, where its styles (a plain popup) say there is none.
    unsafe extern "system" fn three_px_frame(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_NCCALCSIZE && wparam != 0 {
            // SAFETY: with wParam TRUE, lParam points to NCCALCSIZE_PARAMS (Win32 contract).
            let r = unsafe { &mut (*(lparam as *mut NCCALCSIZE_PARAMS)).rgrc[0] };
            r.left += 3;
            r.right -= 3;
            r.bottom -= 3;
            return 0;
        }
        // SAFETY: the system's own procedure, with the message as it came.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    /// Real (hidden, off-screen) windows: one framed as its styles say has its client at once;
    /// one framed otherwise (0.5.2's frame on Windows 10 at 125 %) has it too, its window
    /// made larger by what the client came back short.
    #[test]
    fn a_window_s_client_is_what_was_asked_whatever_its_frame() {
        for (n, (proc_, style, frame)) in [
            (
                DefWindowProcW as unsafe extern "system" fn(_, _, _, _) -> _,
                WS_POPUP | WS_BORDER,
                [2, 2],
            ),
            (three_px_frame, WS_POPUP, [6, 3]),
        ]
        .into_iter()
        .enumerate()
        {
            let class: Vec<u16> = format!("mdm-size-to-client-test-{n}\0")
                .encode_utf16()
                .collect();
            // SAFETY: a class of this test's, and a hidden window of it on this thread,
            // destroyed below; the strings are NUL-terminated; plain queries to it.
            unsafe {
                let mut wc: WNDCLASSW = zeroed();
                wc.lpfnWndProc = Some(proc_);
                wc.lpszClassName = class.as_ptr();
                RegisterClassW(&wc);
                let hwnd = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    class.as_ptr(),
                    style,
                    -20000,
                    -20000,
                    300,
                    200,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                assert!(!hwnd.is_null());
                size_to_client(hwnd, style, 0, [500, 240]);
                let (mut w, mut c): (RECT, RECT) = (zeroed(), zeroed());
                GetWindowRect(hwnd, &mut w);
                GetClientRect(hwnd, &mut c);
                assert_eq!([c.right, c.bottom], [500, 240], "{n}");
                assert_eq!(
                    [w.right - w.left, w.bottom - w.top],
                    [500 + frame[0], 240 + frame[1]],
                    "{n}"
                );
                DestroyWindow(hwnd);
            }
        }
    }
}

#[cfg(test)]
mod find_own_window_tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, WNDCLASSW,
    };

    /// Our windows are found by their whole title, however long (a progress window is named
    /// after its file, #540), and never by a title that only starts the same.
    #[test]
    fn a_window_is_found_by_its_whole_title_however_long() {
        let class: Vec<u16> = "mdm-find-own-window-test\0".encode_utf16().collect();
        // SAFETY: a class registered with the system's own procedure, and hidden windows of it
        // on this thread, destroyed below; the strings are NUL-terminated.
        unsafe {
            let mut wc: WNDCLASSW = zeroed();
            wc.lpfnWndProc = Some(DefWindowProcW);
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
            let make = |title: &str| {
                let t: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
                let h = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    t.as_ptr(),
                    0,
                    -20000,
                    -20000,
                    100,
                    100,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                assert!(!h.is_null());
                h
            };
            for len in [40, 255, 256, 300, 1000] {
                let head = format!("mdm find test {} {len} ", std::process::id());
                let title = format!("{head}{}", "x".repeat(len - head.len()));
                assert_eq!(title.encode_utf16().count(), len);
                // One title longer by a unit, made first: found first, were it taken for ours.
                let longer = make(&format!("{title}x"));
                let window = make(&title);
                assert_eq!(find_own_window(&title), Some(window), "{len} units");
                DestroyWindow(window);
                assert_eq!(find_own_window(&title), None, "{len} units, gone");
                assert_eq!(find_own_window(&format!("{title}x")), Some(longer));
                DestroyWindow(longer);
            }
        }
    }

    /// #919: a window is given its title without isolates, and found by the title MDM knows it
    /// by, isolates and all.
    #[test]
    fn a_window_titled_without_isolates_is_found() {
        let title = format!(
            "71% {} {}",
            crate::i18n::isolate("mdm isolate test.zip"),
            std::process::id()
        );
        let given = crate::i18n::os_text(&title);
        assert!(
            !given
                .chars()
                .any(|c| ('\u{2066}'..='\u{2069}').contains(&c))
        );
        let window = test_window(&given);
        assert_eq!(find_own_window(&title), Some(window));
        // SAFETY: the test's own window, made on this thread.
        unsafe { DestroyWindow(window) };
    }

    /// A hidden test window of this thread titled `title`, destroyed by the caller.
    fn test_window(title: &str) -> HWND {
        let class: Vec<u16> = "mdm-find-own-window-test-2\0".encode_utf16().collect();
        let t: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
        // SAFETY: a class registered with the system's own procedure (again: a no-op), and a
        // hidden window of it on this thread; the strings are NUL-terminated.
        unsafe {
            let mut wc: WNDCLASSW = zeroed();
            wc.lpfnWndProc = Some(DefWindowProcW);
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
            let h = CreateWindowExW(
                0,
                class.as_ptr(),
                t.as_ptr(),
                0,
                -20000,
                -20000,
                100,
                100,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!h.is_null());
            h
        }
    }

    /// #583: looking for our window doesn't wait for another thread of this process whose window
    /// isn't answering (busy, on a stalled disk): the titles are read without a message.
    #[test]
    fn a_busy_threads_window_does_not_hold_the_search() {
        let pid = std::process::id();
        let (made, window_made) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let busy = std::thread::spawn(move || {
            let h = test_window(&format!("mdm busy window {pid}"));
            made.send(()).unwrap();
            // Busy: no message is taken until the test is done.
            let _ = released.recv();
            // SAFETY: the window made above, on this thread.
            unsafe { DestroyWindow(h) };
        });
        window_made.recv().unwrap();
        let (answer, answered) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = answer.send(find_own_window(&format!("mdm no such window {pid}")).is_some());
        });
        let got = answered.recv_timeout(std::time::Duration::from_secs(3));
        release.send(()).unwrap();
        busy.join().unwrap();
        assert_eq!(
            got,
            Ok(false),
            "the search waited for the busy thread's window"
        );
    }

    /// #633: DWM is asked for a window's caption once per change, not each time it's re-asserted.
    #[test]
    fn the_caption_goes_to_dwm_only_when_it_changes() {
        use std::sync::atomic::Ordering::Relaxed;
        let title = format!("mdm caption test {}", std::process::id());
        let h = test_window(&title);
        let calls = || CAPTION_DWM_CALLS.load(Relaxed);
        let before = calls();
        for _ in 0..50 {
            assert!(caption_dark(&title, true));
        }
        assert_eq!(calls() - before, 1, "dark, asked once");
        for _ in 0..50 {
            assert!(caption_dark(&title, false));
        }
        assert_eq!(calls() - before, 2, "light, once more");
        assert!(caption_dark(&title, true));
        assert_eq!(calls() - before, 3, "dark again");
        // SAFETY: the window made above, on this thread.
        unsafe { DestroyWindow(h) };
        assert!(!caption_dark(&title, true), "gone: not found");
        assert_eq!(calls() - before, 3);
    }
}

#[cfg(test)]
mod come_forward_tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, GW_HWNDNEXT, GetClientRect, GetTopWindow,
        GetWindow, GetWindowRect, RegisterClassW, WNDCLASSW, WS_EX_TOPMOST,
    };

    /// Lifted only when it didn't get the foreground, and never a topmost one (a question, a
    /// notice) or a minimized one.
    #[test]
    fn only_a_window_left_behind_is_lifted() {
        assert!(lifted_when_shown(false, false, false));
        assert!(!lifted_when_shown(true, false, false));
        assert!(!lifted_when_shown(false, true, false));
        assert!(!lifted_when_shown(false, false, true));
    }

    /// Where `hwnd` is in the z-order of the top-level windows (0: the top).
    fn z(hwnd: HWND) -> usize {
        // SAFETY: plain queries walking the desktop's top-level windows.
        unsafe {
            let mut w = GetTopWindow(std::ptr::null_mut());
            let mut i = 0;
            while !w.is_null() && w != hwnd {
                w = GetWindow(w, GW_HWNDNEXT);
                i += 1;
            }
            assert!(!w.is_null(), "not a top-level window");
            i
        }
    }

    /// Two real (hidden, off-screen) windows with winit's styles for the progress window, one
    /// framed by caption_less as on Windows 11: each in turn goes above the other, neither stays
    /// topmost, and nothing else of it changes (styles, place, size, client: the frame #208's
    /// first part made stays as it was).
    #[test]
    fn above_others_lifts_a_real_window_and_changes_nothing_else() {
        const WINIT_PROGRESS: u32 = 0x04CE_0000;
        const WINIT_EX: u32 = 0x0004_0100;
        let class: Vec<u16> = "mdm-come-forward-test\0".encode_utf16().collect();
        // SAFETY: a class registered with the system's own procedure, and hidden windows of it on
        // this thread, destroyed below; the strings are NUL-terminated; plain queries, and
        // z-order changes of our own windows.
        unsafe {
            let mut wc: WNDCLASSW = zeroed();
            wc.lpfnWndProc = Some(DefWindowProcW);
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
            let make = |n: u32| {
                let title: Vec<u16> = format!("mdm come-forward test {n} {}", std::process::id())
                    .encode_utf16()
                    .chain(Some(0))
                    .collect();
                let h = CreateWindowExW(
                    WINIT_EX,
                    class.as_ptr(),
                    title.as_ptr(),
                    WINIT_PROGRESS,
                    -20000,
                    -20000,
                    548,
                    444,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                assert!(!h.is_null());
                h
            };
            let (a, b) = (make(0), make(1));
            caption_less(a, Drawn::Fixed);
            let state = |h: HWND| {
                let (mut w, mut c): (RECT, RECT) = (zeroed(), zeroed());
                GetWindowRect(h, &mut w);
                GetClientRect(h, &mut c);
                (
                    GetWindowLongPtrW(h, GWL_STYLE) as u32,
                    GetWindowLongPtrW(h, GWL_EXSTYLE) as u32,
                    [w.left, w.top, w.right, w.bottom],
                    [c.right, c.bottom],
                )
            };
            for (up, other) in [(a, b), (b, a), (a, b)] {
                let before = state(up);
                above_others(up);
                assert!(z(up) < z(other), "lifted above the other window");
                let after = state(up);
                assert_eq!(after.1 & WS_EX_TOPMOST, 0, "left topmost");
                assert_eq!(before, after, "only its place in the z-order changes");
            }
            DestroyWindow(a);
            DestroyWindow(b);
        }
    }
}

#[cfg(test)]
mod main_window_tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, WNDCLASSW,
    };

    /// #337: two windows under one title, the way a failed Direct3D start leaves them. The
    /// window the app was made for is the one found, whichever is on top; and once it is gone
    /// the search by title still answers.
    #[test]
    fn the_known_window_wins_over_its_namesake() {
        let class: Vec<u16> = "mdm-main-window-test ".encode_utf16().collect();
        let name = format!("mdm main window test {}", std::process::id());
        let title: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: a class registered with the system's own procedure, and hidden windows of it
        // on this thread, destroyed below; the strings are NUL-terminated.
        unsafe {
            let mut wc: WNDCLASSW = zeroed();
            wc.lpfnWndProc = Some(DefWindowProcW);
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
            let make = || {
                let h = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    title.as_ptr(),
                    0,
                    -20000,
                    -20000,
                    100,
                    100,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                assert!(!h.is_null());
                h
            };
            let (a, b) = (make(), make());
            assert_eq!(own_window(&name, Some(a)), Some(a));
            assert_eq!(own_window(&name, Some(b)), Some(b));
            assert!(own_window(&name, None).is_some());
            DestroyWindow(b);
            assert_eq!(own_window(&name, Some(b)), Some(a), "gone: found by title");
            DestroyWindow(a);
            assert_eq!(own_window(&name, Some(a)), None);
        }
    }
}

/// The main window's "Minimize to the notification area" (#63): its minimize command (title bar,
/// system menu, taskbar) asks the app to hide it instead. The app can't see the window go
/// minimized: an idle app runs no frame while minimized, so it would never know.
struct MainMinimize {
    hwnd: isize,
    ctx: Option<egui::Context>,
    enabled: bool,
    asked: bool,
}

thread_local! {
    static MAIN_MINIMIZE: std::cell::RefCell<MainMinimize> = const {
        std::cell::RefCell::new(MainMinimize { hwnd: 0, ctx: None, enabled: false, asked: false })
    };
}

unsafe extern "system" fn main_minimize_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::WM_SYSCOMMAND;
    const SC_MINIMIZE: usize = 0xF020;
    if msg == WM_SYSCOMMAND && (wparam & 0xFFF0) == SC_MINIMIZE {
        let caught = MAIN_MINIMIZE.with(|m| {
            let Ok(mut m) = m.try_borrow_mut() else {
                return false;
            };
            if !m.enabled {
                return false;
            }
            m.asked = true;
            if let Some(ctx) = &m.ctx {
                ctx.request_repaint();
            }
            true
        });
        if caught {
            return 0;
        }
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// Keep the main window's minimize command in step with "Minimize to the notification area"
/// (`enabled`: the option is on and a tray shows MDM's icon). Call every frame (cheap once the
/// window is found). True once after the command came: hide the window now.
pub fn main_minimize_to_tray(ctx: &egui::Context, enabled: bool) -> bool {
    MAIN_MINIMIZE.with(|m| {
        let Ok(mut m) = m.try_borrow_mut() else {
            return false;
        };
        if m.hwnd == 0
            && let Some(hwnd) = find_own_window(crate::MAIN_TITLE)
        {
            // SAFETY: our own window; the proc lives as long as the program.
            unsafe { SetWindowSubclass(hwnd, Some(main_minimize_proc), 0x7a1, 0) };
            m.hwnd = hwnd as isize;
            m.ctx = Some(ctx.clone());
        }
        m.enabled = enabled;
        std::mem::take(&mut m.asked)
    })
}

/// The highest icon id [`tray_icon_ask`] tries. tray-icon gives each icon an id of its own
/// from a counter it shares with its icon names, and doesn't say which (a program's first icon
/// has 2).
const TRAY_ICON_IDS: u32 = 16;

/// Longer than the shell takes to answer about an icon: a "no" that took this long is
/// Shell_NotifyIcon giving up on a busy Explorer, not the shell's answer.
const TRAY_SLOW: std::time::Duration = std::time::Duration::from_millis(1500);

/// What the shell said of MDM's notification-area icon ([`tray_icon_ask`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAnswer {
    /// No taskbar (no `Shell_TrayWnd` window): Explorer isn't running, or a shell without a
    /// notification area is.
    NoTaskbar,
    /// The taskbar is there but didn't answer: Windows says it stopped responding, or the
    /// question took as long as a timeout.
    Silent,
    /// The shell answered: the icon's id when it holds it (wherever it shows it), and whether it
    /// gives the icon a place on screen.
    Answered { held: Option<u32>, placed: bool },
}

/// Ask the shell about the notification-area icon of `hwnd` (the tray's own hidden window, which
/// has one icon); `known` is its id once found. Both questions wait for Explorer, for seconds when
/// it is busy (#747: 8 s), so this runs on the tray's watcher thread (tray.rs), never on the
/// window's.
///
/// - Held: an empty `NIM_MODIFY` changes nothing and succeeds only for an icon the shell has, in
///   the taskbar or among the hidden icons behind ^ (`known` first, else each id up to
///   [`TRAY_ICON_IDS`]).
/// - Placed: `Shell_NotifyIconGetRect`. Not enough on its own: Windows 10 and Windows 11 builds
///   before 26200 give an icon behind ^ no place (#750, #718, #754: 0.5.2 asked only this, and
///   left the window in the taskbar with the icon there); build 26200 gives it the place of ^.
///
/// `rect_withheld` answers "no place" without asking (MDM_TEST_TRAY_RECT=0: the reporters' case,
/// on a Windows that gives one).
pub fn tray_icon_ask(hwnd: isize, known: Option<u32>, rect_withheld: bool) -> TrayAnswer {
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, IsHungAppWindow};
    let hwnd = hwnd as HWND;
    if hwnd.is_null() {
        return TrayAnswer::Answered {
            held: None,
            placed: false,
        };
    }
    let class: Vec<u16> = "Shell_TrayWnd\0".encode_utf16().collect();
    // SAFETY: a NUL-ended class name and no title: no window is sent a message.
    let taskbar = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
    if taskbar.is_null() {
        return TrayAnswer::NoTaskbar;
    }
    // SAFETY: reads the window's state; never waits for it.
    if unsafe { IsHungAppWindow(taskbar) } != 0 {
        return TrayAnswer::Silent;
    }
    let start = std::time::Instant::now();
    let ids = || {
        known
            .into_iter()
            .chain((1..=TRAY_ICON_IDS).filter(move |id| Some(*id) != known))
    };
    let held = ids().find(|id| tray_icon_held(hwnd, *id));
    // An icon it doesn't hold is asked for its place too, under each id: should an empty
    // NIM_MODIFY fail on some Windows for an icon it has, its place still says it is there.
    let placed = !rect_withheld
        && match held {
            Some(id) => tray_icon_placed(hwnd, id),
            None => ids().any(|id| tray_icon_placed(hwnd, id)),
        };
    if held.is_none() && !placed && start.elapsed() >= TRAY_SLOW {
        return TrayAnswer::Silent;
    }
    TrayAnswer::Answered { held, placed }
}

/// Whether the shell holds icon `id` of `hwnd`: an empty `NIM_MODIFY`.
fn tray_icon_held(hwnd: HWND, id: u32) -> bool {
    use windows_sys::Win32::UI::Shell::{NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW};
    let nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: id,
        uFlags: 0,
        // SAFETY: a plain C struct; all zero is its empty value.
        ..unsafe { zeroed() }
    };
    // SAFETY: nid lives for the call; the shell only reads it.
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) != 0 }
}

/// Whether the shell gives icon `id` of `hwnd` a place on screen.
fn tray_icon_placed(hwnd: HWND, id: u32) -> bool {
    use windows_sys::Win32::UI::Shell::{NOTIFYICONIDENTIFIER, Shell_NotifyIconGetRect};
    let ident = NOTIFYICONIDENTIFIER {
        cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
        hWnd: hwnd,
        uID: id,
        // SAFETY: a plain C struct; with a zero GUID the icon is named by its window and id.
        ..unsafe { zeroed() }
    };
    // SAFETY: a plain C struct, written by the call.
    let mut rect: RECT = unsafe { zeroed() };
    // SAFETY: both live for the call.
    unsafe { Shell_NotifyIconGetRect(&ident, &mut rect) >= 0 }
}

/// Set the tip of icon `id` of `hwnd` (127 characters at most, the rest cut). Waits for Explorer
/// as [`tray_icon_ask`] does, so it too runs on the tray's watcher thread. False when the shell
/// didn't take it (no such icon, or no answer).
pub fn tray_icon_tip(hwnd: isize, id: u32, tip: &str) -> bool {
    use windows_sys::Win32::UI::Shell::{NIF_TIP, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW};
    let mut nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd as HWND,
        uID: id,
        uFlags: NIF_TIP,
        // SAFETY: a plain C struct; all zero is its empty value.
        ..unsafe { zeroed() }
    };
    // The last unit stays the NUL that ends it. Filled in a copy: on 32-bit Windows the struct
    // is packed, and its fields can't be borrowed in place.
    let mut text = nid.szTip;
    let room = text.len() - 1;
    for (slot, unit) in text.iter_mut().zip(tip.encode_utf16().take(room)) {
        *slot = unit;
    }
    nid.szTip = text;
    // SAFETY: nid lives for the call; the shell only reads it.
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) != 0 }
}

/// The size the notification area draws MDM's icon at, in px, and the taskbar it draws it in (its
/// window, 0 if none): Windows' small icon size (`SM_CXSMICON`) at the taskbar's DPI (#1161). The
/// taskbar window's own DPI, not the system's: the system DPI is the main display's scale as it
/// was when the user signed in, kept until they sign out, while Explorer draws the taskbar at its
/// display's scale as soon as it changes. With no taskbar (Explorer isn't running), the system
/// DPI. Never waits: Explorer is sent no message.
pub fn tray_icon_px() -> (isize, u32) {
    use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
    use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW;
    let class: Vec<u16> = "Shell_TrayWnd\0".encode_utf16().collect();
    // SAFETY: a NUL-ended class name and no title: no window is sent a message.
    let taskbar = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
    let mut dpi = 0;
    if !taskbar.is_null() {
        // SAFETY: reads a window's DPI, as its own awareness of DPI has it (Explorer's: its
        // display's); 0 for a window gone since.
        dpi = unsafe { GetDpiForWindow(taskbar) };
    }
    if dpi == 0 {
        // SAFETY: no arguments; reads the system's DPI.
        dpi = unsafe { GetDpiForSystem() };
    }
    let px = small_icon_px_for_dpi(dpi)
        .unwrap_or_else(|| crate::tray::small_icon_px(dpi))
        .clamp(16, 256);
    (taskbar as isize, px)
}

/// Windows' small icon size at `dpi` (`GetSystemMetricsForDpi(SM_CXSMICON)`), if it says.
pub fn small_icon_px_for_dpi(dpi: u32) -> Option<u32> {
    use windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi;
    use windows_sys::Win32::UI::WindowsAndMessaging::SM_CXSMICON;
    // SAFETY: a plain query.
    let px = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) };
    u32::try_from(px).ok().filter(|&px| px > 0)
}

/// A picture handed to the shell as the notification-area icon ([`tray_icon_picture`]), kept for
/// as long as the shell may draw it, and destroyed when dropped (the next one handed over, or the
/// icon gone).
pub struct TrayPicture(isize);

impl Drop for TrayPicture {
    fn drop(&mut self) {
        // SAFETY: an icon made by CreateIcon, owned here alone.
        unsafe { DestroyIcon(self.0 as _) };
    }
}

/// Draw icon `id` of `hwnd` with `rgba`, a `size`×`size` picture (straight alpha), from the
/// tray's watcher thread: it waits for Explorer as [`tray_icon_ask`] does. The picture to keep
/// while the shell has it, or None when the shell didn't take it (no such icon, or no answer).
pub fn tray_icon_picture(hwnd: isize, id: u32, rgba: &[u8], size: u32) -> Option<TrayPicture> {
    use windows_sys::Win32::UI::Shell::{NIF_ICON, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW};
    use windows_sys::Win32::UI::WindowsAndMessaging::CreateIcon;
    let side = size as usize;
    if side == 0 || rgba.len() != side * side * 4 {
        return None;
    }
    let bgra: Vec<u8> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0], p[3]])
        .collect();
    // The mask, for whatever draws the icon without its alpha: one bit a pixel, set where it is
    // clear, in rows of whole 16-bit words.
    let row = side.div_ceil(16) * 2;
    let mut mask = vec![0u8; row * side];
    for (i, p) in rgba.as_chunks::<4>().0.iter().enumerate() {
        if p[3] == 0 {
            let (y, x) = (i / side, i % side);
            mask[y * row + x / 8] |= 0x80 >> (x % 8);
        }
    }
    // SAFETY: both buffers live for the call and hold what the size says (32 bits a pixel, and
    // the mask's rows); no module: the icon is made from them.
    let icon = unsafe {
        CreateIcon(
            std::ptr::null_mut(),
            size as i32,
            size as i32,
            1,
            32,
            mask.as_ptr(),
            bgra.as_ptr(),
        )
    };
    if icon.is_null() {
        return None;
    }
    let picture = TrayPicture(icon as isize);
    let nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd as HWND,
        uID: id,
        uFlags: NIF_ICON,
        hIcon: icon,
        // SAFETY: a plain C struct; all zero is its empty value.
        ..unsafe { zeroed() }
    };
    // SAFETY: nid lives for the call; the shell only reads it (the icon stays alive in `picture`).
    (unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) } != 0).then_some(picture)
}

/// Whether the window in front (the foreground window, which takes the keyboard) is another
/// program's: a click outside MDM's windows (the desktop, the taskbar, another program) put it
/// there. False while one of ours is in front, and while none is (Windows passing the foreground
/// on). An open menu closes at once on it (chrome.rs, #859).
pub fn foreground_is_another_program() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    // SAFETY: plain queries about a window handle, which may be gone (then pid stays 0); the
    // out pointer is a local.
    unsafe {
        let front = GetForegroundWindow();
        if front.is_null() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(front, &mut pid);
        pid != 0 && pid != GetCurrentProcessId()
    }
}

/// What a borderless window of ours is, for [`popup_window`]: every one is a plain popup
/// ([`popup_style`], #208); they differ in how they take the pointer and how they look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Popup {
    /// A menu (chrome.rs): never activated, so the main window keeps the focus and the
    /// keyboard; Windows 11's small rounded corners and a 1 px border in this colour
    /// (0x00BBGGRR); no window-open animation (menus just appear).
    Menu { border: u32 },
    /// A small floating window (the drop target, droptarget.rs): activated when clicked, as
    /// any window; Windows 11's rounded corners, border and shadow, as it had with its caption
    /// style.
    Floating,
}

/// The subclass of every borderless window of ours ([`popup_proc`]): its id, and its data's
/// flag for a window never activated (a menu).
const POPUP_SUBCLASS: usize = 0x3e1;
const POPUP_NO_ACTIVATE: usize = 1;

/// A borderless window's style: a plain popup, as Win32's own menus are, without the caption,
/// frame and caption buttons winit gives every borderless window (it keeps WS_CAPTION and hides
/// the frame in WM_NCCALCSIZE). With a caption style, whatever lays the window out by its
/// styles rather than by our WM_NCCALCSIZE puts the client a caption lower and a frame to the
/// right: on a Windows 11 Insider build (22635) each menu, and the drop target, showed a black
/// band a caption tall across its top and its picture moved down by as much (#208). Without
/// one, every reading agrees: the client is the whole window.
pub(crate) fn popup_style(style: u32) -> u32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        WS_CAPTION, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU,
    };
    (style & !(WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX))
        | WS_POPUP
}

/// The extended styles a borderless window keeps: no taskbar button or Alt+Tab entry (a tool
/// window, without the app window style winit gives every window, which asks for a button),
/// and a menu is never activated.
fn popup_ex_style(ex: u32, no_activate: bool) -> u32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };
    (ex & !WS_EX_APPWINDOW) | WS_EX_TOOLWINDOW | if no_activate { WS_EX_NOACTIVATE } else { 0 }
}

#[cfg(test)]
mod popup_style_tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AdjustWindowRectEx, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_APPWINDOW, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
    };

    /// winit's borderless window as it creates a menu or the drop target (style 0x16CB0000: a
    /// caption, system menu, minimize and maximize boxes), and the extended styles
    /// popup_window gives a menu and the drop target (always on top, a window edge).
    const WINIT_BORDERLESS: u32 = 0x16CB_0000;
    const MENU_EX: u32 = 0x0804_0198;
    const DROP_TARGET_EX: u32 = 0x0000_0198;

    /// The frame (left, top, right, bottom) the styles alone put round a `w` × `h` client: what
    /// a reader of the styles, rather than of our WM_NCCALCSIZE, takes for it.
    fn frame(style: u32, ex: u32, w: i32, h: i32) -> [i32; 4] {
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        // SAFETY: plain computation on a local rectangle.
        unsafe { AdjustWindowRectEx(&mut rc, style, 0, ex) };
        [-rc.left, -rc.top, rc.right - w, rc.bottom - h]
    }

    #[test]
    fn a_menu_window_is_a_popup_whose_client_is_all_of_it() {
        // As created, the styles alone give it a caption and a frame: what a reader of the
        // styles (rather than of our WM_NCCALCSIZE) took for its client on build 22635 (#208).
        assert_ne!(frame(WINIT_BORDERLESS, MENU_EX, 279, 275), [0, 0, 0, 0]);
        let style = popup_style(WINIT_BORDERLESS);
        assert_eq!(
            style,
            WS_POPUP | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN
        );
        // As a popup, no reading of its styles finds a frame (at any scale: none to scale).
        assert_eq!(frame(style, MENU_EX, 279, 275), [0, 0, 0, 0]);
        // Applied again (winit setting its styles once more), nothing changes.
        assert_eq!(popup_style(style), style);
    }

    #[test]
    fn the_drop_target_is_a_popup_whose_client_is_all_of_it() {
        // The black band left on build 22635 once menus were fixed (#208): a caption about
        // 26 px tall over the 56 px box, its picture moved down by as much.
        let [_, top, _, _] = frame(WINIT_BORDERLESS, 0x0004_0118, 56, 56);
        assert!(top > 20, "a caption's height above its client: {top}");
        let style = popup_style(WINIT_BORDERLESS);
        assert_eq!(
            style,
            WS_POPUP | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN
        );
        // As winit makes it (always on top, a window edge, files accepted, an app window).
        let ex = popup_ex_style(0x0004_0118, false);
        assert_eq!(ex, DROP_TARGET_EX);
        assert_eq!(frame(style, ex, 56, 56), [0, 0, 0, 0]);
        assert_eq!(popup_style(style), style);
    }

    #[test]
    fn a_popup_keeps_its_extended_styles() {
        // winit's own extended styles for it (always on top, a window edge, files accepted,
        // an app window), without ours: they come back, but for the app window's.
        let winit_ex = WS_EX_TOPMOST | 0x0000_0100 | 0x0000_0010 | WS_EX_APPWINDOW;
        let menu = popup_ex_style(winit_ex, true);
        assert_eq!(menu & WS_EX_APPWINDOW, 0);
        assert_eq!(menu & WS_EX_NOACTIVATE, WS_EX_NOACTIVATE);
        assert_eq!(menu & WS_EX_TOOLWINDOW, WS_EX_TOOLWINDOW);
        assert_eq!(menu & WS_EX_TOPMOST, WS_EX_TOPMOST);
        // The drop target is activated as any window (a click on it, a drag), but has no
        // taskbar button either; an off-screen test copy's never-activate stays.
        let floating = popup_ex_style(winit_ex, false);
        assert_eq!(floating & WS_EX_NOACTIVATE, 0);
        assert_eq!(floating & WS_EX_TOOLWINDOW, WS_EX_TOOLWINDOW);
        assert_eq!(floating & WS_EX_TOPMOST, WS_EX_TOPMOST);
        assert_eq!(
            popup_ex_style(winit_ex | WS_EX_NOACTIVATE, false) & WS_EX_NOACTIVATE,
            WS_EX_NOACTIVATE
        );
    }

    /// A real (hidden, off-screen) window with winit's borderless styles, made a popup by
    /// popup_window: plain popup styles, its client the whole window, and both kept when its
    /// styles are set again (winit does on every change of its flags).
    #[test]
    fn popup_window_makes_a_real_window_a_popup_and_keeps_it_one() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, GetClientRect, GetWindowRect, WS_CAPTION,
        };
        for (n, kind) in [
            Popup::Floating,
            Popup::Menu {
                border: 0x00ff_0000,
            },
        ]
        .into_iter()
        .enumerate()
        {
            let title = format!("mdm popup test {n} {}", std::process::id());
            let wide: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            // SAFETY: a hidden window of the system's STATIC class on this thread, destroyed
            // below; the strings are NUL-terminated.
            let hwnd = unsafe {
                CreateWindowExW(
                    0x0000_0108,
                    class.as_ptr(),
                    wide.as_ptr(),
                    WINIT_BORDERLESS & !WS_VISIBLE,
                    -20000,
                    -20000,
                    56,
                    56,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                )
            };
            assert!(!hwnd.is_null());
            assert!(popup_window(&title, kind), "applied once");
            assert!(!popup_window(&title, kind), "only once");
            // SAFETY: our own window; plain queries and style changes, as winit's.
            unsafe {
                let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
                assert_eq!(style & WS_CAPTION, 0);
                assert_ne!(style & WS_POPUP, 0);
                // winit setting its caption style again: the subclass keeps the popup's.
                SetWindowLongPtrW(hwnd, GWL_STYLE, (style | WS_CAPTION) as isize);
                assert_eq!(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, style);
                let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, 0x0000_0108);
                assert_eq!(GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32, ex);
                assert_eq!(
                    ex & WS_EX_NOACTIVATE != 0,
                    matches!(kind, Popup::Menu { .. })
                );
                // Its size as the drop target's (a window with a caption style can't be made
                // that narrow: a popup can), then its client is the whole window.
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    0,
                    0,
                    56,
                    56,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
                let (mut w, mut c): (RECT, RECT) = (zeroed(), zeroed());
                GetWindowRect(hwnd, &mut w);
                GetClientRect(hwnd, &mut c);
                assert_eq!([w.right - w.left, w.bottom - w.top], [56, 56]);
                assert_eq!([c.left, c.top, c.right, c.bottom], [0, 0, 56, 56]);
                DestroyWindow(hwnd);
            }
        }
    }
}

/// Every borderless window of ours: it keeps its popup styles when winit sets the styles again
/// (it does on every change of its window flags, showing the window included), and its client
/// is the whole window, with no frame (winit's borderless frame moves the client a pixel down,
/// for a shadow drawn round a caption style, which a popup has no need of). A menu's (`data`
/// has [`POPUP_NO_ACTIVATE`]) answers mouse-activation with "don't activate": clicking an item
/// must not take focus from the main window (which would close the menu before the click
/// completes).
unsafe extern "system" fn popup_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{STYLESTRUCT, WM_STYLECHANGING};
    const WM_MOUSEACTIVATE: u32 = 0x0021;
    const MA_NOACTIVATE: LRESULT = 3;
    let no_activate = data & POPUP_NO_ACTIVATE != 0;
    if msg == WM_MOUSEACTIVATE && no_activate {
        return MA_NOACTIVATE;
    }
    if msg == WM_STYLECHANGING && lparam != 0 {
        // SAFETY: for WM_STYLECHANGING, lParam points to the STYLESTRUCT of the change, whose
        // styleNew the window may alter (Win32 contract).
        let s = unsafe { &mut *(lparam as *mut STYLESTRUCT) };
        match wparam as i32 {
            GWL_STYLE => s.styleNew = popup_style(s.styleNew),
            GWL_EXSTYLE => s.styleNew = popup_ex_style(s.styleNew, no_activate),
            _ => {}
        }
        return 0;
    }
    if msg == WM_NCCALCSIZE {
        // No non-client area: the proposed window rectangle (rgrc[0], or the RECT itself when
        // wParam is FALSE) stays as the client's.
        return 0;
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// Make our borderless window titled `title` a plain popup window whose client is all of it
/// ([`popup_style`], #208), with no taskbar entry, and the rest of what `kind` says. Every
/// borderless window of ours comes here (chrome.rs's `popup_viewport` and `style_popup`). Call
/// it in the window's own pass, before anything is drawn in it: its frame changes, and a
/// picture drawn before that is lost with the old frame (black until something makes it draw
/// again). Cheap to call every frame: returns true only on the call that applied it; the window
/// is created hidden and should be shown then.
pub fn popup_window(title: &str, kind: Popup) -> bool {
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_TRANSITIONS_FORCEDISABLED, DWMWA_WINDOW_CORNER_PREFERENCE,
        DWMWCP_ROUND, DWMWCP_ROUNDSMALL, DwmSetWindowAttribute,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetPropW, SWP_NOSIZE, SetPropW};
    let Some(hwnd) = find_own_window(title) else {
        return false;
    };
    let no_activate = matches!(kind, Popup::Menu { .. });
    // SAFETY: our own window, on its thread; the subclass proc lives as long as the program;
    // the DWM attributes are read from locals during the calls.
    unsafe {
        // Done once (comctl32 5.8, which MDM links, has no GetWindowSubclass to ask by).
        let done: Vec<u16> = "MDM.popup\0".encode_utf16().collect();
        if !GetPropW(hwnd, done.as_ptr()).is_null() {
            return false;
        }
        SetPropW(hwnd, done.as_ptr(), 1usize as _);
        if let Popup::Menu { .. } = kind {
            let off: i32 = 1;
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_TRANSITIONS_FORCEDISABLED as u32,
                &off as *const i32 as *const c_void,
                size_of::<i32>() as u32,
            );
        }
        // The subclass first: it keeps the styles below when winit sets its own again.
        let data = if no_activate { POPUP_NO_ACTIVATE } else { 0 };
        SetWindowSubclass(hwnd, Some(popup_proc), POPUP_SUBCLASS, data);
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, popup_ex_style(ex, no_activate) as isize);
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        SetWindowLongPtrW(hwnd, GWL_STYLE, popup_style(style) as isize);
        // The frame measured again (WM_NCCALCSIZE): no caption, the client the whole window.
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        // Windows 11 rounds a window with a caption style by itself, not a popup: asked for.
        let corner = match kind {
            Popup::Menu { .. } => DWMWCP_ROUNDSMALL,
            Popup::Floating => DWMWCP_ROUND,
        };
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &corner as *const _ as *const c_void,
            size_of::<i32>() as u32,
        );
        if let Popup::Menu { border } = kind {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_BORDER_COLOR as u32,
                &border as *const u32 as *const c_void,
                size_of::<u32>() as u32,
            );
        }
    }
    true
}

/// Width of `text` in Segoe UI 12 px as GDI measures it (whole-pixel hinted advances) — the
/// width Win32 menus size themselves by. Cached per string.
pub fn menu_text_width(text: &str) -> i32 {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use windows_sys::Win32::Foundation::SIZE;
    use windows_sys::Win32::Graphics::Gdi::{
        CreateFontW, GetTextExtentPoint32W, HFONT, SelectObject,
    };
    thread_local! {
        static FONT: HFONT = {
            let face: Vec<u16> = "Segoe UI\0".encode_utf16().collect();
            // SAFETY: plain GDI object creation; the font lives for the thread.
            unsafe { CreateFontW(-12, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 0, 0, face.as_ptr()) }
        };
        static CACHE: RefCell<HashMap<String, i32>> = RefCell::new(HashMap::new());
    }
    if let Some(w) = CACHE.with(|c| c.borrow().get(text).copied()) {
        return w;
    }
    let wide: Vec<u16> = text.encode_utf16().collect();
    let w = FONT.with(|font| unsafe {
        let dc = GetDC(std::ptr::null_mut());
        let old = SelectObject(dc, *font);
        let mut size = SIZE { cx: 0, cy: 0 };
        GetTextExtentPoint32W(dc, wide.as_ptr(), wide.len() as i32, &mut size);
        SelectObject(dc, old);
        ReleaseDC(std::ptr::null_mut(), dc);
        size.cx
    });
    CACHE.with(|c| c.borrow_mut().insert(text.to_owned(), w));
    w
}

thread_local! {
    /// The caption each of our windows was last given by [`caption_dark`] (dark: true).
    static CAPTION_DARK: std::cell::RefCell<std::collections::HashMap<isize, bool>> =
        Default::default();
}

/// Whether DWM is to be told `dark` for window `hwnd`: not when that is what it was last told.
/// Windows that are gone are forgotten.
fn caption_news(hwnd: isize, dark: bool, alive: impl Fn(isize) -> bool) -> bool {
    CAPTION_DARK.with(|m| {
        let Ok(mut m) = m.try_borrow_mut() else {
            return true;
        };
        if m.get(&hwnd) == Some(&dark) {
            return false;
        }
        m.retain(|h, _| alive(*h));
        m.insert(hwnd, dark);
        true
    })
}

/// How many times [`caption_dark`] has asked DWM, for the tests.
#[cfg(test)]
static CAPTION_DWM_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Give our window titled `title` a dark or light caption (DWM). Returns false if not found.
/// DWM is asked only when the window's caption changes, never again for the same one: a call
/// into DWM waits on a system that is stalled (its disk, its memory), and the window's thread
/// waited with it, every 2 s (#633). Callers call it when the theme changes and for a new window.
pub fn caption_dark(title: &str, dark: bool) -> bool {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
    use windows_sys::Win32::Graphics::Gdi::{RDW_FRAME, RDW_INVALIDATE, RedrawWindow};
    use windows_sys::Win32::UI::WindowsAndMessaging::IsWindow;
    let Some(hwnd) = find_own_window(title) else {
        return false;
    };
    // SAFETY: a plain query; any value is accepted as the handle.
    let alive = |h: isize| unsafe { IsWindow(h as HWND) } != 0;
    if !caption_news(hwnd as isize, dark, alive) {
        return true;
    }
    #[cfg(test)]
    CAPTION_DWM_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let v: i32 = dark.into();
    // SAFETY: documented attribute with a 4-byte BOOL value.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &v as *const i32 as *const c_void,
            size_of::<i32>() as u32,
        );
        // Windows repaints a caption only on the next activation change; force it now.
        RedrawWindow(
            hwnd,
            std::ptr::null(),
            std::ptr::null_mut(),
            RDW_FRAME | RDW_INVALIDATE,
        );
    }
    true
}

/// The popup menus Windows draws for us — the notification-area icon's (tray-icon's), the
/// window menu of a drawn title bar — in MDM's mode, not Windows' (#682): dark on a dark surface
/// (dark mode, a dark theme), light otherwise, whatever Windows' own app mode says. Windows has
/// no documented way: uxtheme's SetPreferredAppMode (ordinal 135, ForceDark 2 / ForceLight 3)
/// sets the process's mode and FlushMenuThemes (ordinal 136) makes menus read it again, as the
/// win32-darkmode project and the apps built on it do. They exist from Windows 10 1903 (build
/// 18362) on; 1809 has another function at 135 and older builds no dark menus, so there menus
/// stay as Windows draws them. Called when the mode changes; asks uxtheme only when it is news.
pub fn menus_dark(dark: bool) {
    use std::sync::atomic::{AtomicU8, Ordering};
    static TOLD: AtomicU8 = AtomicU8::new(u8::MAX);
    let mode: i32 = if dark { 2 } else { 3 };
    if TOLD.swap(mode as u8, Ordering::Relaxed) == mode as u8 {
        return;
    }
    let Some(f) = menu_mode_fns() else {
        return;
    };
    // SAFETY: the functions behind these ordinals on build 18362 and later (menu_mode_fns),
    // SetPreferredAppMode(int) -> int and FlushMenuThemes(void); process-wide, any thread.
    unsafe {
        (f.set_preferred_app_mode)(mode);
        (f.flush_menu_themes)();
    }
}

/// uxtheme's functions that set the mode of the menus Windows draws ([`menus_dark`]).
#[derive(Clone, Copy)]
struct MenuModeFns {
    set_preferred_app_mode: unsafe extern "system" fn(i32) -> i32,
    flush_menu_themes: unsafe extern "system" fn(),
}

/// [`MenuModeFns`], looked up once: None before Windows 10 1903 or if uxtheme lacks them.
fn menu_mode_fns() -> Option<MenuModeFns> {
    use windows_sys::Win32::System::LibraryLoader::{
        GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
    };
    use windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    static FNS: std::sync::OnceLock<Option<MenuModeFns>> = std::sync::OnceLock::new();
    *FNS.get_or_init(|| {
        let build = reg_string_in(
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "CurrentBuild",
        )
        .and_then(|b| b.parse::<u32>().ok())?;
        if build < 18362 {
            return None;
        }
        let name: Vec<u16> = "uxtheme.dll\0".encode_utf16().collect();
        // SAFETY: a NUL-terminated name, loaded from System32 only; the module stays loaded for
        // the process's life (never freed), so the pointers taken from it stay valid. An ordinal
        // goes as the name pointer's low word (MAKEINTRESOURCEA).
        unsafe {
            let module = LoadLibraryExW(
                name.as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            );
            if module.is_null() {
                return None;
            }
            let set = GetProcAddress(module, 135usize as *const u8)?;
            let flush = GetProcAddress(module, 136usize as *const u8)?;
            Some(MenuModeFns {
                set_preferred_app_mode: std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "system" fn(i32) -> i32,
                >(set),
                flush_menu_themes: std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "system" fn(),
                >(flush),
            })
        }
    })
}

#[cfg(test)]
mod menus_dark_tests {
    /// #682: uxtheme has the menu mode functions on every Windows MDM is tested on (10 1903 and
    /// later, Server 2022's runner included), and setting the mode either way returns.
    #[test]
    fn the_menu_mode_functions_are_found_and_answer() {
        assert!(super::menu_mode_fns().is_some());
        super::menus_dark(true);
        super::menus_dark(false);
    }
}

/// A drag out (of Download complete or the list) is under way ([`drag_files_out`]): one at a
/// time, the mouse holds only one.
static DRAGGING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// When the last drag out ended.
static DRAG_ENDED: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// A drag out is under way, or ended a moment ago. Its keys are its own: Escape cancels the
/// drag in OLE's loop, on the drag's thread, while the window (whose thread the keys go to)
/// sees them too, often after the drag already ended, and must not take Escape as Close.
pub fn dragging_out() -> bool {
    use std::sync::atomic::Ordering;
    DRAGGING.load(Ordering::Acquire)
        || DRAG_ENDED
            .lock()
            .ok()
            .and_then(|t| *t)
            .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(500))
}

/// Start dragging `files` out of our window (to Explorer, a mail, a chat, a player…): from
/// Download complete's grip, as IDM's, or from the download list's rows (#1210). The drag is the
/// shell's own for the files, as Explorer's is (its preview, its formats). The window is the one
/// holding the mouse (the button is down on it: winit captured the mouse for it), so it is the
/// right one when several show the same title; else our window titled `title`, given in the
/// interface language, as it shows. The files are looked for on the drag's thread (#743): those
/// that aren't there are left out, and with none there is no drag.
///
/// The drop target decides what the drag does (#869): MDM offers a move, a copy and a link, so
/// Explorer applies its own rules, as for files dragged in Explorer: moved on the same drive or
/// with Shift, copied to another drive or with Ctrl, a shortcut with Alt. A move is followed:
/// `followed` is told, for each file moved (as it was given), where it went (from the shell's
/// change notifications), or that it went where MDM can't learn; a copy, a link or a cancelled
/// drag tell nothing. It may come long after the drop (a move to another drive copies first),
/// from the drag's thread. MDM never deletes a file itself, even for a target that asks the
/// source to (a move it doesn't optimize): the file stays, so nothing is lost.
///
/// The drag runs on a thread of its own (#561): OLE's drag loop lasts as long as the button is
/// held, and each step calls the program under the pointer (its drop target, in its own process),
/// so on the window thread a busy or hung target, or a target that copies the file before it
/// answers the drop, froze every window until it answered. That thread shares the window
/// thread's input ([`with_shared_input`]): OLE reads the held button and takes the mouse from
/// there, as it did from the window thread.
pub fn drag_files_out(
    title: &str,
    files: Vec<std::path::PathBuf>,
    followed: impl FnMut(std::path::PathBuf, crate::dialogs::DraggedOut) + Send + 'static,
) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    let hwnd = own_capture()
        .or_else(|| find_own_window(title))
        .ok_or("window not found")? as isize;
    if DRAGGING.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let started = with_shared_input("drag out", move |shared| {
        if !shared {
            tracing::warn!("drag out: the window's input isn't shared");
        }
        // Looked for here, beside the window (#743): a drive slow to answer doesn't hold it.
        let there: Vec<_> = files.into_iter().filter(|f| f.exists()).collect();
        let result = if there.is_empty() {
            Ok(None)
        } else {
            dragout::drag(hwnd as HWND, &there)
        };
        if let Ok(mut t) = DRAG_ENDED.lock() {
            *t = Some(std::time::Instant::now());
        }
        DRAGGING.store(false, Ordering::Release);
        // OLE's drag loop took the button's release, so the window never saw it: without it
        // the next press on the grip only ended the drag the window still thought was on, and
        // the grip worked every other time.
        release_button(hwnd as HWND);
        let dropped = result.unwrap_or_else(|e| {
            tracing::warn!("drag out: {e}");
            None
        });
        // Once the window's input is given back: what became of the files.
        move || {
            if let Some(dropped) = dropped {
                dropped.follow(followed);
            }
        }
    });
    if started.is_err() {
        DRAGGING.store(false, Ordering::Release);
    }
    started
}

/// A drag out of ours is over our own windows' drop target now: it refuses it (#1210), so a
/// download's file dropped back on MDM isn't taken for a list of links or a torrent to add.
fn own_drag_over_us() -> bool {
    DRAGGING.load(std::sync::atomic::Ordering::Acquire)
}

/// Tell `hwnd` the left button is up, at the pointer, as Windows would have done had OLE's
/// drag loop not taken the release. A move first: the pointer left the window during the drag,
/// and a release with no pointer position is dropped before it reaches the interface.
fn release_button(hwnd: HWND) {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    // The left button's flag in a mouse message's wParam (MK_LBUTTON, winuser.h).
    const MK_LBUTTON: usize = 0x0001;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, PostMessageW, WM_LBUTTONUP, WM_MOUSEMOVE,
    };
    let mut pt = POINT { x: 0, y: 0 };
    // SAFETY: reads into a local; `hwnd` is our own window (a closed one fails harmlessly).
    unsafe {
        GetCursorPos(&mut pt);
        ScreenToClient(hwnd, &mut pt);
    }
    let lparam = (((pt.y as u16 as u32) << 16) | pt.x as u16 as u32) as isize;
    // SAFETY: messages posted to our own window.
    unsafe {
        PostMessageW(hwnd, WM_MOUSEMOVE, MK_LBUTTON, lparam);
        PostMessageW(hwnd, WM_LBUTTONUP, 0, lparam);
    }
}

/// Run `work` on a new thread named `name`, with OLE (a single-threaded apartment) and the
/// calling thread's input state: the keys and mouse buttons held, the mouse capture and the
/// focus (AttachThreadInput), as a modal loop like OLE's drag needs when the button went down in
/// the caller's window. `work` is told whether the input is shared. Returns at once; the input
/// is detached when `work` ends, then what `work` returned runs (still with OLE: the drag's
/// follow-up), and OLE is closed. The caller has a message queue (a window thread).
fn with_shared_input<A: FnOnce()>(
    name: &str,
    work: impl FnOnce(bool) -> A + Send + 'static,
) -> Result<(), String> {
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::WindowsAndMessaging::{MSG, PM_NOREMOVE, PeekMessageW};
    #[link(name = "ole32")]
    unsafe extern "system" {
        fn OleInitialize(reserved: *mut c_void) -> i32;
        fn OleUninitialize();
    }
    // SAFETY: a plain query.
    let caller = unsafe { GetCurrentThreadId() };
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            // SAFETY: OLE for this new thread, closed below on the same thread when it took.
            let ole = unsafe { OleInitialize(std::ptr::null_mut()) } >= 0;
            // A thread is given its message queue by its first window call, and only threads
            // with one can share input: a look at the (empty) queue makes it.
            // SAFETY: a read into a local MSG; nothing is removed.
            let mut msg: MSG = unsafe { zeroed() };
            unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE) };
            // SAFETY: plain calls with two live threads of this process; undone below.
            let me = unsafe { GetCurrentThreadId() };
            let attached = unsafe { AttachThreadInput(me, caller, 1) } != 0;
            let after = work(attached);
            if attached {
                // SAFETY: as above.
                unsafe { AttachThreadInput(me, caller, 0) };
            }
            after();
            if ole {
                // SAFETY: matches the OleInitialize above, on this thread.
                unsafe { OleUninitialize() };
            }
        })
        .map(drop)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod drag_thread_tests {
    use super::*;

    /// The drag's thread (#561): another thread than the window's, which goes on at once, in a
    /// single-threaded apartment (OLE's drag needs one), sharing the window thread's input.
    #[test]
    fn the_drag_runs_beside_the_window_thread_with_its_input() {
        use windows_sys::Win32::System::Com::{APTTYPE, APTTYPEQUALIFIER, CoGetApartmentType};
        use windows_sys::Win32::System::Threading::GetCurrentThreadId;
        use windows_sys::Win32::UI::WindowsAndMessaging::{MSG, PM_NOREMOVE, PeekMessageW};
        // This test's thread stands for the window thread: it has a message queue.
        // SAFETY: a read into a local MSG; nothing is removed.
        let mut msg: MSG = unsafe { zeroed() };
        unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE) };
        // SAFETY: a plain query.
        let window_thread = unsafe { GetCurrentThreadId() };
        let (tx, rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        with_shared_input("drag test", move |shared| {
            // Held until the caller has gone on: the work doesn't hold the caller up.
            go_rx.recv().unwrap();
            let mut kind: APTTYPE = 0;
            let mut qualifier: APTTYPEQUALIFIER = 0;
            // SAFETY: reads into two locals.
            let hr = unsafe { CoGetApartmentType(&mut kind, &mut qualifier) };
            // SAFETY: a plain query.
            let me = unsafe { GetCurrentThreadId() };
            tx.send((me, shared, hr, kind)).unwrap();
            || {}
        })
        .unwrap();
        go_tx.send(()).unwrap();
        let (me, shared, hr, kind) = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert_ne!(me, window_thread);
        assert!(shared, "the input is shared with the window thread");
        assert_eq!(hr, 0);
        // APTTYPE_STA (0); the first apartment of the process would be APTTYPE_MAINSTA (3).
        assert!(
            kind == 0 || kind == 3,
            "a single-threaded apartment, not {kind}"
        );
    }
}

/// Dragging downloaded files out of Download complete (#869) or the list (#1210): the shell's own
/// data object for them (Explorer's drag image, its formats), offered for a move, a copy or a
/// link so the drop target applies Explorer's rules, and a move followed through the shell's
/// change notifications, so each download can point where its file went.
mod dragout {
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::mem::zeroed;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, HWND_MESSAGE, MSG,
        MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT, RegisterClassW,
        TranslateMessage, WM_APP, WNDCLASSW,
    };

    use crate::dialogs::DraggedOut;

    type HResult = i32;
    const DROPEFFECT_COPY: u32 = 1;
    const DROPEFFECT_MOVE: u32 = 2;
    const DROPEFFECT_LINK: u32 = 4;
    /// SHDoDragDrop's (DoDragDrop's) answer when the file was dropped, not cancelled.
    const DRAGDROP_S_DROP: HResult = 0x0004_0100;
    /// SHChangeNotifyRegister: the shell's own notifications (what Explorer and every program
    /// using the shell's file operations do), not the disks' (which would watch every drive),
    /// delivered as a handle to lock.
    const SHCNRF_SHELL_LEVEL: i32 = 0x0002;
    const SHCNRF_NEW_DELIVERY: i32 = 0x8000;
    const SHCNE_RENAMEITEM: i32 = 0x0000_0001;
    const SHCNE_CREATE: i32 = 0x0000_0002;
    const SHCNE_DELETE: i32 = 0x0000_0004;
    const SHCNE_MKDIR: i32 = 0x0000_0008;
    const SHCNE_RMDIR: i32 = 0x0000_0010;
    const SHCNE_RENAMEFOLDER: i32 = 0x0002_0000;
    /// The notification's message to our watching window.
    const WM_SHELL_CHANGED: u32 = WM_APP + 0x69;
    /// The formats a drop target sets on the data object to say what it did (Shell Clipboard
    /// Formats): an optimized move (Explorer's, on any drive) answers "none" to the drag, so
    /// the source doesn't delete the file the target already moved, and says "move" here.
    const PERFORMED: &str = "Performed DropEffect";
    const LOGICAL_PERFORMED: &str = "Logical Performed DropEffect";
    /// How long a dropped file is watched while it is still there: a target that said it moves
    /// it may be copying it to another drive first; one that said nothing may do so on its own
    /// thread.
    const MOVING_FOR: Duration = Duration::from_secs(30 * 60);
    const DROPPED_FOR: Duration = Duration::from_secs(60);
    /// How long the shell's word on where it went is waited for once the file is gone.
    const GONE_FOR: Duration = Duration::from_secs(3);

    #[repr(C)]
    struct Guid(u32, u16, u16, [u8; 8]);
    const IID_IDATAOBJECT: Guid = Guid(0x0000_010e, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);

    #[repr(C)]
    struct NotifyEntry {
        pidl: *const c_void,
        recursive: i32,
    }

    #[repr(C)]
    struct FormatEtc {
        cf: u16,
        ptd: *mut c_void,
        aspect: u32,
        lindex: i32,
        tymed: u32,
    }

    #[repr(C)]
    struct StgMedium {
        tymed: u32,
        handle: *mut c_void,
        unk_for_release: *mut c_void,
    }

    /// IDataObject's vtable up to GetData (index 3), all we call.
    #[repr(C)]
    struct DataObjectVtbl {
        _query_interface: usize,
        _add_ref: usize,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        get_data:
            unsafe extern "system" fn(*mut c_void, *const FormatEtc, *mut StgMedium) -> HResult,
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ILCreateFromPathW(path: *const u16) -> *mut c_void;
        fn ILFree(pidl: *mut c_void);
        fn ILClone(pidl: *const c_void) -> *mut c_void;
        fn ILRemoveLastID(pidl: *mut c_void) -> i32;
        fn ILFindLastID(pidl: *const c_void) -> *mut c_void;
        fn ILIsParent(parent: *const c_void, child: *const c_void, immediate: i32) -> i32;
        fn SHCreateDataObject(
            folder: *const c_void,
            count: u32,
            items: *const *const c_void,
            inner: *mut c_void,
            riid: *const Guid,
            out: *mut *mut c_void,
        ) -> HResult;
        fn SHDoDragDrop(
            hwnd: HWND,
            data: *mut c_void,
            source: *mut c_void,
            effects: u32,
            effect: *mut u32,
        ) -> HResult;
        fn SHGetSpecialFolderLocation(hwnd: HWND, csidl: i32, pidl: *mut *mut c_void) -> HResult;
        fn SHChangeNotifyRegister(
            hwnd: HWND,
            sources: i32,
            events: i32,
            msg: u32,
            count: i32,
            entries: *const NotifyEntry,
        ) -> u32;
        fn SHChangeNotifyDeregister(id: u32) -> i32;
        fn SHChangeNotification_Lock(
            change: *mut c_void,
            pid: u32,
            pidls: *mut *mut *mut c_void,
            event: *mut i32,
        ) -> *mut c_void;
        fn SHChangeNotification_Unlock(lock: *mut c_void) -> i32;
        fn SHGetPathFromIDListEx(pidl: *const c_void, path: *mut u16, len: u32, opts: i32) -> i32;
    }
    #[link(name = "ole32")]
    unsafe extern "system" {
        fn ReleaseStgMedium(medium: *mut StgMedium);
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalLock(h: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(h: *mut c_void) -> i32;
        fn GlobalSize(h: *mut c_void) -> usize;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn RegisterClipboardFormatW(name: *const u16) -> u32;
    }

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        s.encode_wide().chain(Some(0)).collect()
    }

    /// What the shell said happened to a file or folder, as paths.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(super) enum Seen {
        Renamed(PathBuf, PathBuf),
        Created(PathBuf),
        Deleted(PathBuf),
    }

    thread_local! {
        /// What this drag's watch saw (one drag per thread).
        static SEEN: RefCell<Vec<Seen>> = const { RefCell::new(Vec::new()) };
    }

    /// Two paths name the same file: Windows' names don't tell case apart.
    fn same(a: &Path, b: &Path) -> bool {
        let norm = |p: &Path| {
            p.to_string_lossy()
                .replace('/', "\\")
                .trim_end_matches('\\')
                .to_lowercase()
        };
        norm(a) == norm(b)
    }

    /// `path` without the `\\?\` that canonicalize puts before it (the shell's paths have none).
    pub(super) fn plain(path: PathBuf) -> PathBuf {
        let s = path.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            PathBuf::from(format!(r"\\{rest}"))
        } else if let Some(rest) = s.strip_prefix(r"\\?\") {
            PathBuf::from(rest)
        } else {
            path
        }
    }

    /// In a drive's Recycle Bin (`$Recycle.Bin`): thrown away, not put somewhere to open.
    pub(super) fn in_recycle_bin(path: &Path) -> bool {
        path.components().any(|c| {
            c.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case("$Recycle.Bin")
        })
    }

    /// Where `original` went, from what the shell saw: renamed or moved on its drive (one
    /// notification with both paths, followed as often as it moved), or, gone from its place,
    /// the one file of its name made elsewhere meanwhile (a move to another drive copies then
    /// deletes). None while nothing says, or when two places could be it.
    pub(super) fn followed(original: &Path, seen: &[Seen]) -> Option<PathBuf> {
        let mut at = original.to_path_buf();
        let (mut renamed, mut deleted) = (false, false);
        for s in seen {
            match s {
                Seen::Renamed(from, to) if same(from, &at) => {
                    at = to.clone();
                    renamed = true;
                }
                Seen::Deleted(p) if same(p, original) => deleted = true,
                _ => {}
            }
        }
        if renamed {
            return Some(at);
        }
        if !deleted {
            return None;
        }
        let name = original.file_name()?.to_string_lossy().to_lowercase();
        let mut made = seen.iter().filter_map(|s| match s {
            Seen::Created(p)
                if !same(p, original)
                    && p.file_name()
                        .is_some_and(|n| n.to_string_lossy().to_lowercase() == name) =>
            {
                Some(p)
            }
            _ => None,
        });
        let first = made.next()?;
        if made.any(|p| !same(p, first)) {
            return None;
        }
        Some(first.clone())
    }

    /// The shell's path of an item, if it is one on a disk.
    fn path_of(pidl: *const c_void) -> Option<PathBuf> {
        if pidl.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 32_768];
        // SAFETY: `pidl` is the shell's, valid while its notification is locked; `buf` is as
        // long as said.
        let ok = unsafe { SHGetPathFromIDListEx(pidl, buf.as_mut_ptr(), buf.len() as u32, 0) };
        let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
        (ok != 0 && len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len])))
    }

    /// The watching window's procedure: each notification, as paths, into [`SEEN`].
    unsafe extern "system" fn watch_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg != WM_SHELL_CHANGED {
            // SAFETY: the system's default for every other message.
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }
        let mut pidls: *mut *mut c_void = std::ptr::null_mut();
        let mut event = 0i32;
        // SAFETY: a new-delivery notification: wParam its handle, lParam the process to lock
        // it for; the two items read before it is unlocked.
        unsafe {
            let lock = SHChangeNotification_Lock(
                wparam as *mut c_void,
                lparam as u32,
                &mut pidls,
                &mut event,
            );
            if lock.is_null() || pidls.is_null() {
                return 0;
            }
            let item = |i: usize| path_of(*pidls.add(i));
            // The low bits: the event (the high one says it came from the disk).
            let seen = match event & 0x7FFF_FFFF {
                SHCNE_RENAMEITEM | SHCNE_RENAMEFOLDER => {
                    item(0).zip(item(1)).map(|(a, b)| Seen::Renamed(a, b))
                }
                SHCNE_CREATE | SHCNE_MKDIR => item(0).map(Seen::Created),
                SHCNE_DELETE | SHCNE_RMDIR => item(0).map(Seen::Deleted),
                _ => None,
            };
            SHChangeNotification_Unlock(lock);
            if let Some(s) = seen {
                tracing::debug!("drag out: the shell saw {s:?}");
                SEEN.with(|v| v.borrow_mut().push(s));
            }
        }
        0
    }

    /// A hidden window of the drag's thread, told by the shell of every file renamed, made or
    /// deleted through it (the whole namespace: a move goes anywhere).
    struct Watch {
        hwnd: HWND,
        id: u32,
    }

    impl Watch {
        fn start() -> Option<Watch> {
            static CLASS: std::sync::Once = std::sync::Once::new();
            let class: Vec<u16> = "mdm-drag-out-watch\0".encode_utf16().collect();
            // SAFETY: a class with our procedure, registered once; a message-only window of
            // this thread, destroyed in Drop (or below when the shell won't watch); the desktop's
            // item list freed once registered (the shell keeps its own copy).
            unsafe {
                let module = GetModuleHandleW(std::ptr::null());
                CLASS.call_once(|| {
                    let mut wc: WNDCLASSW = zeroed();
                    wc.lpfnWndProc = Some(watch_proc);
                    wc.hInstance = module;
                    wc.lpszClassName = class.as_ptr();
                    RegisterClassW(&wc);
                });
                let hwnd = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    std::ptr::null(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    HWND_MESSAGE,
                    std::ptr::null_mut(),
                    module,
                    std::ptr::null(),
                );
                if hwnd.is_null() {
                    return None;
                }
                let mut desktop = std::ptr::null_mut();
                // CSIDL_DESKTOP (0): the namespace's root.
                if SHGetSpecialFolderLocation(std::ptr::null_mut(), 0, &mut desktop) < 0 {
                    DestroyWindow(hwnd);
                    return None;
                }
                let entry = NotifyEntry {
                    pidl: desktop,
                    recursive: 1,
                };
                let id = SHChangeNotifyRegister(
                    hwnd,
                    SHCNRF_SHELL_LEVEL | SHCNRF_NEW_DELIVERY,
                    SHCNE_RENAMEITEM
                        | SHCNE_RENAMEFOLDER
                        | SHCNE_CREATE
                        | SHCNE_MKDIR
                        | SHCNE_DELETE
                        | SHCNE_RMDIR,
                    WM_SHELL_CHANGED,
                    1,
                    &entry,
                );
                ILFree(desktop);
                if id == 0 {
                    DestroyWindow(hwnd);
                    return None;
                }
                SEEN.with(|v| v.borrow_mut().clear());
                Some(Watch { hwnd, id })
            }
        }

        /// Wait up to `wait` for messages, and hand every one of this thread's to its window.
        fn pump(&self, wait: Duration) {
            // SAFETY: waits on no handle, then the thread's own message loop.
            unsafe {
                MsgWaitForMultipleObjects(
                    0,
                    std::ptr::null(),
                    0,
                    wait.as_millis() as u32,
                    QS_ALLINPUT,
                );
                let mut msg: MSG = zeroed();
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }

    impl Drop for Watch {
        fn drop(&mut self) {
            // SAFETY: what `start` registered and made, on the same thread.
            unsafe {
                SHChangeNotifyDeregister(self.id);
                DestroyWindow(self.hwnd);
            }
            SEEN.with(|v| v.borrow_mut().clear());
        }
    }

    /// The shell's data object for files: what Explorer itself drags (the files, their items,
    /// the drag image), and where a drop target says what it did.
    struct DataObject(*mut c_void);

    impl DataObject {
        fn of(paths: &[PathBuf]) -> Option<DataObject> {
            let free = |pidls: Vec<*mut c_void>| {
                // SAFETY: item lists made by ILCreateFromPathW below, freed once.
                pidls.into_iter().for_each(|p| unsafe { ILFree(p) })
            };
            // SAFETY: the item lists made from NUL-terminated paths (and their folder's) are
            // freed once the data object has its own copies; `out` receives an IDataObject we
            // release in Drop.
            unsafe {
                let mut pidls = Vec::with_capacity(paths.len());
                for p in paths {
                    let w = wide(p.as_os_str());
                    let pidl = ILCreateFromPathW(w.as_ptr());
                    if pidl.is_null() {
                        free(pidls);
                        return None;
                    }
                    pidls.push(pidl);
                }
                let folder = ILClone(*pidls.first()?);
                if folder.is_null() {
                    free(pidls);
                    return None;
                }
                ILRemoveLastID(folder);
                // Files of one folder, as Explorer's own drags give them: the folder, and each
                // file within it. Files of several folders (the list shows every category's):
                // the desktop, the namespace's root, and each file's whole path from there.
                let one_folder = pidls.iter().all(|p| ILIsParent(folder, *p, 1) != 0);
                let (root, items): (*mut c_void, Vec<*const c_void>) = if one_folder {
                    let items = pidls.iter().map(|p| ILFindLastID(*p) as *const c_void);
                    (folder, items.collect())
                } else {
                    ILFree(folder);
                    let mut desktop = std::ptr::null_mut();
                    // CSIDL_DESKTOP (0).
                    if SHGetSpecialFolderLocation(std::ptr::null_mut(), 0, &mut desktop) < 0 {
                        free(pidls);
                        return None;
                    }
                    let items = pidls.iter().map(|p| *p as *const c_void);
                    (desktop, items.collect())
                };
                let mut out = std::ptr::null_mut();
                let hr = SHCreateDataObject(
                    root,
                    items.len() as u32,
                    items.as_ptr(),
                    std::ptr::null_mut(),
                    &IID_IDATAOBJECT,
                    &mut out,
                );
                ILFree(root);
                free(pidls);
                (hr >= 0 && !out.is_null()).then_some(DataObject(out))
            }
        }

        /// A drop effect the drop target set on it, under the format `name`.
        fn effect(&self, name: &str) -> Option<u32> {
            let w = wide(std::ffi::OsStr::new(name));
            // SAFETY: GetData through the object's vtable with a FORMATETC for an HGLOBAL; the
            // medium is read under its lock and released.
            unsafe {
                let cf = RegisterClipboardFormatW(w.as_ptr());
                if cf == 0 {
                    return None;
                }
                let fmt = FormatEtc {
                    cf: cf as u16,
                    ptd: std::ptr::null_mut(),
                    aspect: 1,
                    lindex: -1,
                    tymed: 1,
                };
                let mut medium: StgMedium = zeroed();
                let vtbl = *(self.0 as *const *const DataObjectVtbl);
                if ((*vtbl).get_data)(self.0, &fmt, &mut medium) < 0 {
                    return None;
                }
                let mut value = None;
                if medium.tymed == 1 && GlobalSize(medium.handle) >= 4 {
                    let p = GlobalLock(medium.handle) as *const u32;
                    if !p.is_null() {
                        value = Some(p.read_unaligned());
                        GlobalUnlock(medium.handle);
                    }
                }
                ReleaseStgMedium(&mut medium);
                value
            }
        }

        fn release(&mut self) {
            if !self.0.is_null() {
                // SAFETY: our one reference, released once.
                unsafe {
                    let vtbl = *(self.0 as *const *const DataObjectVtbl);
                    ((*vtbl).release)(self.0);
                }
                self.0 = std::ptr::null_mut();
            }
        }
    }

    impl Drop for DataObject {
        fn drop(&mut self) {
            self.release();
        }
    }

    /// Files dropped somewhere: what it took to learn what became of them.
    pub(super) struct Dropped {
        /// Each file as it was given, and as the shell names it.
        files: Vec<(PathBuf, PathBuf)>,
        data: DataObject,
        watch: Option<Watch>,
        /// What the drag returned.
        effect: u32,
    }

    /// Drag `files` out of `hwnd` (on the drag's thread, OLE set up), offering a move, a copy and
    /// a link. Some when they were dropped.
    pub(super) fn drag(hwnd: HWND, files: &[PathBuf]) -> Result<Option<Dropped>, String> {
        let files = files
            .iter()
            .map(|f| Ok((f.clone(), plain(std::fs::canonicalize(f)?))))
            .collect::<Result<Vec<_>, std::io::Error>>()
            .map_err(|e| e.to_string())?;
        // Watched from before the drop: a move on the same drive is done before the drag ends.
        // MDM_TEST_NO_SHELL_WATCH=1 (debug builds, for checks): not watched, as when the shell
        // won't say, so a move is one MDM can't follow.
        let unwatched = cfg!(debug_assertions)
            && std::env::var_os("MDM_TEST_NO_SHELL_WATCH").is_some_and(|v| v == "1");
        let watch = if unwatched { None } else { Watch::start() };
        if watch.is_none() {
            tracing::warn!("drag out: the shell won't say where files go: a move isn't followed");
        }
        let named: Vec<PathBuf> = files.iter().map(|(_, f)| f.clone()).collect();
        let data = DataObject::of(&named).ok_or("the shell has no data object for the files")?;
        let mut effect = 0u32;
        // SAFETY: our window and data object; the shell makes the drop source and drag image,
        // and runs OLE's drag loop on this thread until the drop.
        let hr = unsafe {
            SHDoDragDrop(
                hwnd,
                data.0,
                std::ptr::null_mut(),
                DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK,
                &mut effect,
            )
        };
        if hr < 0 {
            return Err(format!("SHDoDragDrop failed, {hr:#010x}"));
        }
        if hr != DRAGDROP_S_DROP {
            return Ok(None);
        }
        tracing::info!(
            "drag out: {} dropped, effect {effect}, performed {:?}, logical {:?}",
            files.len(),
            data.effect(PERFORMED),
            data.effect(LOGICAL_PERFORMED)
        );
        Ok(Some(Dropped {
            files,
            data,
            watch,
            effect,
        }))
    }

    impl Dropped {
        /// What became of each file: `told` the ones moved (as they were given: where to, or
        /// that MDM can't learn where), nothing of those that stayed (a copy, a link, a drop
        /// that did nothing). Waits as long as a move may take, on the drag's thread.
        pub(super) fn follow(self, mut told: impl FnMut(PathBuf, DraggedOut)) {
            let since = Instant::now();
            // The files not settled yet, and since when each is gone from its place.
            let mut open: Vec<(PathBuf, PathBuf, Option<Instant>)> = self
                .files
                .iter()
                .map(|(given, file)| (given.clone(), file.clone(), None))
                .collect();
            while !open.is_empty() {
                match &self.watch {
                    Some(w) => w.pump(Duration::from_millis(250)),
                    None => std::thread::sleep(Duration::from_millis(250)),
                }
                let said = [
                    Some(self.effect),
                    self.data.effect(PERFORMED),
                    self.data.effect(LOGICAL_PERFORMED),
                ];
                let moving = said.iter().flatten().any(|e| e & DROPEFFECT_MOVE != 0);
                // A copy or a link, said so: the files stay.
                let kept = !moving
                    && said
                        .iter()
                        .flatten()
                        .any(|e| e & (DROPEFFECT_COPY | DROPEFFECT_LINK) != 0);
                if kept {
                    return;
                }
                let seen = SEEN.with(|v| v.borrow().clone());
                let waited = since.elapsed() > if moving { MOVING_FOR } else { DROPPED_FOR };
                open.retain_mut(|(given, file, gone)| {
                    if let Some(to) = followed(file, &seen) {
                        tracing::info!("drag out: a file was moved");
                        let out = if in_recycle_bin(&to) {
                            DraggedOut::MovedAway
                        } else {
                            DraggedOut::Moved(to)
                        };
                        told(given.clone(), out);
                        false
                    } else if std::fs::symlink_metadata(&*file).is_ok() {
                        *gone = None;
                        !waited
                    } else if gone.get_or_insert_with(Instant::now).elapsed() > GONE_FOR {
                        tracing::info!("drag out: a file was moved where the shell didn't say");
                        told(given.clone(), DraggedOut::MovedAway);
                        false
                    } else {
                        true
                    }
                });
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn p(s: &str) -> PathBuf {
            PathBuf::from(s)
        }

        /// A move on the same drive is one notification with both paths, followed however
        /// often the file moved; a move to another drive is a copy made there and the file
        /// deleted here; the Recycle Bin is no place to open.
        #[test]
        fn a_moved_file_is_followed_by_what_the_shell_saw() {
            let file = p(r"C:\Downloads\video.mp4");
            let moved = Seen::Renamed(file.clone(), p(r"C:\Movies\video.mp4"));
            assert_eq!(
                followed(&file, std::slice::from_ref(&moved)),
                Some(p(r"C:\Movies\video.mp4"))
            );
            // Case and separators don't matter (Windows' names).
            let other_case = Seen::Renamed(p(r"c:\downloads\VIDEO.mp4"), p(r"C:\Movies\video.mp4"));
            assert!(followed(&file, &[other_case]).is_some());
            // Moved again: its last place; a name taken there: the numbered one.
            let again = Seen::Renamed(p(r"C:\Movies\video.mp4"), p(r"C:\Old\video (2).mp4"));
            assert_eq!(
                followed(&file, &[moved.clone(), again]),
                Some(p(r"C:\Old\video (2).mp4"))
            );
            // Another drive: made there, then deleted here.
            let made = Seen::Created(p(r"E:\Backup\video.mp4"));
            let deleted = Seen::Deleted(file.clone());
            assert_eq!(
                followed(&file, std::slice::from_ref(&made)),
                None,
                "still here"
            );
            assert_eq!(
                followed(&file, &[made.clone(), deleted.clone()]),
                Some(p(r"E:\Backup\video.mp4"))
            );
            // Two files of its name made meanwhile: can't tell which.
            let made2 = Seen::Created(p(r"F:\video.mp4"));
            assert_eq!(followed(&file, &[made, made2, deleted.clone()]), None);
            // Deleted, nothing of its name made: not known.
            assert_eq!(followed(&file, &[deleted]), None);
            // Other files' changes say nothing.
            let others = [
                Seen::Renamed(p(r"C:\Downloads\a.zip"), p(r"C:\b.zip")),
                Seen::Deleted(p(r"C:\Downloads\a.zip")),
            ];
            assert_eq!(followed(&file, &others), None);
            assert!(in_recycle_bin(&p(
                r"C:\$Recycle.Bin\S-1-5-21-1\$RAB12CD.mp4"
            )));
            assert!(!in_recycle_bin(&p(r"C:\Movies\video.mp4")));
        }

        #[test]
        fn canonical_paths_as_the_shell_writes_them() {
            assert_eq!(
                plain(p(r"\\?\C:\Downloads\a.zip")),
                p(r"C:\Downloads\a.zip")
            );
            assert_eq!(
                plain(p(r"\\?\UNC\server\share\a.zip")),
                p(r"\\server\share\a.zip")
            );
            assert_eq!(plain(p(r"C:\a.zip")), p(r"C:\a.zip"));
        }
    }
}

/// The key of Windows' "Choose your app mode" (`AppsUseLightTheme`), under HKEY_CURRENT_USER.
/// For tests, `MDM_PERSONALIZE_KEY` names another key to read it from (and to watch), so a check
/// can switch the mode MDM sees without touching the system's.
fn personalize_key() -> String {
    std::env::var("MDM_PERSONALIZE_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .unwrap_or_else(|| r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize".into())
}

/// Windows' "Choose your app mode": true when apps should be dark (`AppsUseLightTheme` = 0).
/// Missing value (older Windows): light. A registry read, which can wait for seconds (#719): for
/// a thread other than the window's (`lookwatch`).
pub fn apps_use_dark() -> bool {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let key: Vec<u16> = personalize_key().encode_utf16().chain([0]).collect();
    let name: Vec<u16> = "AppsUseLightTheme\0".encode_utf16().collect();
    let mut value: u32 = 1;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: plain registry read into a u32 of the stated size.
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut value as *mut u32 as *mut c_void,
            &mut size,
        )
    };
    rc == 0 && value == 0
}

/// Windows' accent colour and its shades, from the personalisation settings (`AccentPalette`:
/// 8 colours of 4 bytes, R G B and an unused byte, lightest first: Light3, Light2, Light1, the
/// accent, Dark1, Dark2, Dark3, and one more). None when Windows has none (older versions).
pub fn accent_palette() -> Option<[[u8; 3]; 7]> {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RegGetValueW,
    };
    let key: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent\0"
        .encode_utf16()
        .collect();
    let name: Vec<u16> = "AccentPalette\0".encode_utf16().collect();
    let mut bytes = [0u8; 32];
    let mut size = bytes.len() as u32;
    // SAFETY: a registry read into a 32-byte buffer of the stated size.
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_BINARY,
            std::ptr::null_mut(),
            bytes.as_mut_ptr() as *mut c_void,
            &mut size,
        )
    };
    (rc == 0 && size >= 28).then(|| crate::native::accent_palette_bytes(&bytes))
}

/// Waits for a change to what the theme follows in the registry (#719): the app mode
/// ([`personalize_key`]), the accent colour, high contrast and the system colours, each key
/// watched with `RegNotifyChangeKeyValue`. Opened, armed and waited on by `lookwatch`'s thread,
/// never the window's: those calls can wait as long as a read.
pub struct LookKeys {
    /// The keys open for watching and the event each signals (HKEYs and HANDLEs, kept as numbers
    /// so the watcher can be moved to its thread).
    keys: Vec<(isize, isize)>,
    /// Their notifications are registered (each fires once, and is registered again then).
    armed: bool,
}

impl LookKeys {
    /// The keys that exist (the accent's is missing on older Windows).
    pub fn open() -> Self {
        use windows_sys::Win32::System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_NOTIFY, RegCloseKey, RegOpenKeyExW,
        };
        use windows_sys::Win32::System::Threading::CreateEventW;
        let paths = [
            personalize_key(),
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent".into(),
            r"Control Panel\Accessibility\HighContrast".into(),
            r"Control Panel\Colors".into(),
        ];
        let keys = paths
            .iter()
            .filter_map(|p| {
                let wide: Vec<u16> = p.encode_utf16().chain([0]).collect();
                let mut h: HKEY = std::ptr::null_mut();
                // SAFETY: opens a key into `h`, closed in Drop (or below).
                let rc = unsafe {
                    RegOpenKeyExW(HKEY_CURRENT_USER, wide.as_ptr(), 0, KEY_NOTIFY, &mut h)
                };
                if rc != 0 {
                    return None;
                }
                // SAFETY: an unnamed auto-reset event, not signalled; closed in Drop.
                let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
                if event.is_null() {
                    // SAFETY: the key just opened, closed once.
                    unsafe { RegCloseKey(h) };
                    return None;
                }
                Some((h as isize, event as isize))
            })
            .collect();
        LookKeys { keys, armed: false }
    }

    /// Wait until one of the keys changed, or `limit` passed (what the keys miss is read then).
    pub fn wait(&mut self, limit: std::time::Duration) {
        use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Registry::{
            REG_NOTIFY_CHANGE_LAST_SET, REG_NOTIFY_THREAD_AGNOSTIC, RegNotifyChangeKeyValue,
        };
        use windows_sys::Win32::System::Threading::WaitForMultipleObjects;
        if self.keys.is_empty() {
            std::thread::sleep(limit);
            return;
        }
        let arm = |(key, event): (isize, isize)| {
            // SAFETY: an open key and its event; the notification is asynchronous, fires once and
            // ends when the key is closed.
            unsafe {
                RegNotifyChangeKeyValue(
                    key as _,
                    0,
                    REG_NOTIFY_CHANGE_LAST_SET | REG_NOTIFY_THREAD_AGNOSTIC,
                    event as _,
                    1,
                )
            };
        };
        if !self.armed {
            self.keys.iter().copied().for_each(arm);
            self.armed = true;
        }
        let events: Vec<HANDLE> = self.keys.iter().map(|&(_, e)| e as HANDLE).collect();
        let ms = limit.as_millis().min(u128::from(u32::MAX - 1)) as u32;
        // SAFETY: our own events, as many as the count says.
        let rc = unsafe { WaitForMultipleObjects(events.len() as u32, events.as_ptr(), 0, ms) };
        let fired = rc.wrapping_sub(WAIT_OBJECT_0) as usize;
        if let Some(&k) = self.keys.get(fired) {
            // Registered again before the read that follows, so a change during it is seen too.
            arm(k);
        }
    }
}

impl Drop for LookKeys {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Registry::RegCloseKey;
        for &(key, event) in &self.keys {
            // SAFETY: a key and an event made in `open`, each closed once.
            unsafe {
                RegCloseKey(key as _);
                CloseHandle(event as _);
            }
        }
    }
}

/// Windows' high-contrast colours while high contrast is on (Settings → Accessibility →
/// Contrast themes): background, text, selection, selection's text, links, greyed text. None
/// while it is off.
pub fn high_contrast_colors() -> Option<[egui::Color32; 6]> {
    use windows_sys::Win32::Graphics::Gdi::{
        COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_HOTLIGHT, COLOR_WINDOW,
        COLOR_WINDOWTEXT, GetSysColor,
    };
    use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETHIGHCONTRAST, SystemParametersInfoW};
    // SAFETY: HIGHCONTRASTW is plain data; its size is set as the call requires.
    let mut hc: HIGHCONTRASTW = unsafe { zeroed() };
    hc.cbSize = size_of::<HIGHCONTRASTW>() as u32;
    // SAFETY: a read into the struct above, of the size it states.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.cbSize,
            &mut hc as *mut HIGHCONTRASTW as *mut c_void,
            0,
        )
    };
    if ok == 0 || hc.dwFlags & HCF_HIGHCONTRASTON == 0 {
        return None;
    }
    let color = |i| {
        // SAFETY: a plain call; COLORREF is 0x00BBGGRR.
        let c = unsafe { GetSysColor(i) };
        egui::Color32::from_rgb(c as u8, (c >> 8) as u8, (c >> 16) as u8)
    };
    Some([
        color(COLOR_WINDOW),
        color(COLOR_WINDOWTEXT),
        color(COLOR_HIGHLIGHT),
        color(COLOR_HIGHLIGHTTEXT),
        color(COLOR_HOTLIGHT),
        color(COLOR_GRAYTEXT),
    ])
}

/// Sleep now (not hibernate: rundll32's SetSuspendState read its own window handle as
/// "hibernate"). `force`: don't let programs delay it. False when Windows refused.
pub fn sleep(force: bool) -> bool {
    use windows_sys::Win32::System::Power::SetSuspendState;
    // SAFETY: a plain call; FALSE / force / FALSE (no wake events disabled).
    unsafe { SetSuspendState(false, force, false) }
}

/// Is this screen point (a saved window's corner, in points) on a monitor now? A window put
/// back where a since-removed screen was would open out of sight.
pub fn on_a_screen(x: i32, y: i32) -> bool {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromPoint};
    // SAFETY: a plain query.
    !unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONULL) }.is_null()
}

/// The monitors in screen pixels (each one's rect and work area, [left, top, right, bottom]),
/// the primary first.
pub fn screens() -> Vec<([i32; 4], [i32; 4])> {
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    };
    const MONITORINFOF_PRIMARY: u32 = 1;
    type Found = Vec<(bool, [i32; 4], [i32; 4])>;
    unsafe extern "system" fn cb(
        mon: HMONITOR,
        _dc: HDC,
        _rc: *mut RECT,
        lparam: LPARAM,
    ) -> windows_sys::core::BOOL {
        let rect = |r: RECT| [r.left, r.top, r.right, r.bottom];
        // SAFETY: lparam is the Vec below, alive for the enumeration; mi is a writable local.
        unsafe {
            let found = &mut *(lparam as *mut Found);
            let mut mi: MONITORINFO = zeroed();
            mi.cbSize = size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(mon, &mut mi) != 0 {
                found.push((
                    mi.dwFlags & MONITORINFOF_PRIMARY != 0,
                    rect(mi.rcMonitor),
                    rect(mi.rcWork),
                ));
            }
        }
        1
    }
    let mut found: Found = Vec::new();
    // SAFETY: the callback only pushes to `found`, which outlives the call.
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(cb),
            &mut found as *mut Found as LPARAM,
        );
    }
    // The primary first (a stable sort keeps the others' order).
    found.sort_by_key(|(primary, ..)| !primary);
    found.into_iter().map(|(_, r, w)| (r, w)).collect()
}

/// Our window titled `title`: its rect in screen pixels ([left, top, right, bottom]) and
/// whether it is visible. None while it is minimized (its rect is then the taskbar's icon's).
pub fn window_rect(title: &str) -> Option<([i32; 4], bool)> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsIconic, IsWindowVisible};
    let hwnd = find_own_window(title)?;
    let mut r: RECT = unsafe { zeroed() };
    // SAFETY: plain queries about our own window; r is a writable local.
    let (got, visible, minimized) = unsafe {
        (
            GetWindowRect(hwnd, &mut r) != 0,
            IsWindowVisible(hwnd) != 0,
            IsIconic(hwnd) != 0,
        )
    };
    (got && !minimized).then_some(([r.left, r.top, r.right, r.bottom], visible))
}

/// Our window titled `title` moved to screen pixel (x, y), nothing else changed (its size, its
/// place among the windows, the focus). Moved onto a screen of another scale, Windows gives it
/// that scale (WM_DPICHANGED), which resizes it.
pub fn move_window(title: &str, x: i32, y: i32) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SWP_NOOWNERZORDER, SWP_NOSIZE};
    let Some(hwnd) = find_own_window(title) else {
        return false;
    };
    // SAFETY: our own window, on its thread.
    unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        ) != 0
    }
}

/// Whether an assistive technology (a screen reader, a UI Automation client) is listening to
/// this desktop's windows now (#131): the windows then keep their accessibility tree ready, so
/// a window read for the first time answers with its contents at once.
pub fn assistive_technology_listening() -> bool {
    // SAFETY: a plain query, no arguments.
    unsafe { windows_sys::Win32::UI::Accessibility::UiaClientsAreListening() != 0 }
}

/// Rows a notch of the mouse wheel scrolls (Mouse settings → "lines to scroll at a time", 3
/// unless changed); None when it is set to a screen at a time.
pub fn wheel_lines() -> Option<f32> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SPI_GETWHEELSCROLLLINES, SystemParametersInfoW,
    };
    let mut lines: u32 = 3;
    // SAFETY: a plain query into a u32 it writes.
    let ok =
        unsafe { SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, (&raw mut lines).cast(), 0) };
    match lines {
        _ if ok == 0 => Some(3.0),
        // WHEEL_PAGESCROLL
        u32::MAX => None,
        n => Some(n as f32),
    }
}

/// Seconds since the last keyboard or mouse input anywhere ("limit while I'm using the
/// computer").
pub fn idle_seconds() -> u64 {
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let mut info = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: `info` is a LASTINPUTINFO with its size set, as the call requires.
    let (ok, now) = unsafe { (GetLastInputInfo(&mut info), GetTickCount()) };
    if ok == 0 {
        return 0;
    }
    u64::from(now.wrapping_sub(info.dwTime) / 1000)
}

// ---- taskbar progress (ITaskbarList3) -----------------------------------------------------
//
// windows-sys has no COM interface tables, so the three calls used are declared here in
// ITaskbarList3's vtable order: IUnknown (3), ITaskbarList (HrInit, AddTab, DeleteTab,
// ActivateTab, SetActiveAlt), ITaskbarList2 (MarkFullscreenWindow), ITaskbarList3
// (SetProgressValue, SetProgressState, …).

#[repr(C)]
struct TaskbarVtbl {
    _query_interface: usize,
    _add_ref: usize,
    _release: usize,
    hr_init: unsafe extern "system" fn(*mut c_void) -> i32,
    _add_tab: usize,
    _delete_tab: usize,
    _activate_tab: usize,
    _set_active_alt: usize,
    _mark_fullscreen_window: usize,
    set_progress_value: unsafe extern "system" fn(*mut c_void, HWND, u64, u64) -> i32,
    set_progress_state: unsafe extern "system" fn(*mut c_void, HWND, u32) -> i32,
}

/// The taskbar object, created on first use on the UI thread.
struct Taskbar {
    obj: *mut *const TaskbarVtbl,
}

thread_local! {
    static TASKBAR: std::cell::RefCell<Option<Taskbar>> = const { std::cell::RefCell::new(None) };
    static MAIN_TASKBAR: std::cell::RefCell<TaskbarProgress> = const { std::cell::RefCell::new(TaskbarProgress::new()) };
    // No borrow survives a shell call: its nested message loop can recreate a button.
    static TASKBAR_BUTTONS: std::cell::RefCell<(u64, std::collections::HashMap<isize, u64>)> = Default::default();
}

fn taskbar_button_created_message() -> u32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW;
    static ID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *ID.get_or_init(|| {
        let name: Vec<u16> = "TaskbarButtonCreated\0".encode_utf16().collect();
        // SAFETY: a NUL-terminated string read during the call.
        unsafe { RegisterWindowMessageW(name.as_ptr()) }
    })
}

/// One window's taskbar progress. The cache belongs to the window, not the COM object:
/// two downloads (or the main window) can have the same fraction and still need an update.
/// Dropping it clears the indicator before the viewport goes away.
#[derive(Default)]
pub struct TaskbarProgress {
    window: isize,
    shown: Option<Option<u32>>,
    button_generation: u64,
}

impl TaskbarProgress {
    pub const fn new() -> Self {
        Self {
            window: 0,
            shown: None,
            button_generation: 0,
        }
    }

    pub fn window(&self) -> Option<isize> {
        (self.window != 0).then_some(self.window)
    }

    /// A vanished viewport has nothing to clear. Its old HWND may already belong to another
    /// window of this process, so do not send to it while forgetting the binding.
    pub fn window_gone(&mut self) {
        self.window = 0;
        self.shown = None;
        self.button_generation = 0;
    }

    pub fn set_window(&mut self, window: isize) {
        self.set_window_with(window, write_taskbar_progress);
    }

    fn set_window_with(&mut self, window: isize, mut send: impl FnMut(isize, Option<u32>) -> bool) {
        if self.window != window {
            self.update_with(None, &mut send);
            self.window = window;
            self.shown = None;
            self.button_generation = 0;
        }
    }

    /// Only changed values or recreated buttons reach the shell; failed calls are retried.
    pub fn update(&mut self, progress: Option<f64>) {
        self.update_with(progress, write_taskbar_progress);
    }

    fn update_with(
        &mut self,
        progress: Option<f64>,
        mut send: impl FnMut(isize, Option<u32>) -> bool,
    ) {
        // The main cache can be dropped after the notification storage during thread exit.
        let generation = TASKBAR_BUTTONS
            .try_with(|buttons| buttons.borrow().1.get(&self.window).copied().unwrap_or(0))
            .unwrap_or(0);
        if self.button_generation != generation {
            self.shown = None;
            // Take the generation before send: a notification during COM must survive it.
            self.button_generation = generation;
        }
        let value = progress.map(|p| (p.clamp(0.0, 1.0) * 1000.0).round() as u32);
        if self.window != 0 && self.shown != Some(value) && send(self.window, value) {
            self.shown = Some(value);
        }
    }
}

impl Drop for TaskbarProgress {
    fn drop(&mut self) {
        self.update(None);
    }
}

/// Aggregate progress on the main window; per-download windows own their own cache and handle.
pub fn taskbar_progress(title: &str, progress: Option<f64>) {
    if let Some(hwnd) = find_own_window(title) {
        MAIN_TASKBAR.with_borrow_mut(|bar| {
            bar.set_window(hwnd as isize);
            bar.update(progress);
        });
    }
}

fn write_taskbar_progress(window: isize, value: Option<u32>) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsWindow;
    let hwnd = window as HWND;
    let mut pid = 0;
    // SAFETY: queries accepting any handle; pid is a writable local. A closed or foreign
    // window has no indicator of ours to clear, even if Windows recycled the bound handle.
    if unsafe { IsWindow(hwnd) } == 0
        || unsafe { GetWindowThreadProcessId(hwnd, &mut pid) } == 0
        || pid != unsafe { GetCurrentProcessId() }
    {
        return true;
    }
    use windows_sys::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    };
    use windows_sys::core::GUID;
    const CLSID_TASKBARLIST: GUID = GUID::from_u128(0x56fdf344_fd6d_11d0_958a_006097c9a090);
    const IID_ITASKBARLIST3: GUID = GUID::from_u128(0xea1afb91_9e28_4b86_90e9_9e9f8a5eefaf);
    const TBPF_NOPROGRESS: u32 = 0;
    const TBPF_NORMAL: u32 = 2;
    TASKBAR
        .try_with(|cell| {
            let mut slot = cell.borrow_mut();
            if slot.is_none() {
                let mut obj: *mut c_void = std::ptr::null_mut();
                // SAFETY: COM on the UI thread (a second CoInitializeEx is harmless); the out
                // pointer receives an ITaskbarList3 on success, checked before use.
                unsafe {
                    CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
                    let hr = CoCreateInstance(
                        &CLSID_TASKBARLIST,
                        std::ptr::null_mut(),
                        CLSCTX_INPROC_SERVER,
                        &IID_ITASKBARLIST3,
                        &mut obj,
                    );
                    if hr < 0 || obj.is_null() {
                        return false;
                    }
                    let tb = obj as *mut *const TaskbarVtbl;
                    ((**tb).hr_init)(obj);
                    *slot = Some(Taskbar { obj: tb });
                }
            }
            let Some(tb) = slot.as_mut() else {
                return false;
            };
            // SAFETY: `obj` is a live ITaskbarList3 (never released: it lives as long as the UI
            // thread); the calls match the vtable above.
            unsafe {
                let this = tb.obj as *mut c_void;
                match value {
                    Some(v) => {
                        ((**tb.obj).set_progress_state)(this, hwnd, TBPF_NORMAL) >= 0
                            && ((**tb.obj).set_progress_value)(this, hwnd, u64::from(v), 1000) >= 0
                    }
                    None => ((**tb.obj).set_progress_state)(this, hwnd, TBPF_NOPROGRESS) >= 0,
                }
            }
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod taskbar_tests {
    use super::TaskbarProgress;

    #[test]
    fn taskbar_cache_can_drop_after_notification_storage() {
        std::thread::spawn(|| {
            // The main cache is initialized first, so it is destroyed after TASKBAR_BUTTONS.
            super::MAIN_TASKBAR.with_borrow_mut(|bar| {
                bar.set_window_with(1, |_, _| true);
                bar.update_with(Some(0.5), |_, _| true);
            });
        })
        .join()
        .unwrap();
    }

    #[test]
    fn taskbar_button_creation_resends_stalled_progress_on_the_next_tick() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, SW_HIDE, SW_SHOWNOACTIVATE, SendMessageW, ShowWindow,
            WS_POPUP,
        };
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let message = super::taskbar_button_created_message();
        assert_ne!(message, 0);
        let mut bars = [TaskbarProgress::new(), TaskbarProgress::new()];
        let mut windows = Vec::new();
        let mut calls = Vec::new();
        // SAFETY: hidden/off-screen windows of this thread, destroyed below. Notifications
        // use the shell's registered message, without restarting the user's Explorer.
        unsafe {
            for bar in &mut bars {
                let h = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    class.as_ptr(),
                    WS_POPUP,
                    -20000,
                    -20000,
                    100,
                    100,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                assert!(!h.is_null());
                super::caption_less(h, super::Drawn::Fixed);
                SendMessageW(h, message, 0, 0); // A notification before binding is safe too.
                bar.set_window_with(h as isize, |_, _| true);
                bar.update_with(Some(0.25), |h, v| {
                    calls.push((h, v));
                    true
                });
                windows.push(h);
            }
            for _ in 0..100 {
                for bar in &mut bars {
                    bar.update_with(Some(0.25), |_, _| panic!("unchanged progress"));
                }
            }
            SendMessageW(windows[0], message, 0, 0); // Delayed initial button creation.
            bars[1].update_with(Some(0.25), |_, _| panic!("another window's cache"));
            bars[0].update_with(Some(0.25), |h, v| {
                calls.push((h, v));
                true
            });
            assert_eq!(calls.len(), 3);

            for &h in &windows {
                SendMessageW(h, message, 0, 0); // Explorer recreates both taskbar buttons.
            }
            for bar in &mut bars {
                bar.update_with(Some(0.25), |h, v| {
                    calls.push((h, v));
                    true
                });
            }
            assert_eq!(calls.len(), 5);

            ShowWindow(windows[0], SW_SHOWNOACTIVATE);
            ShowWindow(windows[0], SW_HIDE);
            ShowWindow(windows[0], SW_SHOWNOACTIVATE);
            SendMessageW(windows[0], message, 0, 0); // Back from the tray, same value.
            bars[0].update_with(Some(0.25), |h, v| {
                calls.push((h, v));
                // COM's nested message loop can recreate the button during this send.
                SendMessageW(windows[0], message, 0, 0);
                true
            });
            bars[0].update_with(Some(0.25), |h, v| {
                calls.push((h, v));
                true
            });
            assert_eq!(
                calls.len(),
                7,
                "a notification during send must not be lost"
            );
            bars[0].update_with(Some(0.25), |_, _| panic!("already resent"));
            assert!(calls.iter().all(|(_, v)| *v == Some(250)));
            bars[0].update_with(None, |_, v| {
                assert_eq!(v, None);
                true
            });
            SendMessageW(windows[0], message, 0, 0);
            bars[0].update_with(None, |_, v| {
                assert_eq!(v, None);
                true
            });

            for h in windows {
                DestroyWindow(h);
                super::TASKBAR_BUTTONS.with_borrow(|(_, windows)| {
                    assert!(
                        !windows.contains_key(&(h as isize)),
                        "destroyed HWND forgotten"
                    );
                });
                assert_eq!(
                    super::thin_frame_on(h as isize, [100.0; 2], 1.0, None),
                    None
                );
                assert!(super::write_taskbar_progress(h as isize, Some(250)));
            }
        }
    }

    #[test]
    #[ignore = "child process for foreign_taskbar_windows_are_never_written_or_framed"]
    fn foreign_taskbar_window_host() {
        use std::io::{Write, stdin, stdout};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, WS_POPUP,
        };
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        // SAFETY: this subprocess's hidden window; only its HWND crosses the process boundary.
        unsafe {
            let h = CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                WS_POPUP,
                -20000,
                -20000,
                100,
                100,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!h.is_null());
            println!("TASKBAR_TEST_HWND={}", h as isize);
            stdout().flush().unwrap();
            stdin().read_line(&mut String::new()).unwrap();
            DestroyWindow(h);
        }
    }

    #[test]
    fn foreign_taskbar_windows_are_never_written_or_framed() {
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};
        struct Host(std::process::Child);
        impl Drop for Host {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut host = Host(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "win32::taskbar_tests::foreign_taskbar_window_host",
                    "--ignored",
                    "--nocapture",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let window: isize = BufReader::new(host.0.stdout.take().unwrap())
            .lines()
            .map(|l| l.unwrap())
            .find_map(|l| {
                l.strip_prefix("TASKBAR_TEST_HWND=")
                    .map(|h| h.parse().unwrap())
            })
            .expect("child window handle");
        let mut pid = 0;
        // SAFETY: plain queries of the subprocess's live HWND.
        unsafe {
            assert_ne!(
                windows_sys::Win32::UI::WindowsAndMessaging::IsWindow(window as _),
                0
            );
            super::GetWindowThreadProcessId(window as _, &mut pid);
        }
        assert_eq!(pid, host.0.id());
        assert_ne!(pid, std::process::id());
        assert_eq!(
            super::thin_frame_on(window, [548.0, 472.0], 1.0, None),
            None
        );
        // The stale binding must be rejected before even borrowing COM, for updates and clears.
        super::TASKBAR.with_borrow_mut(|_| {
            assert!(super::write_taskbar_progress(window, Some(250)));
            assert!(super::write_taskbar_progress(window, None));
        });
    }

    #[test]
    fn taskbar_binding_keeps_shell_titles_without_isolates() {
        use crate::{
            model::{Item, Status},
            progress::{Action, Progress},
        };
        use egui::{Pos2, ViewportId, vec2};
        use std::{cell::RefCell, rc::Rc};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, SetWindowTextW, WS_POPUP,
        };

        let windows = Rc::new(RefCell::new(std::collections::HashMap::new()));
        let titles = Rc::new(RefCell::new(Vec::new()));
        let owned = windows.clone();
        let given = titles.clone();
        egui::Context::set_immediate_viewport_renderer(move |ctx, viewport| {
            let title = viewport.builder.title.as_deref().unwrap();
            given.borrow_mut().push(title.to_owned());
            let wide: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
            let hwnd = *owned
                .borrow_mut()
                .entry(viewport.ids.this)
                .or_insert_with(|| {
                    let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
                    // SAFETY: a hidden test window on this thread, using a system class.
                    unsafe {
                        CreateWindowExW(
                            0,
                            class.as_ptr(),
                            wide.as_ptr(),
                            WS_POPUP,
                            -20000,
                            -20000,
                            548,
                            472,
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                            std::ptr::null(),
                        )
                    }
                });
            assert!(!hwnd.is_null());
            // SAFETY: this test's own window and a terminated title.
            unsafe { SetWindowTextW(hwnd, wide.as_ptr()) };
            let input = egui::RawInput {
                viewport_id: viewport.ids.this,
                viewports: std::iter::once((viewport.ids.this, egui::ViewportInfo::default()))
                    .collect(),
                screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(548.0, 472.0))),
                ..Default::default()
            };
            let mut callback = viewport.viewport_ui_cb;
            let mut out = ctx.run_ui(input, |ui| callback(ui));
            out.textures_delta.clear();
        });
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        let mut progress = Vec::new();
        for (n, name) in ["same.bin", "same.bin", "שלום (1).txt"]
            .into_iter()
            .enumerate()
        {
            let id = u64::from(std::process::id()) * 1000 + n as u64;
            let mut item = Item::new(
                id,
                "https://example.com/a".parse().unwrap(),
                name.into(),
                crate::model::Category::General,
            );
            item.size = Some(100);
            item.downloaded = 25;
            item.status = Status::Downloading;
            progress.push((Progress::new(id, false), item));
        }
        for opening in 0..3 {
            for pass in 0..2 {
                titles.borrow_mut().clear();
                // show() binds and draws only. Shell writes belong to tick()'s watchdog step.
                let mut out = super::TASKBAR.with_borrow_mut(|_| {
                    ctx.run_ui(egui::RawInput::default(), |ui| {
                        for (window, item) in &mut progress {
                            assert!(matches!(window.show(ui.ctx(), item, None), Action::None));
                        }
                    })
                });
                out.textures_delta.clear();
                assert_eq!(titles.borrow().len(), progress.len());
                for ((_, item), title) in progress.iter().zip(titles.borrow().iter()) {
                    let vid = ViewportId::from_hash_of(("progress", item.id));
                    let expected = crate::i18n::os_text(&format!(
                        "25% {}",
                        crate::i18n::isolate(&item.file_name)
                    ));
                    if pass == 0 {
                        assert_eq!(*title, format!("mdm-progress-{}", item.id));
                        assert!(
                            out.viewport_output[&vid].commands.iter().any(|cmd| {
                                matches!(cmd, egui::ViewportCommand::Title(t) if t == &expected)
                            }),
                            "binding must set the shell-safe title before the window is revealed"
                        );
                    } else {
                        assert_eq!(*title, expected);
                    }
                    assert!(
                        !expected
                            .chars()
                            .any(|c| ('\u{2066}'..='\u{2069}').contains(&c))
                    );
                }
            }
            if opening == 1 {
                // Completion can replace the controller too; exercise both lifecycle paths.
                for (window, item) in &mut progress {
                    *window = Progress::new(item.id, false);
                }
            }
            for (_, hwnd) in windows.borrow_mut().drain() {
                // SAFETY: a hidden window created by this test on this thread.
                unsafe { DestroyWindow(hwnd) };
            }
            // Keep the controller and egui's frame cache through a missing pass, without a
            // close action: keep_theme and the HWND binding must both start fresh on reopening.
            let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
            out.textures_delta.clear();
        }
    }

    #[test]
    fn taskbar_windows_with_equal_titles_keep_their_own_frames() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowRect, RegisterClassW,
            SetWindowTextW, WNDCLASSW, WS_EX_APPWINDOW, WS_POPUP,
        };
        let class: Vec<u16> = "mdm-taskbar-binding-test\0".encode_utf16().collect();
        let titles: Vec<Vec<u16>> = (0..2)
            .map(|id| format!("mdm-progress-test-{id}\0").encode_utf16().collect())
            .collect();
        let same: Vec<u16> = "25% same.bin\0".encode_utf16().collect();
        // SAFETY: hidden test windows on this thread, with the system's window procedure.
        // All strings are terminated; no input or messages go to any other process.
        unsafe {
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(DefWindowProcW);
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
            let windows: Vec<_> = titles
                .iter()
                .map(|title| {
                    CreateWindowExW(
                        WS_EX_APPWINDOW,
                        class.as_ptr(),
                        title.as_ptr(),
                        WS_POPUP,
                        -20000,
                        -20000,
                        548,
                        472,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null(),
                    )
                })
                .collect();
            assert!(windows.iter().all(|h| !h.is_null()));
            for (i, h) in windows.iter().enumerate() {
                let title = format!("mdm-progress-test-{i}");
                assert_eq!(super::find_own_window(&title), Some(*h));
                SetWindowTextW(*h, same.as_ptr());
            }
            for (i, h) in windows.iter().enumerate() {
                assert_eq!(
                    super::thin_frame_on(*h as isize, [548.0, 472.0], 1.0 + i as f32 * 0.25, None),
                    Some(*h as isize)
                );
            }
            let mut first = std::mem::zeroed();
            let mut second = std::mem::zeroed();
            GetWindowRect(windows[0], &mut first);
            GetWindowRect(windows[1], &mut second);
            assert!(
                second.right - second.left > first.right - first.left,
                "scaling one of two equal titles must resize that window alone"
            );
            for h in windows {
                DestroyWindow(h);
            }
        }
    }

    #[test]
    fn equal_fractions_reach_each_window_and_clear_independently() {
        let mut main = TaskbarProgress::new();
        let mut first = TaskbarProgress::new();
        let mut second = TaskbarProgress::new();
        let mut calls = Vec::new();
        let mut send = |h, v| {
            calls.push((h, v));
            true
        };
        for (bar, h) in [(&mut main, 1), (&mut first, 2), (&mut second, 3)] {
            bar.set_window_with(h, &mut send);
            bar.update_with(Some(0.25), &mut send);
            bar.update_with(Some(0.2501), &mut send);
        }
        first.update_with(None, &mut send);
        second.update_with(Some(0.75), &mut send);
        main.update_with(Some(0.5), &mut send);
        assert_eq!(
            calls,
            [
                (1, Some(250)),
                (2, Some(250)),
                (3, Some(250)),
                (2, None),
                (3, Some(750)),
                (1, Some(500))
            ]
        );
    }

    #[test]
    fn replacing_or_closing_a_window_clears_it_and_reopening_sends_again() {
        let mut bar = TaskbarProgress::new();
        let mut calls = Vec::new();
        let mut send = |h, v| {
            calls.push((h, v));
            true
        };
        bar.set_window_with(1, &mut send);
        bar.update_with(Some(0.5), &mut send);
        bar.set_window_with(2, &mut send);
        bar.update_with(Some(0.5), &mut send);
        bar.set_window_with(0, &mut send);
        bar.update_with(Some(0.9), &mut send); // no window
        bar.set_window_with(2, &mut send); // even a reused HWND is a fresh cache
        bar.update_with(Some(0.5), &mut send);
        assert_eq!(
            calls,
            [
                (1, Some(500)),
                (1, None),
                (2, Some(500)),
                (2, None),
                (2, Some(500))
            ]
        );
    }

    #[test]
    fn failed_shell_calls_are_retried() {
        let mut bar = TaskbarProgress::new();
        bar.set_window_with(1, |_, _| true);
        bar.update_with(Some(0.4), |_, _| false);
        let mut calls = Vec::new();
        bar.update_with(Some(0.4), |h, v| {
            calls.push((h, v));
            true
        });
        assert_eq!(calls, [(1, Some(400))]);
    }

    #[test]
    fn a_vanished_window_is_forgotten_without_writing_to_a_recycled_handle() {
        let mut bar = TaskbarProgress::new();
        bar.set_window_with(1, |_, _| true);
        bar.update_with(Some(0.5), |_, _| true);
        // Even a live, same-process replacement must not receive the old window's clear.
        super::TASKBAR.with_borrow_mut(|_| bar.window_gone());
        assert_eq!(bar.window(), None);
        bar.update_with(Some(0.5), |_, _| panic!("no bound window"));
        bar.set_window_with(1, |_, _| panic!("the vanished window needs no clear"));
        let mut calls = Vec::new();
        bar.update_with(Some(0.5), |h, v| {
            calls.push((h, v));
            true
        });
        assert_eq!(calls, [(1, Some(500))]);
    }
}

/// A REG_SZ / REG_DWORD value under HKEY_CURRENT_USER\`key` (None when absent).
fn reg_string(key: &str, name: &str) -> Option<String> {
    reg_string_in(
        windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
        key,
        name,
    )
}

/// The language the installer's wizard ran in (tools/installer/mdm.iss writes it on every
/// install), taken once: read, then deleted. A languages.tsv code (`pt-br`); Inno's language
/// names have `_` for `-`.
pub fn take_install_language() -> Option<String> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteKeyValueW};
    const KEY: &str = r"Software\MisterDownloadManager";
    const NAME: &str = "InstallLanguage";
    let value = reg_string(KEY, NAME)?;
    let w = |s: &str| -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() };
    let (key, name) = (w(KEY), w(NAME));
    // SAFETY: NUL-terminated key and value names.
    unsafe {
        RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr());
    }
    Some(install_language_code(&value))
}

/// What the installer's task "Open .torrent files and magnet links with Mister Download
/// Manager" was (tools/installer/mdm.iss writes `1` or `0` on every install): None when no
/// installer wrote it. Only read: the app settles Torrent settings' switch from it once
/// (`Settings::settle_torrent_handler`, #1058), and an upgrade's value is never asked again.
pub fn install_torrents() -> Option<bool> {
    reg_string(r"Software\MisterDownloadManager", "InstallTorrents").map(|v| v.trim() == "1")
}

/// Is this program what Windows opens .torrent files with by their class (the user's or
/// everyone's, as HKEY_CLASSES_ROOT merges them): the installer's task, or Torrent settings'
/// switch? A copy elsewhere (another install, a portable one) isn't this one.
pub fn torrent_handler_is_ours() -> bool {
    use windows_sys::Win32::System::Registry::HKEY_CLASSES_ROOT;
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(class) = reg_string_in(HKEY_CLASSES_ROOT, ".torrent", "") else {
        return false;
    };
    let command = format!(r"{}\shell\open\command", class.trim());
    reg_string_in(HKEY_CLASSES_ROOT, &command, "")
        .is_some_and(|c| runs_program(&c, &exe.display().to_string()))
}

/// Does an open command (`"C:\…\misterdownloadmanager.exe" "%1"`) start `exe`? Windows' paths
/// are case-insensitive.
fn runs_program(command: &str, exe: &str) -> bool {
    !exe.is_empty() && command.to_lowercase().contains(&exe.to_lowercase())
}

#[cfg(test)]
mod torrent_handler_tests {
    #[test]
    fn an_open_command_names_its_program() {
        let exe = r"C:\Program Files\Mister Download Manager\misterdownloadmanager.exe";
        let ours = r#""c:\program files\mister download manager\MisterDownloadManager.exe" "%1""#;
        assert!(super::runs_program(ours, exe));
        let other = r#""C:\Program Files\qBittorrent\qbittorrent.exe" "%1""#;
        assert!(!super::runs_program(other, exe));
        assert!(!super::runs_program(ours, ""));
    }
}

/// An Inno language name (`lang_pt_br`, tools/installer/languages.py) as a languages.tsv code.
fn install_language_code(inno: &str) -> String {
    inno.strip_prefix("lang_").unwrap_or(inno).replace('_', "-")
}

#[cfg(test)]
mod install_language_tests {
    #[test]
    fn inno_names_become_codes() {
        assert_eq!(super::install_language_code("lang_pt_br"), "pt-br");
        assert_eq!(super::install_language_code("lang_or"), "or");
        assert_eq!(
            super::install_language_code("lang_ca_es_valencia"),
            "ca-es-valencia"
        );
    }
}

/// The user's Windows region (Control Panel → Region): its short date's order, whether its
/// clock is 24-hour, and the first day of the week (0 = Sunday). None if Windows can't say.
pub fn region_formats() -> Option<(crate::regional::DateOrder, bool, u8)> {
    use windows_sys::Win32::Globalization::{
        GetLocaleInfoEx, LOCALE_IFIRSTDAYOFWEEK, LOCALE_SSHORTDATE, LOCALE_STIMEFORMAT,
    };
    let info = |what: u32| -> Option<String> {
        let mut buf = [0u16; 80];
        // SAFETY: null locale = the user's; the buffer's length is passed in characters.
        let n =
            unsafe { GetLocaleInfoEx(std::ptr::null(), what, buf.as_mut_ptr(), buf.len() as i32) };
        (n > 1).then(|| String::from_utf16_lossy(&buf[..n as usize - 1]))
    };
    let date = info(LOCALE_SSHORTDATE)?;
    let at = |c: char| date.find(c).unwrap_or(usize::MAX);
    let (d, m, y) = (at('d'), at('M'), at('y'));
    let order = if y < m && y < d {
        crate::regional::DateOrder::Ymd
    } else if m < d {
        crate::regional::DateOrder::Mdy
    } else {
        crate::regional::DateOrder::Dmy
    };
    // "HH:mm:ss" (24-hour) or "h:mm:ss tt".
    let clock24 = info(LOCALE_STIMEFORMAT).is_some_and(|t| t.contains('H'));
    // Windows counts from Monday (0) to Sunday (6).
    let first = info(LOCALE_IFIRSTDAYOFWEEK)
        .and_then(|s| s.trim().parse::<u8>().ok())
        .map_or(0, |n| (n + 1) % 7);
    Some((order, clock24, first))
}

/// `t` (local time) as the user's Windows region writes a short date and a time with seconds
/// (Control Panel → Region): `30/10/2026 00:38:09`, `2026/10/30 0:38:09`… None if Windows can't.
pub fn local_date_time(t: chrono::NaiveDateTime) -> Option<String> {
    use chrono::{Datelike, Timelike};
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::Globalization::{DATE_SHORTDATE, GetDateFormatEx, GetTimeFormatEx};
    let st = SYSTEMTIME {
        wYear: t.year() as u16,
        wMonth: t.month() as u16,
        wDayOfWeek: t.weekday().num_days_from_sunday() as u16,
        wDay: t.day() as u16,
        wHour: t.hour() as u16,
        wMinute: t.minute() as u16,
        wSecond: t.second() as u16,
        wMilliseconds: 0,
    };
    let mut date = [0u16; 80];
    let mut time = [0u16; 80];
    // SAFETY: null locale = the user's; the buffers' lengths are passed in characters.
    let (d, h) = unsafe {
        (
            GetDateFormatEx(
                std::ptr::null(),
                DATE_SHORTDATE,
                &st,
                std::ptr::null(),
                date.as_mut_ptr(),
                date.len() as i32,
                std::ptr::null(),
            ),
            GetTimeFormatEx(
                std::ptr::null(),
                0,
                &st,
                std::ptr::null(),
                time.as_mut_ptr(),
                time.len() as i32,
            ),
        )
    };
    if d <= 1 || h <= 1 {
        return None;
    }
    let date = String::from_utf16_lossy(&date[..d as usize - 1]);
    let time = String::from_utf16_lossy(&time[..h as usize - 1]);
    Some(format!("{date} {time}"))
}

/// Windows' edition, release and build, e.g. "Windows 11 Pro 24H2 (build 26200)" — for problem
/// reports. ProductName still says "Windows 10" on 11; the build tells them apart.
pub fn windows_version() -> String {
    use windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let get = |n| reg_string_in(HKEY_LOCAL_MACHINE, KEY, n);
    let build = get("CurrentBuild").unwrap_or_default();
    let mut product = get("ProductName").unwrap_or_else(|| "Windows".into());
    if build.parse::<u32>().is_ok_and(|b| b >= 22000) {
        product = product.replace("Windows 10", "Windows 11");
    }
    let release = get("DisplayVersion")
        .map(|v| format!(" {v}"))
        .unwrap_or_default();
    format!("{product}{release} (build {build})")
}

/// The default browser (#668): the program https links open with (the user's choice in Default
/// apps, `UserChoice`'s ProgId, and that ProgId's `shell\open\command`), and its file's version.
/// None when it isn't a browser MDM knows (MDM then speaks as Chrome).
pub fn default_browser() -> Option<mdm_engine::agent::Found> {
    use mdm_engine::agent::{Browser, Found};
    use windows_sys::Win32::System::Registry::HKEY_CLASSES_ROOT;
    const HTTPS: &str = r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https";
    // Windows 11 keeps the choice in UserChoiceLatest since 2025, its ProgId in a key of its
    // own (UserChoiceLatest\ProgId's ProgId); UserChoice stays, and may be an older choice.
    let prog_id = [r"UserChoiceLatest\ProgId", "UserChoiceLatest", "UserChoice"]
        .iter()
        .find_map(|k| {
            reg_string(&format!(r"{HTTPS}\{k}"), "ProgId").filter(|p| !p.trim().is_empty())
        })?;
    let command = reg_string_in(
        HKEY_CLASSES_ROOT,
        &format!(r"{prog_id}\shell\open\command"),
        "",
    )?;
    let exe = std::path::PathBuf::from(crate::defaultbrowser::windows_program(&command)?);
    let stem = exe.file_stem()?.to_string_lossy().into_owned();
    // Opera starts through its launcher.exe, in a folder named after it ("Opera", "Opera GX").
    let name = if stem.eq_ignore_ascii_case("launcher") {
        exe.parent()?.file_name()?.to_string_lossy().into_owned()
    } else {
        stem
    };
    let browser = Browser::named(&name).or_else(|| Browser::named(&prog_id))?;
    Some(Found {
        browser,
        version: file_version(&exe),
        ubuntu: false,
    })
}

/// A program's file version ("157.1.99.55"): its version resource's fixed part.
fn file_version(exe: &std::path::Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
    };
    let path: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: a NUL-terminated path; the handle argument is optional and ignored.
    let size = unsafe { GetFileVersionInfoSizeW(path.as_ptr(), std::ptr::null_mut()) };
    if size == 0 {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    // SAFETY: `data` holds `size` bytes, as GetFileVersionInfoSizeW asked.
    if unsafe { GetFileVersionInfoW(path.as_ptr(), 0, size, data.as_mut_ptr().cast()) } == 0 {
        return None;
    }
    let root: Vec<u16> = "\\".encode_utf16().chain(Some(0)).collect();
    let (mut info, mut len) = (std::ptr::null_mut::<c_void>(), 0u32);
    // SAFETY: `data` is the version block just read; the pointer returned points into it.
    let ok = unsafe { VerQueryValueW(data.as_ptr().cast(), root.as_ptr(), &mut info, &mut len) };
    if ok == 0 || info.is_null() || (len as usize) < size_of::<VS_FIXEDFILEINFO>() {
        return None;
    }
    // SAFETY: VerQueryValueW returned a VS_FIXEDFILEINFO of `len` bytes inside `data`, which is
    // still alive; read unaligned, as a byte buffer promises no alignment.
    let fixed = unsafe { std::ptr::read_unaligned(info.cast::<VS_FIXEDFILEINFO>()) };
    let (ms, ls) = (fixed.dwFileVersionMS, fixed.dwFileVersionLS);
    Some(format!(
        "{}.{}.{}.{}",
        ms >> 16,
        ms & 0xffff,
        ls >> 16,
        ls & 0xffff
    ))
}

fn reg_string_in(
    root: windows_sys::Win32::System::Registry::HKEY,
    key: &str,
    name: &str,
) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RRF_RT_REG_SZ, RegGetValueW};
    let key: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: registry read into `buf`, whose size in bytes is passed in `size`.
    let rc = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut c_void,
            &mut size,
        )
    };
    if rc != 0 {
        return None;
    }
    let len = (size as usize / 2).saturating_sub(1).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// Whether Windows lets programs that ask for it use paths past `MAX_PATH` (Group Policy's
/// "Enable Win32 long paths", `LongPathsEnabled`; off on a new install): yt-dlp and ffmpeg ask,
/// so a video's files may then pass 259 characters (#1102). Read only.
pub fn long_paths_enabled() -> bool {
    reg_dword_in(
        windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE,
        r"SYSTEM\CurrentControlSet\Control\FileSystem",
        "LongPathsEnabled",
    ) == Some(1)
}

fn reg_dword(key: &str, name: &str) -> Option<u32> {
    reg_dword_in(
        windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
        key,
        name,
    )
}

fn reg_dword_in(
    root: windows_sys::Win32::System::Registry::HKEY,
    key: &str,
    name: &str,
) -> Option<u32> {
    use windows_sys::Win32::System::Registry::{RRF_RT_REG_DWORD, RegGetValueW};
    let key: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let mut value: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: plain registry read into a u32 of the stated size.
    let rc = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut value as *mut u32 as *mut c_void,
            &mut size,
        )
    };
    (rc == 0).then_some(value)
}

/// Windows' manual proxy (Settings → Network → Proxy), its `ProxyServer` value as written
/// (`host:port`, or `http=host:port;https=…;socks=…`), when it is on. Read by
/// `settings::parse_system_proxy`.
pub fn system_proxy() -> Option<String> {
    const KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings";
    if reg_dword(KEY, "ProxyEnable")? == 0 {
        return None;
    }
    reg_string(KEY, "ProxyServer")
}

/// Windows' "Use setup script" (Settings → Network → Proxy; Internet Options' "Use automatic
/// configuration script"): the script's address, when it is on. Proxy programs (Clash, v2rayN)
/// set it in their PAC mode (#1448). As WinHTTP reads this user's settings.
pub fn system_proxy_script() -> Option<String> {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::Networking::WinHttp::{
        WINHTTP_CURRENT_USER_IE_PROXY_CONFIG, WinHttpGetIEProxyConfigForCurrentUser,
    };
    let mut config = WINHTTP_CURRENT_USER_IE_PROXY_CONFIG::default();
    // SAFETY: the struct is ours to fill; on success its strings are WinHTTP's, each freed
    // below with GlobalFree as its documentation says, once read.
    if unsafe { WinHttpGetIEProxyConfigForCurrentUser(&mut config) } == 0 {
        return None;
    }
    let read = |p: windows_sys::core::PWSTR| -> Option<String> {
        if p.is_null() {
            return None;
        }
        // SAFETY: a NUL-terminated string WinHTTP allocated, read up to its NUL.
        let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
        // SAFETY: `len` units were just read at `p`.
        let units = unsafe { std::slice::from_raw_parts(p, len) };
        Some(String::from_utf16_lossy(units))
    };
    let script = read(config.lpszAutoConfigUrl);
    for p in [
        config.lpszAutoConfigUrl,
        config.lpszProxy,
        config.lpszProxyBypass,
    ] {
        if !p.is_null() {
            // SAFETY: WinHTTP's own allocation, freed once.
            unsafe { GlobalFree(p.cast()) };
        }
    }
    script
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Play a .wav file without waiting (Options → Sounds).
pub fn play_sound(file: &str) {
    use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_FILENAME, SND_NODEFAULT};
    let wide: Vec<u16> = file.encode_utf16().chain(Some(0)).collect();
    // SAFETY: a NUL-terminated path; SND_ASYNC copies what it needs before returning.
    unsafe {
        PlaySoundW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            SND_FILENAME | SND_ASYNC | SND_NODEFAULT,
        );
    }
}

/// The name MDM is listed under in Windows' Default apps: its value in RegisteredApplications,
/// the same as its Capabilities' ApplicationName (Microsoft: "ApplicationName must always match
/// the name that is registered under RegisteredApplications"). tools/installer/mdm.iss writes
/// the same.
pub const REGISTERED_NAME: &str = "Mister Download Manager";
/// MDM's Capabilities (Default apps' list of what it can open), under HKCU or HKLM.
const CAPABILITIES: &str = r"Software\MisterDownloadManager\Capabilities";
/// The ProgIDs MDM's Capabilities name: Default apps wants an application-specific one for each
/// file type and protocol, never the generic `magnet` class.
const TORRENT_PROGID: &str = "MisterDownloadManager.Torrent";
const MAGNET_PROGID: &str = "MisterDownloadManager.Magnet";

/// Make MDM (on) or stop it being (off) what Windows opens magnet links and .torrent files
/// with, for the current user (HKCU; no admin). Off removes only what points at us.
///
/// On writes the classes (`magnet`, `.torrent`), which Windows follows while no one has chosen
/// in Default apps, and registers MDM with Default apps (#1091: Capabilities and
/// RegisteredApplications, so MDM is listed for MAGNET and .torrent). A choice recorded there
/// (UserChoice, hash-protected) stays first: no program can set it, only the user, in Settings →
/// Default apps (`torrent_defaults_elsewhere` tells, `default_apps_page` opens it). An all-users
/// install that registered this program (the installer's HKLM) isn't registered twice.
pub fn set_torrent_handlers(on: bool) {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegDeleteTreeW, RegGetValueW,
        RegSetKeyValueW,
    };
    let w = |s: &str| -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() };
    let set = |key: &str, name: Option<&str>, value: &str| {
        let (key, value) = (w(key), w(value));
        let name = name.map(w);
        // SAFETY: NUL-terminated strings; the size covers the value's bytes.
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()),
                REG_SZ,
                value.as_ptr() as *const c_void,
                (value.len() * 2) as u32,
            );
        }
    };
    let get = |key: &str, name: Option<&str>| -> Option<String> {
        let key = w(key);
        let name = name.map(w);
        let mut buf = vec![0u16; 1024];
        let mut len = (buf.len() * 2) as u32;
        // SAFETY: NUL-terminated key/name; `len` is the buffer's size in bytes.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut c_void,
                &mut len,
            )
        };
        (r == 0).then(|| String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]))
    };
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let exe = exe.display().to_string();
    let open = format!("\"{exe}\" \"%1\"");
    let icon = format!("\"{exe}\",0");
    if on {
        set("Software\\Classes\\magnet", None, "URL:Magnet link");
        set("Software\\Classes\\magnet", Some("URL Protocol"), "");
        set("Software\\Classes\\magnet\\DefaultIcon", None, &icon);
        set(
            "Software\\Classes\\magnet\\shell\\open\\command",
            None,
            &open,
        );
        let prog = format!("Software\\Classes\\{TORRENT_PROGID}");
        set(&prog, None, "Torrent file");
        set(&format!("{prog}\\DefaultIcon"), None, &icon);
        set(&format!("{prog}\\shell\\open\\command"), None, &open);
        set("Software\\Classes\\.torrent", None, TORRENT_PROGID);
        // Default apps (#1091): MDM listed for MAGNET and .torrent, unless everyone's
        // registration (an all-users install of this program) already lists it.
        if !registered_for_everyone(&exe) {
            let magnet = format!("Software\\Classes\\{MAGNET_PROGID}");
            set(&magnet, None, "URL:Magnet link");
            set(&magnet, Some("URL Protocol"), "");
            set(&format!("{magnet}\\DefaultIcon"), None, &icon);
            set(&format!("{magnet}\\shell\\open\\command"), None, &open);
            set(CAPABILITIES, Some("ApplicationName"), REGISTERED_NAME);
            set(
                CAPABILITIES,
                Some("ApplicationDescription"),
                // i18n: Windows' Settings → Default apps: what Mister Download Manager is
                crate::i18n::t("Downloads files, videos and torrents"),
            );
            set(CAPABILITIES, Some("ApplicationIcon"), &icon);
            set(
                &format!("{CAPABILITIES}\\FileAssociations"),
                Some(".torrent"),
                TORRENT_PROGID,
            );
            set(
                &format!("{CAPABILITIES}\\UrlAssociations"),
                Some("magnet"),
                MAGNET_PROGID,
            );
            set(
                "Software\\RegisteredApplications",
                Some(REGISTERED_NAME),
                CAPABILITIES,
            );
        }
    } else {
        let ours = get("Software\\Classes\\magnet\\shell\\open\\command", None)
            .is_some_and(|c| c.contains(&exe));
        let tree = |k: &str| {
            let k = w(k);
            // SAFETY: NUL-terminated key under HKCU.
            unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, k.as_ptr()) };
        };
        if ours {
            tree("Software\\Classes\\magnet");
        }
        tree(&format!("Software\\Classes\\{TORRENT_PROGID}"));
        let value = |key: &str, name: &str| {
            let (k, n) = (w(key), w(name));
            // SAFETY: NUL-terminated key and value names (empty: the default value).
            unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr()) };
        };
        if get("Software\\Classes\\.torrent", None).as_deref() == Some(TORRENT_PROGID) {
            value("Software\\Classes\\.torrent", "");
        }
        // Off Default apps' list too (#1091): what this user's registration wrote. A choice made
        // there for MDM then finds no program, and Windows asks again, as for an app removed.
        tree(&format!("Software\\Classes\\{MAGNET_PROGID}"));
        tree(CAPABILITIES);
        if get("Software\\RegisteredApplications", Some(REGISTERED_NAME)).as_deref()
            == Some(CAPABILITIES)
        {
            value("Software\\RegisteredApplications", REGISTERED_NAME);
        }
    }
    // Tell Explorer the associations changed.
    // SAFETY: no pointers are passed.
    unsafe {
        windows_sys::Win32::UI::Shell::SHChangeNotify(
            windows_sys::Win32::UI::Shell::SHCNE_ASSOCCHANGED as i32,
            windows_sys::Win32::UI::Shell::SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        );
    }
}

/// Does everyone's registration (an all-users install, tools/installer/mdm.iss's HKLM) list this
/// program with Default apps? Its magnet ProgID's command starts `exe`.
fn registered_for_everyone(exe: &str) -> bool {
    use windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    reg_string_in(
        HKEY_LOCAL_MACHINE,
        r"Software\RegisteredApplications",
        REGISTERED_NAME,
    )
    .is_some_and(|c| c.eq_ignore_ascii_case(CAPABILITIES))
        && reg_string_in(
            HKEY_LOCAL_MACHINE,
            &format!(r"Software\Classes\{MAGNET_PROGID}\shell\open\command"),
            "",
        )
        .is_some_and(|c| runs_program(&c, exe))
}

/// Settings → Default apps at MDM's own page (#1091), where the user picks it for MAGNET and
/// .torrent: `registeredAppMachine` names an all-users registration, `registeredAppUser` this
/// user's (Microsoft's "Launch the Default Apps settings page"; Windows 11 since its 2023-04
/// update, earlier ones open Default apps itself).
pub fn default_apps_page() -> String {
    let Ok(exe) = std::env::current_exe() else {
        return "ms-settings:defaultapps".into();
    };
    default_apps_uri(registered_for_everyone(&exe.display().to_string()))
}

fn default_apps_uri(everyone: bool) -> String {
    let who = if everyone {
        "registeredAppMachine"
    } else {
        "registeredAppUser"
    };
    // The name, URI-escaped: its only character to escape is the space.
    let name = REGISTERED_NAME.replace(' ', "%20");
    format!("ms-settings:defaultapps?{who}={name}")
}

/// What Windows opens magnet links and .torrent files with when that isn't this program (#1091):
/// the other programs' names (as Windows shows them, else their file), none when MDM opens both.
/// Asked as Windows decides (AssocQueryString: the user's choice in Default apps first, then the
/// classes), after Torrent settings' switch was turned on.
pub fn torrent_defaults_elsewhere() -> Vec<String> {
    use windows_sys::Win32::UI::Shell::{
        ASSOCF_IS_PROTOCOL, ASSOCF_NONE, ASSOCSTR_EXECUTABLE, ASSOCSTR_FRIENDLYAPPNAME,
        AssocQueryStringW,
    };
    let Ok(exe) = std::env::current_exe() else {
        return Vec::new();
    };
    let ask = |what: &str, flags, string| -> Option<String> {
        let what: Vec<u16> = what.encode_utf16().chain(Some(0)).collect();
        let open: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
        let mut out = vec![0u16; 1024];
        let mut len = out.len() as u32;
        // SAFETY: the strings end in NUL and outlive the call; `len` is `out`'s size in
        // characters, and the call writes at most that many.
        let hr = unsafe {
            AssocQueryStringW(
                flags,
                string,
                what.as_ptr(),
                open.as_ptr(),
                out.as_mut_ptr(),
                &mut len,
            )
        };
        (hr == 0).then(|| {
            let end = out.iter().position(|&c| c == 0).unwrap_or(out.len());
            String::from_utf16_lossy(&out[..end])
        })
    };
    let handlers =
        [("magnet", ASSOCF_IS_PROTOCOL), (".torrent", ASSOCF_NONE)].map(|(what, flags)| Handler {
            program: ask(what, flags, ASSOCSTR_EXECUTABLE),
            name: ask(what, flags, ASSOCSTR_FRIENDLYAPPNAME),
        });
    others(&handlers, &exe.display().to_string())
}

/// The program Windows opens one kind (magnet links, .torrent files) with: its file and its name.
struct Handler {
    program: Option<String>,
    name: Option<String>,
}

/// The names of the handlers that aren't `exe`, each once. One Windows doesn't name (none
/// chosen, nothing found) isn't another program's.
fn others(handlers: &[Handler], exe: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for h in handlers {
        let Some(program) = h.program.as_deref().filter(|p| !p.trim().is_empty()) else {
            continue;
        };
        if program.eq_ignore_ascii_case(exe) || runs_program(program, exe) {
            continue;
        }
        let name = h
            .name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| {
                program
                    .rsplit(['\\', '/'])
                    .next()
                    .unwrap_or(program)
                    .to_owned()
            });
        if !names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
            names.push(name);
        }
    }
    names
}

#[cfg(test)]
mod default_apps_tests {
    use super::{Handler, default_apps_uri, others};

    const MDM: &str = r"C:\Program Files\Mister Download Manager\misterdownloadmanager.exe";
    const FDM: &str = r"C:\Program Files\Softdeluxe\Free Download Manager\fdm.exe";

    fn h(program: Option<&str>, name: Option<&str>) -> Handler {
        Handler {
            program: program.map(str::to_owned),
            name: name.map(str::to_owned),
        }
    }

    /// #1091: Settings opens at MDM's page, by the hive its registration is in.
    #[test]
    fn the_settings_page_names_the_registration() {
        assert_eq!(
            default_apps_uri(false),
            "ms-settings:defaultapps?registeredAppUser=Mister%20Download%20Manager"
        );
        assert_eq!(
            default_apps_uri(true),
            "ms-settings:defaultapps?registeredAppMachine=Mister%20Download%20Manager"
        );
    }

    /// #1091: another program chosen in Default apps for magnet links or .torrent files is named,
    /// once; MDM itself (any case) and nothing found are not.
    #[test]
    fn another_default_is_named_once() {
        let fdm = || h(Some(FDM), Some("Free Download Manager"));
        let ours = || h(Some(&MDM.to_uppercase()), Some("Mister Download Manager"));
        assert_eq!(others(&[fdm(), fdm()], MDM), ["Free Download Manager"]);
        assert_eq!(others(&[fdm(), ours()], MDM), ["Free Download Manager"]);
        assert!(others(&[ours(), ours()], MDM).is_empty());
        assert!(others(&[h(None, None), h(Some(" "), None)], MDM).is_empty());
        // No name from Windows: the program's file.
        let qb = h(Some(r"C:\Program Files\qBittorrent\qbittorrent.exe"), None);
        assert_eq!(
            others(&[fdm(), qb], MDM),
            ["Free Download Manager", "qbittorrent.exe"]
        );
    }

    /// #1091, run by hand only, as it redirects this process's HKEY_CURRENT_USER, which other
    /// tests read: `cargo test -p mdm-app --bin misterdownloadmanager scratch_hive -- --ignored
    /// --test-threads=1`. `set_torrent_handlers` writes into a scratch key standing for HKCU
    /// (RegOverridePredefKey), never the account's own associations: on, the classes and MDM's
    /// Default apps registration as Microsoft documents it; off, all of it gone.
    #[test]
    #[ignore]
    fn registers_with_default_apps_in_a_scratch_hive() {
        use super::{
            CAPABILITIES, MAGNET_PROGID, REGISTERED_NAME, TORRENT_PROGID, reg_string,
            registered_for_everyone, set_torrent_handlers,
        };
        use windows_sys::Win32::System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_ALL_ACCESS, REG_OPTION_NON_VOLATILE, RegCloseKey,
            RegCreateKeyExW, RegDeleteKeyW, RegDeleteTreeW, RegOverridePredefKey,
        };
        let scratch: Vec<u16> = r"Software\MisterDownloadManagerTest1091"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: a NUL-terminated name; `key` receives the opened handle.
        let created = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                scratch.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_ALL_ACCESS,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        };
        assert_eq!(created, 0, "scratch key");
        // SAFETY: `key` is open; from here HKEY_CURRENT_USER is the scratch key in this process.
        let overridden = unsafe { RegOverridePredefKey(HKEY_CURRENT_USER, key) };
        assert_eq!(overridden, 0, "not redirected: nothing written");
        let exe = std::env::current_exe().unwrap().display().to_string();
        let everyone = registered_for_everyone(&exe);
        let read = || {
            [
                (r"Software\Classes\magnet\shell\open\command", ""),
                (r"Software\Classes\.torrent", ""),
                (
                    r"Software\Classes\MisterDownloadManager.Magnet",
                    "URL Protocol",
                ),
                (
                    r"Software\Classes\MisterDownloadManager.Magnet\shell\open\command",
                    "",
                ),
                (CAPABILITIES, "ApplicationName"),
                (CAPABILITIES, "ApplicationDescription"),
                (
                    r"Software\MisterDownloadManager\Capabilities\UrlAssociations",
                    "magnet",
                ),
                (
                    r"Software\MisterDownloadManager\Capabilities\FileAssociations",
                    ".torrent",
                ),
                (r"Software\RegisteredApplications", REGISTERED_NAME),
            ]
            .map(|(k, n)| reg_string(k, n))
        };
        set_torrent_handlers(true);
        let on = read();
        set_torrent_handlers(false);
        let off = read();
        // SAFETY: back to the real HKEY_CURRENT_USER, then the scratch key is removed.
        unsafe {
            RegOverridePredefKey(HKEY_CURRENT_USER, std::ptr::null_mut());
            RegCloseKey(key);
            RegDeleteTreeW(HKEY_CURRENT_USER, scratch.as_ptr());
            RegDeleteKeyW(HKEY_CURRENT_USER, scratch.as_ptr());
        }
        println!("on: {on:#?}\noff: {off:#?}");
        let open = format!("\"{exe}\" \"%1\"");
        assert_eq!(on[0].as_deref(), Some(open.as_str()));
        assert_eq!(on[1].as_deref(), Some(TORRENT_PROGID));
        if !everyone {
            assert_eq!(on[2].as_deref(), Some(""));
            assert_eq!(on[3].as_deref(), Some(open.as_str()));
            assert_eq!(on[4].as_deref(), Some(REGISTERED_NAME));
            assert!(on[5].as_deref().is_some_and(|d| !d.is_empty()));
            assert_eq!(on[6].as_deref(), Some(MAGNET_PROGID));
            assert_eq!(on[7].as_deref(), Some(TORRENT_PROGID));
            assert_eq!(on[8].as_deref(), Some(CAPABILITIES));
        }
        assert!(off.iter().all(Option::is_none), "{off:#?}");
    }
}

/// The Run key's value MDM keeps its startup entry in (Options → "Launch on startup", the
/// installer's "Start when Windows starts"). An off-screen test copy never touches the user's:
/// it uses `MDM_TEST_RUN_VALUE`'s name when given one, else none at all (#226's checks).
fn run_value() -> Option<String> {
    if crate::offscreen() {
        return std::env::var("MDM_TEST_RUN_VALUE")
            .ok()
            .filter(|v| v.starts_with("MisterDownloadManagerTest"));
    }
    Some("MisterDownloadManager".to_owned())
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// The startup entry's command for the program `exe`: started with the session, MDM waits in
/// the notification area (`/startup`, #226).
fn startup_command(exe: &std::path::Path) -> String {
    format!("\"{}\" {}", exe.display(), mdm_ipc::STARTUP)
}

/// A startup entry `value` (a command line) written before #226 for the program `exe`: the same
/// program, without the argument. Another program's entry (a copy of MDM elsewhere) is not this
/// one's to rewrite.
fn startup_outdated(value: &str, exe: &std::path::Path) -> bool {
    // The program compared as Windows compares paths: in any case, either slash.
    let norm = |s: &str| s.replace('/', "\\").to_lowercase();
    let exe = norm(&exe.display().to_string());
    let value = value.trim();
    let args = match value.strip_prefix('"') {
        Some(rest) => match rest.split_once('"') {
            Some((program, args)) if norm(program) == exe => args,
            _ => return false,
        },
        // Unquoted (written by hand): the program's path, spaces and all, then its arguments.
        None => match value.get(..exe.len()).zip(value.get(exe.len()..)) {
            Some((program, args))
                if norm(program) == exe && (args.is_empty() || args.starts_with(' ')) =>
            {
                args
            }
            _ => return false,
        },
    };
    !args
        .split_whitespace()
        .any(|a| a.eq_ignore_ascii_case(mdm_ipc::STARTUP))
}

/// Is Mister Download Manager in the user's Run key (set by Options or by the installer's
/// "Start when Windows starts")? The registry is the truth for Options → "Launch on startup".
pub fn run_at_startup() -> bool {
    run_value().is_some_and(|name| {
        reg_string_in(
            windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
            RUN_KEY,
            &name,
        )
        .is_some()
    })
}

/// The startup entry rewritten with its argument when it was written without (by versions
/// before 0.5.0, or by their installer) for this very program (#226).
pub fn update_run_at_startup() {
    let (Some(name), Ok(exe)) = (run_value(), std::env::current_exe()) else {
        return;
    };
    let value = reg_string_in(
        windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
        RUN_KEY,
        &name,
    );
    if value.is_some_and(|v| startup_outdated(&v, &exe)) {
        tracing::info!("startup entry written again, to start in the notification area");
        set_run_at_startup(true);
    }
}

pub fn set_run_at_startup(on: bool) {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW,
    };
    let Some(name) = run_value() else {
        return;
    };
    let key: Vec<u16> = RUN_KEY.encode_utf16().chain(Some(0)).collect();
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    if on {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let value: Vec<u16> = startup_command(&exe)
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // SAFETY: NUL-terminated key, name and value; the size covers the value's bytes.
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                value.as_ptr() as *const c_void,
                (value.len() * 2) as u32,
            );
        }
    } else {
        // SAFETY: NUL-terminated key and value name.
        unsafe {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr());
        }
    }
}

#[cfg(test)]
mod startup_tests {
    use super::{startup_command, startup_outdated};
    use std::path::Path;

    const EXE: &str = r"C:\Program Files\Mister Download Manager\misterdownloadmanager.exe";

    /// The entry Options and the installer write (#226): the program, quoted, then `/startup`.
    #[test]
    fn the_startup_entry_starts_mdm_in_the_tray() {
        let cmd = startup_command(Path::new(EXE));
        assert_eq!(
            cmd,
            format!("\"{EXE}\" /startup"),
            "as tools/installer/mdm.iss writes it too"
        );
        let args = mdm_ipc::parse_args(&["/startup".to_owned()]).unwrap();
        assert!(args.startup);
        assert!(!startup_outdated(&cmd, Path::new(EXE)));
    }

    /// An entry of a version before 0.5.0, or of its installer, is written again; one for another
    /// copy of MDM, or one already up to date, is left alone.
    #[test]
    fn old_startup_entries_are_rewritten() {
        let exe = Path::new(EXE);
        // Options before 0.5.0, and the installer's: the program alone, quoted.
        assert!(startup_outdated(&format!("\"{EXE}\""), exe));
        // Written by hand: unquoted, in another case, with other slashes.
        assert!(startup_outdated(
            r"c:\program files\mister download manager\MISTERDOWNLOADMANAGER.EXE",
            exe
        ));
        assert!(startup_outdated(
            &format!("\"{}\"", EXE.replace('\\', "/").to_uppercase()),
            exe
        ));
        assert!(!startup_outdated(&format!("{EXE}2"), exe));
        // Already with it (any case).
        assert!(!startup_outdated(&format!("\"{EXE}\" /startup"), exe));
        assert!(!startup_outdated(&format!("\"{EXE}\" /STARTUP"), exe));
        // Another copy's (a portable one, a build elsewhere).
        assert!(!startup_outdated(
            r#""D:\mdm\misterdownloadmanager.exe""#,
            exe
        ));
        assert!(!startup_outdated("", exe));
    }

    /// The installer's "Start when Windows starts" writes the entry Options writes.
    #[test]
    fn the_installer_writes_the_same_entry() {
        let iss = include_str!("../../../tools/installer/mdm.iss");
        assert!(iss.contains(
            r#"ValueName: "MisterDownloadManager"; ValueData: """{app}\{#AppExe}"" /startup""#
        ));
    }
}

/// The work area (left, top, right, bottom) of the monitor nearest to screen point (x, y).
pub fn work_area(x: i32, y: i32) -> Option<[i32; 4]> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    };
    unsafe {
        let mon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        (GetMonitorInfoW(mon, &mut mi) != 0).then_some([
            mi.rcWork.left,
            mi.rcWork.top,
            mi.rcWork.right,
            mi.rcWork.bottom,
        ])
    }
}

/// The shell's icon for a file type or a folder (fileicons.rs), as Explorer shows it, from the
/// system image list that fits `px` pixels: small, large, extra large (16, 32, 48 at 100 %, more
/// with the system's scale), then jumbo (256) for anything bigger. The file need not exist. Run
/// on fileicons.rs's lookup thread: the first lookup of a type loads its shell handler.
///
/// One lookup at a time in the process (the drag picture asks from the UI thread): two threads
/// in the shell at once could miss an icon. A miss is tried once more (a handler's first load).
pub fn file_icon(what: &crate::fileicons::What, px: u32) -> Option<crate::fileicons::Native> {
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _held = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    file_icon_once(what, px).or_else(|| file_icon_once(what, px))
}

fn file_icon_once(what: &crate::fileicons::What, px: u32) -> Option<crate::fileicons::Native> {
    use crate::fileicons::What;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
    use windows_sys::Win32::UI::Shell::SHGFI_SYSICONINDEX;
    com_on_this_thread();
    let (name, attrs) = match what {
        What::FolderOpen => return list_icon(open_folder_index()?, px),
        What::Folder => ("folder".to_string(), FILE_ATTRIBUTE_DIRECTORY),
        What::File(Some(ext)) => (format!("file.{ext}"), FILE_ATTRIBUTE_NORMAL),
        What::File(None) => ("file".to_string(), FILE_ATTRIBUTE_NORMAL),
    };
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    // SAFETY: `info` is a zeroed SHFILEINFOW of the stated size; the name is NUL-terminated;
    // with SHGFI_USEFILEATTRIBUTES the file need not exist. SYSICONINDEX returns no icon to free.
    let index = unsafe {
        let mut info: SHFILEINFOW = zeroed();
        let ok = SHGetFileInfoW(
            wide.as_ptr(),
            attrs,
            &mut info,
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX | SHGFI_USEFILEATTRIBUTES,
        );
        if ok == 0 {
            return None;
        }
        info.iIcon
    };
    list_icon(index, px)
}

/// The system image lists' icon `index` at the list that fits `px` pixels (see `file_icon`).
fn list_icon(index: i32, px: u32) -> Option<crate::fileicons::Native> {
    use windows_sys::Win32::UI::Shell::{SHIL_EXTRALARGE, SHIL_JUMBO, SHIL_LARGE, SHIL_SMALL};
    let mut got: Option<(u32, Vec<u8>)> = None;
    for list in [SHIL_SMALL, SHIL_LARGE, SHIL_EXTRALARGE, SHIL_JUMBO] {
        if got.as_ref().is_some_and(|(side, _)| *side >= px) {
            break;
        }
        let Some((side, rgba)) = image_list_icon(list, index) else {
            continue;
        };
        // A type with no picture that big gets its smaller one in the jumbo list's top-left
        // corner: keep the smaller one then.
        if list == SHIL_JUMBO
            && let Some((prev, _)) = &got
            && drawn_within(&rgba, side, *prev)
        {
            break;
        }
        got = Some((side, rgba));
    }
    let (side, rgba) = got?;
    Some(crate::fileicons::Native::Rgba {
        side,
        rgba,
        premultiplied: false,
    })
}

/// Whether every visible pixel of the `side`-square `rgba` lies in its top-left `within` square.
fn drawn_within(rgba: &[u8], side: u32, within: u32) -> bool {
    rgba.as_chunks::<4>().0.iter().enumerate().all(|(i, c)| {
        let (x, y) = (i as u32 % side, i as u32 / side);
        c[3] == 0 || (x < within && y < within)
    })
}

/// COM for this thread (apartment-threaded, as the shell's icon handlers expect), once.
fn com_on_this_thread() {
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    thread_local!(static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    DONE.with(|done| {
        if !done.replace(true) {
            // SAFETY: no reserved pointer; the thread keeps COM for its lifetime.
            unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        }
    });
}

/// The start of the shell's `IImageList` vtable (commoncontrols.h), as far as `GetIcon`. The list
/// is called through it rather than comctl32's `ImageList_GetIcon`, which needs the version 6
/// controls to read the shell's lists.
#[repr(C)]
struct ImageListVtbl {
    _query_interface: usize,
    _add_ref: usize,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    // Add, ReplaceIcon, SetOverlayImage, Replace, AddMasked, Draw, Remove.
    _before_get_icon: [usize; 7],
    get_icon: unsafe extern "system" fn(
        *mut c_void,
        i32,
        u32,
        *mut windows_sys::Win32::UI::WindowsAndMessaging::HICON,
    ) -> windows_sys::core::HRESULT,
}

/// IID_IImageList, {46EB5926-582E-4017-9FDF-E8998DAA0950}.
const IID_IMAGE_LIST: windows_sys::core::GUID = windows_sys::core::GUID {
    data1: 0x46EB_5926,
    data2: 0x582E,
    data3: 0x4017,
    data4: [0x9F, 0xDF, 0xE8, 0x99, 0x8D, 0xAA, 0x09, 0x50],
};

/// Icon `index` of the system image list `list` (SHIL_*), as (side, RGBA).
fn image_list_icon(list: u32, index: i32) -> Option<(u32, Vec<u8>)> {
    use windows_sys::Win32::UI::Controls::ILD_TRANSPARENT;
    use windows_sys::Win32::UI::Shell::SHGetImageList;
    // SAFETY: SHGetImageList hands out an IImageList with a reference for us, released below; the
    // vtable is read only as far as GetIcon, whose icon hicon_rgba takes and destroys.
    unsafe {
        let mut list_ptr: *mut c_void = std::ptr::null_mut();
        if SHGetImageList(list as i32, &IID_IMAGE_LIST, &mut list_ptr) < 0 || list_ptr.is_null() {
            return None;
        }
        let vtbl = &**(list_ptr as *const *const ImageListVtbl);
        let mut icon = std::ptr::null_mut();
        let hr = (vtbl.get_icon)(list_ptr, index, ILD_TRANSPARENT, &mut icon);
        (vtbl.release)(list_ptr);
        if hr < 0 || icon.is_null() {
            return None;
        }
        hicon_rgba(icon)
    }
}

/// Windows' type name for a file name ("BIN File", "Compressed (zipped) Folder").
pub fn type_name(file_name: &str) -> Option<String> {
    use windows_sys::Win32::UI::Shell::SHGFI_TYPENAME;
    let name: Vec<u16> = file_name.encode_utf16().chain([0]).collect();
    // SAFETY: `info` is a zeroed SHFILEINFOW of the stated size; the name is NUL-terminated;
    // with SHGFI_USEFILEATTRIBUTES the file need not exist.
    unsafe {
        let mut info: SHFILEINFOW = zeroed();
        let ok = SHGetFileInfoW(
            name.as_ptr(),
            FILE_ATTRIBUTE_NORMAL,
            &mut info,
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_TYPENAME | SHGFI_USEFILEATTRIBUTES,
        );
        // A copy: on 32-bit Windows the struct is packed, and its fields can't be borrowed.
        let type_name = info.szTypeName;
        let len = type_name.iter().position(|c| *c == 0).unwrap_or(0);
        (ok != 0 && len > 0).then(|| String::from_utf16_lossy(&type_name[..len]))
    }
}

/// `icon`'s pixels as (side, straight RGBA), at the size its bitmap has; the icon is destroyed.
/// An icon without alpha (an old one) takes its transparency from its mask.
unsafe fn hicon_rgba(
    icon: windows_sys::Win32::UI::WindowsAndMessaging::HICON,
) -> Option<(u32, Vec<u8>)> {
    use windows_sys::Win32::Graphics::Gdi::{BITMAP, GetObjectW};
    unsafe {
        let mut ii: ICONINFO = zeroed();
        let got = GetIconInfo(icon, &mut ii) != 0;
        let mut out = None;
        if got && !ii.hbmColor.is_null() {
            let mut bm: BITMAP = zeroed();
            let have = GetObjectW(
                ii.hbmColor as _,
                size_of::<BITMAP>() as i32,
                &mut bm as *mut BITMAP as *mut c_void,
            ) != 0;
            let side = bm.bmWidth;
            if have
                && side > 0
                && side == bm.bmHeight.abs()
                && side <= 1024
                && let Some(mut px) = dib_rgba(ii.hbmColor, side)
            {
                if !px.as_chunks::<4>().0.iter().any(|c| c[3] != 0) {
                    // The mask: white (set) where the icon is transparent.
                    let mask = (!ii.hbmMask.is_null())
                        .then(|| dib_rgba(ii.hbmMask, side))
                        .flatten();
                    for (i, c) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                        c[3] = match &mask {
                            Some(m) if m[i * 4] != 0 => 0,
                            _ => 255,
                        };
                    }
                }
                out = Some((side as u32, px));
            }
        }
        if got {
            DeleteObject(ii.hbmColor as _);
            DeleteObject(ii.hbmMask as _);
        }
        DestroyIcon(icon);
        out
    }
}

/// The first `side` × `side` pixels of `bitmap`, top-down, as RGBA (32 bits per pixel).
unsafe fn dib_rgba(
    bitmap: windows_sys::Win32::Graphics::Gdi::HBITMAP,
    side: i32,
) -> Option<Vec<u8>> {
    unsafe {
        let mut bmi: BITMAPINFO = zeroed();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: side,
            biHeight: -side, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..zeroed()
        };
        let mut px = vec![0u8; (side * side * 4) as usize];
        let dc = GetDC(std::ptr::null_mut());
        let lines = GetDIBits(
            dc,
            bitmap,
            0,
            side as u32,
            px.as_mut_ptr() as *mut c_void,
            &mut bmi,
            DIB_RGB_COLORS,
        );
        ReleaseDC(std::ptr::null_mut(), dc);
        (lines == side).then(|| {
            for c in px.as_chunks_mut::<4>().0 {
                c.swap(0, 2); // BGRA → RGBA
            }
            px
        })
    }
}

/// Windows' own "open folder" (Download complete's, as IDM shows), as its system image list index.
fn open_folder_index() -> Option<i32> {
    use windows_sys::Win32::UI::Shell::{
        SHGSI_SYSICONINDEX, SHGetStockIconInfo, SHSTOCKICONINFO, SIID_FOLDEROPEN,
    };
    // SAFETY: `info` is zeroed with its size set; SYSICONINDEX returns no icon to free.
    unsafe {
        let mut info: SHSTOCKICONINFO = zeroed();
        info.cbSize = size_of::<SHSTOCKICONINFO>() as u32;
        (SHGetStockIconInfo(SIID_FOLDEROPEN, SHGSI_SYSICONINDEX, &mut info) == 0)
            .then_some(info.iSysImageIndex)
    }
}

/// The drop target's OLE side (`droptarget.rs`): a minimal `IDropTarget` that accepts links
/// (UniformResourceLocatorW), text and files.
pub mod ole {

    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, mpsc::Sender};

    use windows_sys::Win32::Foundation::HWND;

    type HResult = i32;
    const S_OK: HResult = 0;
    const E_NOINTERFACE: HResult = 0x8000_4002_u32 as i32;
    const DROPEFFECT_NONE: u32 = 0;
    const DROPEFFECT_COPY: u32 = 1;
    const CF_UNICODETEXT: u16 = 13;
    const CF_HDROP: u16 = 15;
    const TYMED_HGLOBAL: u32 = 1;
    const DVASPECT_CONTENT: u32 = 1;

    #[repr(C)]
    #[derive(PartialEq)]
    struct Guid(u32, u16, u16, [u8; 8]);
    const IID_IUNKNOWN: Guid = Guid(0, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
    const IID_IDROPTARGET: Guid = Guid(0x0000_0122, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);

    #[repr(C)]
    struct PointL {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    struct FormatEtc {
        cf: u16,
        ptd: *mut c_void,
        aspect: u32,
        lindex: i32,
        tymed: u32,
    }

    #[repr(C)]
    struct StgMedium {
        tymed: u32,
        handle: *mut c_void,
        unk_for_release: *mut c_void,
    }

    #[link(name = "ole32")]
    unsafe extern "system" {
        fn OleInitialize(reserved: *mut c_void) -> HResult;
        fn RegisterDragDrop(hwnd: HWND, target: *mut c_void) -> HResult;
        fn RevokeDragDrop(hwnd: HWND) -> HResult;
        fn ReleaseStgMedium(medium: *mut StgMedium);
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalLock(h: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(h: *mut c_void) -> i32;
        fn GlobalSize(h: *mut c_void) -> usize;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn RegisterClipboardFormatW(name: *const u16) -> u32;
    }
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn DragQueryFileW(hdrop: *mut c_void, i: u32, buf: *mut u16, cch: u32) -> u32;
    }

    /// IDataObject's vtable up to GetData (index 3), all we call.
    #[repr(C)]
    struct DataObjectVtbl {
        _query_interface: usize,
        _add_ref: usize,
        _release: usize,
        get_data:
            unsafe extern "system" fn(*mut c_void, *const FormatEtc, *mut StgMedium) -> HResult,
    }

    #[repr(C)]
    struct Vtbl {
        query_interface:
            unsafe extern "system" fn(*mut Target, *const Guid, *mut *mut c_void) -> HResult,
        add_ref: unsafe extern "system" fn(*mut Target) -> u32,
        release: unsafe extern "system" fn(*mut Target) -> u32,
        drag_enter:
            unsafe extern "system" fn(*mut Target, *mut c_void, u32, PointL, *mut u32) -> HResult,
        drag_over: unsafe extern "system" fn(*mut Target, u32, PointL, *mut u32) -> HResult,
        drag_leave: unsafe extern "system" fn(*mut Target) -> HResult,
        drop: unsafe extern "system" fn(*mut Target, *mut c_void, u32, PointL, *mut u32) -> HResult,
    }

    #[repr(C)]
    struct Target {
        vtbl: &'static Vtbl,
        refs: AtomicU32,
        tx: Sender<String>,
        over: Arc<AtomicBool>,
        /// What the current drag carries (decided at DragEnter).
        accepts: AtomicBool,
        /// Woken at enter, leave and drop: OLE's drag loop sends the window no input, so
        /// egui wouldn't draw the highlight, or take the drop, until something else came.
        wake: egui::Context,
    }

    static VTBL: Vtbl = Vtbl {
        query_interface,
        add_ref,
        release,
        drag_enter,
        drag_over,
        drag_leave,
        drop,
    };

    unsafe extern "system" fn query_interface(
        this: *mut Target,
        iid: *const Guid,
        out: *mut *mut c_void,
    ) -> HResult {
        // SAFETY: COM calls with valid pointers.
        unsafe {
            if *iid == IID_IUNKNOWN || *iid == IID_IDROPTARGET {
                add_ref(this);
                *out = this.cast();
                S_OK
            } else {
                *out = std::ptr::null_mut();
                E_NOINTERFACE
            }
        }
    }

    unsafe extern "system" fn add_ref(this: *mut Target) -> u32 {
        // SAFETY: `this` is a live Target (COM reference counting).
        unsafe { (*this).refs.fetch_add(1, Ordering::AcqRel) + 1 }
    }

    unsafe extern "system" fn release(this: *mut Target) -> u32 {
        // SAFETY: the last release frees the Box made in `Registration::new`.
        unsafe {
            let n = (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
            if n == 0 {
                std::mem::drop(Box::from_raw(this));
            }
            n
        }
    }

    unsafe extern "system" fn drag_enter(
        this: *mut Target,
        data: *mut c_void,
        _keys: u32,
        _pt: PointL,
        effect: *mut u32,
    ) -> HResult {
        // SAFETY: valid for the call.
        unsafe {
            // Our own drag out (a download's file): not something to add (#1210).
            let ok = !super::own_drag_over_us() && !read(data).is_empty();
            (*this).accepts.store(ok, Ordering::Relaxed);
            (*this).over.store(ok, Ordering::Relaxed);
            (*this).wake.request_repaint();
            *effect = if ok { DROPEFFECT_COPY } else { DROPEFFECT_NONE };
        }
        S_OK
    }

    unsafe extern "system" fn drag_over(
        this: *mut Target,
        _keys: u32,
        _pt: PointL,
        effect: *mut u32,
    ) -> HResult {
        // SAFETY: valid for the call.
        unsafe {
            *effect = if (*this).accepts.load(Ordering::Relaxed) {
                DROPEFFECT_COPY
            } else {
                DROPEFFECT_NONE
            };
        }
        S_OK
    }

    unsafe extern "system" fn drag_leave(this: *mut Target) -> HResult {
        // SAFETY: valid for the call.
        unsafe {
            (*this).over.store(false, Ordering::Relaxed);
            (*this).wake.request_repaint();
        }
        S_OK
    }

    unsafe extern "system" fn drop(
        this: *mut Target,
        data: *mut c_void,
        _keys: u32,
        _pt: PointL,
        effect: *mut u32,
    ) -> HResult {
        // SAFETY: valid for the call.
        unsafe {
            (*this).over.store(false, Ordering::Relaxed);
            let found = if super::own_drag_over_us() {
                Vec::new()
            } else {
                read(data)
            };
            *effect = if found.is_empty() {
                DROPEFFECT_NONE
            } else {
                DROPEFFECT_COPY
            };
            for s in found {
                let _ = (*this).tx.send(s);
            }
            (*this).wake.request_repaint();
        }
        S_OK
    }

    /// The addresses (or file paths) a drag carries: a link, else text that holds addresses,
    /// else dropped files.
    unsafe fn read(data: *mut c_void) -> Vec<String> {
        // SAFETY: `data` is the IDataObject OLE passed in.
        unsafe {
            let name: Vec<u16> = "UniformResourceLocatorW\0".encode_utf16().collect();
            let url_format = RegisterClipboardFormatW(name.as_ptr()) as u16;
            for cf in [url_format, CF_UNICODETEXT] {
                if let Some(text) = get_text(data, cf) {
                    let urls: Vec<String> = text
                        .split_whitespace()
                        .filter(|w| {
                            let w = w.to_ascii_lowercase();
                            w.starts_with("http://")
                                || w.starts_with("https://")
                                || w.starts_with("ftp://")
                                || w.starts_with("ftps://")
                                || w.starts_with("magnet:")
                        })
                        .map(str::to_owned)
                        .collect();
                    if !urls.is_empty() {
                        return urls;
                    }
                }
            }
            files(data)
        }
    }

    unsafe fn medium(data: *mut c_void, cf: u16) -> Option<StgMedium> {
        // SAFETY: IDataObject::GetData through its vtable.
        unsafe {
            let vtbl = *(data as *const *const DataObjectVtbl);
            let fmt = FormatEtc {
                cf,
                ptd: std::ptr::null_mut(),
                aspect: DVASPECT_CONTENT,
                lindex: -1,
                tymed: TYMED_HGLOBAL,
            };
            let mut m = StgMedium {
                tymed: 0,
                handle: std::ptr::null_mut(),
                unk_for_release: std::ptr::null_mut(),
            };
            (((*vtbl).get_data)(data, &fmt, &mut m) == S_OK && !m.handle.is_null()).then_some(m)
        }
    }

    unsafe fn get_text(data: *mut c_void, cf: u16) -> Option<String> {
        // SAFETY: a locked HGLOBAL of UTF-16, read within its size, then released.
        unsafe {
            let mut m = medium(data, cf)?;
            let p = GlobalLock(m.handle) as *const u16;
            let text = (!p.is_null()).then(|| {
                let n = GlobalSize(m.handle) / 2;
                let s = std::slice::from_raw_parts(p, n);
                let end = s.iter().position(|c| *c == 0).unwrap_or(n);
                String::from_utf16_lossy(&s[..end])
            });
            GlobalUnlock(m.handle);
            ReleaseStgMedium(&mut m);
            text
        }
    }

    unsafe fn files(data: *mut c_void) -> Vec<String> {
        // SAFETY: an HDROP medium, queried with DragQueryFileW, then released.
        unsafe {
            let Some(mut m) = medium(data, CF_HDROP) else {
                return Vec::new();
            };
            let n = DragQueryFileW(m.handle, u32::MAX, std::ptr::null_mut(), 0);
            let mut out = Vec::new();
            for i in 0..n {
                let len = DragQueryFileW(m.handle, i, std::ptr::null_mut(), 0);
                let mut buf = vec![0u16; len as usize + 1];
                DragQueryFileW(m.handle, i, buf.as_mut_ptr(), len + 1);
                out.push(String::from_utf16_lossy(&buf[..len as usize]));
            }
            ReleaseStgMedium(&mut m);
            out
        }
    }

    /// Our target registered on the drop window; revoked when dropped.
    pub struct Registration {
        hwnd: HWND,
        target: *mut Target,
    }

    impl Registration {
        /// None until the window exists (tried again next frame).
        pub fn new(
            title: &str,
            tx: Sender<String>,
            over: Arc<AtomicBool>,
            wake: &egui::Context,
        ) -> Option<Self> {
            let hwnd = crate::win32::find_own_window(title)?;
            let target = Box::into_raw(Box::new(Target {
                vtbl: &VTBL,
                refs: AtomicU32::new(1),
                tx,
                over,
                accepts: AtomicBool::new(false),
                wake: wake.clone(),
            }));
            // SAFETY: OLE on the UI thread (winit initialised it already: S_FALSE is fine);
            // any target winit registered goes first, then ours (RegisterDragDrop AddRefs it).
            unsafe {
                OleInitialize(std::ptr::null_mut());
                RevokeDragDrop(hwnd);
                if RegisterDragDrop(hwnd, target.cast()) != S_OK {
                    release(target);
                    return None;
                }
            }
            Some(Registration { hwnd, target })
        }

        /// Off-screen test copies only (MDM_TEST_DROP, droptarget.rs): `text` dropped on the
        /// window as a browser drops a link, without a mouse: a data object holding it as
        /// "UniformResourceLocatorW" goes to the drop target OLE has registered for the window
        /// (DragEnter, then Drop), as OLE's drag loop hands it. False when the window holds no
        /// registration of ours.
        pub fn test_drop(&self, text: &str) -> bool {
            use windows_sys::Win32::UI::WindowsAndMessaging::GetPropW;
            // OLE keeps the target registered for a window in this property of it (in the
            // process that registered it: the target itself).
            let prop: Vec<u16> = "OleDropTargetInterface\0".encode_utf16().collect();
            // SAFETY: a property read on our own window.
            let registered = unsafe { GetPropW(self.hwnd, prop.as_ptr()) };
            if registered != self.target.cast() {
                tracing::warn!("test drop: the window holds no drop target of ours");
                return false;
            }
            let data = Box::into_raw(Box::new(TestData {
                vtbl: &TEST_DATA_VTBL,
                text: text.encode_utf16().chain(Some(0)).collect(),
            }));
            let target = registered as *mut Target;
            let mut effect = DROPEFFECT_COPY;
            // SAFETY: our live target, called through its vtable as OLE does, with a data
            // object that lives until both calls return.
            unsafe {
                let pt = PointL { x: 0, y: 0 };
                ((*target).vtbl.drag_enter)(target, data.cast(), 0, pt, &mut effect);
                let pt = PointL { x: 0, y: 0 };
                ((*target).vtbl.drop)(target, data.cast(), 0, pt, &mut effect);
                std::mem::drop(Box::from_raw(data));
            }
            tracing::info!("test drop: {text} (effect {effect})");
            effect == DROPEFFECT_COPY
        }
    }

    /// The data object of [`Registration::test_drop`]: its text as a link (or plain text), in
    /// memory the receiver frees (ReleaseStgMedium).
    #[repr(C)]
    struct TestData {
        vtbl: &'static TestDataVtbl,
        text: Vec<u16>,
    }

    /// IDataObject's vtable up to GetData, all the receiver calls.
    #[repr(C)]
    struct TestDataVtbl {
        query_interface:
            unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        get_data:
            unsafe extern "system" fn(*mut TestData, *const FormatEtc, *mut StgMedium) -> HResult,
    }

    static TEST_DATA_VTBL: TestDataVtbl = TestDataVtbl {
        query_interface: test_data_query,
        add_ref: test_data_ref,
        release: test_data_ref,
        get_data: test_data_get,
    };

    unsafe extern "system" fn test_data_query(
        _this: *mut c_void,
        _iid: *const Guid,
        out: *mut *mut c_void,
    ) -> HResult {
        // SAFETY: `out` is the caller's.
        unsafe { *out = std::ptr::null_mut() };
        E_NOINTERFACE
    }

    /// Owned by test_drop for the calls: no counting.
    unsafe extern "system" fn test_data_ref(_this: *mut c_void) -> u32 {
        1
    }

    unsafe extern "system" fn test_data_get(
        this: *mut TestData,
        fmt: *const FormatEtc,
        out: *mut StgMedium,
    ) -> HResult {
        const DV_E_FORMATETC: HResult = 0x8004_0064_u32 as i32;
        const GMEM_MOVEABLE: u32 = 2;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GlobalAlloc(flags: u32, bytes: usize) -> *mut c_void;
        }
        // SAFETY: valid pointers for the call; the memory is filled within its size and handed
        // to the caller, who releases it.
        unsafe {
            let name: Vec<u16> = "UniformResourceLocatorW\0".encode_utf16().collect();
            let url_format = RegisterClipboardFormatW(name.as_ptr()) as u16;
            if (*fmt).cf != url_format && (*fmt).cf != CF_UNICODETEXT {
                return DV_E_FORMATETC;
            }
            let text = &(*this).text;
            let h = GlobalAlloc(GMEM_MOVEABLE, text.len() * 2);
            let p = GlobalLock(h) as *mut u16;
            if p.is_null() {
                return DV_E_FORMATETC;
            }
            std::ptr::copy_nonoverlapping(text.as_ptr(), p, text.len());
            GlobalUnlock(h);
            *out = StgMedium {
                tymed: TYMED_HGLOBAL,
                handle: h,
                unk_for_release: std::ptr::null_mut(),
            };
        }
        S_OK
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            // SAFETY: revoke OLE's reference, then ours.
            unsafe {
                RevokeDragDrop(self.hwnd);
                release(self.target);
            }
        }
    }
}

/// A named mutex held while the app runs, so the installer can tell (Inno Setup's `AppMutex`)
/// and wait for `/exit` to finish before replacing or removing the program. The handle closes
/// with the process.
pub fn mark_running() {
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = "MisterDownloadManager.Running\0".encode_utf16().collect();
    // SAFETY: NUL-terminated name; the handle is deliberately kept for the process's life.
    unsafe {
        CreateMutexW(std::ptr::null(), 0, name.as_ptr());
    }
}

// ---- Crash reports (crash.rs, mdm-crash) -------------------------------------------------------

static CRASH_HANDLER: std::sync::OnceLock<mdm_crash::crash_handler::CrashHandler> =
    std::sync::OnceLock::new();

/// Catch native crashes (access violations, stack overflows, … : Windows' structured exceptions)
/// and have `reporter`'s monitor dump this process when one comes. False when it can't attach.
pub fn attach_crash_handler(reporter: std::sync::Arc<mdm_crash::client::Reporter>) -> bool {
    use mdm_crash::crash_handler::{CrashEventResult, CrashHandler, make_crash_event};
    // SAFETY: the closure runs in the exception's context; it only sends the dump request over
    // the monitor's socket, when its connection is open (no allocation needed by minidumper's
    // request; before it is, the dump is refused and the process dies the system's way).
    let event = unsafe { make_crash_event(move |cc| CrashEventResult::Handled(reporter.dump(cc))) };
    match CrashHandler::attach(event) {
        Ok(h) => CRASH_HANDLER.set(h).is_ok(),
        Err(e) => {
            tracing::warn!("crash handler not attached: {e}");
            false
        }
    }
}

/// Have the monitor dump this process now (a panic, a freeze): every thread, this one asking.
/// The process goes on.
pub fn dump_now() -> bool {
    CRASH_HANDLER.get().is_some_and(|h| {
        matches!(
            h.simulate_exception(None),
            mdm_crash::crash_handler::CrashEventResult::Handled(true)
        )
    })
}

/// This thread's id as Windows (and so the dump) knows it.
pub fn thread_id() -> u32 {
    // SAFETY: a plain query.
    unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() }
}

/// Give this process's working set back to the system while the window is hidden (#72): the
/// pages go to the standby list, still in RAM, and come back on first touch (soft faults, no
/// disk). What Task Manager's "Memory" column shows for a tray app is this working set.
pub fn trim_working_set() {
    use windows_sys::Win32::System::ProcessStatus::EmptyWorkingSet;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: a plain call on this process's own pseudo-handle.
    let ok = unsafe { EmptyWorkingSet(GetCurrentProcess()) } != 0;
    if !ok {
        tracing::debug!("EmptyWorkingSet failed");
    }
}

/// Move `files` to the Recycle Bin, silently (no confirmation, no progress, no error box): an
/// archive and its volumes after extracting (#37), restorable from there. False when the shell
/// refused or stopped part way; the files left are kept.
pub fn to_recycle_bin(files: &[std::path::PathBuf]) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW,
        SHFileOperationW,
    };
    if files.is_empty() {
        return true;
    }
    // Whole paths, each ended by a NUL, the list by another (the shell's own format).
    let mut from: Vec<u16> = Vec::new();
    for f in files {
        let whole = std::path::absolute(f).unwrap_or_else(|_| f.clone());
        from.extend(whole.as_os_str().encode_wide());
        from.push(0);
    }
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as u16,
        ..Default::default()
    };
    // SAFETY: `op` points at `from`, a double-NUL-ended list alive for the call; no window.
    let r = unsafe { SHFileOperationW(&mut op) };
    r == 0 && op.fAnyOperationsAborted == 0
}

/// Thread `tid`'s processor time so far, and this process's page faults so far (soft ones
/// included: Windows counts no hard faults per process here): crash.rs's freeze notes.
pub fn thread_usage(tid: u32) -> Option<crate::crash::Usage> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetThreadTimes, OpenThread, THREAD_QUERY_LIMITED_INFORMATION,
    };
    // 100 ns units to ms.
    let ms = |t: FILETIME| (((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64) / 10_000;
    // SAFETY: the handle is checked and closed; the out-parameters are locals of the right size.
    unsafe {
        let h = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, tid);
        if h.is_null() {
            return None;
        }
        let z = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (z, z, z, z);
        let ok = GetThreadTimes(h, &mut created, &mut exited, &mut kernel, &mut user) != 0;
        CloseHandle(h);
        if !ok {
            return None;
        }
        let mut m: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        m.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let faults = (GetProcessMemoryInfo(GetCurrentProcess(), &mut m, m.cb) != 0)
            .then_some(m.PageFaultCount as u64);
        Some(crate::crash::Usage {
            cpu_ms: ms(kernel) + ms(user),
            faults,
            // Windows counts no page-ins per process here (macOS's, unix.rs).
            pageins: None,
        })
    }
}

/// Is process `pid` still running?
pub fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    // SAFETY: the handle is checked and closed.
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return false;
        }
        let running = WaitForSingleObject(h, 0) == WAIT_TIMEOUT;
        CloseHandle(h);
        running
    }
}

/// The command line in this windowed program (main.rs, `misterdownloadmanager-cli`'s
/// launcher started it): the console it was started from, for what it prints and reads and for
/// Ctrl+C. Input and output the launcher handed over stay as they are (a file, a pipe, the console
/// itself); only those it didn't have get the console's. With the launcher's process id, this
/// process ends when the launcher does first (killed): nothing else would wait for it.
pub fn cli_console(launcher: Option<u32>) {
    use windows_sys::Win32::Foundation::{
        GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
        STD_OUTPUT_HANDLE, SetStdHandle,
    };
    use windows_sys::Win32::System::Threading::{
        INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    let usable = |h: HANDLE| !h.is_null() && h != INVALID_HANDLE_VALUE;
    let ids = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE];
    // SAFETY: plain queries and settings of this process's standard handles; the names are
    // NUL-terminated, and the handles opened are kept for the process's life.
    unsafe {
        let given = ids.map(|id| GetStdHandle(id));
        if AttachConsole(ATTACH_PARENT_PROCESS) != 0 {
            for (id, h) in ids.into_iter().zip(given) {
                if usable(h) {
                    SetStdHandle(id, h);
                } else if !usable(GetStdHandle(id)) {
                    let name = if id == STD_INPUT_HANDLE {
                        "CONIN$\0"
                    } else {
                        "CONOUT$\0"
                    };
                    let name: Vec<u16> = name.encode_utf16().collect();
                    let console = CreateFileW(
                        name.as_ptr(),
                        GENERIC_READ | GENERIC_WRITE,
                        FILE_SHARE_READ | FILE_SHARE_WRITE,
                        std::ptr::null(),
                        OPEN_EXISTING,
                        0,
                        std::ptr::null_mut(),
                    );
                    if usable(console) {
                        SetStdHandle(id, console);
                    }
                }
            }
        }
        let watched = launcher.map_or(std::ptr::null_mut(), |pid| {
            OpenProcess(PROCESS_SYNCHRONIZE, 0, pid)
        });
        if !watched.is_null() {
            // A handle isn't Send; its value is, and the handle stays open.
            let watched = watched as usize;
            std::thread::spawn(move || {
                WaitForSingleObject(watched as HANDLE, INFINITE);
                std::process::exit(1);
            });
        }
    }
}

/// The OpenGL driver in its own words, for the log and the crash reports (#280): its renderer
/// (the graphics card) and version, which on Windows carries the driver's build (Intel's
/// "4.6.0 - Build 31.0.101.…", NVIDIA's "4.6.0 NVIDIA 5xx.xx"). Linux and macOS: unix.rs.
pub fn gl_driver(gl: &eframe::glow::Context) -> String {
    use eframe::glow::{self, HasContext as _};
    // SAFETY: called while eframe makes the app, with its context current on this thread;
    // glGetString only reads the driver's strings.
    let (renderer, version) = unsafe {
        (
            gl.get_parameter_string(glow::RENDERER),
            gl.get_parameter_string(glow::VERSION),
        )
    };
    format!("{renderer} ({version})")
}

/// The graphics adapters and their drivers' versions, as Direct3D 12 sees them (DXGI; the
/// version as wgpu writes it, "32.0.15.6224"), and whether each is a software renderer (WARP):
/// a remembered failure is tried again once they change (renderer.rs, #1205). Read without
/// starting Direct3D 12 on any of them. None when DXGI can't say.
pub fn graphics_drivers() -> Option<Vec<(String, String, bool)>> {
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIDevice, IDXGIFactory1,
    };
    use windows::core::Interface as _;
    // SAFETY: a DXGI factory and its adapters, asked for their descriptions and their drivers'
    // versions only; each COM object is released when dropped.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let mut list = Vec::new();
        // EnumAdapters1 fails past the last one (DXGI_ERROR_NOT_FOUND).
        for i in 0..32 {
            let Ok(adapter) = factory.EnumAdapters1(i) else {
                break;
            };
            let Ok(desc) = adapter.GetDesc1() else {
                continue;
            };
            let end = desc.Description.iter().position(|&c| c == 0);
            let name = String::from_utf16_lossy(&desc.Description[..end.unwrap_or(128)]);
            let version = adapter
                .CheckInterfaceSupport(&IDXGIDevice::IID)
                .map_or_else(
                    |_| String::new(),
                    |v| {
                        let v = v as u64;
                        format!(
                            "{}.{}.{}.{}",
                            (v >> 48) as u16,
                            (v >> 32) as u16,
                            (v >> 16) as u16,
                            v as u16
                        )
                    },
                );
            let software = desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0;
            list.push((name.trim().to_owned(), version, software));
        }
        Some(list)
    }
}

/// Windows' clipboard sequence number: it changes each time the clipboard does, and asking for
/// it neither opens the clipboard nor waits for its owner (clipboard.rs: the watch for copied
/// links reads the clipboard only when it changed).
pub fn clipboard_sequence() -> u32 {
    // SAFETY: takes nothing, returns a number.
    unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() }
}

/// Tests only: a clipboard apart from the person's at this computer (#1103).
///
/// Windows keeps one clipboard per window station; the person's is on the interactive one,
/// `WinSta0`. [`in_private_clipboard`] runs a test again in a process of its own that moves to
/// another window station (and a desktop on it) before it touches the clipboard: there the test
/// reads and writes with the real clipboard functions, arboard's and MDM's own, and the person's
/// clipboard is neither read nor written. Without a name, Windows makes (or opens) the window
/// station of the logon session, `Service-0x0-…$`, which shows nothing and whose clipboard no
/// program of the person's uses; the tests that use it run one at a time.
///
/// [`in_private_clipboard`]: private_clipboard::in_private_clipboard
#[cfg(test)]
pub mod private_clipboard {
    use std::ffi::c_void;

    /// Set in the process [`in_private_clipboard`] starts: the test runs there, and only there.
    const CHILD: &str = "MDM_TEST_PRIVATE_CLIPBOARD";

    #[link(name = "user32")]
    unsafe extern "system" {
        fn CreateWindowStationW(
            name: *const u16,
            flags: u32,
            access: u32,
            sa: *const c_void,
        ) -> *mut c_void;
        fn SetProcessWindowStation(winsta: *mut c_void) -> i32;
        fn GetProcessWindowStation() -> *mut c_void;
        fn GetUserObjectInformationW(
            object: *mut c_void,
            index: i32,
            info: *mut c_void,
            len: u32,
            needed: *mut u32,
        ) -> i32;
        fn CreateDesktopW(
            name: *const u16,
            device: *const u16,
            devmode: *const c_void,
            flags: u32,
            access: u32,
            sa: *const c_void,
        ) -> *mut c_void;
        fn SetThreadDesktop(desktop: *mut c_void) -> i32;
        fn GetThreadDesktop(thread: u32) -> *mut c_void;
        fn OpenClipboard(owner: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThreadId() -> u32;
        fn GetProcessHeap() -> *mut c_void;
        fn HeapValidate(heap: *mut c_void, flags: u32, block: *const c_void) -> i32;
    }

    const WINSTA_ALL_ACCESS: u32 = 0x37F;
    const GENERIC_ALL: u32 = 0x1000_0000;
    const UOI_NAME: i32 = 2;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// A window station's name (empty: unknown).
    fn station_name(winsta: *mut c_void) -> String {
        let mut buf = [0u16; 256];
        let mut needed = 0;
        // SAFETY: the buffer and its size in bytes; the name comes back NUL-terminated.
        let ok = unsafe {
            GetUserObjectInformationW(
                winsta,
                UOI_NAME,
                buf.as_mut_ptr().cast(),
                size_of_val(&buf) as u32,
                &mut needed,
            )
        };
        if ok == 0 {
            return String::new();
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len])
    }

    /// The test's window station and desktop (their handles, as numbers: every thread joins).
    pub struct PrivateClipboard {
        winsta: usize,
        desktop: usize,
    }

    impl PrivateClipboard {
        /// Move this process to a window station other than the one it started on (the
        /// person's), and this thread to a desktop on it. Panics, before anything uses the
        /// clipboard, unless both moved.
        fn enter() -> Self {
            assert!(
                std::env::var_os(CHILD).is_some(),
                "only in the process in_private_clipboard starts"
            );
            // SAFETY: takes nothing; the process's handle, not closed.
            let started_on = station_name(unsafe { GetProcessWindowStation() });
            // SAFETY: no name, no security attributes; the handle is checked.
            let winsta = unsafe {
                CreateWindowStationW(std::ptr::null(), 0, WINSTA_ALL_ACCESS, std::ptr::null())
            };
            assert!(
                !winsta.is_null(),
                "no window station: {}",
                std::io::Error::last_os_error()
            );
            let name = station_name(winsta);
            assert!(
                !name.is_empty() && name != started_on && !name.eq_ignore_ascii_case("WinSta0"),
                "window station {name:?}, started on {started_on:?}"
            );
            // SAFETY: the handle just made.
            let moved = unsafe { SetProcessWindowStation(winsta) };
            assert!(
                moved != 0,
                "process not moved: {}",
                std::io::Error::last_os_error()
            );
            let desk = wide(&format!("mdm-test-{}", std::process::id()));
            // SAFETY: a NUL-terminated name, no device, mode or security attributes; it is made
            // on the process's window station, the test's.
            let desktop = unsafe {
                CreateDesktopW(
                    desk.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    GENERIC_ALL,
                    std::ptr::null(),
                )
            };
            assert!(
                !desktop.is_null(),
                "no desktop: {}",
                std::io::Error::last_os_error()
            );
            let clip = PrivateClipboard {
                winsta: winsta as usize,
                desktop: desktop as usize,
            };
            clip.join();
            clip
        }

        /// Put the calling thread on the test's desktop: each thread that uses the clipboard,
        /// first. Panics unless the thread and its process are on the test's window station.
        pub fn join(&self) {
            // SAFETY: the desktop made in `enter`, open for the process's life; the handles
            // compared are only read.
            unsafe {
                let set = SetThreadDesktop(self.desktop as *mut c_void);
                assert!(
                    set != 0,
                    "thread not moved: {}",
                    std::io::Error::last_os_error()
                );
                assert_eq!(
                    GetThreadDesktop(GetCurrentThreadId()) as usize,
                    self.desktop
                );
                assert_eq!(GetProcessWindowStation() as usize, self.winsta);
            }
        }
    }

    /// Run `body` in a clipboard of its own: the test (`test`, its path in the crate as `cargo
    /// test` names it) runs again in a process of its own, which moves to a new window station
    /// first ([`PrivateClipboard`]), and passes when `body` passed there.
    pub fn in_private_clipboard(test: &str, body: impl FnOnce(&PrivateClipboard)) {
        if std::env::var_os(CHILD).is_some() {
            body(&PrivateClipboard::enter());
            return;
        }
        // One at a time: they share the logon session's window station, and its clipboard.
        static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _one = ONE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let exe = std::env::current_exe().expect("the test program");
        let out = std::process::Command::new(exe)
            .args([test, "--exact", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1")
            .output()
            .expect("the test's own process");
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success(), "{test}: {:?}\n{said}", out.status);
        assert!(said.contains("1 passed"), "{test} didn't run:\n{said}");
    }

    /// `OpenClipboard` without a window, as arboard opens it; whether it opened.
    pub fn open_without_window() -> bool {
        // SAFETY: no window; the caller closes it ([`close`]).
        unsafe { OpenClipboard(std::ptr::null_mut()) != 0 }
    }

    /// `CloseClipboard`, after [`open_without_window`].
    pub fn close() -> bool {
        // SAFETY: takes nothing.
        unsafe { CloseClipboard() != 0 }
    }

    /// Whether the process's heap (Rust's allocations and the clipboard's memory alike) checks
    /// out whole, every block of it.
    pub fn process_heap_is_whole() -> bool {
        // SAFETY: the process's own heap, checked whole (no block given).
        unsafe { HeapValidate(GetProcessHeap(), 0, std::ptr::null()) != 0 }
    }

    /// egui's clipboard, as a window's text boxes paste and copy with it (arboard, under
    /// egui_winit's `lock_system_clipboard`).
    pub fn egui_clipboard() -> egui_winit::clipboard::Clipboard {
        // SAFETY: no display handle to keep alive: on Windows egui's clipboard is arboard's.
        unsafe { egui_winit::clipboard::Clipboard::new(None) }
    }

    mod tests {
        use super::*;

        /// #629, #1103: why MDM's reads and writes of the clipboard, egui's copy and paste
        /// among them, take one lock (main.rs `clipboard_lock`). Opened without a window, as
        /// arboard opens it, the clipboard lets a second thread of the process in while the
        /// first still has it open, and the second's close closes it under the first: a read
        /// still under way then works on data the clipboard has let go of, and the heap is
        /// corrupted (STATUS_HEAP_CORRUPTION, on the "clipboard read" thread).
        #[test]
        fn two_threads_open_the_clipboard_at_once_without_a_window() {
            in_private_clipboard(
                "win32::private_clipboard::tests::two_threads_open_the_clipboard_at_once_without_a_window",
                |clip| {
                    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    while !open_without_window() {
                        assert!(std::time::Instant::now() < until, "never opened");
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    let (opened, closed) = std::thread::scope(|s| {
                        s.spawn(|| {
                            clip.join();
                            let opened = open_without_window();
                            (opened, opened && close())
                        })
                        .join()
                        .unwrap()
                    });
                    assert!(opened, "Windows kept the second thread out");
                    assert!(closed, "the second closes it");
                    assert!(!close(), "the first's was still open");
                },
            );
        }
    }
}

/// End process `pid` (a frozen MDM the user chose to close from the crash report).
pub fn end_process(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
    // SAFETY: the handle is checked and closed.
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if h.is_null() {
            return false;
        }
        let ok = TerminateProcess(h, 1) != 0;
        CloseHandle(h);
        ok
    }
}

/// The crash test's access violation (MDM_CRASH_TEST=1, `/show crash-test:crash`).
pub fn crash_now() {
    // SAFETY: none: a write through a null pointer, on purpose, to test the crash reports.
    unsafe { std::ptr::write_volatile(std::ptr::null_mut::<u32>(), 1) };
}

/// The names of the subkeys of `key` under `root`.
fn reg_subkeys(root: windows_sys::Win32::System::Registry::HKEY, key: &str) -> Vec<String> {
    use windows_sys::Win32::System::Registry::{
        HKEY, KEY_READ, RegCloseKey, RegEnumKeyExW, RegOpenKeyExW,
    };
    let wide: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
    let mut h: HKEY = std::ptr::null_mut();
    // SAFETY: opens a key for reading into `h`, closed below.
    if unsafe { RegOpenKeyExW(root, wide.as_ptr(), 0, KEY_READ, &mut h) } != 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0.. {
        let mut buf = [0u16; 256];
        let mut len = buf.len() as u32;
        // SAFETY: the subkey's name into `buf`, whose length in characters is in `len`.
        let rc = unsafe {
            RegEnumKeyExW(
                h,
                i,
                buf.as_mut_ptr(),
                &mut len,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if rc != 0 {
            break;
        }
        out.push(String::from_utf16_lossy(&buf[..len as usize]));
    }
    // SAFETY: `h` was opened above.
    unsafe { RegCloseKey(h) };
    out
}

/// The browsers installed, by name, with their program: those Windows lists for "default
/// browser" (StartMenuInternet, this user's then everyone's), named by their program
/// (mdm_ipc::browsers::windows_named: Helium's chrome.exe isn't Chrome) or else by their own
/// display name; then the known programs registered only under the shell's App Paths (a zip or
/// portable install). The browser extension's offer (extension.rs) opens its page in each.
pub fn browsers() -> Vec<(String, std::path::PathBuf)> {
    use mdm_ipc::browsers::{command_program, windows_can_run_extension, windows_named};
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    const CLIENTS: &str = r"Software\Clients\StartMenuInternet";
    const APP_PATHS: &str = r"Software\Microsoft\Windows\CurrentVersion\App Paths";
    let mut out: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut add = |name: String, path: std::path::PathBuf| {
        let same = |p: &std::path::PathBuf| {
            p.to_string_lossy()
                .eq_ignore_ascii_case(&path.to_string_lossy())
        };
        if path.is_file() && !out.iter().any(|(n, p)| same(p) || *n == name) {
            out.push((name, path));
        }
    };
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        for sub in reg_subkeys(root, CLIENTS) {
            let key = format!(r"{CLIENTS}\{sub}");
            let Some(program) = reg_string_in(root, &format!(r"{key}\shell\open\command"), "")
                .as_deref()
                .and_then(command_program)
            else {
                continue;
            };
            if !windows_can_run_extension(&program) {
                continue;
            }
            let name = windows_named(&program)
                .map(str::to_owned)
                .or_else(|| reg_string_in(root, &key, "").filter(|n| !n.trim().is_empty()));
            add(name.unwrap_or(sub), std::path::PathBuf::from(program));
        }
    }
    for (exe, _, _) in mdm_ipc::browsers::WINDOWS_PROGRAMS {
        let key = format!(r"{APP_PATHS}\{exe}");
        let Some(path) = reg_string_in(HKEY_CURRENT_USER, &key, "")
            .or_else(|| reg_string_in(HKEY_LOCAL_MACHINE, &key, ""))
        else {
            continue;
        };
        let path = path.trim().trim_matches('"').to_owned();
        if let Some(name) = windows_named(&path) {
            add(name.to_owned(), std::path::PathBuf::from(path));
        }
    }
    out
}

/// Open `url` in the browser whose program is `program` (from `browsers`).
pub fn open_in(program: &std::path::Path, url: &str) {
    let program = program.to_owned();
    let url = url.to_owned();
    crate::launch::spawn("open browser", move || {
        let _ = std::process::Command::new(program).arg(url).spawn();
    });
}

/// Windows' Group Policy "Do not preserve zone information in file attachments" (#813), as
/// (the computer's, the user's) `SaveZoneInformation` values: 1 keeps no zone information, 2
/// keeps it; None where it isn't set. Browsers save their downloads through Windows'
/// attachment services, which follow it; `webmark::keeps_zones` decides the same way. Two
/// registry reads: for a thread other than the window's (the download's, as it starts).
pub fn zone_policy() -> (Option<u32>, Option<u32>) {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\Attachments";
    const NAME: &str = "SaveZoneInformation";
    (
        reg_dword_in(HKEY_LOCAL_MACHINE, KEY, NAME),
        reg_dword_in(HKEY_CURRENT_USER, KEY, NAME),
    )
}

#[cfg(test)]
mod zone_policy_tests {
    use super::*;
    use windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE;

    /// The policy's values are read as Windows keeps them (a DWORD under either root), and one
    /// that isn't set reads as none. The policy key itself is the administrators' to write, even
    /// the user's: a DWORD every Windows 10 and 11 has stands in for it.
    #[test]
    fn a_dword_is_read_and_a_missing_value_is_none() {
        let key = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
        assert_eq!(
            reg_dword_in(HKEY_LOCAL_MACHINE, key, "CurrentMajorVersionNumber"),
            Some(10)
        );
        assert_eq!(
            reg_dword_in(HKEY_LOCAL_MACHINE, key, "NoSuchValueOfMdm"),
            None
        );
        // A string isn't taken for a number.
        assert_eq!(reg_dword_in(HKEY_LOCAL_MACHINE, key, "ProductName"), None);
        // Whatever this PC's policy is, it is read without failing.
        let _ = zone_policy();
    }
}

/// Mark-of-the-Web on a file being downloaded from `url` (`referrer`'s page): the zone stream
/// browsers write, so SmartScreen checks a program, Office opens a document in Protected View and
/// archive tools pass the mark on, as for the browser's own download. Written on the part file
/// as it is created (the engine renames it, the stream going along). Through `webmark::mark`,
/// which asks the option and Windows' policy first.
pub fn mark_downloaded(path: &std::path::Path, url: &str, referrer: Option<&str>) {
    // Only addresses from the web: never a login's password (the engine has none in `url`).
    let clean = |u: &str| u.split(['\r', '\n']).next().unwrap_or("").to_owned();
    let mut text = String::from("[ZoneTransfer]\r\nZoneId=3\r\n");
    if let Some(r) = referrer.filter(|r| r.starts_with("http")) {
        text.push_str(&format!("ReferrerUrl={}\r\n", clean(r)));
    }
    text.push_str(&format!("HostUrl={}\r\n", clean(url)));
    let mut stream = path.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    let _ = std::fs::write(stream, text);
}

/// Rename `from` to `to` without ever replacing an existing entry (`MoveFileExW` without
/// `MOVEFILE_REPLACE_EXISTING`, returning `ErrorKind::AlreadyExists` when `to` is already there).
pub fn rename_no_replace(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
    let encode = |p: &std::path::Path| -> std::io::Result<Vec<u16>> {
        let mut wide: Vec<u16> = p.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "interior NUL",
            ));
        }
        wide.push(0);
        Ok(wide)
    };
    let from_w = encode(from)?;
    let to_w = encode(to)?;
    // SAFETY: both UTF-16 buffers are NUL-terminated and live through the call; flags = 0 refuses
    // cross-volume copies and refuses replacing an existing destination.
    let ok = unsafe { MoveFileExW(from_w.as_ptr(), to_w.as_ptr(), 0) };
    if ok != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// What each browser's registry entries for the native host lead to: the user's (HKCU) and the
/// computer's (HKLM), for every key the browsers read (`mdm_ipc::browsers::WINDOWS_HOST_KEYS`).
fn host_holders() -> Vec<(
    Option<mdm_ipc::browsers::Holder>,
    Option<mdm_ipc::browsers::Holder>,
)> {
    use mdm_ipc::browsers::{HOST_NAME, Holder, WINDOWS_HOST_KEYS, registered_manifest};
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    WINDOWS_HOST_KEYS
        .iter()
        .map(|(key, _)| {
            let entry = format!(r"{key}\{HOST_NAME}");
            let holder = |root| {
                reg_string_in(root, &entry, "").map(|v| Holder::of(&registered_manifest(&v)))
            };
            (holder(HKEY_CURRENT_USER), holder(HKEY_LOCAL_MACHINE))
        })
        .collect()
}

/// Where browser integration stands: what a browser looking for the native host finds, for
/// every key the browsers read: the user's entry (HKCU), or without one the computer's (HKLM),
/// as the browsers read them (Chromium's launch_context_win.cc). One naming a manifest that is
/// gone (a copy run from inside a zip, #818) or a helper program that isn't there is the
/// problem, whatever the others say (Options → General → Test browser integration).
pub fn host_registration() -> crate::extension::Registration {
    use crate::extension::Registration;
    use mdm_ipc::browsers::Holder;
    let mut ready = false;
    let mut dead = None;
    for (user, machine) in host_holders() {
        match user.or(machine) {
            Some(Holder::Alive(_)) => ready = true,
            Some(gone) => dead = dead.or(Some(gone)),
            None => {}
        }
    }
    match dead {
        Some(Holder::ManifestGone(manifest)) => Registration::Gone(manifest),
        Some(Holder::ProgramGone(program)) => Registration::HelperMissing(program),
        _ if ready => Registration::Ready,
        _ => Registration::Missing,
    }
}

/// At each start, on a thread of its own (#818): when a browser's registration leads nowhere
/// (`mdm_ipc::browsers::windows_repair`: its manifest or helper gone, such as a copy run from
/// inside a zip whose temporary folder was deleted), the native host's `--repair` points it at
/// this copy, for this user. Nothing runs while every one works or there is none: one that works
/// naming another copy is that copy's, and none is the setup's box unticked.
pub fn repair_browsers() {
    use mdm_ipc::browsers::{Repair, windows_repair};
    let started = std::thread::Builder::new()
        .name("repair-browsers".into())
        .spawn(|| {
            let needed = host_holders().iter().any(|(user, machine)| {
                windows_repair(user.as_ref(), machine.as_ref()) != Repair::Keep
            });
            if !needed {
                return;
            }
            let result = std::env::current_exe()
                .map_err(|e| e.to_string())
                .and_then(|exe| {
                    crate::extension::run_host(
                        &exe.with_file_name("misterdownloadmanager-nmhost.exe"),
                        "--repair",
                        crate::extension::SET_UP_LIMIT,
                    )
                });
            crate::extension::registered_as(&result);
        });
    if let Err(e) = started {
        tracing::warn!("browser integration: not repaired, no thread for it: {e}");
    }
}

/// `cmd` runs without a console window of its own (the native host is a console program).
pub fn no_console(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    // CREATE_NO_WINDOW
    cmd.creation_flags(0x0800_0000);
}

/// Options → Sites Logins' passwords, encrypted for this Windows account (DPAPI) before the list
/// is written: another account, or a copy of the file, can't read them.
#[allow(unsafe_code)]
pub fn protect(plain: &str) -> Option<Vec<u8>> {
    use windows_sys::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptProtectData};
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(plain.len()).ok()?,
        pbData: plain.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: `input` points at `plain` for the call (only read); `output` is filled by the
    // system with a buffer we copy and then free with LocalFree, as CryptProtectData requires.
    let ok = unsafe {
        CryptProtectData(
            &input,
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut output,
        )
    };
    blob_bytes(ok, output)
}

/// The password `protect` encrypted, for this account.
#[allow(unsafe_code)]
pub fn unprotect(sealed: &[u8]) -> Option<String> {
    use windows_sys::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptUnprotectData};
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(sealed.len()).ok()?,
        pbData: sealed.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: as in `protect`.
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut output,
        )
    };
    String::from_utf8(blob_bytes(ok, output)?).ok()
}

/// A DPAPI result's bytes, its buffer freed.
#[allow(unsafe_code)]
fn blob_bytes(
    ok: i32,
    blob: windows_sys::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB,
) -> Option<Vec<u8>> {
    if ok == 0 || blob.pbData.is_null() {
        return None;
    }
    // SAFETY: the system gave `cbData` bytes at `pbData`, ours until LocalFree.
    let bytes = unsafe { std::slice::from_raw_parts(blob.pbData, blob.cbData as usize) }.to_vec();
    // SAFETY: the buffer came from DPAPI, which allocates with LocalAlloc.
    unsafe { windows_sys::Win32::Foundation::LocalFree(blob.pbData.cast()) };
    Some(bytes)
}

// Sign-in secrets in Windows Credential Manager (#44, secrets.rs): generic credentials named
// `<service>/<account>`, for this Windows user, roaming with the profile as MDM's data folder
// (%APPDATA%) does. Credential Manager (Control Panel → Credential Manager → Windows
// Credentials) shows them, the label as their user name, and can remove them.

fn cred_target(service: &str, account: &str) -> Vec<u16> {
    format!("{service}/{account}")
        .encode_utf16()
        .chain(Some(0))
        .collect()
}

/// The last error as a fault: no logon session (a service account) means no keychain.
#[allow(unsafe_code)]
fn cred_fault(what: &str) -> crate::secrets::Fault {
    use crate::secrets::Fault;
    use windows_sys::Win32::Foundation::{ERROR_NO_SUCH_LOGON_SESSION, GetLastError};
    // SAFETY: no arguments; reads this thread's last error.
    let code = unsafe { GetLastError() };
    if code == ERROR_NO_SUCH_LOGON_SESSION {
        Fault::Missing
    } else {
        Fault::Failed(format!("{what}: Windows error {code}"))
    }
}

#[allow(unsafe_code)]
fn cred_not_found() -> bool {
    use windows_sys::Win32::Foundation::{ERROR_NOT_FOUND, GetLastError};
    // SAFETY: no arguments.
    unsafe { GetLastError() == ERROR_NOT_FOUND }
}

/// Whether Credential Manager answers (listing MDM's own entries).
pub fn keychain_probe(service: &str) -> Result<(), crate::secrets::Fault> {
    keychain_list(service).map(|_| ())
}

#[allow(unsafe_code)]
pub fn keychain_read(
    service: &str,
    account: &str,
) -> Result<Option<String>, crate::secrets::Fault> {
    use windows_sys::Win32::Security::Credentials::{
        CRED_TYPE_GENERIC, CREDENTIALW, CredFree, CredReadW,
    };
    let target = cred_target(service, account);
    let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `target` is a NUL-terminated UTF-16 string alive for the call; `cred` receives a
    // buffer the system allocated, freed below with CredFree.
    let ok = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) };
    if ok == 0 {
        return if cred_not_found() {
            Ok(None)
        } else {
            Err(cred_fault("CredReadW"))
        };
    }
    // SAFETY: on success `cred` points at a CREDENTIALW whose blob holds `CredentialBlobSize`
    // bytes; both are ours until CredFree.
    let bytes = unsafe {
        let c = &*cred;
        let bytes = if c.CredentialBlob.is_null() {
            Vec::new()
        } else {
            std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec()
        };
        CredFree(cred.cast());
        bytes
    };
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| crate::secrets::Fault::Failed("not UTF-8".into()))
}

/// Stored as UTF-8. Credential Manager takes at most 2560 bytes: a longer secret stays in the
/// settings, sealed with DPAPI.
#[allow(unsafe_code)]
pub fn keychain_write(
    service: &str,
    account: &str,
    label: &str,
    secret: &str,
) -> Result<(), crate::secrets::Fault> {
    use windows_sys::Win32::Security::Credentials::{
        CRED_MAX_CREDENTIAL_BLOB_SIZE, CRED_PERSIST_ENTERPRISE, CRED_TYPE_GENERIC, CREDENTIALW,
        CredWriteW,
    };
    if secret.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
        return Err(crate::secrets::Fault::Failed(
            "longer than Credential Manager takes".into(),
        ));
    }
    let mut target = cred_target(service, account);
    let mut user: Vec<u16> = label.encode_utf16().chain(Some(0)).collect();
    let mut blob = secret.as_bytes().to_vec();
    // SAFETY: all-zero is a valid CREDENTIALW (null pointers, zero sizes).
    let mut cred: CREDENTIALW = unsafe { std::mem::zeroed() };
    cred.Type = CRED_TYPE_GENERIC;
    cred.TargetName = target.as_mut_ptr();
    cred.UserName = user.as_mut_ptr();
    cred.CredentialBlobSize = blob.len() as u32;
    cred.CredentialBlob = blob.as_mut_ptr();
    cred.Persist = CRED_PERSIST_ENTERPRISE;
    // SAFETY: every pointer in `cred` points at a buffer alive for the call (only read).
    let ok = unsafe { CredWriteW(&cred, 0) };
    let fault = (ok == 0).then(|| cred_fault("CredWriteW"));
    // The plain copy doesn't linger in this buffer.
    blob.iter_mut().for_each(|b| *b = 0);
    fault.map_or(Ok(()), Err)
}

#[allow(unsafe_code)]
pub fn keychain_delete(service: &str, account: &str) -> Result<(), crate::secrets::Fault> {
    use windows_sys::Win32::Security::Credentials::{CRED_TYPE_GENERIC, CredDeleteW};
    let target = cred_target(service, account);
    // SAFETY: `target` is NUL-terminated and alive for the call.
    let ok = unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) };
    if ok == 0 && !cred_not_found() {
        return Err(cred_fault("CredDeleteW"));
    }
    Ok(())
}

/// The accounts of `service`'s generic credentials (the filter `<service>/*`: only MDM's; the
/// others' blobs are never read).
#[allow(unsafe_code)]
pub fn keychain_list(service: &str) -> Result<Vec<String>, crate::secrets::Fault> {
    use windows_sys::Win32::Security::Credentials::{
        CRED_TYPE_GENERIC, CREDENTIALW, CredEnumerateW, CredFree,
    };
    let filter: Vec<u16> = format!("{service}/*")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut count = 0u32;
    let mut creds: *mut *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `filter` is NUL-terminated; `creds` receives an array of `count` pointers the
    // system allocated, freed below with CredFree.
    let ok = unsafe { CredEnumerateW(filter.as_ptr(), 0, &mut count, &mut creds) };
    if ok == 0 {
        return if cred_not_found() {
            Ok(Vec::new())
        } else {
            Err(cred_fault("CredEnumerateW"))
        };
    }
    let prefix = format!("{service}/");
    let mut accounts = Vec::new();
    // SAFETY: on success `creds` holds `count` valid CREDENTIALW pointers, ours until CredFree;
    // each TargetName is a NUL-terminated UTF-16 string.
    unsafe {
        for i in 0..count as usize {
            let c = &**creds.add(i);
            if c.Type != CRED_TYPE_GENERIC || c.TargetName.is_null() {
                continue;
            }
            let mut n = 0;
            while *c.TargetName.add(n) != 0 {
                n += 1;
            }
            let name = String::from_utf16_lossy(std::slice::from_raw_parts(c.TargetName, n));
            if let Some(account) = name.strip_prefix(&prefix) {
                accounts.push(account.to_owned());
            }
        }
        CredFree(creds.cast());
    }
    Ok(accounts)
}

// ---- waking the computer for a queue (#142) -----------------------------------------------

/// Windows' "Allow wake timers" power option for the active plan (0 off, 1 on, 2 important
/// wake timers only), on mains power (`ac`) and on battery (`dc`), and the power source now.
pub struct WakeTimers {
    pub ac: u32,
    pub dc: u32,
    pub on_battery: bool,
    pub has_battery: bool,
}

/// Power Options → Sleep → Allow wake timers, as `powercfg /q SCHEME_CURRENT SUB_SLEEP
/// RTCWAKE` shows it. Read only; a setting that can't be read counts as on (nothing to warn of).
pub fn wake_timers() -> WakeTimers {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::System::Power::{
        GetSystemPowerStatus, PowerGetActiveScheme, PowerReadACValueIndex, PowerReadDCValueIndex,
        SYSTEM_POWER_STATUS,
    };
    use windows_sys::core::GUID;
    const SUB_SLEEP: GUID = GUID::from_u128(0x238c9fa8_0aad_41ed_83f4_97be242c8f20);
    const RTCWAKE: GUID = GUID::from_u128(0xbd3b718a_0680_4d9d_8ab2_e1d2b4ac806d);
    let (mut ac, mut dc) = (1u32, 1u32);
    let mut scheme: *mut GUID = std::ptr::null_mut();
    // SAFETY: PowerGetActiveScheme allocates the GUID it returns (freed with LocalFree below);
    // the reads only write the u32 given.
    unsafe {
        if PowerGetActiveScheme(std::ptr::null_mut(), &mut scheme) == 0 && !scheme.is_null() {
            let mut v = 0u32;
            if PowerReadACValueIndex(std::ptr::null_mut(), scheme, &SUB_SLEEP, &RTCWAKE, &mut v)
                == 0
            {
                ac = v;
            }
            if PowerReadDCValueIndex(std::ptr::null_mut(), scheme, &SUB_SLEEP, &RTCWAKE, &mut v)
                == 0
            {
                dc = v;
            }
            LocalFree(scheme.cast());
        }
    }
    // SAFETY: plain data, zero is valid; a plain query into it.
    let mut st: SYSTEM_POWER_STATUS = unsafe { zeroed() };
    let ok = unsafe { GetSystemPowerStatus(&mut st) } != 0;
    WakeTimers {
        ac,
        dc,
        // ACLineStatus 0: on battery; BatteryFlag 128: no system battery, 255: unknown.
        on_battery: ok && st.ACLineStatus == 0,
        has_battery: ok && st.BatteryFlag != 128 && st.BatteryFlag != 255,
    }
}

/// Power Options' advanced settings, where "Allow wake timers" is (under Sleep). Never from an
/// off-screen test run: its windows belong to the person at the computer.
pub fn open_power_options() {
    if crate::offscreen() {
        return;
    }
    use std::os::windows::process::CommandExt;
    crate::launch::spawn("power options", move || {
        let _ = std::process::Command::new("control.exe")
            .arg("powercfg.cpl,,3")
            .creation_flags(0x0800_0000)
            .spawn();
    });
}

/// A change to the wake timer, and where its result goes.
struct WakeCmd {
    /// When (seconds since 1970, UTC) and for which queue; None cancels.
    at: Option<(i64, String)>,
    reply: std::sync::mpsc::Sender<Result<(), String>>,
}

/// Wake the computer at `at` (seconds since 1970, UTC; for the queue named), or cancel the wake
/// (None). A waitable timer set to resume the computer (SetWaitableTimer, fResume): it lives in
/// MDM's process (Windows cancels it if the thread that set it ends, so one thread of its own
/// keeps it), lists in `powercfg /waketimers`, and goes with MDM: nothing is left behind when
/// MDM quits or is uninstalled.
pub fn wake_at(at: Option<(i64, &str)>) -> Result<(), String> {
    use std::sync::{Mutex, OnceLock, mpsc};
    static THREAD: OnceLock<Mutex<mpsc::Sender<WakeCmd>>> = OnceLock::new();
    let tx = THREAD.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("mdm-wake".into())
            .spawn(move || wake_thread(rx));
        Mutex::new(tx)
    });
    let (reply, answer) = mpsc::channel();
    let cmd = WakeCmd {
        at: at.map(|(t, q)| (t, q.to_owned())),
        reply,
    };
    let sent = tx.lock().map(|tx| tx.send(cmd).is_ok()).unwrap_or(false);
    let lost = || "the wake timer's thread is gone".to_owned();
    if !sent {
        return Err(lost());
    }
    answer
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap_or_else(|_| Err(lost()))
}

fn wake_thread(rx: std::sync::mpsc::Receiver<WakeCmd>) {
    use std::sync::mpsc::RecvTimeoutError;
    use windows_sys::Win32::Foundation::{ERROR_NOT_SUPPORTED, GetLastError, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        CancelWaitableTimer, CreateWaitableTimerExW, SetWaitableTimer, TIMER_ALL_ACCESS,
        WaitForSingleObject,
    };
    // A synchronization timer (auto-reset): the check below sees each firing once.
    // SAFETY: no name, default security; the handle is this thread's for the process's life.
    let timer =
        unsafe { CreateWaitableTimerExW(std::ptr::null(), std::ptr::null(), 0, TIMER_ALL_ACCESS) };
    let mut queue = String::new();
    loop {
        match rx.recv_timeout(std::time::Duration::from_secs(1)) {
            Ok(cmd) => {
                let result = if timer.is_null() {
                    // SAFETY: a plain query.
                    Err(format!("CreateWaitableTimerExW failed ({})", unsafe {
                        GetLastError()
                    }))
                } else if let Some((unix, q)) = cmd.at {
                    // FILETIME: 100 ns steps since 1601, UTC (positive: an absolute time).
                    let due: i64 = (unix + 11_644_473_600) * 10_000_000;
                    // SAFETY: our timer; no completion routine; fResume = TRUE. GetLastError
                    // right after, as the call's documentation asks.
                    let (ok, err) = unsafe {
                        let ok = SetWaitableTimer(timer, &due, 0, None, std::ptr::null(), 1) != 0;
                        (ok, GetLastError())
                    };
                    queue = q;
                    if !ok {
                        Err(format!("SetWaitableTimer failed ({err})"))
                    } else if err == ERROR_NOT_SUPPORTED {
                        // Set, but this computer can't be woken by it.
                        Err(crate::i18n::t(
                            // i18n: the Scheduler, under "Wake the computer to run this queue": Windows set the timer but can't wake this computer with it
                            "This computer can't be woken by a timer",
                        )
                        .to_owned())
                    } else {
                        Ok(())
                    }
                } else {
                    // SAFETY: our timer.
                    unsafe { CancelWaitableTimer(timer) };
                    Ok(())
                };
                let _ = cmd.reply.send(result);
            }
            Err(RecvTimeoutError::Timeout) => {
                // SAFETY: our timer; a zero wait only looks.
                if !timer.is_null() && unsafe { WaitForSingleObject(timer, 0) } == WAIT_OBJECT_0 {
                    tracing::info!("wake: the wake timer went off (queue {queue:?})");
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Keep the computer from sleeping while `reason` is given (a power request, listed with its
/// reason in `powercfg /requests`); None lets it sleep again as its power plan says. False when
/// Windows refused.
pub fn keep_awake(reason: Option<&str>) -> bool {
    use std::sync::Mutex;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Power::{
        PowerClearRequest, PowerCreateRequest, PowerRequestSystemRequired, PowerSetRequest,
    };
    use windows_sys::Win32::System::Threading::{
        POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT,
    };
    static REQUEST: Mutex<Option<isize>> = Mutex::new(None);
    let mut held = REQUEST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(h) = held.take() {
        // SAFETY: the request made below, cleared and closed once.
        unsafe {
            PowerClearRequest(h as _, PowerRequestSystemRequired);
            CloseHandle(h as _);
        }
    }
    let Some(reason) = reason else {
        return true;
    };
    let mut text: Vec<u16> = reason.encode_utf16().chain(Some(0)).collect();
    // SAFETY: plain data, zero is valid.
    let mut ctx: REASON_CONTEXT = unsafe { zeroed() };
    // POWER_REQUEST_CONTEXT_VERSION is 0.
    ctx.Version = 0;
    ctx.Flags = POWER_REQUEST_CONTEXT_SIMPLE_STRING;
    ctx.Reason.SimpleReasonString = text.as_mut_ptr();
    // SAFETY: `ctx` and the string it points to live through the call (Windows copies it).
    let h = unsafe { PowerCreateRequest(&ctx) };
    if h.is_null() || h as isize == -1 {
        tracing::warn!("wake: PowerCreateRequest failed");
        return false;
    }
    // SAFETY: the request just made; closed if it can't be set.
    if unsafe { PowerSetRequest(h, PowerRequestSystemRequired) } == 0 {
        tracing::warn!("wake: PowerSetRequest failed");
        unsafe { CloseHandle(h) };
        return false;
    }
    *held = Some(h as isize);
    true
}

/// Latin, Greek and Cyrillic text as Windows' classic controls draw it, in grey (#262, #323,
/// #334): each letter's shape and width come from the font's own hinting for this size, through
/// DirectWrite's GDI-classic mode. Before, egui placed letters by their unhinted widths rounded
/// to whole pixels while the letters themselves were fitted to pixels: the two disagreed by up to
/// a pixel, so letters touched in one place ("download") and stood apart in another ("Resum e").
///
/// `smooth` picks how a letter is drawn: `false`, the classic controls' look (hinted in both
/// directions at a whole pixel size, letters on whole pixels: crisp, and coarse on a large,
/// dense screen), or `true`, DirectWrite's natural look (the size as it is, hinted up and down
/// only, each letter where it falls between two pixels, with all 256 levels of grey: the text of
/// Windows' own modern apps, much easier to read at 1440p).
///
/// `None` from DirectWrite's factory or a font's file keeps egui's rasterizer. A cluster it isn't
/// for ([`classic_char`]) is left to egui too, which shapes it. The rasterizer is told the
/// interface's direction: a bracket in a right-to-left line is mirrored by the shaper, so those
/// stay with egui there.
pub fn text_rasterizer(
    font_dir: std::path::PathBuf,
    rtl: bool,
    smooth: bool,
) -> Option<egui::epaint::text::GlyphRasterizer> {
    let glyphs = classic_text::Glyphs::new(font_dir)
        .map_err(|e| tracing::warn!("text: DirectWrite unavailable, egui draws the text ({e})"))
        .ok()?;
    let glyphs = std::sync::Mutex::new(glyphs);
    Some(
        egui::epaint::text::GlyphRasterizer::new(
            "mdm::win32::classic_text",
            move |req: &egui::epaint::text::GlyphRasterizerRequest<'_>| {
                glyphs.lock().ok()?.glyph(req, rtl, smooth)
            },
        )
        .with_priority(egui::epaint::text::FontPriority::Highest),
    )
}

/// Does [`text_rasterizer`] draw this character? One code point that needs no shaping: Latin,
/// Greek and Cyrillic letters, digits and punctuation, and the CJK characters whose face comes
/// from the interface language's script font. Scripts whose letters change shape (Arabic, Indic,
/// ...) and the invisible marks are left to egui and its shaper; so are the brackets of a
/// right-to-left line, which the shaper mirrors.
fn classic_char(c: char, rtl: bool) -> bool {
    if rtl
        && matches!(
            c,
            '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | '«' | '»' | '‹' | '›'
        )
    {
        return false;
    }
    matches!(c,
        ' '..='~'
        | '\u{A0}'..='\u{AC}' // Latin-1, without the soft hyphen
        | '\u{AE}'..='\u{2AF}' // Latin Extended and IPA
        | '\u{370}'..='\u{3FF}' // Greek
        | '\u{400}'..='\u{52F}' // Cyrillic
        | '\u{1E00}'..='\u{1EFF}' // Vietnamese and other precomposed Latin
        | '\u{2010}'..='\u{2027}' // dashes, quotes, bullets, ellipsis
        | '\u{2030}'..='\u{205E}' // per mille, primes, guillemets
        | '\u{20A0}'..='\u{20BF}' // currency
    ) || crate::theme::cjk_script(c).is_some()
}

/// The font file of a family `theme.rs` sets up (Segoe UI leads the two built-in ones).
fn classic_font_file(family: &egui::FontFamily) -> Option<&'static str> {
    match family {
        egui::FontFamily::Proportional | egui::FontFamily::Monospace => Some("segoeui.ttf"),
        egui::FontFamily::Name(name) => match &**name {
            "tahoma" => Some("tahoma.ttf"),
            "tahoma-bold" => Some("tahomabd.ttf"),
            "arial-bold" => Some("arialbd.ttf"),
            _ => None,
        },
    }
}

mod classic_text {
    use std::collections::HashMap;
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::PathBuf;

    use egui::epaint::text::{GlyphBitmap, GlyphRasterizerRequest, RasterizedGlyph};
    use egui::epaint::{Color32, ColorImage};
    use windows::Win32::Graphics::DirectWrite::{
        DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_FACE_TYPE_TRUETYPE,
        DWRITE_FONT_FACE_TYPE_TRUETYPE_COLLECTION, DWRITE_FONT_METRICS,
        DWRITE_FONT_SIMULATIONS_NONE, DWRITE_GLYPH_METRICS, DWRITE_GLYPH_OFFSET, DWRITE_GLYPH_RUN,
        DWRITE_GRID_FIT_MODE_ENABLED, DWRITE_MEASURING_MODE_GDI_CLASSIC,
        DWRITE_MEASURING_MODE_NATURAL, DWRITE_RENDERING_MODE_GDI_CLASSIC,
        DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC, DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
        DWRITE_TEXTURE_ALIASED_1x1, DWriteCreateFactory, IDWriteFactory2, IDWriteFontFace,
    };

    /// How much the smooth look thins its edges: 1.0 keeps DirectWrite's coverage as it is.
    const SMOOTH_COVERAGE_GAMMA: f32 = 1.8;

    pub struct Glyphs {
        factory: IDWriteFactory2,
        dir: PathBuf,
        /// The font files opened so far; `None` for one DirectWrite can't use.
        faces: HashMap<&'static str, Option<IDWriteFontFace>>,
    }

    // SAFETY: DirectWrite's shared factory and the objects it makes are free-threaded
    // (documented thread-safe); `Glyphs` is only used behind a Mutex.
    unsafe impl Send for Glyphs {}

    impl Glyphs {
        pub fn new(dir: PathBuf) -> windows::core::Result<Glyphs> {
            // SAFETY: a plain COM call.
            let factory: IDWriteFactory2 =
                unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
            Ok(Glyphs {
                factory,
                dir,
                faces: HashMap::new(),
            })
        }

        fn face(&mut self, file: &'static str) -> Option<IDWriteFontFace> {
            let (factory, dir) = (&self.factory, &self.dir);
            self.faces
                .entry(file)
                .or_insert_with(|| {
                    let full = dir.join(file);
                    // A .ttc holds a collection: the face egui draws with too (#1483).
                    let collection = file.to_ascii_lowercase().ends_with(".ttc");
                    let index = if collection {
                        crate::theme::font_face_index(&full)
                    } else {
                        0
                    };
                    let path: Vec<u16> = full.as_os_str().encode_wide().chain([0]).collect();
                    // SAFETY: `path` is NUL-terminated and outlives the calls; plain COM calls.
                    unsafe {
                        let font = factory
                            .CreateFontFileReference(windows::core::PCWSTR(path.as_ptr()), None)
                            .ok()?;
                        factory
                            .CreateFontFace(
                                if collection {
                                    DWRITE_FONT_FACE_TYPE_TRUETYPE_COLLECTION
                                } else {
                                    DWRITE_FONT_FACE_TYPE_TRUETYPE
                                },
                                &[Some(font)],
                                index,
                                DWRITE_FONT_SIMULATIONS_NONE,
                            )
                            .ok()
                    }
                })
                .clone()
        }

        /// The face and glyph id for `ch`: the family's own face for the scripts it draws, or the
        /// interface language's CJK face for Chinese, Japanese and Korean text. None when no face
        /// has a glyph for it: egui and its shaper draw it instead.
        fn lookup(
            &mut self,
            family: &egui::FontFamily,
            ch: char,
        ) -> Option<(IDWriteFontFace, u16, DWRITE_FONT_METRICS)> {
            let files: Vec<&'static str> = if crate::theme::cjk_script(ch).is_some() {
                crate::theme::cjk_font_files(ch)
            } else {
                super::classic_font_file(family).into_iter().collect()
            };
            for file in files {
                let Some(face) = self.face(file) else {
                    continue;
                };
                let mut id = 0u16;
                // SAFETY: the pointer is to one element, the count given.
                if unsafe { face.GetGlyphIndices(&(ch as u32), 1, &mut id) }.is_err() {
                    continue;
                }
                if id == 0 {
                    continue;
                }
                let mut font = DWRITE_FONT_METRICS::default();
                // SAFETY: the pointer is to one element.
                unsafe {
                    face.GetMetrics(&mut font);
                }
                return Some((face, id, font));
            }
            None
        }

        pub fn glyph(
            &mut self,
            req: &GlyphRasterizerRequest<'_>,
            rtl: bool,
            smooth: bool,
        ) -> Option<RasterizedGlyph> {
            let mut chars = req.cluster.chars();
            let ch = chars.next()?;
            if chars.next().is_some() || !super::classic_char(ch, rtl) {
                return None;
            }
            if req.font_size_px.is_nan() || req.font_size_px < 1.0 {
                return None;
            }
            let (face, id, font) = self.lookup(req.family, ch)?;
            // GDI's sizes are whole pixels; the natural look keeps the size it is asked for.
            let em = if smooth {
                req.font_size_px
            } else {
                req.font_size_px.round()
            };
            let mut metrics = DWRITE_GLYPH_METRICS::default();
            // SAFETY: each pointer is to one element, the count given.
            unsafe {
                if smooth {
                    face.GetDesignGlyphMetrics(&id, 1, &mut metrics, false)
                        .ok()?;
                } else {
                    face.GetGdiCompatibleGlyphMetrics(
                        em,
                        1.0,
                        None,
                        false,
                        &id,
                        1,
                        &mut metrics,
                        false,
                    )
                    .ok()?;
                }
            }
            let advance = metrics.advanceWidth as f32 * em / f32::from(font.designUnitsPerEm);
            // Smooth: the font's own width, as egui lays a line out by it (and places each letter
            // at a quarter pixel). Classic: the whole pixels GDI's hinting gives.
            let advance_px = if smooth { advance } else { advance.round() };
            let origin = if smooth { req.subpixel_offset_px } else { 0.0 };
            let bitmap = self
                .draw(face, id, em, smooth, origin)
                .unwrap_or(GlyphBitmap {
                    // A space: egui still wants a bitmap to carry the advance.
                    image: ColorImage::new([1, 1], vec![Color32::TRANSPARENT]),
                    offset_px: egui::vec2(0.0, 0.0),
                    is_color: false,
                });
            Some(RasterizedGlyph { bitmap, advance_px })
        }

        /// The glyph in grey, placed from its origin; `None` when it has no ink.
        fn draw(
            &self,
            face: IDWriteFontFace,
            id: u16,
            em: f32,
            smooth: bool,
            origin_x: f32,
        ) -> Option<GlyphBitmap> {
            let (ids, advances, offsets) = ([id], [0f32], [DWRITE_GLYPH_OFFSET::default()]);
            let run = DWRITE_GLYPH_RUN {
                fontFace: std::mem::ManuallyDrop::new(Some(face)),
                fontEmSize: em,
                glyphCount: 1,
                glyphIndices: ids.as_ptr(),
                glyphAdvances: advances.as_ptr(),
                glyphOffsets: offsets.as_ptr(),
                isSideways: false.into(),
                bidiLevel: 0,
            };
            // SAFETY: `run` and the arrays it points to live through the calls; the texture
            // buffer is exactly the bounds' size.
            let drawn = unsafe {
                self.factory
                    .CreateGlyphRunAnalysis(
                        &run,
                        None,
                        if smooth {
                            DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC
                        } else {
                            DWRITE_RENDERING_MODE_GDI_CLASSIC
                        },
                        if smooth {
                            DWRITE_MEASURING_MODE_NATURAL
                        } else {
                            DWRITE_MEASURING_MODE_GDI_CLASSIC
                        },
                        DWRITE_GRID_FIT_MODE_ENABLED,
                        DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
                        origin_x,
                        0.0,
                    )
                    .and_then(|a| {
                        let r = a.GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1)?;
                        let w = (r.right - r.left).max(0) as usize;
                        let h = (r.bottom - r.top).max(0) as usize;
                        let mut coverage = vec![0u8; w * h];
                        if w > 0 && h > 0 {
                            a.CreateAlphaTexture(DWRITE_TEXTURE_ALIASED_1x1, &r, &mut coverage)?;
                        }
                        Ok((r, w, h, coverage))
                    })
            };
            drop(std::mem::ManuallyDrop::into_inner(run.fontFace));
            let (r, w, h, coverage) = drawn.ok()?;
            if w == 0 || h == 0 {
                return None;
            }
            // White with alpha, premultiplied (`GlyphBitmap`).
            // Smooth: less ink at the edges, so the weight matches what Windows' own
            // DirectWrite text shows (WPF's grey-scale text, compared side by side): coverage
            // blended as it is reads bolder, most of all light on dark.
            let gamma = if smooth { SMOOTH_COVERAGE_GAMMA } else { 1.0 };
            let pixels = coverage
                .into_iter()
                .map(|a| {
                    let a = ((f32::from(a) / 255.0).powf(gamma) * 255.0).round() as u8;
                    Color32::from_rgba_premultiplied(a, a, a, a)
                })
                .collect();
            Some(GlyphBitmap {
                image: ColorImage::new([w, h], pixels),
                offset_px: egui::vec2(r.left as f32, r.top as f32),
                is_color: false,
            })
        }
    }
}

/// What Explorer's "Open with → Choose another app" asks of the chooser: the "Always use this
/// app" choice may be saved for the file's type (`ALLOW_REGISTRATION`, `REGISTER_EXT`), and the
/// app chosen opens the file (`EXEC`; without it the chooser only records the choice).
const OPEN_WITH_FLAGS: windows_sys::Win32::UI::Shell::OPEN_AS_INFO_FLAGS = {
    use windows_sys::Win32::UI::Shell::{OAIF_ALLOW_REGISTRATION, OAIF_EXEC, OAIF_REGISTER_EXT};
    OAIF_ALLOW_REGISTRATION | OAIF_REGISTER_EXT | OAIF_EXEC
};

/// `path` as `SHOpenWithDialog` reads it: the path itself, wide and NUL-ended, with no quotes
/// round it (a path with spaces is one string here, not a command line).
fn open_with_file(path: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// Windows' own "Open with" chooser for the file `path` ("How do you want to open this file?"),
/// as Explorer shows it: the app chosen there opens the file (#720). It waits until the chooser
/// is closed, so it is called on a thread of its own (with COM, apartment-threaded, as the shell
/// expects). `Err(HRESULT)` when Windows couldn't show it; closing the chooser without a choice
/// is `Ok`.
///
/// It used to run `rundll32 shell32.dll,OpenAs_RunDLL <path>`, which takes the rest of its
/// command line as the path, quotes included: the quotes a path with a space gets made it a file
/// that isn't there, and the chooser never showed.
pub fn open_with_dialog(path: &std::path::Path) -> Result<(), i32> {
    use windows_sys::Win32::Foundation::ERROR_CANCELLED;
    use windows_sys::Win32::UI::Shell::{OPENASINFO, SHOpenWithDialog};
    com_on_this_thread();
    let file = open_with_file(path);
    let info = OPENASINFO {
        pcszFile: file.as_ptr(),
        pcszClass: std::ptr::null(),
        oaifInFlags: OPEN_WITH_FLAGS,
    };
    // SAFETY: `info` and the NUL-ended `file` it points to outlive the call, which only reads
    // them. No owner window: the window's thread isn't this one, and an owner would tie the two
    // threads' input together while the chooser is open.
    let hr = unsafe { SHOpenWithDialog(std::ptr::null_mut(), &info) };
    // HRESULT_FROM_WIN32(ERROR_CANCELLED): the chooser closed without a choice.
    let cancelled = (0x8007_0000 | ERROR_CANCELLED) as i32;
    if hr >= 0 || hr == cancelled {
        Ok(())
    } else {
        Err(hr)
    }
}

#[cfg(test)]
mod open_with_tests {
    use super::*;

    #[test]
    fn the_chooser_opens_the_file_chosen() {
        use windows_sys::Win32::UI::Shell::{OAIF_EXEC, OAIF_REGISTER_EXT};
        assert_eq!(OPEN_WITH_FLAGS & OAIF_EXEC, OAIF_EXEC);
        assert_eq!(OPEN_WITH_FLAGS & OAIF_REGISTER_EXT, OAIF_REGISTER_EXT);
    }

    #[test]
    fn a_path_with_spaces_is_given_whole_and_unquoted() {
        let path = std::path::Path::new(r"C:\Users\A B\Downloads\my file.zip");
        let wide = open_with_file(path);
        assert_eq!(wide.last(), Some(&0));
        assert_eq!(
            String::from_utf16(&wide[..wide.len() - 1]).unwrap(),
            r"C:\Users\A B\Downloads\my file.zip"
        );
        assert!(!wide.contains(&u16::from(b'"')));
    }
}

/// Open a file with its registered default application via Win32 `ShellExecuteW` (#811).
///
/// Unlike invoking `explorer.exe <file>`, which fails with `ExitStatus(1)` on archive files
/// (.zip, .7z) when a third-party tool such as 7-Zip is the default handler, `ShellExecuteW`
/// invokes the user's registered file association directly.
///
/// The program starts in the file's folder, as Explorer starts it (#1216): without a folder it
/// would start in MDM's own working folder, which is `C:\Windows\System32` when Windows started
/// MDM with the session, and a downloaded program writing beside itself would be refused there.
pub fn open_file_shell(path: &std::path::Path) -> Result<(), std::io::Error> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    com_on_this_thread();
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let folder: Option<Vec<u16>> = std::path::absolute(path).ok().and_then(|p| {
        p.parent()
            .map(|d| d.as_os_str().encode_wide().chain(Some(0)).collect())
    });
    // SAFETY: lpFile and lpDirectory (or NULL) are null-terminated UTF-16 strings that outlive
    // the call; lpOperation is NULL so Windows executes the default action for the file type.
    let ret = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            std::ptr::null(),
            wide.as_ptr(),
            std::ptr::null(),
            folder.as_ref().map_or(std::ptr::null(), |f| f.as_ptr()),
            SW_SHOWNORMAL,
        )
    };
    let code = ret as usize;
    if code <= 32 {
        let err = std::io::Error::last_os_error();
        return Err(if err.raw_os_error().unwrap_or(0) != 0 {
            err
        } else {
            std::io::Error::from_raw_os_error(code as i32)
        });
    }
    Ok(())
}

#[cfg(test)]
mod open_file_tests {
    /// #1216: the program a file opens with starts in the file's folder, not in MDM's own
    /// working folder. A batch file writes where it runs; Windows shows its console a moment,
    /// so it is run by hand: `cargo test -p mdm-app opened_in_its_folder -- --ignored`.
    #[test]
    #[ignore = "opens a console window for a moment"]
    fn a_file_is_opened_in_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("where.cmd");
        std::fs::write(&script, "@cd > \"%~dp0cwd.txt\"\r\n").unwrap();
        assert_ne!(std::env::current_dir().unwrap(), dir.path());
        super::open_file_shell(&script).unwrap();
        let said = dir.path().join("cwd.txt");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let text = loop {
            match std::fs::read_to_string(&said) {
                Ok(t) if t.ends_with('\n') => break t,
                _ if std::time::Instant::now() > deadline => panic!("the batch file didn't run"),
                _ => std::thread::sleep(std::time::Duration::from_millis(100)),
            }
        };
        let real = |p: &std::path::Path| std::fs::canonicalize(p).unwrap();
        assert_eq!(real(std::path::Path::new(text.trim())), real(dir.path()));
    }
}

/// The `(Default)` verb under `HKCR\<class>\shell`, when a file manager registers its own verb
/// name (such as `OpenInOneCommander`) instead of `open`: `AssocQueryStringW` with `verb = None`
/// looks up `open` rather than `shell`'s `(Default)` value. The value may list several verbs,
/// comma-separated, in order: the first is the default.
fn default_shell_verb(class: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::HKEY_CLASSES_ROOT;
    reg_string_in(HKEY_CLASSES_ROOT, &format!(r"{class}\shell"), "")
        .as_deref()
        .and_then(first_verb)
}

/// The first verb of a `shell` key's `(Default)` value, which may list several (`a, b`).
fn first_verb(value: &str) -> Option<String> {
    let first = value.split(',').next().unwrap_or("").trim();
    (!first.is_empty()).then(|| first.to_owned())
}

/// The program Windows runs to open folders, when that isn't Explorer (a file manager such as
/// Directory Opus or One Commander set up to replace it, #446, #1335); None for Explorer, or
/// when Windows doesn't say.
pub fn folder_opener() -> Option<String> {
    use windows_sys::Win32::UI::Shell::{ASSOCF_NONE, ASSOCSTR_EXECUTABLE, AssocQueryStringW};
    other_folder_opener(default_shell_verb, |class, verb| {
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let class = wide(class);
        let verb = verb.map(wide);
        let mut out = vec![0u16; 1024];
        let mut len = out.len() as u32;
        // SAFETY: the strings end in NUL and outlive the call; `len` is `out`'s size in
        // characters, and the call writes at most that many.
        let hr = unsafe {
            AssocQueryStringW(
                ASSOCF_NONE,
                ASSOCSTR_EXECUTABLE,
                class.as_ptr(),
                verb.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
                out.as_mut_ptr(),
                &mut len,
            )
        };
        (hr == 0).then(|| {
            let end = out.iter().position(|&c| c == 0).unwrap_or(out.len());
            String::from_utf16_lossy(&out[..end])
        })
    })
}

/// Who opens a folder, asked of `default_verb_for(class)` and `executable_for(class, verb)` as
/// the shell reads it: a folder is a `Directory` first (where a replacement registers its verb,
/// as `shell`'s `(Default)` verb, as the default query, or as `open`), then a `Folder` (where
/// Windows registers Explorer, or another manager sets `Folder\shell`'s `(Default)`). An installed
/// program's other verbs there ("Open with Code") are not the default and aren't asked for.
fn other_folder_opener(
    default_verb_for: impl Fn(&str) -> Option<String>,
    executable_for: impl Fn(&str, Option<&str>) -> Option<String>,
) -> Option<String> {
    let dir_default = default_verb_for("Directory");
    let folder_default = default_verb_for("Folder");
    let candidates = [
        dir_default.as_deref().map(|v| ("Directory", Some(v))),
        Some(("Directory", None)),
        Some(("Directory", Some("open"))),
        folder_default.as_deref().map(|v| ("Folder", Some(v))),
        Some(("Folder", None)),
    ];
    let opener = candidates
        .into_iter()
        .flatten()
        .find_map(|(class, verb)| executable_for(class, verb))?;
    let name = opener.rsplit(['\\', '/']).next().unwrap_or(&opener);
    (!name.is_empty() && !name.eq_ignore_ascii_case("explorer.exe")).then_some(opener)
}

#[cfg(test)]
mod folder_opener_tests {
    use super::other_folder_opener;

    const EXPLORER: &str = r"C:\Windows\Explorer.exe";
    const OPUS: &str = r"C:\Program Files\GPSoftware\Directory Opus\dopusrt.exe";
    const ONE_COMMANDER: &str =
        r"C:\Users\example\scoop\apps\onecommander\current\OneCommander.exe";

    /// #446: as Windows is shipped (what `reg query` shows on a clean system), Explorer it is.
    #[test]
    fn explorer_as_shipped_is_no_other_opener() {
        let shipped =
            |c: &str, v: Option<&str>| (c == "Folder" && v.is_none()).then(|| EXPLORER.to_owned());
        assert_eq!(other_folder_opener(|_| None, shipped), None);
    }

    /// A replacement's default verb on `Directory` wins over Explorer on `Folder`.
    #[test]
    fn a_replacement_on_directory_is_the_opener() {
        let opus = |c: &str, v: Option<&str>| match (c, v) {
            ("Directory", None) => Some(OPUS.to_owned()),
            ("Folder", None) => Some(EXPLORER.to_owned()),
            _ => None,
        };
        assert_eq!(other_folder_opener(|_| None, opus).as_deref(), Some(OPUS));
    }

    /// A `shell` key's `(Default)` may list verbs in order; the first is the default.
    #[test]
    fn the_first_listed_verb_is_the_default() {
        use super::first_verb;
        assert_eq!(
            first_verb("OpenInOneCommander").as_deref(),
            Some("OpenInOneCommander")
        );
        assert_eq!(
            first_verb(" OpenInOneCommander , open").as_deref(),
            Some("OpenInOneCommander")
        );
        assert_eq!(first_verb("  "), None);
        assert_eq!(first_verb(",open"), None);
    }

    /// #1335: a replacement registering a custom `(Default)` verb under `Directory\shell` (such
    /// as `OpenInOneCommander`), or `Folder\shell`, where `AssocQueryStringW(..., None)` returns
    /// no association.
    #[test]
    fn a_custom_default_verb_in_registry_is_the_opener() {
        let dir_verb = |c: &str| (c == "Directory").then(|| "OpenInOneCommander".to_owned());
        let one_commander = |c: &str, v: Option<&str>| match (c, v) {
            ("Directory", Some("OpenInOneCommander")) => Some(ONE_COMMANDER.to_owned()),
            ("Folder", Some("open")) => Some(EXPLORER.to_owned()),
            _ => None,
        };
        assert_eq!(
            other_folder_opener(dir_verb, one_commander).as_deref(),
            Some(ONE_COMMANDER)
        );

        let folder_verb = |c: &str| (c == "Folder").then(|| "openindopus".to_owned());
        let opus = |c: &str, v: Option<&str>| match (c, v) {
            ("Folder", Some("openindopus")) => Some(OPUS.to_owned()),
            _ => None,
        };
        assert_eq!(
            other_folder_opener(folder_verb, opus).as_deref(),
            Some(OPUS)
        );

        let custom_explorer = |c: &str, v: Option<&str>| match (c, v) {
            ("Directory", Some("OpenInExplorer")) => Some(EXPLORER.to_owned()),
            _ => None,
        };
        assert_eq!(
            other_folder_opener(
                |c| (c == "Directory").then(|| "OpenInExplorer".to_owned()),
                custom_explorer
            ),
            None
        );
    }

    /// One that replaces `Directory`'s `open` verb, or `Folder`'s own.
    #[test]
    fn a_replaced_open_verb_or_folder_is_the_opener() {
        let open = |c: &str, v: Option<&str>| match (c, v) {
            ("Directory", Some("open")) => Some(r"C:\Apps\files.exe".to_owned()),
            ("Folder", None) => Some(EXPLORER.to_owned()),
            _ => None,
        };
        assert_eq!(
            other_folder_opener(|_| None, open).as_deref(),
            Some(r"C:\Apps\files.exe")
        );
        let folder = |c: &str, v: Option<&str>| {
            (c == "Folder" && v.is_none()).then(|| r"C:\Apps\tc.exe".to_owned())
        };
        assert_eq!(
            other_folder_opener(|_| None, folder).as_deref(),
            Some(r"C:\Apps\tc.exe")
        );
    }

    /// Explorer written any way stays Explorer; nothing registered, nothing to say.
    #[test]
    fn explorer_any_case_or_no_answer_is_no_other_opener() {
        let upper = |_: &str, _: Option<&str>| Some(r"C:\WINDOWS\EXPLORER.EXE".to_owned());
        assert_eq!(other_folder_opener(|_| None, upper), None);
        assert_eq!(other_folder_opener(|_| None, |_, _| None), None);
    }
}

#[cfg(test)]
mod classic_text_tests {
    use egui::FontFamily;
    use egui::epaint::text::GlyphRasterizerRequest;

    fn glyphs() -> super::classic_text::Glyphs {
        let dir = std::env::var_os("WINDIR")
            .map(|w| std::path::PathBuf::from(w).join("Fonts"))
            .unwrap();
        super::classic_text::Glyphs::new(dir).unwrap()
    }

    fn req<'a>(cluster: &'a str, family: &'a FontFamily, size: f32) -> GlyphRasterizerRequest<'a> {
        GlyphRasterizerRequest {
            cluster,
            family,
            font_size_px: size,
            subpixel_offset_px: 0.0,
        }
    }

    /// #334: every letter gets a whole pixel width, and is drawn within it: the two can't drift
    /// apart any more. The letters of the words the report names, at the sizes of 100 % to 200 %.
    #[test]
    fn letters_have_whole_pixel_advances_and_stay_inside_them() {
        let mut g = glyphs();
        for family in [FontFamily::Proportional, FontFamily::Name("tahoma".into())] {
            for size in [11.0, 12.0, 13.2, 15.0, 16.5, 18.0, 24.0] {
                for ch in "downloadefaultakeDRsum".chars() {
                    let text = ch.to_string();
                    let glyph = g.glyph(&req(&text, &family, size), false, false).unwrap();
                    assert_eq!(glyph.advance_px.fract(), 0.0, "{ch} at {size}");
                    assert!(glyph.advance_px >= 1.0, "{ch} at {size}");
                    let right = glyph.bitmap.offset_px.x + glyph.bitmap.image.size[0] as f32;
                    assert!(
                        right <= glyph.advance_px + 1.0,
                        "{ch} at {size}: ink ends at {right}, advance {}",
                        glyph.advance_px
                    );
                }
            }
        }
    }

    /// #837: Chinese, Japanese and Korean characters are drawn by the same rasterizer as the
    /// Latin beside them, at whole-pixel widths in the classic look. Fractional interface sizes
    /// (110 %, 115 %…) round to a whole em instead of blurring the CJK fallback.
    #[test]
    fn cjk_letters_are_drawn_with_whole_pixel_advances() {
        let mut g = glyphs();
        let dir = std::env::var_os("WINDIR")
            .map(|w| std::path::PathBuf::from(w).join("Fonts"))
            .unwrap();
        for ch in ['中', '文', '語', 'か', '한', '，', '。'] {
            // A computer without that script's font (a CI image with no CJK fonts) draws it
            // through egui's provider, if at all: nothing to check here.
            if !crate::theme::cjk_font_files(ch)
                .iter()
                .any(|f| dir.join(f).exists())
            {
                eprintln!("no font for {ch}, not checked");
                continue;
            }
            for size in [11.0, 12.0, 13.2] {
                let text = ch.to_string();
                let glyph = g
                    .glyph(&req(&text, &FontFamily::Proportional, size), false, false)
                    .unwrap_or_else(|| panic!("{ch} at {size} not drawn"));
                assert_eq!(glyph.advance_px.fract(), 0.0, "{ch} at {size}");
                assert!(glyph.advance_px >= 1.0, "{ch} at {size}");
                assert!(glyph.bitmap.image.size[0] > 0, "{ch} at {size}: no bitmap");
            }
        }
    }

    /// #837: a mark that a shaper must place with its base, and every multi-codepoint cluster,
    /// stay with egui.
    #[test]
    fn cjk_marks_and_clusters_stay_with_egui() {
        let mut g = glyphs();
        let f = FontFamily::Proportional;
        for not in [
            "\u{302A}",         // combining left tone mark
            "\u{3099}",         // combining voiced sound mark
            "\u{304B}\u{3099}", // か + voiced mark
            "\u{4E2D}\u{302A}", // 中 + tone mark
        ] {
            assert!(
                g.glyph(&req(not, &f, 12.0), false, false).is_none(),
                "{not:?}"
            );
        }
    }

    /// #837: the boundaries of the ranges the rasterizer takes, and the ones a shaper must keep
    /// (conjoining Jamo, combining marks, halfwidth sound marks).
    #[test]
    fn cjk_script_boundaries() {
        use crate::theme::cjk_script;
        for not in [
            '\u{1100}',
            '\u{11FF}',
            '\u{A960}',
            '\u{A97F}',
            '\u{D7B0}',
            '\u{D7FF}',
            '\u{302A}',
            '\u{302F}',
            '\u{3099}',
            '\u{309A}',
            '\u{FF9E}',
            '\u{FF9F}',
            '\u{2000}',
            '\u{1FFFF}',
            '\u{40000}',
        ] {
            assert_eq!(cjk_script(not), None, "U+{:04X}", not as u32);
        }
        for c in ['\u{3040}', '\u{30FF}', '\u{FF9D}'] {
            assert_eq!(cjk_script(c), Some("japanese"), "U+{:04X}", c as u32);
        }
        for c in ['\u{3130}', '\u{318F}', '\u{AC00}', '\u{D7A3}'] {
            assert_eq!(cjk_script(c), Some("hangul"), "U+{:04X}", c as u32);
        }
        for c in ['\u{4E00}', '\u{9FFF}', '\u{20000}', '\u{3FFFF}'] {
            assert_eq!(cjk_script(c), Some("han"), "U+{:04X}", c as u32);
        }
    }

    /// #837: the interface language's own face leads its Han text; kana and Hangul keep their
    /// script's face whatever the interface language. A language parameter, so no global race.
    #[test]
    fn cjk_faces_follow_the_interface_language() {
        use crate::theme::cjk_font_files_for;
        assert_eq!(cjk_font_files_for('\u{4E2D}', "zh-hans")[0], "msyh.ttc");
        assert_eq!(cjk_font_files_for('\u{4E2D}', "zh-hant")[0], "msjh.ttc");
        assert_eq!(cjk_font_files_for('\u{4E2D}', "ja")[0], "YuGothM.ttc");
        assert_eq!(cjk_font_files_for('\u{4E2D}', "ko")[0], "malgun.ttf");
        assert_eq!(cjk_font_files_for('\u{4E2D}', "en")[0], "msyh.ttc");
        assert_eq!(cjk_font_files_for('\u{304B}', "ko")[0], "YuGothM.ttc");
        assert_eq!(cjk_font_files_for('\u{D55C}', "zh-hans")[0], "malgun.ttf");
    }

    /// #837: a supplementary Han character no face has returns to egui instead of drawing tofu.
    #[test]
    fn an_unassigned_supplementary_han_falls_back_to_egui() {
        let mut g = glyphs();
        assert!(
            g.glyph(
                &req("\u{3FFFF}", &FontFamily::Proportional, 12.0),
                false,
                false
            )
            .is_none()
        );
    }

    /// The smooth look keeps the font's own width and draws more levels of grey than the
    /// classic one, whose letters are fitted to whole pixels.
    #[test]
    fn smooth_letters_keep_their_width_and_their_greys() {
        let mut g = glyphs();
        let f = FontFamily::Proportional;
        let levels = |glyph: &egui::epaint::text::RasterizedGlyph| {
            let mut l: Vec<u8> = glyph.bitmap.image.pixels.iter().map(|p| p.a()).collect();
            l.sort_unstable();
            l.dedup();
            l.len()
        };
        let classic = g.glyph(&req("o", &f, 13.2), false, false).unwrap();
        let smooth = g.glyph(&req("o", &f, 13.2), false, true).unwrap();
        assert_eq!(classic.advance_px.fract(), 0.0);
        assert!(smooth.advance_px.fract() != 0.0, "{}", smooth.advance_px);
        assert!(levels(&smooth) > levels(&classic));
        // A quarter pixel to the right changes the picture, not the width.
        let mut shifted = req("o", &f, 13.2);
        shifted.subpixel_offset_px = 0.5;
        let moved = g.glyph(&shifted, false, true).unwrap();
        assert_eq!(moved.advance_px, smooth.advance_px);
        assert_ne!(moved.bitmap.image.pixels, smooth.bitmap.image.pixels);
    }

    #[test]
    fn a_space_keeps_its_advance() {
        let mut g = glyphs();
        let s = g
            .glyph(&req(" ", &FontFamily::Proportional, 12.0), false, false)
            .unwrap();
        assert!(s.advance_px > 0.0);
    }

    #[test]
    fn what_needs_shaping_stays_with_egui() {
        let mut g = glyphs();
        let f = FontFamily::Proportional;
        for not in [
            "\u{62a}",
            "\u{5d0}",
            "\u{93e}",
            "\u{e14}",
            "e\u{301}",
            "\u{1F600}",
        ] {
            assert!(
                g.glyph(&req(not, &f, 12.0), false, false).is_none(),
                "{not:?}"
            );
        }
        // A bracket is mirrored by the shaper in a right-to-left line, only.
        assert!(g.glyph(&req("(", &f, 12.0), false, false).is_some());
        assert!(g.glyph(&req("(", &f, 12.0), true, false).is_none());
        // A family it isn't told about stays with egui.
        let other = FontFamily::Name("other".into());
        assert!(g.glyph(&req("a", &other, 12.0), false, false).is_none());
    }
}

/// Global shortcut registration via Win32 `RegisterHotKey` on a dedicated message-queue thread (#558).
pub struct GlobalHotKeyOs {
    thread_id: std::sync::Arc<std::sync::atomic::AtomicU32>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl GlobalHotKeyOs {
    pub fn register(
        shortcut: crate::hotkey::Shortcut,
        fired: std::sync::Arc<std::sync::atomic::AtomicBool>,
        taken: std::sync::Arc<std::sync::atomic::AtomicBool>,
        ctx: egui::Context,
    ) -> Option<Self> {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        use windows_sys::Win32::System::Threading::GetCurrentThreadId;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, WM_HOTKEY,
        };

        let thread_id = Arc::new(AtomicU32::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let tid_clone = thread_id.clone();
        let stop_clone = stop.clone();
        let vk = shortcut.key.win32_vk();
        let mods = shortcut.win32_modifiers();
        let handle = std::thread::Builder::new()
            .name("mdm-global-hotkey".into())
            .spawn(move || {
                // SAFETY: creates the thread's message queue (`PeekMessageW` with `PM_NOREMOVE`)
                // before publishing `thread_id`, so `PostThreadMessageW(WM_QUIT)` from `Drop`
                // never targets a thread without a queue; clears `thread_id` if `RegisterHotKey`
                // fails or when the message loop ends.
                unsafe {
                    let mut msg: MSG = zeroed();
                    PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
                    tid_clone.store(GetCurrentThreadId(), Ordering::SeqCst);
                    if stop_clone.load(Ordering::SeqCst) {
                        tid_clone.store(0, Ordering::SeqCst);
                        return;
                    }
                    if RegisterHotKey(std::ptr::null_mut(), 1, mods, vk) == 0 {
                        tid_clone.store(0, Ordering::SeqCst);
                        taken.store(true, Ordering::SeqCst);
                        ctx.request_ui_pass(egui::ViewportId::ROOT);
                        ctx.request_repaint();
                        return;
                    }
                    while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                        if msg.message == WM_HOTKEY {
                            // The window is asked for: a stall now is felt (crash.rs, #1126).
                            crate::crash::asked();
                            fired.store(true, Ordering::SeqCst);
                            ctx.request_ui_pass(egui::ViewportId::ROOT);
                            ctx.request_repaint();
                        }
                    }
                    let _ = UnregisterHotKey(std::ptr::null_mut(), 1);
                    tid_clone.store(0, Ordering::SeqCst);
                }
            })
            .ok()?;
        Some(Self {
            thread_id,
            stop,
            handle: Some(handle),
        })
    }
}

impl Drop for GlobalHotKeyOs {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        use windows_sys::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};

        self.stop.store(true, Ordering::SeqCst);
        let tid = self.thread_id.swap(0, Ordering::SeqCst);
        if tid != 0 {
            // SAFETY: posts `WM_QUIT` to our own hotkey listener thread whose message queue exists.
            unsafe {
                let _ = PostThreadMessageW(tid, WM_QUIT, 0, 0);
            }
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

// ---- a downloaded file handed to other programs (#1496, fileclip.rs) ---------------------------

/// Let the program MDM starts now come to the front (#1496): the right the browser's native
/// host passed to MDM (`AllowSetForegroundWindow` on MDM's process) is passed on to any, so a
/// file opened, Explorer's window or the share sheet asked for from the browser popup's list
/// doesn't open behind the browser. Explorer's window comes from an Explorer process already
/// running, not from one MDM starts: hence any, not a process id.
pub fn pass_foreground() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
    // SAFETY: a plain call; it fails harmlessly when MDM has no right to pass on.
    unsafe {
        AllowSetForegroundWindow(ASFW_ANY);
    }
}

/// Let the process `pid` (the running copy of MDM, from a second launch) come to the front
/// now: this process's right to the foreground, if it has one, passed on.
pub fn allow_foreground(pid: u32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
    // SAFETY: a plain call; it fails harmlessly when this process has no right to pass on.
    unsafe {
        AllowSetForegroundWindow(pid);
    }
}

/// The `CF_HDROP` block for `paths` (`DROPFILES`, shellapi.h): its 20-byte header (where the
/// list starts, a point, whether it is a non-client drop, wide characters), then each path in
/// UTF-16 ending with a null, then one more null.
fn drop_files(paths: &[&std::path::Path]) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    let mut out = Vec::new();
    for v in [20u32, 0, 0, 0, 1] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for p in paths {
        for u in p.as_os_str().encode_wide().chain(Some(0)) {
            out.extend_from_slice(&u.to_le_bytes());
        }
    }
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// Put `paths` on the clipboard as Explorer's Copy does (#1496): `CF_HDROP` with their paths,
/// and `Preferred DropEffect` = copy, so a paste in a folder copies them (never moves them) and
/// a chat, a mail or a web page's upload box takes them as files. Called under MDM's clipboard
/// lock (fileclip.rs).
pub fn copy_files(paths: &[&std::path::Path]) -> Result<(), String> {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
    };
    const CF_HDROP: u32 = 15;
    const DROPEFFECT_COPY: u32 = 1;
    let name: Vec<u16> = "Preferred DropEffect"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    // SAFETY: a NUL-terminated name; 0 when it can't be registered (then left out).
    let effect = unsafe { RegisterClipboardFormatW(name.as_ptr()) };
    let blocks = [
        (CF_HDROP, drop_files(paths)),
        (effect, DROPEFFECT_COPY.to_le_bytes().to_vec()),
    ];
    // Another program may have it open a moment (a clipboard manager reading what changed).
    let mut opened = false;
    for _ in 0..10 {
        // SAFETY: no owner window, as arboard opens it; closed below.
        if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
            opened = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    if !opened {
        return Err(format!(
            "another program holds the clipboard ({})",
            std::io::Error::last_os_error()
        ));
    }
    let put = || -> Result<(), String> {
        // SAFETY: the clipboard is open on this thread. Each block is copied into memory of its
        // own size, which the clipboard owns once SetClipboardData takes it (freed here when it
        // doesn't).
        unsafe {
            if EmptyClipboard() == 0 {
                return Err(format!("{}", std::io::Error::last_os_error()));
            }
            for (format, bytes) in &blocks {
                if *format == 0 {
                    continue;
                }
                let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
                if mem.is_null() {
                    return Err("out of memory".into());
                }
                let at = GlobalLock(mem);
                if at.is_null() {
                    GlobalFree(mem);
                    return Err("out of memory".into());
                }
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), at.cast::<u8>(), bytes.len());
                GlobalUnlock(mem);
                if SetClipboardData(*format, mem).is_null() {
                    let e = std::io::Error::last_os_error();
                    GlobalFree(mem);
                    return Err(format!("{e}"));
                }
            }
            Ok(())
        }
    };
    let done = put();
    // SAFETY: opened above, on this thread.
    unsafe {
        CloseClipboard();
    }
    done
}

/// How long the share sheet's window waits for the share to end before it goes (a share the
/// sheet never says ended: closed without choosing, on a Windows before 11).
const SHARE_WAIT_MS: u32 = 5 * 60 * 1000;

/// Windows' share sheet for the file at `path` (#1496): DataTransferManager through its interop
/// for desktop programs (`IDataTransferManagerInterop`: `ShowShareUIForWindow` alone fails for
/// them), the file as a `StorageFile` (any size, nothing copied). The sheet belongs to a window:
/// one of MDM's own at the pointer, 1×1, all but transparent and clicks going through it,
/// which the browser's right to come to the front ([`pass_foreground`]) brings up. On a thread
/// of its own, the window's: the sheet asks that thread for the file (`DataRequested`), and
/// the window stays until the share is done or cancelled ([`SHARE_WAIT_MS`] at most). Returns
/// once the sheet was asked for, or why it couldn't be.
pub fn share_file(path: &std::path::Path) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let path = path.to_owned();
    std::thread::Builder::new()
        .name("share sheet".into())
        .spawn(move || share_on_this_thread(&path, &tx))
        .map_err(|e| e.to_string())?;
    // The file and the sheet come within a moment; a slower one is taken as asked for (the
    // app's answer to the browser waits meanwhile).
    rx.recv_timeout(std::time::Duration::from_millis(1500))
        .unwrap_or(Ok(()))
}

fn share_on_this_thread(
    path: &std::path::Path,
    said: &std::sync::mpsc::Sender<Result<(), String>>,
) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, KillTimer, MSG, SetTimer, TranslateMessage, WM_CLOSE,
        WM_TIMER,
    };
    com_on_this_thread();
    let Some(window) = ShareAnchor::new(true) else {
        let _ = said.send(Err("no window for the share sheet".into()));
        return;
    };
    let hwnd = window.0;
    // Its first messages handled (shown, activated) before the sheet is asked for.
    // SAFETY: this thread's own queue.
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{PM_REMOVE, PeekMessageW};
        let mut msg: MSG = zeroed();
        let until = std::time::Instant::now() + std::time::Duration::from_millis(200);
        while std::time::Instant::now() < until {
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    let asked = share_prepare(hwnd, path).and_then(|interop| {
        // SAFETY: our window, shown and in front.
        unsafe { interop.ShowShareUIForWindow(windows::Win32::Foundation::HWND(hwnd)) }
    });
    let failed = asked.as_ref().err().map(|e| e.to_string());
    let _ = said.send(asked.map_err(|e| e.to_string()));
    if let Some(e) = failed {
        tracing::warn!("share sheet: {e}");
        return;
    }
    // SAFETY: this thread's own window and message loop; the timer is ours and killed after.
    unsafe {
        let timer = SetTimer(hwnd, 1, SHARE_WAIT_MS, None);
        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if msg.hwnd == hwnd && (msg.message == WM_CLOSE || msg.message == WM_TIMER) {
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        KillTimer(hwnd, timer);
    }
    drop(window);
}

/// Everything the share sheet needs for the file at `path` and window `hwnd` but showing it:
/// the file as a `StorageFile`, the window's DataTransferManager (through the desktop interop),
/// and its `DataRequested` handler, which gives the sheet the file and closes the window once
/// the share is done or cancelled. On the window's thread, with COM on it.
fn share_prepare(
    hwnd: HWND,
    path: &std::path::Path,
) -> windows::core::Result<windows::Win32::UI::Shell::IDataTransferManagerInterop> {
    use windows::ApplicationModel::DataTransfer::{
        DataPackage, DataRequestedEventArgs, DataTransferManager, ShareCompletedEventArgs,
    };
    use windows::Foundation::TypedEventHandler;
    use windows::Storage::{IStorageItem, StorageFile};
    use windows::Win32::UI::Shell::IDataTransferManagerInterop;
    use windows::core::{AgileReference, HSTRING, IInspectable, Interface, Ref};
    use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
    {
        let file: StorageFile = StorageFile::GetFileFromPathAsync(&HSTRING::from(path))?.join()?;
        // The handler may run on another thread: the file reaches it through an agile
        // reference (a StorageFile itself isn't sent between threads).
        let item = AgileReference::new(&file.cast::<IStorageItem>()?)?;
        let title = HSTRING::from(
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
        let interop = windows::core::factory::<DataTransferManager, IDataTransferManagerInterop>()?;
        let raw = windows::Win32::Foundation::HWND(hwnd);
        // SAFETY: our window, alive while its thread's loop runs.
        let manager: DataTransferManager = unsafe { interop.GetForWindow(raw)? };
        // The window's handle as a number, for the handlers (on this thread or another).
        let anchor = hwnd as usize;
        let close = move || {
            // SAFETY: a message to our window; harmless once it is gone.
            unsafe { PostMessageW(anchor as HWND, WM_CLOSE, 0, 0) };
        };
        manager.DataRequested(&TypedEventHandler::new(
            move |_: Ref<DataTransferManager>, args: Ref<DataRequestedEventArgs>| {
                let data = args.ok()?.Request()?.Data()?;
                data.Properties()?.SetTitle(&title)?;
                let items: windows_collections::IIterable<IStorageItem> =
                    vec![Some(item.resolve()?)].into();
                data.SetStorageItemsReadOnly(&items)?;
                // Done or cancelled (Windows 11): the window goes. A Windows without these
                // events keeps it until SHARE_WAIT_MS.
                let _ = data.ShareCompleted(&TypedEventHandler::new(
                    move |_: Ref<DataPackage>, _: Ref<ShareCompletedEventArgs>| {
                        close();
                        Ok(())
                    },
                ));
                let _ = data.ShareCanceled(&TypedEventHandler::new(
                    move |_: Ref<DataPackage>, _: Ref<IInspectable>| {
                        close();
                        Ok(())
                    },
                ));
                Ok(())
            },
        ))?;
        Ok(interop)
    }
}

/// The share sheet's window ([`share_file`]): destroyed when dropped.
struct ShareAnchor(HWND);

impl ShareAnchor {
    /// A 1×1 window at the pointer, all but transparent (a window at alpha 0 counts as hidden);
    /// `show`: shown, activated and brought to the front (the tests make one never shown). The
    /// share sheet belongs to the active window, so a tool window or one never activated gets
    /// none.
    fn new(show: bool) -> Option<ShareAnchor> {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, GetCursorPos, GetForegroundWindow, LWA_ALPHA,
            RegisterClassW, SW_SHOW, SetLayeredWindowAttributes, ShowWindow, WNDCLASSW,
            WS_EX_LAYERED, WS_EX_TOPMOST, WS_POPUP,
        };
        let class: Vec<u16> = "MDM share sheet".encode_utf16().chain(Some(0)).collect();
        // SAFETY: a window class of plain defaults, registered once per process (a second
        // registration fails harmlessly); the window is ours, checked before use.
        unsafe {
            let module = GetModuleHandleW(std::ptr::null());
            let mut wc: WNDCLASSW = zeroed();
            wc.lpfnWndProc = Some(DefWindowProcW);
            wc.hInstance = module;
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
            let mut at = POINT { x: 0, y: 0 };
            GetCursorPos(&mut at);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOPMOST,
                class.as_ptr(),
                class.as_ptr(),
                WS_POPUP,
                at.x,
                at.y,
                1,
                1,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                module,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                return None;
            }
            SetLayeredWindowAttributes(hwnd, 0, 1, LWA_ALPHA);
            if show {
                // Activated with the right the browser's host passed on, or by joining the
                // foreground window's input ([`bring_forward`]).
                ShowWindow(hwnd, SW_SHOW);
                bring_forward(hwnd);
                tracing::info!(
                    "share sheet: its window in front: {}",
                    GetForegroundWindow() == hwnd
                );
            }
            Some(ShareAnchor(hwnd))
        }
    }
}

impl Drop for ShareAnchor {
    fn drop(&mut self) {
        use windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow;
        // SAFETY: our window, on the thread that made it.
        unsafe {
            DestroyWindow(self.0);
        }
    }
}

#[cfg(test)]
mod fileclip_tests {
    use std::path::Path;

    /// The block a paste reads the files from: its header, then each path and the list's end.
    #[test]
    fn the_file_list_is_dropfiles() {
        let a = Path::new(r"C:\Downloads\report.pdf");
        let b = Path::new(r"D:\é\b.zip");
        let block = super::drop_files(&[a, b]);
        let header: Vec<u32> = block[..20]
            .chunks(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(header, [20, 0, 0, 0, 1]);
        let units: Vec<u16> = block[20..]
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let text = String::from_utf16(&units).unwrap();
        assert_eq!(text, "C:\\Downloads\\report.pdf\0D:\\é\\b.zip\0\0");
    }

    /// The share sheet's preparation, everything but showing it (nothing appears on the screen):
    /// a downloaded file as a StorageFile from its path, the DataTransferManager of an MDM window
    /// through the desktop interop, and the handler that gives the sheet the file.
    #[test]
    fn the_share_sheet_is_prepared_for_a_file() {
        super::com_on_this_thread();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("report é.pdf");
        std::fs::write(&file, b"%PDF").unwrap();
        let window = super::ShareAnchor::new(false).expect("a window");
        super::share_prepare(window.0, &file).expect("prepared");
        // A file that isn't there is refused before any sheet.
        assert!(super::share_prepare(window.0, &dir.path().join("gone.pdf")).is_err());
    }

    /// The real clipboard, a private one (`private_clipboard`: the person's is neither read nor
    /// written): the file goes on it as Explorer's Copy puts it, a paste reads it back as a
    /// file, and Preferred DropEffect says copy.
    #[test]
    fn a_file_goes_on_the_clipboard_as_explorer_copies_it() {
        use crate::win32::private_clipboard::in_private_clipboard;
        in_private_clipboard(
            "win32::fileclip_tests::a_file_goes_on_the_clipboard_as_explorer_copies_it",
            |_| {
                use windows_sys::Win32::System::DataExchange::{
                    CloseClipboard, GetClipboardData, OpenClipboard, RegisterClipboardFormatW,
                };
                use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
                let dir = tempfile::tempdir().unwrap();
                let file = dir.path().join("a file é.zip");
                std::fs::write(&file, b"zip").unwrap();
                super::copy_files(&[&file]).unwrap();
                let back = arboard::Clipboard::new()
                    .unwrap()
                    .get()
                    .file_list()
                    .unwrap();
                assert_eq!(back.len(), 1);
                assert_eq!(
                    std::fs::canonicalize(&back[0]).unwrap(),
                    std::fs::canonicalize(&file).unwrap()
                );
                let name: Vec<u16> = "Preferred DropEffect"
                    .encode_utf16()
                    .chain(Some(0))
                    .collect();
                // SAFETY: the test's own clipboard, opened and closed here; the block read
                // within its size.
                let effect = unsafe {
                    let format = RegisterClipboardFormatW(name.as_ptr());
                    assert!(OpenClipboard(std::ptr::null_mut()) != 0);
                    let mem = GetClipboardData(format);
                    assert!(!mem.is_null(), "no Preferred DropEffect");
                    assert!(GlobalSize(mem) >= 4);
                    let at = GlobalLock(mem).cast::<u32>();
                    let v = at.read_unaligned();
                    GlobalUnlock(mem);
                    CloseClipboard();
                    v
                };
                assert_eq!(effect, 1, "copy, not move");
            },
        );
    }
}

/// The visible top-level windows of other processes whose title names `file` (its name, or its
/// name without the extension), with their titles: the window of a program showing that file.
fn windows_naming(file: &str) -> Vec<(HWND, String)> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{InternalGetWindowText, IsWindowVisible};
    struct Look {
        own: u32,
        names: Vec<String>,
        found: Vec<(HWND, String)>,
    }
    unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
        // SAFETY: lparam is the &mut Look passed to EnumWindows below, alive for the call.
        let l = unsafe { &mut *(lparam as *mut Look) };
        let mut pid = 0u32;
        // SAFETY: plain queries about any window.
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid == l.own || unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }
        let mut buf = vec![0u16; 512];
        // SAFETY: writes at most buf.len() units, the NUL included.
        let n = unsafe { InternalGetWindowText(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        let title = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
        let lower = title.to_lowercase();
        if l.names
            .iter()
            .any(|n| !n.is_empty() && lower.contains(n.as_str()))
        {
            l.found.push((hwnd, title));
        }
        1
    }
    let name = file.to_lowercase();
    let stem = std::path::Path::new(&name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut look = Look {
        // SAFETY: takes nothing.
        own: unsafe { GetCurrentProcessId() },
        names: vec![name, stem],
        found: Vec::new(),
    };
    // SAFETY: the callback only reads windows and writes to `look`, alive for the call.
    unsafe { EnumWindows(Some(each), &mut look as *mut Look as LPARAM) };
    look.found
}

/// Bring the window `hwnd` (another program's, or a window of ours that must be active) to the
/// foreground. Windows refuses it to a program in the background unless the right was passed on
/// (the browser's native host passes it to MDM) or the caller shares the foreground window's
/// input: so, when refused, lifted above the others ([`above_others`]) and asked again while
/// joined to the foreground window's thread. Whether it is the foreground window now.
fn bring_forward(hwnd: HWND) -> bool {
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, IsIconic, SW_RESTORE, SetForegroundWindow,
        ShowWindow,
    };
    // SAFETY: plain calls on window handles and thread ids; they fail harmlessly for a window
    // that is gone; the threads' input is joined only for the call and parted after it.
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        if SetForegroundWindow(hwnd) == 0 || GetForegroundWindow() != hwnd {
            above_others(hwnd);
            let front = GetForegroundWindow();
            let theirs = GetWindowThreadProcessId(front, std::ptr::null_mut());
            let mine = GetCurrentThreadId();
            let joined = theirs != 0 && theirs != mine && AttachThreadInput(mine, theirs, 1) != 0;
            BringWindowToTop(hwnd);
            SetForegroundWindow(hwnd);
            if joined {
                AttachThreadInput(mine, theirs, 0);
            }
        }
        GetForegroundWindow() == hwnd
    }
}

/// Our window titled `title` brought to the foreground (`dialogs::focus`, #1645): by
/// [`bring_forward`], with no made-up input (winit's way, a Left Alt press and release sent to
/// the whole system, set off other programs' Alt hotkeys). Refused, its taskbar button flashes
/// until it is clicked, as Windows has a program in the background ask for the user's eye. None
/// while there is no such window on the screen yet (ask again in a later frame); else whether it
/// is the foreground window now.
pub fn bring_own_forward(title: &str) -> Option<bool> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FLASHW_TIMERNOFG, FLASHW_TRAY, FLASHWINFO, FlashWindowEx, GetForegroundWindow,
        IsWindowVisible,
    };
    let hwnd = find_own_window(title)?;
    // SAFETY: plain queries about our own window.
    let (visible, foreground) =
        unsafe { (IsWindowVisible(hwnd) != 0, GetForegroundWindow() == hwnd) };
    if !visible {
        return None;
    }
    if foreground || bring_forward(hwnd) {
        return Some(true);
    }
    let flash = FLASHWINFO {
        cbSize: size_of::<FLASHWINFO>() as u32,
        hwnd,
        dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
        uCount: 0,
        dwTimeout: 0,
    };
    // SAFETY: a filled structure about our own window, read for the call only.
    unsafe { FlashWindowEx(&flash) };
    Some(false)
}

/// Open the file at `path` with its program and bring the window showing it in front (#1496:
/// from the browser popup's list). `ShellExecuteW` alone hands a file to a program already
/// running (Explorer for a zip, a browser for a PDF, Notepad's open window), which then shows it
/// in a window of its own and doesn't take the foreground: the window came up behind the browser.
/// So the right to come to the front is passed on first (to any process, and to the program
/// started, when one is), and the window whose title names the file, new or retitled, is looked
/// for for a few seconds and brought forward.
pub fn open_file_in_front(path: &std::path::Path) -> Result<(), std::io::Error> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Threading::GetProcessId;
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, SW_SHOWNORMAL};
    com_on_this_thread();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let before: Vec<(HWND, String)> = windows_naming(&name);
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let folder: Option<Vec<u16>> = std::path::absolute(path).ok().and_then(|p| {
        p.parent()
            .map(|d| d.as_os_str().encode_wide().chain(Some(0)).collect())
    });
    pass_foreground();
    // SAFETY: every field admits zero; the strings are NUL-terminated and outlive the call; the
    // process handle, when one comes back, is closed below.
    let mut info: SHELLEXECUTEINFOW = unsafe { zeroed() };
    info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    info.lpFile = wide.as_ptr();
    info.lpDirectory = folder.as_ref().map_or(std::ptr::null(), |f| f.as_ptr());
    info.nShow = SW_SHOWNORMAL;
    // SAFETY: as above.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    if !info.hProcess.is_null() {
        // SAFETY: the handle ShellExecuteExW gave, closed once used.
        unsafe {
            AllowSetForegroundWindow(GetProcessId(info.hProcess));
            windows_sys::Win32::Foundation::CloseHandle(info.hProcess);
        }
    }
    // The window naming the file: a new one, or one whose title changed to it (a browser's tab).
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(8) {
        std::thread::sleep(std::time::Duration::from_millis(150));
        let now = windows_naming(&name);
        let fresh = now
            .iter()
            .find(|(h, t)| !before.iter().any(|(b, bt)| b == h && bt == t))
            .or(if before.is_empty() { None } else { now.first() });
        if let Some((hwnd, title)) = fresh {
            // A moment for it to finish showing (Explorer paints, then activates).
            std::thread::sleep(std::time::Duration::from_millis(250));
            let front = bring_forward(*hwnd);
            tracing::info!("open: {title:?} brought in front ({front})");
            return Ok(());
        }
    }
    tracing::info!("open: no window named {name:?} came within 8 s");
    Ok(())
}
