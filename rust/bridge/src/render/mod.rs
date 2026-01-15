pub mod cache;
pub mod device;
pub(crate) mod executor;
mod frame;
mod shared;

pub use frame::{ColorTransform, FramePacket, Matrix2D, RenderCmd, RectI, TexUvRect};
pub use shared::SharedCaches;

use crate::render::device::fb3ds::Fb3dsDevice;
use crate::render::device::RenderDevice;
use crate::render::executor::CommandExecutor;

/// High-level renderer facade used by the engine.
///
/// This contains no Ruffle types and talks to the platform only through `RenderDevice`.
pub struct Renderer {
    device: Fb3dsDevice,
    exec: CommandExecutor,
    caches: SharedCaches,
}

impl Renderer {
    pub fn new(caches: SharedCaches) -> Self {
        Self {
            device: Fb3dsDevice::new(),
            exec: CommandExecutor::new(),
            caches,
        }
    }

    pub fn render(&mut self, packet: &FramePacket) {
        self.device.begin_frame();
        self.device.clear(packet.clear);
        self.exec.execute(packet, &mut self.device, &self.caches);
        self.device.end_frame();
    }
}
