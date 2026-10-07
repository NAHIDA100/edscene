//! The soft image behind the settings panels.
//!
//! Two sources, in order of preference: the desktop's own wallpaper *file*
//! (sharp, cheap, and unaffected by whatever windows are on top of it) and,
//! only when there is no file to read, a grab of the screen. Sampling the
//! screen is what made the backdrop look like a smear of the user's windows.
use eds_core::fx::Order;
use std::path::Path;

/// Decode image bytes of any supported format and shrink them into a backdrop.
pub fn soften_bytes(bytes: &[u8]) -> Option<slint::Image> {
    let img = image::ImageReader::new(std::io::Cursor::new(bytes.to_vec()))
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let (w, h) = (img.width() as usize, img.height() as usize);
    soften(img.to_rgba8().into_raw(), w, h, Order::Rgba)
}

pub fn soften_path(path: &Path) -> Option<slint::Image> {
    soften_bytes(&std::fs::read(path).ok()?)
}

/// A 32-bit GDI grab is BGRA, which is not the order the box passes expect.
pub fn from_bgra(bgra: &[u8], w: usize, h: usize) -> Option<slint::Image> {
    soften(bgra.to_vec(), w, h, Order::Bgra)
}

/// A 1/8 sample with two box passes reads as a soft backdrop at ~1 % of the
/// pixel cost; Slint's bilinear stretch does the rest.
fn soften(px: Vec<u8>, w: usize, h: usize, order: Order) -> Option<slint::Image> {
    if w == 0 || h == 0 {
        return None;
    }
    let (soft, ow, oh) = eds_core::fx::soften(&px, w, h, 8, order);
    let buf = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::clone_from_slice(&soft, ow as u32, oh as u32);
    Some(slint::Image::from_rgb8(buf))
}
