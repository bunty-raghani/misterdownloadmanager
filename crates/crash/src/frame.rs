//! A crash signal's frame, for the handler in front of crash-handler's (#1239; the handler itself,
//! its system calls and `unsafe`, is in the app's unix.rs).
//!
//! crash-handler copies the context a crash signal comes with as glibc's `ucontext_t` (968 bytes
//! on x86_64). The kernel's frame holds less: its own `ucontext` (without glibc's room for the
//! floating-point registers and the shadow stack), the signal's info, then the processor's state,
//! up to the top of the alternate signal stack the handler runs on. Where that state is large (a
//! processor with XSAVE: AVX and later) the copy stays inside it. Without XSAVE (an older
//! processor, or a virtual machine that doesn't pass it on) the state is 512 bytes and the copy
//! reads 8 bytes past the stack's top. On the main thread the page there is the guard page of
//! Rust's own alternate stack: the handler faulted a second time and the process died with no
//! dump. The handler in front gives crash-handler a copy of the part that is there instead.

use std::ops::Range;

/// How many of the `len` bytes of a context at `at` can be read: those below the top of the
/// alternate stack (`stack`) when the frame is on it, all of them on the thread's own stack
/// (`None`: the interrupted code's frames are above it).
pub fn readable(at: usize, len: usize, stack: Option<Range<usize>>) -> usize {
    match stack {
        Some(s) if s.contains(&at) => len.min(s.end - at),
        _ => len,
    }
}

/// The bytes crash-handler copies from a signal's context: its `CrashContext::context`.
/// Linux only: crash-handler takes Windows exceptions and macOS's Mach exceptions with no signal
/// frame to copy.
#[cfg(target_os = "linux")]
pub fn context_len() -> usize {
    fn size_of_field<F>(_: fn(&crash_handler::CrashContext) -> &F) -> usize {
        std::mem::size_of::<F>()
    }
    size_of_field(|c| &c.context)
}

/// A zeroed context of the type crash-handler copies (its `CrashContext::context`), on this
/// stack, for `f`: [`context_len`] bytes to copy the readable part of the kernel's into.
#[cfg(target_os = "linux")]
pub fn with_copy(f: impl FnOnce(*mut std::ffi::c_void)) {
    fn zeroed<F>(_: fn(&crash_handler::CrashContext) -> &F) -> std::mem::MaybeUninit<F> {
        std::mem::MaybeUninit::zeroed()
    }
    let mut copy = zeroed(|c| &c.context);
    f(copy.as_mut_ptr().cast());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frame of #1239 (a virtual machine without XSAVE): crash-handler's 16 KiB stack, the
    /// context 960 bytes under its top; 968 bytes copied would cross into the guard page above.
    /// That crash's stack was `0x7fff_f7fb_4000..0x7fff_f7fb_8000`; the top here is one a 32-bit
    /// `usize` can hold, with the context the same distance under it, so the clamp is checked on
    /// every platform.
    #[test]
    fn a_context_is_read_up_to_the_top_of_the_alternate_stack() {
        let top: usize = 0x7ffb_8000;
        let stack = top - 0x4000..top;
        assert_eq!(readable(stack.end - 960, 968, Some(stack.clone())), 960);
        // With XSAVE the processor's state is larger: the frame starts lower, all of it is read.
        assert_eq!(readable(stack.end - 2_000, 968, Some(stack.clone())), 968);
        // On the thread's own stack, and anywhere else, as before.
        assert_eq!(readable(stack.end - 960, 968, None), 968);
        assert_eq!(readable(stack.end + 4_096, 968, Some(stack)), 968);
    }

    /// crash-handler's copy is glibc's `ucontext_t`, larger than the kernel's frame below the
    /// processor's state (304 bytes of `ucontext`, then 128 of signal info): what read past it.
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn the_copy_is_glibcs_context() {
        assert_eq!(context_len(), 968);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_copy_is_handed_over() {
        let mut seen = None;
        with_copy(|copy| seen = Some(copy as usize));
        assert!(seen.is_some_and(|p| p != 0));
        assert!(context_len() > 0);
    }
}
