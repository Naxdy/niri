use smithay::reexports::{
    wayland_protocols::ext::background_effect::v1::server::{
        ext_background_effect_manager_v1::{self, ExtBackgroundEffectManagerV1},
        ext_background_effect_surface_v1::{self, ExtBackgroundEffectSurfaceV1},
    },
    wayland_server::{
        protocol::wl_surface::WlSurface, Client, Dispatch, DisplayHandle, GlobalDispatch,
    },
};

const PROTOCOL_VERSION: u32 = 1;

pub struct ExtBackgroundEffectSurfaceState {
    pub surface: WlSurface,
}

pub struct ExtBackgroundEffectManagerState {}

impl ExtBackgroundEffectManagerState {
    pub fn new<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: GlobalDispatch<ExtBackgroundEffectManagerV1, ExtBackgroundEffectManagerGlobalData>,
        D: Dispatch<ExtBackgroundEffectManagerV1, ()>,
        D: ExtBackgroundEffectManagerHandler,
        D: 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let global_data = ExtBackgroundEffectManagerGlobalData {
            filter: Box::new(filter),
        };

        display.create_global::<D, ExtBackgroundEffectManagerV1, _>(PROTOCOL_VERSION, global_data);

        Self {}
    }
}

pub trait ExtBackgroundEffectManagerHandler {
    fn ext_background_effect_manager_state(&mut self) -> &mut ExtBackgroundEffectManagerState;
    fn enable_blur(&mut self, surface: &WlSurface);
    fn disable_blur(&mut self, surface: &WlSurface);
}

pub struct ExtBackgroundEffectManagerGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

impl<D> GlobalDispatch<ExtBackgroundEffectManagerV1, ExtBackgroundEffectManagerGlobalData, D>
    for ExtBackgroundEffectManagerState
where
    D: GlobalDispatch<ExtBackgroundEffectManagerV1, ExtBackgroundEffectManagerGlobalData>,
    D: Dispatch<ExtBackgroundEffectManagerV1, ()>,
    D: Dispatch<ExtBackgroundEffectSurfaceV1, ExtBackgroundEffectSurfaceState>,
    D: ExtBackgroundEffectManagerHandler,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _handle: &smithay::reexports::wayland_server::DisplayHandle,
        _client: &smithay::reexports::wayland_server::Client,
        resource: smithay::reexports::wayland_server::New<ExtBackgroundEffectManagerV1>,
        _global_data: &ExtBackgroundEffectManagerGlobalData,
        data_init: &mut smithay::reexports::wayland_server::DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(
        client: smithay::reexports::wayland_server::Client,
        global_data: &ExtBackgroundEffectManagerGlobalData,
    ) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> Dispatch<ExtBackgroundEffectManagerV1, (), D> for ExtBackgroundEffectManagerState
where
    D: Dispatch<ExtBackgroundEffectManagerV1, ()>,
    D: Dispatch<ExtBackgroundEffectSurfaceV1, ExtBackgroundEffectSurfaceState>,
    D: ExtBackgroundEffectManagerHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &ExtBackgroundEffectManagerV1,
        request: <ExtBackgroundEffectManagerV1 as smithay::reexports::wayland_server::Resource>::Request,
        _data: &(),
        _dhandle: &smithay::reexports::wayland_server::DisplayHandle,
        data_init: &mut smithay::reexports::wayland_server::DataInit<'_, D>,
    ) {
        match request {
            ext_background_effect_manager_v1::Request::Destroy => {}
            ext_background_effect_manager_v1::Request::GetBackgroundEffect { id, surface } => {
                data_init.init(id, ExtBackgroundEffectSurfaceState { surface });
            }
            e => warn!("unsupported call to ExtBackgroundEffectManager: {e:?}"),
        }
    }
}

impl<D> Dispatch<ExtBackgroundEffectSurfaceV1, ExtBackgroundEffectSurfaceState, D>
    for ExtBackgroundEffectManagerState
where
    D: Dispatch<ExtBackgroundEffectSurfaceV1, ExtBackgroundEffectSurfaceState, D>,
    D: ExtBackgroundEffectManagerHandler,
{
    fn request(
        state: &mut D,
        _client: &Client,
        _resource: &ExtBackgroundEffectSurfaceV1,
        request: <ExtBackgroundEffectSurfaceV1 as smithay::reexports::wayland_server::Resource>::Request,
        data: &ExtBackgroundEffectSurfaceState,
        _dhandle: &smithay::reexports::wayland_server::DisplayHandle,
        _data_init: &mut smithay::reexports::wayland_server::DataInit<'_, D>,
    ) {
        match request {
            ext_background_effect_surface_v1::Request::Destroy => {
                state.disable_blur(&data.surface);
            }
            ext_background_effect_surface_v1::Request::SetBlurRegion { region } => {
                // We currently only have "all or nothing" blur, meaning if the region is not
                // `NULL`, we enable it, otherwise we disable it.
                if region.is_some() {
                    state.enable_blur(&data.surface);
                } else {
                    state.disable_blur(&data.surface);
                }
            }
            e => warn!("unsupported call to ExtBackgroundEffectSurface: {e:?}"),
        }
    }
}

#[macro_export]
macro_rules! delegate_ext_background_effect {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols::ext::background_effect::v1::server::ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1: $crate::protocols::ext_background_effect::ExtBackgroundEffectManagerGlobalData
        ] => $crate::protocols::ext_background_effect::ExtBackgroundEffectManagerState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols::ext::background_effect::v1::server::ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1: ()
        ] => $crate::protocols::ext_background_effect::ExtBackgroundEffectManagerState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols::ext::background_effect::v1::server::ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1: $crate::protocols::ext_background_effect::ExtBackgroundEffectSurfaceState
        ] => $crate::protocols::ext_background_effect::ExtBackgroundEffectManagerState);
    };
}
