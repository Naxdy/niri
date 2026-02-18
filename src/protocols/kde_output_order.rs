use smithay::{
    output::Output,
    reexports::wayland_server::{Client, Dispatch, DisplayHandle, GlobalDispatch, Resource},
};
use wayland_protocols_plasma::output_order::v1::server::kde_output_order_v1::{
    KdeOutputOrderV1, Request,
};

const PROTOCOL_VERSION: u32 = 1;

pub struct KdeOutputOrderV1GlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

pub struct KdeOutputOrderV1State {
    output_order: Vec<Output>,
    bindings: Vec<KdeOutputOrderV1>,
}

impl KdeOutputOrderV1State {
    pub fn new<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: GlobalDispatch<KdeOutputOrderV1, KdeOutputOrderV1GlobalData>,
        D: Dispatch<KdeOutputOrderV1, ()>,
        D: KdeOutputOrderV1Handler,
        D: 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let global_data = KdeOutputOrderV1GlobalData {
            filter: Box::new(filter),
        };

        display.create_global::<D, KdeOutputOrderV1, _>(PROTOCOL_VERSION, global_data);

        Self {
            bindings: Vec::new(),
            output_order: Vec::new(),
        }
    }

    pub fn update_output_order(&mut self, outputs: Vec<Output>) {
        self.output_order = outputs;
        self.broadcast_output_change();
    }

    fn broadcast_output_change(&self) {
        for binding in &self.bindings {
            self.send_events(binding);
        }
    }

    fn add_binding(&mut self, resource: KdeOutputOrderV1) {
        if !self.bindings.iter().any(|e| e.id() == resource.id()) {
            self.bindings.push(resource);
        }
    }

    fn remove_binding(&mut self, resource: &KdeOutputOrderV1) {
        self.bindings.retain(|e| e.id() != resource.id());
    }

    fn send_events(&self, resource: &KdeOutputOrderV1) {
        for output in &self.output_order {
            resource.output(output.name());
        }

        resource.done();
    }
}

pub trait KdeOutputOrderV1Handler {
    fn kde_output_order_v1_state(&mut self) -> &mut KdeOutputOrderV1State;
}

impl<D> GlobalDispatch<KdeOutputOrderV1, KdeOutputOrderV1GlobalData, D> for KdeOutputOrderV1State
where
    D: GlobalDispatch<KdeOutputOrderV1, KdeOutputOrderV1GlobalData>,
    D: Dispatch<KdeOutputOrderV1, ()>,
    D: KdeOutputOrderV1Handler,
    D: 'static,
{
    fn bind(
        state: &mut D,
        _handle: &smithay::reexports::wayland_server::DisplayHandle,
        _client: &Client,
        resource: smithay::reexports::wayland_server::New<KdeOutputOrderV1>,
        _global_data: &KdeOutputOrderV1GlobalData,
        data_init: &mut smithay::reexports::wayland_server::DataInit<'_, D>,
    ) {
        let r = data_init.init(resource, ());
        let kde_state = state.kde_output_order_v1_state();
        kde_state.send_events(&r);
        kde_state.add_binding(r);
    }

    fn can_view(client: Client, global_data: &KdeOutputOrderV1GlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> Dispatch<KdeOutputOrderV1, (), D> for KdeOutputOrderV1State
where
    D: Dispatch<KdeOutputOrderV1, ()>,
    D: KdeOutputOrderV1Handler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        resource: &KdeOutputOrderV1,
        request: <KdeOutputOrderV1 as Resource>::Request,
        _data: &(),
        _dhandle: &smithay::reexports::wayland_server::DisplayHandle,
        _data_init: &mut smithay::reexports::wayland_server::DataInit<'_, D>,
    ) {
        match request {
            Request::Destroy => {
                state.kde_output_order_v1_state().remove_binding(resource);
            }
            e => {
                warn!("unsupported cvall to KdeOutputOrderV1: {e:?}");
            }
        }
    }
}

#[macro_export]
macro_rules! delegate_kde_output_order_v1 {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            wayland_protocols_plasma::output_order::v1::server::kde_output_order_v1::KdeOutputOrderV1: $crate::protocols::kde_output_order::KdeOutputOrderV1GlobalData
        ] => $crate::protocols::kde_output_order::KdeOutputOrderV1State);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            wayland_protocols_plasma::output_order::v1::server::kde_output_order_v1::KdeOutputOrderV1: ()
        ] => $crate::protocols::kde_output_order::KdeOutputOrderV1State);
    };
}
