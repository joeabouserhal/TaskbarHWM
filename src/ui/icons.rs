//! Small line icons for the hardware sections, drawn with Direct2D primitives.
//!
//! Drawn rather than taken from an icon font: Segoe Fluent Icons has no dependable CPU/GPU/RAM glyphs, and
//! primitives stay crisp at any size and follow the text color. Everything is laid out on a 24-unit grid and
//! scaled, with deliberately few details (2 pins per side, 4 contacts) so it still reads at ~15 px.
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::*;
use windows_numerics::Vector2;

use crate::metrics::Section;

/// Draws `section`'s icon in the `size` x `size` square whose top-left corner is (x, y), using the brush's color.
pub fn draw(rt: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, section: Section, x: f32, y: f32, size: f32) {
    let k = size / 24.0;
    let sw = (k * 2.0).clamp(1.1, 2.4);
    let pt = |u: f32, v: f32| Vector2 { X: x + u * k, Y: y + v * k };
    let rc = |l: f32, t: f32, r: f32, b: f32| D2D_RECT_F { left: x + l * k, top: y + t * k, right: x + r * k, bottom: y + b * k };
    unsafe {
        let line = |a: (f32, f32), b: (f32, f32)| rt.DrawLine(pt(a.0, a.1), pt(b.0, b.1), brush, sw, None);
        let boxed = |l: f32, t: f32, r: f32, b: f32| rt.DrawRectangle(&rc(l, t, r, b), brush, sw, None);
        let rounded = |l: f32, t: f32, r: f32, b: f32, rad: f32| {
            rt.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rc(l, t, r, b), radiusX: rad * k, radiusY: rad * k }, brush, sw, None)
        };
        let circle = |cx: f32, cy: f32, r: f32| {
            rt.DrawEllipse(&D2D1_ELLIPSE { point: pt(cx, cy), radiusX: r * k, radiusY: r * k }, brush, sw, None)
        };

        match section {
            // Chip: square package, inner die, pins on every side.
            Section::Cpu => {
                rounded(6.0, 6.0, 18.0, 18.0, 2.0);
                boxed(9.5, 9.5, 14.5, 14.5);
                for t in [9.5, 14.5] {
                    line((3.0, t), (6.0, t));
                    line((18.0, t), (21.0, t));
                    line((t, 3.0), (t, 6.0));
                    line((t, 18.0), (t, 21.0));
                }
            }
            // Graphics card: long board with two fans and the PCIe edge connector.
            Section::Gpu => {
                // Vertical extent 5..19 so the icon's visual middle is the grid's middle (12).
                rounded(2.0, 5.0, 22.0, 16.0, 2.0);
                circle(8.5, 10.5, 3.2);
                circle(15.5, 10.5, 3.2);
                line((4.5, 19.0), (14.0, 19.0));
            }
            // Drive: slim enclosure with an activity slot and an LED.
            Section::Disk => {
                rounded(2.5, 7.0, 21.5, 17.0, 2.5);
                line((6.0, 13.5), (13.5, 13.5));
                circle(18.0, 13.5, 1.1);
            }
            // Memory module: board, three chips, gold contacts.
            Section::Ram => {
                // Vertical extent 5.5..18.5, centered on 12.
                rounded(2.0, 5.5, 22.0, 15.0, 1.5);
                for cx in [4.5, 10.0, 15.5] {
                    boxed(cx, 8.0, cx + 4.0, 12.5);
                }
                for cx in [6.0, 10.0, 14.0, 18.0] {
                    line((cx, 15.0), (cx, 18.5));
                }
            }
        }
    }
}
