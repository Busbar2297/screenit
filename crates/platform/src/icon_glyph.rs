//! The screenIt mark, drawn as pixels so the taskbar, window and tray can share one picture.
//! A violet disc, a white lens and a cyan centre. No bundled image file.

/// RGBA (unmultiplied) for a square icon of side `n`.
pub fn rgba(n: u32) -> Vec<u8> {
    let n = n.max(1);
    let mut out = Vec::with_capacity((n * n * 4) as usize);
    let c = (n as f32 - 1.0) / 2.0;
    let outer = c * 0.94;
    let hole = c * 0.46;
    let pupil = c * 0.18;
    for y in 0..n {
        for x in 0..n {
            let dx = x as f32 - c;
            let dy = y as f32 - c;
            let r = (dx * dx + dy * dy).sqrt();
            let (px, a) = if r <= pupil {
                ([34, 211, 238], 255)
            } else if r <= hole {
                ([255, 255, 255], 255)
            } else if r <= outer - 1.0 {
                ([124, 92, 255], 255)
            } else if r < outer {
                ([124, 92, 255], ((outer - r) * 255.0) as u8)
            } else {
                ([0, 0, 0], 0)
            };
            out.extend_from_slice(&[px[0], px[1], px[2], a]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::rgba;

    #[test]
    fn centre_is_opaque_and_the_corner_is_clear() {
        for n in [16, 32, 256] {
            let px = rgba(n);
            assert_eq!(px.len(), (n * n * 4) as usize);
            let mid = (n as usize / 2 * n as usize + n as usize / 2) * 4;
            assert_eq!(px[mid + 3], 255, "centre of {n}");
            assert_eq!(px[3], 0, "corner of {n}");
        }
    }
}
