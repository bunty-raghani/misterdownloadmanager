//! The native host's Windows calls: letting the app come to the front, reading its registry
//! entries.
#![allow(unsafe_code)]

/// Let process `pid` (the app) bring its windows to the front. Windows refuses that to a program
/// in the background (the foreground lock), so a window opened at the browser's request (Options
/// from the video button, Download File Info) would open behind it. The browser is in front and
/// started this host, which lets the host pass that right on. Nothing happens when it can't.
pub fn allow_foreground(pid: u32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
    // SAFETY: a plain call with a process id; it fails harmlessly for a wrong or ended one.
    unsafe {
        AllowSetForegroundWindow(pid);
    }
}

/// The default value of `key` under HKLM (`machine`) or HKCU, when it is a string.
pub fn reg_string(machine: bool, key: &str) -> Option<String> {
    use std::ffi::c_void;
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW,
    };
    let root = if machine {
        HKEY_LOCAL_MACHINE
    } else {
        HKEY_CURRENT_USER
    };
    let key: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
    let mut buf = vec![0u16; 2048];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: a registry read into `buf`, whose size in bytes is passed in `size`; a null value
    // name reads the key's default value.
    let rc = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            std::ptr::null(),
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
