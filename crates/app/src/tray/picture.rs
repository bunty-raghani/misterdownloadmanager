//! The tray icon's picture at the size the shell draws it (#1161). The app icon comes in
//! hand-made sizes (tools/icon/build_icon.py: 16 to 32 px a tighter crop, the full artwork from 48
//! px on); the tray gets the one the notification area draws, or the nearest larger one scaled
//! down here, never the 256 px one: Windows shrank that to 16–32 px with a poor filter, and the
//! icon looked blocky beside the others.
//!
//! The size, per system:
//! - Windows: the notification area's small icon size (`SM_CXSMICON`) at the taskbar's DPI:
//!   16 px at 100 %, 20 at 125 %, 24 at 150 %, 28 at 175 %, 32 at 200 % (`win32::tray_icon_px`).
//!   It is asked again as the shell is (`watch`), and the icon drawn again when the scale changes
//!   or Explorer starts again.
//! - macOS: 44 px, the menu bar's 22 points (tray-icon's icon height) at 2× (Retina); a 1×
//!   screen halves it.
//! - Linux: 32 px. StatusNotifierItem hosts scale the one picture they are given to their size
//!   (GNOME's AppIndicator extension 16 px a scale, KDE 22): the small design, at most a third
//!   down at 1×.

/// The hand-made sizes, smallest first.
pub const MADE: [u32; 8] = [16, 20, 24, 32, 48, 64, 128, 256];

/// The hand-made picture of `size` px (one of [`MADE`]), a PNG.
fn png(size: u32) -> &'static [u8] {
    match size {
        16 => include_bytes!("../../assets/icons/app/mdm-16.png"),
        20 => include_bytes!("../../assets/icons/app/mdm-20.png"),
        24 => include_bytes!("../../assets/icons/app/mdm-24.png"),
        32 => include_bytes!("../../assets/icons/app/mdm-32.png"),
        48 => include_bytes!("../../assets/icons/app/mdm-48.png"),
        64 => include_bytes!("../../assets/icons/app/mdm-64.png"),
        128 => include_bytes!("../../assets/icons/app/mdm-128.png"),
        _ => include_bytes!("../../assets/icons/app/mdm-256.png"),
    }
}

/// The hand-made size a `px` px picture is made from: that size when there is one, else the
/// nearest larger (scaled down), else the largest.
pub fn source(px: u32) -> u32 {
    MADE.into_iter()
        .find(|&s| s >= px)
        .unwrap_or(MADE[MADE.len() - 1])
}

/// The small icon size at `dpi`, as Windows works it out (`GetSystemMetricsForDpi(SM_CXSMICON)`:
/// 16 px at 96 DPI, rounded): what Windows' own answer is checked against, and stands for when
/// there is none.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn small_icon_px(dpi: u32) -> u32 {
    (16 * dpi.max(1) + 48) / 96
}

/// The size the tray icon is drawn at on macOS and Linux (module comment; Windows asks the
/// taskbar: `win32::tray_icon_px`).
#[cfg(not(windows))]
pub fn fitting() -> u32 {
    if cfg!(target_os = "macos") { 44 } else { 32 }
}

/// A square RGBA picture.
pub struct Picture {
    pub rgba: Vec<u8>,
    pub size: u32,
}

/// The app icon at `px` px (1 to 256): the hand-made picture of that size, or the nearest larger
/// one scaled down (Lanczos). Never an empty picture (#529, `tray::picture`).
pub fn at(px: u32) -> Result<Picture, String> {
    let px = px.clamp(1, MADE[MADE.len() - 1]);
    let from = source(px);
    let image = image::load_from_memory_with_format(png(from), image::ImageFormat::Png)
        .map_err(|e| format!("{from} px picture: {e}"))?
        .into_rgba8();
    if image.width() != from || image.height() != from {
        return Err(format!(
            "{from} px picture is {}×{} px",
            image.width(),
            image.height()
        ));
    }
    let image = if from == px {
        image
    } else {
        image::imageops::resize(&image, px, px, image::imageops::FilterType::Lanczos3)
    };
    Ok(Picture {
        rgba: image.into_raw(),
        size: px,
    })
}

/// When the watcher draws the icon again (Windows): once the size it would draw at, with the
/// taskbar it would draw for (`K`), differs from the one drawn and two questions in a row agree
/// on it. Explorer may add the icons again as the scale changes (tray-icon adds its own picture
/// then); waiting one question lets that happen first, so it is drawn over, not under.
#[cfg_attr(not(windows), allow(dead_code))]
pub struct Fit<K> {
    drawn: Option<K>,
    last: Option<K>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl<K: Copy + PartialEq> Fit<K> {
    /// The icon as made, drawn for `drawn`.
    pub fn new(drawn: K) -> Fit<K> {
        Fit {
            drawn: Some(drawn),
            last: Some(drawn),
        }
    }

    /// Whether to draw the icon for `now`, the size and taskbar asked just now.
    pub fn due(&mut self, now: K) -> bool {
        let settled = self.last.replace(now) == Some(now);
        settled && self.drawn != Some(now)
    }

    /// The icon is drawn for `now`.
    pub fn drawn(&mut self, now: K) {
        self.drawn = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows' scales give the notification area's small icon size, and each its picture: a
    /// hand-made one where there is one (100 %, 125 %, 150 %, 200 %, 300 %), else the nearest
    /// larger one scaled down (175 % from 32, the small design; 250 % from 48).
    #[test]
    fn each_scale_has_its_size_and_picture() {
        for (dpi, px, from) in [
            (96, 16, 16),
            (120, 20, 20),
            (144, 24, 24),
            (168, 28, 32),
            (192, 32, 32),
            (240, 40, 48),
            (288, 48, 48),
            (336, 56, 64),
            (384, 64, 64),
        ] {
            assert_eq!(small_icon_px(dpi), px, "{dpi} DPI");
            assert_eq!(source(px), from, "{px} px");
            let p = at(px).unwrap();
            assert_eq!(p.size, px);
            assert_eq!(p.rgba.len(), (px * px * 4) as usize);
            assert!(p.rgba.chunks(4).any(|c| c[3] > 0), "{px} px: blank");
        }
        // Beyond the largest, the largest; nothing is ever empty.
        assert_eq!(source(300), 256);
        assert_eq!(at(0).unwrap().size, 1);
        assert_eq!(at(1000).unwrap().size, 256);
    }

    /// Windows' own answer is the one worked out here, at every scale it offers.
    #[cfg(windows)]
    #[test]
    fn windows_agrees_on_the_small_icon_size() {
        for dpi in [96, 120, 144, 168, 192, 216, 240, 288] {
            assert_eq!(
                crate::win32::small_icon_px_for_dpi(dpi),
                Some(small_icon_px(dpi)),
                "{dpi} DPI"
            );
        }
    }

    /// The hand-made pictures are the sizes their names say, so none is ever scaled.
    #[test]
    fn hand_made_pictures_are_their_size() {
        for size in MADE {
            let i = image::load_from_memory(png(size)).unwrap();
            assert_eq!((i.width(), i.height()), (size, size));
        }
    }

    /// The system's own size, a small one: on Windows what the taskbar draws (up to 64 px, 400 %).
    #[test]
    fn the_system_size_is_a_small_one() {
        #[cfg(windows)]
        let px = crate::win32::tray_icon_px().1;
        #[cfg(not(windows))]
        let px = fitting();
        assert!((16..=64).contains(&px), "{px}");
        assert!(source(px) <= 64, "{px}");
    }

    /// The icon is drawn again once a new size (or taskbar) holds for two questions, and only
    /// once; a size back to the one drawn draws nothing.
    #[test]
    fn a_new_size_is_drawn_once_it_settles() {
        let mut fit = Fit::new((1, 16));
        assert!(!fit.due((1, 16)));
        // The scale changes to 150 %: the first question waits, the second draws.
        assert!(!fit.due((1, 24)));
        assert!(fit.due((1, 24)));
        fit.drawn((1, 24));
        assert!(!fit.due((1, 24)));
        // A draw the shell refused is tried again.
        assert!(!fit.due((1, 32)));
        assert!(fit.due((1, 32)));
        assert!(fit.due((1, 32)));
        // Back to the size drawn before the refusal: nothing to draw.
        assert!(!fit.due((1, 24)));
        assert!(!fit.due((1, 24)));
        // Explorer started again (a new taskbar): drawn again at the same size.
        assert!(!fit.due((2, 24)));
        assert!(fit.due((2, 24)));
    }
}
