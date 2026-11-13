//! Lightweight drawing stack for always-on windows: Direct2D (software) draws into an in-memory
//! premultiplied-alpha bitmap, which is pushed to a layered window with UpdateLayeredWindow.
//! No D3D device and no GPU vendor driver is loaded, which keeps the resident footprint tiny.
use windows::core::Result;
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};

pub struct Gfx {
    pub d2d: ID2D1Factory,
    pub dwrite: IDWriteFactory,
}

impl Gfx {
    pub fn new() -> Result<Gfx> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            Ok(Gfx { d2d, dwrite })
        }
    }
}

/// A transparent bitmap bound to one layered HWND.
pub struct Surface {
    hdc: HDC,
    dib: HBITMAP,
    old: HGDIOBJ,
    rt: ID2D1DCRenderTarget,
    pub width: u32,
    pub height: u32,
    pub dpi: f32,
}

fn make_dib(hdc: HDC, w: u32, h: u32) -> Result<HBITMAP> {
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    unsafe { CreateDIBSection(Some(hdc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) }
}

impl Surface {
    pub fn new(gfx: &Gfx, width: u32, height: u32, dpi: f32) -> Result<Surface> {
        let (width, height) = (width.max(1), height.max(1));
        unsafe {
            let hdc = CreateCompatibleDC(None);
            let dib = make_dib(hdc, width, height)?;
            let old = SelectObject(hdc, dib.into());
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: dpi,
                dpiY: dpi,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let rt = gfx.d2d.CreateDCRenderTarget(&props)?;
            // Per-pixel-alpha surfaces must use grayscale AA; ClearType would fringe on transparency.
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            Ok(Surface { hdc, dib, old, rt, width, height, dpi })
        }
    }

    pub fn resize(&mut self, width: u32, height: u32, dpi: f32) -> Result<()> {
        let (width, height) = (width.max(1), height.max(1));
        if width != self.width || height != self.height {
            unsafe {
                let new = make_dib(self.hdc, width, height)?;
                SelectObject(self.hdc, new.into());
                let _ = DeleteObject(self.dib.into());
                self.dib = new;
            }
            self.width = width;
            self.height = height;
        }
        self.dpi = dpi;
        Ok(())
    }

    /// Draws a frame (coordinates in DIPs) and shows it at screen position (x, y).
    pub fn present(&self, hwnd: HWND, x: i32, y: i32, draw: impl FnOnce(&ID2D1DCRenderTarget)) -> Result<()> {
        unsafe {
            let rect = RECT { left: 0, top: 0, right: self.width as i32, bottom: self.height as i32 };
            self.rt.BindDC(self.hdc, &rect)?;
            self.rt.SetDpi(self.dpi, self.dpi);
            self.rt.BeginDraw();
            self.rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            draw(&self.rt);
            self.rt.EndDraw(None, None)?;

            let dst = POINT { x, y };
            let size = SIZE { cx: self.width as i32, cy: self.height as i32 };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            UpdateLayeredWindow(hwnd, None, Some(&dst), Some(&size), Some(self.hdc), Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA)?;
        }
        Ok(())
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.hdc, self.old);
            let _ = DeleteObject(self.dib.into());
            let _ = DeleteDC(self.hdc);
        }
    }
}

pub fn color(r: u8, g: u8, b: u8, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a }
}
