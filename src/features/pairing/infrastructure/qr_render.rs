//! Renders the pairing URI as a terminal QR with Unicode half blocks.

use qrcode::render::unicode::Dense1x2;
use qrcode::{EcLevel, QrCode};

/// The default suits dark terminal themes (modules drawn as background);
/// `dark_on_light` suits light themes.
pub fn render_qr(data: &str, dark_on_light: bool) -> Option<String> {
    let code = QrCode::with_error_correction_level(data.as_bytes(), EcLevel::L).ok()?;
    let (dark, light) = if dark_on_light {
        (Dense1x2::Dark, Dense1x2::Light)
    } else {
        (Dense1x2::Light, Dense1x2::Dark)
    };
    Some(
        code.render::<Dense1x2>()
            .dark_color(dark)
            .light_color(light)
            .quiet_zone(true)
            .build(),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn renders_half_blocks() {
        let q = super::render_qr("openmosh://pair?d=abc", false).unwrap();
        assert!(q.contains('▀') || q.contains('▄') || q.contains('█'));
        assert!(q.lines().count() > 10);
    }
}
