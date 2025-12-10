use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use niri_config::{Config, ModKey};
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

use crate::niri::Niri;
use crate::utils::id::IdCounter;

pub mod tty;
pub use tty::Tty;

pub mod winit;
pub use winit::Winit;

pub mod headless;
pub use headless::Headless;

#[allow(clippy::large_enum_variant)]
pub enum Backend {
    Tty(Tty),
    Winit(Winit),
    Headless(Headless),
}

#[derive(PartialEq, Eq)]
pub enum RenderResult {
    /// The frame was submitted to the backend for presentation.
    Submitted,
    /// Rendering succeeded, but there was no damage.
    NoDamage,
    /// The frame was not rendered and submitted, due to an error or otherwise.
    Skipped,
}

pub type IpcOutputMap = HashMap<OutputId, niri_ipc::Output>;

static OUTPUT_ID_COUNTER: IdCounter = IdCounter::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OutputId(u64);

impl OutputId {
    fn next() -> Self {
        Self(OUTPUT_ID_COUNTER.next())
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl Backend {
    pub fn init(&mut self, niri: &mut Niri) {
        let _span = tracy_client::span!("Backend::init");
        match self {
            Self::Tty(tty) => tty.init(niri),
            Self::Winit(winit) => winit.init(niri),
            Self::Headless(headless) => headless.init(niri),
        }
    }

    pub fn seat_name(&self) -> String {
        match self {
            Self::Tty(tty) => tty.seat_name(),
            Self::Winit(winit) => winit.seat_name(),
            Self::Headless(headless) => headless.seat_name(),
        }
    }

    pub fn with_primary_renderer<T>(
        &mut self,
        f: impl FnOnce(&mut GlesRenderer) -> T,
    ) -> Option<T> {
        match self {
            Self::Tty(tty) => tty.with_primary_renderer(f),
            Self::Winit(winit) => winit.with_primary_renderer(f),
            Self::Headless(headless) => headless.with_primary_renderer(f),
        }
    }

    pub fn render(
        &mut self,
        niri: &mut Niri,
        output: &Output,
        target_presentation_time: Duration,
    ) -> RenderResult {
        match self {
            Self::Tty(tty) => tty.render(niri, output, target_presentation_time),
            Self::Winit(winit) => winit.render(niri, output),
            Self::Headless(headless) => headless.render(niri, output),
        }
    }

    pub fn mod_key(&self, config: &Config) -> ModKey {
        match self {
            Self::Winit(_) => config.input.mod_key_nested.unwrap_or({
                if config.input.mod_key == Some(ModKey::Alt) {
                    ModKey::Super
                } else {
                    ModKey::Alt
                }
            }),
            Self::Tty(_) | Self::Headless(_) => config.input.mod_key.unwrap_or(ModKey::Super),
        }
    }

    pub fn change_vt(&mut self, vt: i32) {
        match self {
            Self::Tty(tty) => tty.change_vt(vt),
            Self::Winit(_) => (),
            Self::Headless(_) => (),
        }
    }

    pub fn suspend(&mut self) {
        match self {
            Self::Tty(tty) => tty.suspend(),
            Self::Winit(_) => (),
            Self::Headless(_) => (),
        }
    }

    pub fn toggle_debug_tint(&mut self) {
        match self {
            Self::Tty(tty) => tty.toggle_debug_tint(),
            Self::Winit(winit) => winit.toggle_debug_tint(),
            Self::Headless(_) => (),
        }
    }

    pub fn import_dmabuf(&mut self, dmabuf: &Dmabuf) -> bool {
        match self {
            Self::Tty(tty) => tty.import_dmabuf(dmabuf),
            Self::Winit(winit) => winit.import_dmabuf(dmabuf),
            Self::Headless(headless) => headless.import_dmabuf(dmabuf),
        }
    }

    pub fn early_import(&mut self, surface: &WlSurface) {
        match self {
            Self::Tty(tty) => tty.early_import(surface),
            Self::Winit(_) => (),
            Self::Headless(_) => (),
        }
    }

    pub fn ipc_outputs(&self) -> Arc<Mutex<IpcOutputMap>> {
        match self {
            Self::Tty(tty) => tty.ipc_outputs(),
            Self::Winit(winit) => winit.ipc_outputs(),
            Self::Headless(headless) => headless.ipc_outputs(),
        }
    }

    #[cfg(feature = "xdp-gnome-screencast")]
    pub fn gbm_device(
        &self,
    ) -> Option<smithay::backend::allocator::gbm::GbmDevice<smithay::backend::drm::DrmDeviceFd>>
    {
        match self {
            Self::Tty(tty) => tty.primary_gbm_device(),
            Self::Winit(_) => None,
            Self::Headless(_) => None,
        }
    }

    pub fn set_monitors_active(&mut self, active: bool) {
        match self {
            Self::Tty(tty) => tty.set_monitors_active(active),
            Self::Winit(_) => (),
            Self::Headless(_) => (),
        }
    }

    pub fn set_output_on_demand_vrr(&mut self, niri: &mut Niri, output: &Output, enable_vrr: bool) {
        match self {
            Self::Tty(tty) => tty.set_output_on_demand_vrr(niri, output, enable_vrr),
            Self::Winit(_) => (),
            Self::Headless(_) => (),
        }
    }

    pub fn update_ignored_nodes_config(&mut self, niri: &mut Niri) {
        match self {
            Self::Tty(tty) => tty.update_ignored_nodes_config(niri),
            Self::Winit(_) => (),
            Self::Headless(_) => (),
        }
    }

    pub fn on_output_config_changed(&mut self, niri: &mut Niri) {
        match self {
            Self::Tty(tty) => tty.on_output_config_changed(niri),
            Self::Winit(_) => (),
            Self::Headless(_) => (),
        }
    }

    pub const fn tty_checked(&mut self) -> Option<&mut Tty> {
        if let Self::Tty(v) = self {
            Some(v)
        } else {
            None
        }
    }

    pub fn tty(&mut self) -> &mut Tty {
        if let Self::Tty(v) = self {
            v
        } else {
            panic!("backend is not Tty");
        }
    }

    pub fn winit(&mut self) -> &mut Winit {
        if let Self::Winit(v) = self {
            v
        } else {
            panic!("backend is not Winit")
        }
    }

    pub fn headless(&mut self) -> &mut Headless {
        if let Self::Headless(v) = self {
            v
        } else {
            panic!("backend is not Headless")
        }
    }
}
