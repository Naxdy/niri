use anyhow::Context;
use smithay::{
    desktop::Window,
    reexports::wayland_server::{Client, Dispatch, DisplayHandle, GlobalDispatch, Resource},
    wayland::seat::WaylandFocus,
};
use wayland_protocols_plasma::plasma_window_management::server::{
    org_kde_plasma_window::OrgKdePlasmaWindow,
    org_kde_plasma_window_management::OrgKdePlasmaWindowManagement,
};

use crate::utils::{get_credentials_for_surface, with_toplevel_role};

pub struct OrgKdePlasmaWindowState {
    window: Window,
}

impl<D> Dispatch<OrgKdePlasmaWindow, OrgKdePlasmaWindowState, D> for OrgKdePlasmaWindowState
where
    D: Dispatch<OrgKdePlasmaWindow, OrgKdePlasmaWindowState, D>,
    D: OrgKdePlasmaWindowManagementHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &OrgKdePlasmaWindow,
        _request: <OrgKdePlasmaWindow as Resource>::Request,
        _data: &OrgKdePlasmaWindowState,
        _dhandle: &DisplayHandle,
        _data_init: &mut smithay::reexports::wayland_server::DataInit<'_, D>,
    ) {
        warn!("manipulating windows using OrgKdePlasmaWindow is not yet supported");
    }
}

pub struct OrgKdePlasmaWindowManagementState {
    windows: Vec<OrgKdePlasmaWindow>,
    /// As per docs: https://wayland.app/protocols/kde-plasma-window-management
    /// "Only one client can bind to this interface at a time."
    binding: Option<OrgKdePlasmaWindowManagement>,
}

impl OrgKdePlasmaWindowManagementState {
    pub fn geometry_changed(&self, window: &Window, plasma_window: Option<&OrgKdePlasmaWindow>) {
        let Some(plasma_window) =
            plasma_window.or_else(|| self.find_plasma_window(window).map(|e| e.0))
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

    pub fn title_changed(&self, window: &Window, plasma_window: Option<&OrgKdePlasmaWindow>) {
        let Some(plasma_window) =
            plasma_window.or_else(|| self.find_plasma_window(window).map(|e| e.0))
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
        let Some((plasma_window, _)) = self.find_plasma_window(window) else {
            warn!("tried to unmap window that isn't mapped");
            return;
        };

        plasma_window.unmapped();

        self.windows.retain(|w| {
            w.data::<OrgKdePlasmaWindowState>()
                .is_some_and(|w| w.window != *window)
        });
    }

    fn find_plasma_window(
        &self,
        window: &Window,
    ) -> Option<(&OrgKdePlasmaWindow, &OrgKdePlasmaWindowState)> {
        self.windows
            .iter()
            .filter_map(|e| Some((e, e.data::<OrgKdePlasmaWindowState>()?)))
            .find(|(_, e)| e.window == *window)
    }

    fn initialize<D>(&mut self, windows: Vec<Window>, handle: &DisplayHandle) -> anyhow::Result<()>
    where
        D: GlobalDispatch<OrgKdePlasmaWindowManagement, OrgKdePlasmaWindowManagementGlobalData>,
        D: Dispatch<OrgKdePlasmaWindowManagement, ()>,
        D: Dispatch<OrgKdePlasmaWindow, OrgKdePlasmaWindowState, D>,
        D: OrgKdePlasmaWindowManagementHandler,
        D: 'static,
    {
        self.windows.clear();

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
        D: Dispatch<OrgKdePlasmaWindow, OrgKdePlasmaWindowState, D>,
        D: OrgKdePlasmaWindowManagementHandler,
        D: 'static,
    {
        let Some(binding) = &self.binding else {
            return Ok(());
        };

        let Some(client) = binding.client() else {
            return Ok(());
        };

        let resource = client
            .create_resource::<OrgKdePlasmaWindow, _, D>(
                handle,
                18,
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

        self.windows.push(resource);

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

        if let Some(binding) = our_state.binding.as_ref()
            && binding.is_alive()
            && binding.client().is_some()
        {
            data_init.post_error(
                resource,
                429u32,
                "OrgKdePlasmaWindowManagement is currently bound by another client",
            );
            return;
        }

        let binding = data_init.init(resource, ());
        our_state.binding = Some(binding);

        if let Err(e) = our_state.initialize::<D>(windows, handle) {
            error!("error during initial state sending: {e:?}");
        }
    }

    fn can_view(client: Client, global_data: &OrgKdePlasmaWindowManagementGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}
