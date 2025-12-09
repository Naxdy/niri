use smithay::reexports::wayland_server::{
    protocol::wl_surface::WlSurface, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch,
    New, Resource,
};
use wayland_protocols_plasma::blur::server::{
    org_kde_kwin_blur::OrgKdeKwinBlur, org_kde_kwin_blur_manager::OrgKdeKwinBlurManager,
};

const PROTOCOL_VERSION: u32 = 1;

pub struct OrgKdeKwinBlurState {
    pub surface: WlSurface,
}

pub struct OrgKdeKwinBlurManagerState {}

impl OrgKdeKwinBlurManagerState {
    pub fn new<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: GlobalDispatch<OrgKdeKwinBlurManager, OrgKdeKwinBlurManagerGlobalData>,
        D: Dispatch<OrgKdeKwinBlurManager, ()>,
        D: OrgKdeKwinBlurManagerHandler,
        D: 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let global_data = OrgKdeKwinBlurManagerGlobalData {
            filter: Box::new(filter),
        };

        display.create_global::<D, OrgKdeKwinBlurManager, _>(PROTOCOL_VERSION, global_data);

        Self {}
    }
}

pub struct OrgKdeKwinBlurManagerGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

pub trait OrgKdeKwinBlurManagerHandler {
    fn org_kde_kwin_blur_manager_state(&mut self) -> &mut OrgKdeKwinBlurManagerState;
    fn enable_blur(&mut self, surface: &WlSurface);
    fn disable_blur(&mut self, surface: &WlSurface);
}

impl<D> GlobalDispatch<OrgKdeKwinBlurManager, OrgKdeKwinBlurManagerGlobalData, D>
    for OrgKdeKwinBlurManagerState
where
    D: GlobalDispatch<OrgKdeKwinBlurManager, OrgKdeKwinBlurManagerGlobalData>,
    D: Dispatch<OrgKdeKwinBlurManager, ()>,
    D: Dispatch<OrgKdeKwinBlur, OrgKdeKwinBlurState>,
    D: OrgKdeKwinBlurManagerHandler,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<OrgKdeKwinBlurManager>,
        _global_data: &OrgKdeKwinBlurManagerGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, global_data: &OrgKdeKwinBlurManagerGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> Dispatch<OrgKdeKwinBlurManager, (), D> for OrgKdeKwinBlurManagerState
where
    D: Dispatch<OrgKdeKwinBlurManager, ()>,
    D: Dispatch<OrgKdeKwinBlur, OrgKdeKwinBlurState>,
    D: OrgKdeKwinBlurManagerHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        _resource: &OrgKdeKwinBlurManager,
        request: <OrgKdeKwinBlurManager as Resource>::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wayland_protocols_plasma::blur::server::org_kde_kwin_blur_manager::Request::Create { id, surface } => {
                data_init.init(id, OrgKdeKwinBlurState {
                    surface
                });
            },
            wayland_protocols_plasma::blur::server::org_kde_kwin_blur_manager::Request::Unset { surface } => {
                state.disable_blur(&surface);
            },
            e => {
                warn!("unsupported call to OrgKdeKwinBlurManager: {e:?}");
            },
        }
    }
}

impl<D> Dispatch<OrgKdeKwinBlur, OrgKdeKwinBlurState, D> for OrgKdeKwinBlurManagerState
where
    D: Dispatch<OrgKdeKwinBlur, OrgKdeKwinBlurState, D>,
    D: OrgKdeKwinBlurManagerHandler,
{
    fn request(
        state: &mut D,
        _client: &Client,
        _resource: &OrgKdeKwinBlur,
        request: <OrgKdeKwinBlur as Resource>::Request,
        data: &OrgKdeKwinBlurState,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wayland_protocols_plasma::blur::server::org_kde_kwin_blur::Request::Commit => {
                state.enable_blur(&data.surface);
            }
            wayland_protocols_plasma::blur::server::org_kde_kwin_blur::Request::SetRegion {
                region: _,
            } => {
                // setting blur on a specific WlRegion is not yet supported
            }
            wayland_protocols_plasma::blur::server::org_kde_kwin_blur::Request::Release => {}
            e => {
                warn!("unsupported call to OrgKdeKwinBlur {e:?}");
            }
        }
    }
}

#[macro_export]
macro_rules! delegate_org_kde_kwin_blur {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            wayland_protocols_plasma::blur::server::org_kde_kwin_blur_manager::OrgKdeKwinBlurManager: $crate::protocols::kde_blur::OrgKdeKwinBlurManagerGlobalData
        ] => $crate::protocols::kde_blur::OrgKdeKwinBlurManagerState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            wayland_protocols_plasma::blur::server::org_kde_kwin_blur_manager::OrgKdeKwinBlurManager: ()
        ] => $crate::protocols::kde_blur::OrgKdeKwinBlurManagerState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            wayland_protocols_plasma::blur::server::org_kde_kwin_blur::OrgKdeKwinBlur: $crate::protocols::kde_blur::OrgKdeKwinBlurState
        ] => $crate::protocols::kde_blur::OrgKdeKwinBlurManagerState);
    };
}
