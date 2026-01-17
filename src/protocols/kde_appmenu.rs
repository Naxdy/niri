use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, Resource,
    protocol::wl_surface::WlSurface,
};
use wayland_protocols_plasma::appmenu::server::{
    org_kde_kwin_appmenu::OrgKdeKwinAppmenu, org_kde_kwin_appmenu_manager::OrgKdeKwinAppmenuManager,
};
use zbus::zvariant::{ObjectPath, OwnedObjectPath};

#[derive(Clone, Debug)]
pub struct AppmenuPath {
    pub service_name: String,
    pub path: OwnedObjectPath,
}
impl AppmenuPath {
    pub fn not_found() -> Self {
        Self {
            service_name: "".to_owned(),
            path: OwnedObjectPath::try_from("/").expect("/ is a valid dbus object path"),
        }
    }
}

use crate::niri::State;

const PROTOCOL_VERSION: u32 = 2;

pub struct OrgKdeKwinAppmenuManagerState {}

pub struct OrgKdeKwinAppmenuState {
    surface: WlSurface,
}

pub struct OrgKdeKwinAppmenuManagerGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

impl OrgKdeKwinAppmenuManagerState {
    pub fn new<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: GlobalDispatch<OrgKdeKwinAppmenuManager, OrgKdeKwinAppmenuManagerGlobalData>,
        D: Dispatch<OrgKdeKwinAppmenuManager, ()>,
        D: OrgKdeKwinAppmenuManagerHandler,
        D: 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let global_data = OrgKdeKwinAppmenuManagerGlobalData {
            filter: Box::new(filter),
        };

        display.create_global::<D, OrgKdeKwinAppmenuManager, _>(PROTOCOL_VERSION, global_data);

        Self {}
    }
}

impl<D> Dispatch<OrgKdeKwinAppmenuManager, (), D> for OrgKdeKwinAppmenuManagerState
where
    D: Dispatch<OrgKdeKwinAppmenuManager, ()>,
    D: Dispatch<OrgKdeKwinAppmenu, OrgKdeKwinAppmenuState>,
    D: OrgKdeKwinAppmenuManagerHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &OrgKdeKwinAppmenuManager,
        request: <OrgKdeKwinAppmenuManager as Resource>::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wayland_protocols_plasma::appmenu::server::org_kde_kwin_appmenu_manager::Request::Create { id, surface } => {
                data_init.init(id, OrgKdeKwinAppmenuState {
                    surface,
                });
            },
            wayland_protocols_plasma::appmenu::server::org_kde_kwin_appmenu_manager::Request::Release => {
            },
            e => {
                warn!("unsupported call to OrgKdeKwinAppmenuManager: {e:?}");
            },
        }
    }
}

impl GlobalDispatch<OrgKdeKwinAppmenuManager, OrgKdeKwinAppmenuManagerGlobalData, State>
    for OrgKdeKwinAppmenuManagerState
{
    fn bind(
        _state: &mut State,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: smithay::reexports::wayland_server::New<OrgKdeKwinAppmenuManager>,
        _global_data: &OrgKdeKwinAppmenuManagerGlobalData,
        data_init: &mut DataInit<'_, State>,
    ) {
        info!("init appmenu manager");
        data_init.init(resource, ());
    }

    fn can_view(client: Client, global_data: &OrgKdeKwinAppmenuManagerGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl Dispatch<OrgKdeKwinAppmenu, OrgKdeKwinAppmenuState, State> for OrgKdeKwinAppmenuManagerState {
    fn request(
        state: &mut State,
        _client: &Client,
        _resource: &OrgKdeKwinAppmenu,
        request: <OrgKdeKwinAppmenu as Resource>::Request,
        data: &OrgKdeKwinAppmenuState,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        match request {
            wayland_protocols_plasma::appmenu::server::org_kde_kwin_appmenu::Request::SetAddress { service_name, object_path } => {
                let Ok(object_path) = ObjectPath::try_from(object_path) else {
                    warn!("SetAddress was called with invalid dbus object path");
                    return;
                };
                state.set_appmenu(&data.surface, Some(AppmenuPath { service_name, path: OwnedObjectPath::from(object_path) }));
            },
            wayland_protocols_plasma::appmenu::server::org_kde_kwin_appmenu::Request::Release => {
                state.set_appmenu(&data.surface, None);
            },
            e => {warn!("unsupported call to OrgKdeKwinAppmenu: {e:?}")}
        }
    }
}

pub trait OrgKdeKwinAppmenuManagerHandler {
    fn set_appmenu(&mut self, surface: &WlSurface, path: Option<AppmenuPath>);
}

#[macro_export]
macro_rules! delegate_org_kde_kwin_appmenu {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            wayland_protocols_plasma::appmenu::server::org_kde_kwin_appmenu_manager::OrgKdeKwinAppmenuManager: $crate::protocols::kde_appmenu::OrgKdeKwinAppmenuManagerGlobalData
        ] => $crate::protocols::kde_appmenu::OrgKdeKwinAppmenuManagerState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            wayland_protocols_plasma::appmenu::server::org_kde_kwin_appmenu_manager::OrgKdeKwinAppmenuManager: ()
        ] => $crate::protocols::kde_appmenu::OrgKdeKwinAppmenuManagerState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            wayland_protocols_plasma::appmenu::server::org_kde_kwin_appmenu::OrgKdeKwinAppmenu: $crate::protocols::kde_appmenu::OrgKdeKwinAppmenuState
        ] => $crate::protocols::kde_appmenu::OrgKdeKwinAppmenuManagerState);
    };
}
