//! Composition surface for windows that need the system acrylic backdrop (the flyout):
//! D3D11 (WARP) -> DXGI composition swapchain with premultiplied alpha -> DirectComposition,
//! drawn with Direct2D. Created when the flyout opens and dropped when it closes, so the
//! always-on widget never pays for it.
//!
//! WARP (software) is deliberate: a hardware device would pull the whole GPU vendor user-mode
//! driver into this process just to draw a few cards of text.
use std::mem::ManuallyDrop;
use windows::core::{Interface, Result};
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

pub struct GpuGfx {
    d3d: ID3D11Device,
    dxgi: IDXGIDevice,
    factory: IDXGIFactory2,
    d2d: ID2D1Device,
}

impl GpuGfx {
    pub fn new() -> Result<GpuGfx> {
        unsafe {
            let mut dev = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_WARP,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut dev),
                None,
                None,
            )?;
            let d3d = dev.unwrap();
            let dxgi: IDXGIDevice = d3d.cast()?;
            let factory: IDXGIFactory2 = dxgi.GetAdapter()?.GetParent()?;
            let d2d_factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let d2d = d2d_factory.CreateDevice(&dxgi)?;
            Ok(GpuGfx { d3d, dxgi, factory, d2d })
        }
    }
}

pub struct GpuSurface {
    swap: IDXGISwapChain1,
    dcomp: IDCompositionDevice,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    pub dc: ID2D1DeviceContext,
    pub width: u32,
    pub height: u32,
    pub dpi: f32,
}

impl GpuSurface {
    pub fn new(gfx: &GpuGfx, hwnd: HWND, width: u32, height: u32, dpi: f32) -> Result<GpuSurface> {
        unsafe {
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width.max(1),
                Height: height.max(1),
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                ..Default::default()
            };
            let swap = gfx.factory.CreateSwapChainForComposition(&gfx.d3d, &desc, None)?;
            let dcomp: IDCompositionDevice = DCompositionCreateDevice(&gfx.dxgi)?;
            let target = dcomp.CreateTargetForHwnd(hwnd, true)?;
            let visual = dcomp.CreateVisual()?;
            visual.SetContent(&swap)?;
            target.SetRoot(&visual)?;
            dcomp.Commit()?;
            let dc = gfx.d2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            dc.SetDpi(dpi, dpi);
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            Ok(GpuSurface { swap, dcomp, _target: target, _visual: visual, dc, width, height, dpi })
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        let (width, height) = (width.max(1), height.max(1));
        if width != self.width || height != self.height {
            unsafe {
                self.dc.SetTarget(None);
                self.swap.ResizeBuffers(0, width, height, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
            }
            self.width = width;
            self.height = height;
        }
        Ok(())
    }

    /// Draws one frame (DIP coordinates) onto a cleared transparent backbuffer and presents it.
    pub fn frame(&self, draw: impl FnOnce(&ID2D1DeviceContext)) -> Result<()> {
        unsafe {
            let surface: IDXGISurface = self.swap.GetBuffer(0)?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: self.dpi,
                dpiY: self.dpi,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                colorContext: ManuallyDrop::new(None),
            };
            let bitmap = self.dc.CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
            self.dc.SetTarget(&bitmap);
            self.dc.BeginDraw();
            self.dc.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            draw(&self.dc);
            self.dc.EndDraw(None, None)?;
            self.swap.Present(1, DXGI_PRESENT(0)).ok()?;
            self.dc.SetTarget(None);
            let _ = &self.dcomp;
        }
        Ok(())
    }
}
