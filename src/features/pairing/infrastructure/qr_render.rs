//! Renders the pairing URI as a terminal QR with Unicode half blocks.

use qrcode::render::unicode::Dense1x2;
use qrcode::{EcLevel, QrCode};

pub fn render_qr(data: &str) -> Option<String> {
    let code = QrCode::with_error_correction_level(data.as_bytes(), EcLevel::L).ok()?;
    Some(
        code.render::<Dense1x2>()
            .dark_color(Dense1x2::Light)
            .light_color(Dense1x2::Dark)
            .quiet_zone(true)
            .build(),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn renders_half_blocks() {
        let q = super::render_qr("openmosh://pair?d=abc").unwrap();
        assert!(q.contains('▀') || q.contains('▄') || q.contains('█'));
        assert!(q.lines().count() > 10);
    }
}
