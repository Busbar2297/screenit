//! DXGI Desktop Duplication: GPU-side screen capture for recording.
//!
//! One duplication per monitor is kept open across frames. The GPU hands over the whole desktop
//! image whenever it changes; that image is copied into a CPU-readable staging texture, and when
//! nothing changed the previous copy is still current. Anything unusual (rotated or HDR outputs,
//! lost access, secure desktop, no GPU) makes the caller fall back to GDI.

use screenit_geom::Rect;
use screenit_raster::Raster;
use windows::Win32::Foundation::{HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CPU_ACCESS_WRITE, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_MAP_READ, D3D11_MAP_READ_WRITE, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device,
    ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_UNSPECIFIED,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
    IDXGIAdapter, IDXGIFactory1, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::Interface;

/// Consecutive failures after which a monitor stops using duplication (GDI takes over).
const MAX_FAILURES: u32 = 3;

/// One duplicated output: the device, the duplication and the latest full-desktop copy.
struct Output {
    /// Desktop coordinates of the output (virtual-screen pixels).
    bounds: RECT,
    adapter: IDXGIAdapter,
    output: IDXGIOutput1,
    live: Option<Live>,
    failures: u32,
}

struct Live {
    context: ID3D11DeviceContext,
    dup: IDXGIOutputDuplication,
    staging: ID3D11Texture2D,
    width: u32,
    height: u32,
    /// The staging texture holds a complete desktop image.
    primed: bool,
}

/// All outputs of this machine, enumerated lazily.
#[derive(Default)]
pub struct Duplicator {
    outputs: Option<Vec<Output>>,
}

fn enumerate() -> Option<Vec<Output>> {
    // SAFETY: plain DXGI enumeration; every returned interface is reference counted.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let mut out = Vec::new();
        let mut ai = 0;
        while let Ok(adapter) = factory.EnumAdapters1(ai) {
            ai += 1;
            let base: IDXGIAdapter = adapter.cast().ok()?;
            let mut oi = 0;
            while let Ok(output) = base.EnumOutputs(oi) {
                oi += 1;
                let Ok(desc) = output.GetDesc() else { continue };
                if !desc.AttachedToDesktop.as_bool() {
                    continue;
                }
                let straight = desc.Rotation == DXGI_MODE_ROTATION_IDENTITY
                    || desc.Rotation == DXGI_MODE_ROTATION_UNSPECIFIED;
                let Ok(output1) = output.cast::<IDXGIOutput1>() else {
                    continue;
                };
                out.push(Output {
                    bounds: desc.DesktopCoordinates,
                    adapter: base.clone(),
                    output: output1,
                    live: None,
                    // Rotated outputs are never duplicated: the image would need un-rotating.
                    failures: if straight { 0 } else { MAX_FAILURES },
                });
            }
        }
        Some(out)
    }
}

impl Output {
    fn contains(&self, r: Rect) -> bool {
        let (l, t) = (i64::from(self.bounds.left), i64::from(self.bounds.top));
        let (rr, b) = (i64::from(self.bounds.right), i64::from(self.bounds.bottom));
        let (x, y) = (i64::from(r.x), i64::from(r.y));
        x >= l && y >= t && x + i64::from(r.w) <= rr && y + i64::from(r.h) <= b
    }

    fn open(&self) -> Option<Live> {
        // SAFETY: device, duplication and staging texture are created from valid interfaces and
        // owned by the returned value.
        unsafe {
            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;
            D3D11CreateDevice(
                &self.adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .ok()?;
            let (device, context) = (device?, context?);
            let dup = self.output.DuplicateOutput(&device).ok()?;
            let width = u32::try_from(self.bounds.right - self.bounds.left).ok()?;
            let height = u32::try_from(self.bounds.bottom - self.bounds.top).ok()?;
            let desc = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: (D3D11_CPU_ACCESS_READ.0 | D3D11_CPU_ACCESS_WRITE.0) as u32,
                MiscFlags: 0,
            };
            let mut staging: Option<ID3D11Texture2D> = None;
            device
                .CreateTexture2D(&desc, None, Some(&mut staging))
                .ok()?;
            Some(Live {
                context,
                dup,
                staging: staging?,
                width,
                height,
                primed: false,
            })
        }
    }

    /// Bring the staging texture up to date and copy `rect` (monitor-relative) out of it.
    fn grab(
        &mut self,
        rect: Rect,
        gdi: &mut dyn FnMut(Rect) -> Option<Raster>,
    ) -> Result<Raster, Failure> {
        if self.live.is_none() {
            self.live = Some(self.open().ok_or(Failure::Other)?);
        }
        let live = self.live.as_mut().ok_or(Failure::Other)?;
        let x0 = u32::try_from(i64::from(rect.x) - i64::from(self.bounds.left))
            .map_err(|_| Failure::Other)?;
        let y0 = u32::try_from(i64::from(rect.y) - i64::from(self.bounds.top))
            .map_err(|_| Failure::Other)?;
        if x0.checked_add(rect.w).is_none_or(|v| v > live.width)
            || y0.checked_add(rect.h).is_none_or(|v| v > live.height)
        {
            return Err(Failure::Other);
        }
        // SAFETY: the duplication, context and staging texture belong to `live`; a frame that
        // was acquired is always released before leaving, and the mapped memory is only read
        // while mapped and within the pitch/size the driver reported.
        unsafe {
            // Later frames only if the desktop changed; the first gets a moment to arrive.
            let timeout = if live.primed { 0 } else { 50 };
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource: Option<IDXGIResource> = None;
            match live.dup.AcquireNextFrame(timeout, &mut info, &mut resource) {
                Ok(()) => {
                    // A zero present time means only the pointer changed: no new desktop image.
                    let image = info.LastPresentTime != 0;
                    let copied = (|| {
                        if !image {
                            return Some(false);
                        }
                        let tex: ID3D11Texture2D = resource.as_ref()?.cast().ok()?;
                        let mut d = D3D11_TEXTURE2D_DESC::default();
                        tex.GetDesc(&mut d);
                        if d.Format != DXGI_FORMAT_B8G8R8A8_UNORM
                            || d.Width != live.width
                            || d.Height != live.height
                        {
                            return None; // HDR / mode change: leave it to GDI
                        }
                        live.context.CopyResource(&live.staging, &tex);
                        Some(true)
                    })();
                    let _ = live.dup.ReleaseFrame();
                    if copied.ok_or(Failure::Unsupported)? {
                        live.primed = true;
                    }
                }
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {}
                Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => return Err(Failure::Lost),
                Err(_) => return Err(Failure::Other),
            }
            if !live.primed {
                // A static desktop sends no image until something changes: seed the staging
                // texture from one GDI grab of the whole output.
                let whole = Rect::new(self.bounds.left, self.bounds.top, live.width, live.height);
                let seed = gdi(whole).ok_or(Failure::Other)?;
                seed_staging(live, &seed)?;
                live.primed = true;
            }
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            live.context
                .Map(&live.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|_| Failure::Other)?;
            let pitch = mapped.RowPitch as usize;
            let total = pitch.saturating_mul(live.height as usize);
            let result = if mapped.pData.is_null() || pitch < live.width as usize * 4 {
                Err(Failure::Other)
            } else {
                let all = std::slice::from_raw_parts(mapped.pData as *const u8, total);
                let (w, h) = (rect.w as usize, rect.h as usize);
                let mut rgba = Vec::with_capacity(w * h * 4);
                let mut ok = true;
                for row in 0..h {
                    let at = (y0 as usize + row) * pitch + x0 as usize * 4;
                    match all.get(at..at + w * 4) {
                        Some(line) => {
                            for px in line.chunks_exact(4) {
                                rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
                            }
                        }
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    Raster::from_rgba(rect.w, rect.h, rgba).map_err(|_| Failure::Other)
                } else {
                    Err(Failure::Other)
                }
            };
            live.context.Unmap(&live.staging, 0);
            result
        }
    }
}

/// Write an RGBA raster of the whole output into the staging texture as BGRA.
///
/// # Safety
/// live.staging must be a CPU-writable staging texture of live.width x live.height.
unsafe fn seed_staging(live: &Live, img: &Raster) -> Result<(), Failure> {
    if img.width() != live.width || img.height() != live.height {
        return Err(Failure::Other);
    }
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    // SAFETY: per the function contract; rows are written within the reported pitch.
    unsafe {
        live.context
            .Map(&live.staging, 0, D3D11_MAP_READ_WRITE, 0, Some(&mut mapped))
            .map_err(|_| Failure::Other)?;
        let pitch = mapped.RowPitch as usize;
        let row_bytes = live.width as usize * 4;
        if mapped.pData.is_null() || pitch < row_bytes {
            live.context.Unmap(&live.staging, 0);
            return Err(Failure::Other);
        }
        let dst =
            std::slice::from_raw_parts_mut(mapped.pData as *mut u8, pitch * live.height as usize);
        for (drow, srow) in dst
            .chunks_exact_mut(pitch)
            .zip(img.data().chunks_exact(row_bytes))
        {
            for (d, s) in drow.chunks_exact_mut(4).zip(srow.chunks_exact(4)) {
                d.copy_from_slice(&[s[2], s[1], s[0], 255]);
            }
        }
        live.context.Unmap(&live.staging, 0);
    }
    Ok(())
}

enum Failure {
    /// Access to the desktop was lost (mode change, secure desktop): reopen once.
    Lost,
    /// Format the duplication cannot give us as BGRA8: stop using it for this output.
    Unsupported,
    Other,
}

impl Duplicator {
    /// Pixels of `rect` (virtual-screen coordinates) through desktop duplication, or `None`
    /// when the caller should use GDI instead.
    pub fn grab(
        &mut self,
        rect: Rect,
        gdi: &mut dyn FnMut(Rect) -> Option<Raster>,
    ) -> Option<Raster> {
        if rect.is_empty() || rect.w > 16_384 || rect.h > 16_384 {
            return None;
        }
        if self.outputs.is_none() {
            self.outputs = Some(enumerate().unwrap_or_default());
        }
        let out = self
            .outputs
            .as_mut()?
            .iter_mut()
            .find(|o| o.failures < MAX_FAILURES && o.contains(rect))?;
        match out.grab(rect, gdi) {
            Ok(r) => {
                out.failures = 0;
                Some(r)
            }
            Err(Failure::Lost) => {
                out.live = None; // reopen on the next call
                out.failures += 1;
                None
            }
            Err(Failure::Unsupported) => {
                out.live = None;
                out.failures = MAX_FAILURES;
                None
            }
            Err(Failure::Other) => {
                out.failures += 1;
                if out.failures >= MAX_FAILURES {
                    out.live = None;
                }
                None
            }
        }
    }

    /// Forget every duplication (monitor layout changed); they are reopened on demand.
    pub fn reset(&mut self) {
        self.outputs = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Win32Screen;
    use screenit_capture::ScreenSource;

    /// Needs a desktop session with a GPU; skips quietly where duplication is unavailable.
    #[test]
    fn duplication_matches_gdi_on_the_primary_monitor() {
        crate::init_dpi_awareness();
        let screen = Win32Screen::new();
        let Ok(monitors) = screen.monitors() else {
            return;
        };
        let Some(m) = monitors.iter().find(|m| m.primary) else {
            return;
        };
        let rect = Rect::new(m.rect.x, m.rect.y, m.rect.w.min(1280), m.rect.h.min(720));
        let mut dup = Duplicator::default();
        let mut gdi = |r: Rect| screen.grab(r).ok();
        let Some(a) = dup.grab(rect, &mut gdi) else {
            eprintln!("desktop duplication unavailable here; skipped");
            return;
        };
        assert_eq!((a.width(), a.height()), (rect.w, rect.h));
        // A second grab reuses the cached image when nothing changed.
        let again = dup.grab(rect, &mut gdi).expect("second grab");
        assert_eq!((again.width(), again.height()), (rect.w, rect.h));
        let b = screen.grab(rect).expect("gdi grab");
        let total: u64 = a
            .data()
            .iter()
            .zip(b.data())
            .map(|(x, y)| u64::from(x.abs_diff(*y)))
            .sum();
        let mean = total as f64 / a.data().len() as f64;
        assert!(
            mean < 12.0,
            "duplication differs from GDI by {mean:.1} levels on average"
        );
        // Rectangles off the monitor are refused, not mis-cropped.
        assert!(
            dup.grab(Rect::new(m.rect.x - 50, m.rect.y, 100, 100), &mut gdi)
                .is_none()
        );
        assert!(
            dup.grab(Rect::new(m.rect.x, m.rect.y, 0, 10), &mut gdi)
                .is_none()
        );
    }
}
