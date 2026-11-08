//! Renders the pairing URI as a terminal QR with Unicode half blocks.

use qrcode::{Color, EcLevel, QrCode};

/// Quiet zone in modules. The standard asks for 4; phone scanners read 2
/// fine and the smaller code fits a phone camera at a closer distance.
const QUIET: usize = 2;

/// The default suits dark terminal themes (modules drawn as background);
/// `dark_on_light` suits light themes.
pub fn render_qr(data: &str, dark_on_light: bool) -> Option<String> {
    let code = QrCode::with_error_correction_level(data.as_bytes(), EcLevel::L).ok()?;
    let width = code.width();
    let colors = code.to_colors();
    let size = width + 2 * QUIET;
    // `true` = draw this module with the terminal foreground.
    let ink = |x: usize, y: usize| -> bool {
        let dark = x >= QUIET
            && y >= QUIET
            && x < QUIET + width
            && y < QUIET + width
            && colors[(y - QUIET) * width + (x - QUIET)] == Color::Dark;
        dark == dark_on_light
    };
    let mut out = String::new();
    for y in (0..size).step_by(2) {
        for x in 0..size {
            let top = ink(x, y);
            let bottom = y + 1 < size && ink(x, y + 1);
            out.push(match (top, bottom) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        out.push('\n');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn renders_half_blocks() {
        let q = super::render_qr("openmosh://pair?d=abc", false).unwrap();
        assert!(q.contains('▀') || q.contains('▄') || q.contains('█'));
        assert!(q.lines().count() > 10);
    }

    #[test]
    fn pairing_uri_fits_in_45_columns() {
        // v3 pairing URI (113 chars) → version 5, 37 modules + 2×2 quiet zone = 41 columns.
        let uri = format!(
            "openmosh://pair?v=3&i=100.100.100.100&k={}&p=40404&t={}",
            "A".repeat(43),
            "B".repeat(22)
        );
        let q = super::render_qr(&uri, false).unwrap();
        assert!(
            q.lines().all(|l| l.chars().count() <= 45),
            "{}",
            q.lines().next().unwrap().chars().count()
        );
        assert!(q.lines().count() <= 23);
    }
}
