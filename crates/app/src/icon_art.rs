// The app's icon, drawn in code: the window, the tray and (through the
// build script, which includes this file) the icon inside the .exe.

/// The app icon: a Cookie-shaped disc in the MD3E primary with a note.
pub fn icon_rgba(size: usize) -> Vec<u8> {
    let mut px = vec![0u8; size * size * 4];
    let c = size as f32 / 2.0;
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = (x as f32 + 0.5 - c, y as f32 + 0.5 - c);
            let a = dy.atan2(dx);
            let r = (dx * dx + dy * dy).sqrt();
            // Nine soft lobes, like Cookie9Sided.
            let edge = c * (0.86 + 0.06 * (9.0 * a).cos());
            let coverage = (edge - r + 0.5).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }
            let i = (y * size + x) * 4;
            // Note head and stem in white.
            let (nx, ny) = (dx / c, dy / c);
            let head = ((nx + 0.12).powi(2) / 0.06 + (ny - 0.28).powi(2) / 0.035) < 1.0;
            let stem = (0.05..0.17).contains(&nx) && (-0.45..0.28).contains(&ny);
            let flag = (0.05..0.42).contains(&nx) && (-0.45..-0.3).contains(&ny);
            let (r_, g_, b_) = if head || stem || flag {
                (255, 255, 255)
            } else {
                (0x67, 0x50, 0xA4)
            };
            px[i] = r_;
            px[i + 1] = g_;
            px[i + 2] = b_;
            px[i + 3] = (coverage * 255.0) as u8;
        }
    }
    px
}
