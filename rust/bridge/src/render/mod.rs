pub mod cache;
pub mod device;
pub(crate) mod executor;
mod frame;
mod shared;

pub use frame::{ColorTransform, FramePacket, Matrix2D, RenderCmd, RectI, TexUvRect};
pub use shared::SharedCaches;

#[cfg(feature = "legacy_sw_render")]
use crate::render::device::RenderDevice;
#[cfg(feature = "legacy_sw_render")]
use crate::render::executor::CommandExecutor;
#[cfg(feature = "legacy_sw_render")]
use crate::render::device::fb3ds::Fb3dsDevice;

/// High-level renderer facade used by the engine.
///
/// This contains no Ruffle types and talks to the platform only through `RenderDevice`.
pub struct Renderer {
    #[cfg(feature = "legacy_sw_render")]
    device: Fb3dsDevice,
    #[cfg(feature = "legacy_sw_render")]
    exec: CommandExecutor,
    #[cfg(feature = "legacy_sw_render")]
    caches: SharedCaches,
}

impl Renderer {
    pub fn new(caches: SharedCaches) -> Self {
        #[cfg(not(feature = "legacy_sw_render"))]
        {
            let _ = caches;
        }
        Self {
            #[cfg(feature = "legacy_sw_render")]
            device: Fb3dsDevice::new(),
            #[cfg(feature = "legacy_sw_render")]
            exec: CommandExecutor::new(),
            #[cfg(feature = "legacy_sw_render")]
            caches,
        }
    }

    pub fn render(&mut self, packet: &FramePacket) {
        #[cfg(feature = "legacy_sw_render")]
        {
            self.device.begin_frame();
            self.device.clear(packet.clear);
            self.exec.execute(packet, &mut self.device, &self.caches);
            self.device.end_frame();
        }
        #[cfg(not(feature = "legacy_sw_render"))]
        {
            let _ = packet;
        }
    }
}
