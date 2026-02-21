use std::collections::HashMap;

use anyhow::Context;
use smithay::{
    desktop::Window,
    reexports::wayland_server::{Client, Dispatch, DisplayHandle, GlobalDispatch, Resource},
    wayland::seat::WaylandFocus,
};
use wayland_protocols_plasma::plasma_window_management::server::{
    org_kde_plasma_stacking_order::OrgKdePlasmaStackingOrder,
    org_kde_plasma_window::OrgKdePlasmaWindow,
    org_kde_plasma_window_management::OrgKdePlasmaWindowManagement,
};

use crate::utils::{get_credentials_for_surface, with_toplevel_role};

const PROTOCOL_VERSION: u32 = 18;

pub struct OrgKdePlasmaWindowState {
    window: Window,
}

pub struct OrgKdePlasmaWindowManagementState {
    bindings: HashMap<OrgKdePlasmaWindowManagement, Vec<OrgKdePlasmaWindow>>,
}

impl OrgKdePlasmaWindowManagementState {
    pub fn new<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: GlobalDispatch<OrgKdePlasmaWindowManagement, OrgKdePlasmaWindowManagementGlobalData>,
        D: Dispatch<OrgKdePlasmaWindowManagement, ()>,
        D: Dispatch<OrgKdePlasmaWindow, OrgKdePlasmaWindowState, D>,
        D: OrgKdePlasmaWindowManagementHandler,
        D: 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let global_data = OrgKdePlasmaWindowManagementGlobalData {
            filter: Box::new(filter),
        };

        display.create_global::<D, OrgKdePlasmaWindowManagement, _>(PROTOCOL_VERSION, global_data);

        Self {
            bindings: HashMap::new(),
        }
    }
}

impl OrgKdePlasmaWindowManagementState {
    pub fn geometry_changed(&self, plasma_window: &OrgKdePlasmaWindow) {
        let Some(window) = plasma_window
            .data::<OrgKdePlasmaWindowState>()
            .map(|e| &e.window)
        else {
            return;
        };

        let geo = window.geometry();
        plasma_window.geometry(
            geo.loc.x,
            geo.loc.y,
            geo.size.w.abs().try_into().expect("should never overflow"),
            geo.size.h.abs().try_into().expect("should never overflow"),
        );
        plasma_window.client_geometry(
            geo.loc.x,
            geo.loc.y,
            geo.size.w.abs().try_into().expect("should never overflow"),
            geo.size.h.abs().try_into().expect("should never overflow"),
        );
    }

    pub fn title_changed(&self, plasma_window: &OrgKdePlasmaWindow) {
        let Some(window) = plasma_window
            .data::<OrgKdePlasmaWindowState>()
            .map(|e| &e.window)
        else {
            return;
        };

        let Some(toplevel) = window.toplevel() else {
            return;
        };

        with_toplevel_role(toplevel, |role| {
            if let Some(title) = role.title.clone() {
                plasma_window.title_changed(title);
            }
        });
    }

    pub fn unmap_window(&mut self, window: &Window) {
        let plasma_windows = self.find_plasma_windows(window);

        plasma_windows.iter().for_each(|(w, _)| w.unmapped());

        for windows in self.bindings.values_mut() {
            windows.retain(|w| {
                w.data::<OrgKdePlasmaWindowState>()
                    .is_some_and(|w| w.window != *window)
            });
        }
    }

    pub fn each_plasma_window<F>(&self, window: &Window, f: F)
    where
        F: Fn(&OrgKdePlasmaWindow),
    {
        self.find_plasma_windows(window)
            .iter()
            .map(|e| e.0)
            .for_each(f);
    }

    pub fn find_plasma_windows(
        &self,
        window: &Window,
    ) -> Vec<(&OrgKdePlasmaWindow, &OrgKdePlasmaWindowState)> {
        if self.bindings.is_empty() {
            return vec![];
        }

        self.bindings
            .iter()
            .filter_map(|(_, windows)| {
                windows
                    .iter()
                    .filter_map(|e| Some((e, e.data::<OrgKdePlasmaWindowState>()?)))
                    .find(|(_, e)| e.window == *window)
            })
            .collect()
    }

    fn initialize<D>(&mut self, windows: Vec<Window>, handle: &DisplayHandle) -> anyhow::Result<()>
    where
        D: GlobalDispatch<OrgKdePlasmaWindowManagement, OrgKdePlasmaWindowManagementGlobalData>,
        D: Dispatch<OrgKdePlasmaWindowManagement, ()>,
        D: Dispatch<OrgKdePlasmaStackingOrder, ()>,
        D: Dispatch<OrgKdePlasmaWindow, OrgKdePlasmaWindowState, D>,
        D: OrgKdePlasmaWindowManagementHandler,
        D: 'static,
    {
        for window in windows {
            self.send_initial_state::<D>(window, handle)?;
        }

        Ok(())
    }

    fn send_initial_state<D>(
        &mut self,
        window: Window,
        handle: &DisplayHandle,
    ) -> anyhow::Result<()>
    where
        D: GlobalDispatch<OrgKdePlasmaWindowManagement, OrgKdePlasmaWindowManagementGlobalData>,
        D: Dispatch<OrgKdePlasmaWindowManagement, ()>,
        D: Dispatch<OrgKdePlasmaStackingOrder, ()>,
        D: Dispatch<OrgKdePlasmaWindow, OrgKdePlasmaWindowState, D>,
        D: OrgKdePlasmaWindowManagementHandler,
        D: 'static,
    {
        let clients = self
            .bindings
            .iter_mut()
            .filter_map(|(e, windows)| e.client().map(|client| (client, windows)));

        for (client, windows) in clients {
            if windows.iter().any(|e| {
                e.data::<OrgKdePlasmaWindowState>()
                    .is_some_and(|e| e.window == window)
            }) {
                continue;
            };

            let resource = client
                .create_resource::<OrgKdePlasmaWindow, _, D>(
                    handle,
                    PROTOCOL_VERSION,
                    OrgKdePlasmaWindowState {
                        window: window.clone(),
                    },
                )
                .context("failed to create window resource")?;

            if let Some(toplevel) = window.toplevel() {
                with_toplevel_role(toplevel, |role| {
                    if let Some(app_id) = role.app_id.clone() {
                        resource.app_id_changed(app_id);
                    }

                    if let Some(title) = role.title.clone() {
                        resource.title_changed(title);
                    }
                });
            }

            if let Some(surface) = window.wl_surface()
                && let Some(credentials) = get_credentials_for_surface(&surface)
                && let Ok(pid) = credentials.pid.try_into()
            {
                resource.pid_changed(pid);
            }

            {
                let geo = window.geometry();
                resource.geometry(
                    geo.loc.x,
                    geo.loc.y,
                    geo.size.w.abs().try_into().expect("should never overflow"),
                    geo.size.h.abs().try_into().expect("should never overflow"),
                );
                resource.client_geometry(
                    geo.loc.x,
                    geo.loc.y,
                    geo.size.w.abs().try_into().expect("should never overflow"),
                    geo.size.h.abs().try_into().expect("should never overflow"),
                );
            }

            // TODO: virtual desktop entered, parent

            resource.initial_state();

            windows.push(resource);
        }

        Ok(())
    }
}

pub struct OrgKdePlasmaWindowManagementGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

pub trait OrgKdePlasmaWindowManagementHandler {
    fn org_kde_plasma_window_management_state(&mut self) -> &mut OrgKdePlasmaWindowManagementState;
    fn get_windows(&self) -> Vec<Window>;
}

impl<D> GlobalDispatch<OrgKdePlasmaWindowManagement, OrgKdePlasmaWindowManagementGlobalData, D>
    for OrgKdePlasmaWindowManagementState
where
    D: GlobalDispatch<OrgKdePlasmaWindowManagement, OrgKdePlasmaWindowManagementGlobalData>,
    D: Dispatch<OrgKdePlasmaWindowManagement, ()>,
    D: Dispatch<OrgKdePlasmaStackingOrder, ()>,
    D: Dispatch<OrgKdePlasmaWindow, OrgKdePlasmaWindowState, D>,
    D: OrgKdePlasmaWindowManagementHandler,
    D: 'static,
{
    fn bind(
        state: &mut D,
        handle: &smithay::reexports::wayland_server::DisplayHandle,
        _client: &Client,
        resource: smithay::reexports::wayland_server::New<OrgKdePlasmaWindowManagement>,
        _global_data: &OrgKdePlasmaWindowManagementGlobalData,
        data_init: &mut smithay::reexports::wayland_server::DataInit<'_, D>,
    ) {
        let windows = state.get_windows();
        let our_state = state.org_kde_plasma_window_management_state();

        let binding = data_init.init(resource, ());
        our_state.bindings.insert(binding, vec![]);

        if let Err(e) = our_state.initialize::<D>(windows, handle) {
            error!("error during initial state sending: {e:?}");
        }
    }

    fn can_view(client: Client, global_data: &OrgKdePlasmaWindowManagementGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> Dispatch<OrgKdePlasmaWindowManagement, (), D> for OrgKdePlasmaWindowManagementState
where
    D: Dispatch<OrgKdePlasmaWindowManagement, ()>,
    D: Dispatch<OrgKdePlasmaStackingOrder, ()>,
    D: OrgKdePlasmaWindowManagementHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &OrgKdePlasmaWindowManagement,
        request: <OrgKdePlasmaWindowManagement as Resource>::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut smithay::reexports::wayland_server::DataInit<'_, D>,
    ) {
        match request {
            wayland_protocols_plasma::plasma_window_management::server::org_kde_plasma_window_management::Request::GetWindow { id: _, internal_window_id: _ } => {
                warn!("GetWindow is not implemented");
            },
            wayland_protocols_plasma::plasma_window_management::server::org_kde_plasma_window_management::Request::GetWindowByUuid { id: _, internal_window_uuid: _ } => {
                warn!("GetWindowByUuid is not implemented");
            },
            wayland_protocols_plasma::plasma_window_management::server::org_kde_plasma_window_management::Request::GetStackingOrder { stacking_order } => {
                warn!("call to GetStackingOrder, but it's not implemented yet!");
                data_init.init(stacking_order, ());
            },
            e => warn!("unsupported call to OrgKdePlasmaWindowManagement: {e:?}"),
        }
    }
}

impl<D> Dispatch<OrgKdePlasmaStackingOrder, (), D> for OrgKdePlasmaWindowManagementState
where
    D: Dispatch<OrgKdePlasmaWindowManagement, ()>,
    D: Dispatch<OrgKdePlasmaStackingOrder, ()>,
    D: OrgKdePlasmaWindowManagementHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &OrgKdePlasmaStackingOrder,
        request: <OrgKdePlasmaStackingOrder as Resource>::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut smithay::reexports::wayland_server::DataInit<'_, D>,
    ) {
        warn!("unsupported call to OrgKdePlasmaStackingOrder: {request:?}");
    }
}

impl<D> Dispatch<OrgKdePlasmaWindow, OrgKdePlasmaWindowState, D>
    for OrgKdePlasmaWindowManagementState
where
    D: Dispatch<OrgKdePlasmaWindow, OrgKdePlasmaWindowState, D>,
    D: OrgKdePlasmaWindowManagementHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &OrgKdePlasmaWindow,
        request: <OrgKdePlasmaWindow as Resource>::Request,
        _data: &OrgKdePlasmaWindowState,
        _dhandle: &DisplayHandle,
        _data_init: &mut smithay::reexports::wayland_server::DataInit<'_, D>,
    ) {
        warn!("unsupported call to OrgKdePlasmaWindow: {request:?}");
    }
}

#[macro_export]
macro_rules! delegate_org_kde_plasma_window_management {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            wayland_protocols_plasma::plasma_window_management::server::org_kde_plasma_window_management::OrgKdePlasmaWindowManagement: $crate::protocols::kde_window_management::OrgKdePlasmaWindowManagementGlobalData
        ] => $crate::protocols::kde_window_management::OrgKdePlasmaWindowManagementState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            wayland_protocols_plasma::plasma_window_management::server::org_kde_plasma_window_management::OrgKdePlasmaWindowManagement: ()
        ] => $crate::protocols::kde_window_management::OrgKdePlasmaWindowManagementState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            wayland_protocols_plasma::plasma_window_management::server::org_kde_plasma_stacking_order::OrgKdePlasmaStackingOrder: ()
        ] => $crate::protocols::kde_window_management::OrgKdePlasmaWindowManagementState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            wayland_protocols_plasma::plasma_window_management::server::org_kde_plasma_window::OrgKdePlasmaWindow: $crate::protocols::kde_window_management::OrgKdePlasmaWindowState
        ] => $crate::protocols::kde_window_management::OrgKdePlasmaWindowManagementState);
    };
}
