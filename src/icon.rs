//! The app icon, drawn in code (a pull-request glyph on a coloured disc) - no image files or
//! image crates needed. A small dot signals open to-dos or an error.

#[derive(Clone, Copy, PartialEq)]
pub enum Badge {
    None,
    Todo,
    Error,
}

const PURPLE: [f32; 3] = [130.0, 80.0, 223.0];
const GREY: [f32; 3] = [120.0, 120.0, 128.0];
const ORANGE: [f32; 3] = [245.0, 159.0, 0.0];
const RED: [f32; 3] = [218.0, 54.0, 51.0];

fn seg_dist(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (px, py) = (p.0 - a.0, p.1 - a.1);
    let (bx, by) = (b.0 - a.0, b.1 - a.1);
    let t = ((px * bx + py * by) / (bx * bx + by * by)).clamp(0.0, 1.0);
    ((px - bx * t).powi(2) + (py - by * t).powi(2)).sqrt()
}

fn circle_dist(p: (f32, f32), c: (f32, f32)) -> f32 {
    ((p.0 - c.0).powi(2) + (p.1 - c.1).powi(2)).sqrt()
}

/// RGBA pixels, `size` x `size`.
pub fn rgba(size: u32, badge: Badge, dimmed: bool) -> Vec<u8> {
    let s = size as f32 / 32.0;
    let mut px = vec![0u8; (size * size * 4) as usize];
    let cover = |d: f32| (0.5 - d / s).clamp(0.0, 1.0); // anti-aliased edge (d in pixels of the 32 grid * s)
    for y in 0..size {
        for x in 0..size {
            let p = ((x as f32 + 0.5) / s, (y as f32 + 0.5) / s);
            let mut col = [0.0f32; 3];
            let mut alpha = 0.0f32;
            let mut paint = |c: [f32; 3], a: f32| {
                if a <= 0.0 {
                    return;
                }
                let out = a + alpha * (1.0 - a);
                for i in 0..3 {
                    col[i] = (c[i] * a + col[i] * alpha * (1.0 - a)) / out.max(1e-6);
                }
                alpha = out;
            };
            // disc
            paint(
                if dimmed { GREY } else { PURPLE },
                cover((circle_dist(p, (16.0, 16.0)) - 15.0) * s),
            );
            // glyph
            let ring = |c: (f32, f32)| ((circle_dist(p, c) - 2.6).abs() - 0.9) * s;
            let line = |a: (f32, f32), b: (f32, f32)| (seg_dist(p, a, b) - 1.0) * s;
            let d = [
                ring((11.0, 9.0)),
                line((11.0, 11.6), (11.0, 20.4)),
                ring((11.0, 23.0)),
                ring((21.0, 23.0)),
                line((21.0, 20.4), (21.0, 13.0)),
                line((21.0, 13.0), (18.0, 9.5)),
                line((18.0, 9.5), (15.0, 9.5)),
                line((15.0, 9.5), (17.2, 7.3)),
                line((15.0, 9.5), (17.2, 11.7)),
            ]
            .into_iter()
            .fold(f32::MAX, f32::min);
            paint([255.0; 3], cover(d));
            // badge with a white ring
            if badge != Badge::None {
                let c = (25.5, 6.5);
                paint([255.0; 3], cover((circle_dist(p, c) - 6.5) * s));
                paint(
                    if badge == Badge::Error { RED } else { ORANGE },
                    cover((circle_dist(p, c) - 5.0) * s),
                );
            }
            let i = ((y * size + x) * 4) as usize;
            px[i] = col[0] as u8;
            px[i + 1] = col[1] as u8;
            px[i + 2] = col[2] as u8;
            px[i + 3] = (alpha * 255.0) as u8;
        }
    }
    px
}
