//! The OS-wide "less animation" switch (Windows: Settings → Accessibility →
//! Visual effects → Animation effects), read so reduced motion follows it.

/// True when the user turned animations off system-wide.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn reduced() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
    };
    let mut on: i32 = 1;
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL into the pointed-to
    // i32, which lives for the whole call.
    let ok = unsafe {
        SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut on).cast(), 0)
    };
    ok != 0 && on == 0
}

#[cfg(not(windows))]
pub fn reduced() -> bool {
    false
}
