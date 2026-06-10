//! Clipboard image capture + PNG encoding for composer attachments.
//!
//! egui only exposes the *text* clipboard, so reading an image needs `arboard`.
//! The PNG-encode + base64 step (what actually gets sent to the sidecar) is
//! pure and unit-tested; the clipboard read itself is OS-bound and isn't.

/// Raw RGBA8 image grabbed from the clipboard.
pub(crate) struct ClipboardImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Read an image from the OS clipboard, if one is present.
pub(crate) fn read_clipboard_image() -> Option<ClipboardImage> {
    let mut cb = arboard::Clipboard::new().ok()?;
    let img = cb.get_image().ok()?;
    Some(ClipboardImage {
        width: img.width,
        height: img.height,
        rgba: img.bytes.into_owned(),
    })
}

/// Encode RGBA8 pixels to a base64 PNG (the wire form sent to the sidecar,
/// where it becomes a pydantic-ai `BinaryContent`). Returns `None` if the
/// buffer length doesn't match `width * height * 4`.
pub(crate) fn encode_png_base64(width: usize, height: usize, rgba: &[u8]) -> Option<String> {
    if width == 0 || height == 0 || rgba.len() != width.checked_mul(height)?.checked_mul(4)? {
        return None;
    }
    let mut png_bytes: Vec<u8> = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, width as u32, height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(rgba).ok()?;
    }
    use base64::Engine as _;
    Some(base64::engine::general_purpose::STANDARD.encode(&png_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    #[test]
    fn encode_png_base64_roundtrips_dimensions() {
        // 2x2 RGBA (16 bytes): red, green, blue, white.
        let rgba = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let b64 = encode_png_base64(2, 2, &rgba).expect("encodes");
        let png_bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .expect("valid base64");
        // Decode the PNG back and confirm it's a real 2x2 image.
        let decoder = png::Decoder::new(png_bytes.as_slice());
        let reader = decoder.read_info().expect("valid png");
        assert_eq!(reader.info().width, 2);
        assert_eq!(reader.info().height, 2);
    }

    #[test]
    fn encode_rejects_mismatched_buffer() {
        assert!(encode_png_base64(2, 2, &[0, 0, 0]).is_none());
        assert!(encode_png_base64(0, 0, &[]).is_none());
    }
}
