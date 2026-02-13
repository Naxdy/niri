use std::collections::HashMap;

use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use wayland_backend::server::ClientId;
use wayland_protocols_plasma::output_order::v1::server::kde_output_order_v1::KdeOutputOrderV1;

use crate::backend::OutputId;

const PROTOCOL_VERSION: u32 = 1;

pub struct KdeOutputOrderV1State {
    instances: HashMap<ClientId, KdeOutputOrderV1>,
    output_order: Vec<String>,
}
impl KdeOutputOrderV1State {
    fn add_instance(&mut self, client_id: ClientId, inst: KdeOutputOrderV1) {
        self.notify_instance(&inst);
        self.instances.insert(client_id, inst);
    }
    fn remove_instance(&mut self, client_id: ClientId) {
        self.instances.remove(&client_id);
    }
    fn notify_instance(&self, inst: &KdeOutputOrderV1) {
        if self.output_order.is_empty() {
            // Output order is not yet determined
            return;
        }
        for output_name in self.output_order.iter().cloned() {
            inst.output(output_name);
        }
        inst.done();
    }
    fn notify_all(&self) {
        for (_, instance) in &self.instances {
            self.notify_instance(instance);
        }
    }
    pub fn notify_changes(&mut self, new_state: &HashMap<OutputId, niri_ipc::Output>) {
        let mut order: Vec<String> = new_state.values().map(|o| o.name.clone()).collect();
        // TODO: Needs to be in more specific order?
        // Sort here is just for order to be stable
        order.sort();

        self.output_order = order;
        self.notify_all();
    }
}

pub struct KdeOutputOrderV1GlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

impl KdeOutputOrderV1State {
    pub fn new<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: GlobalDispatch<KdeOutputOrderV1, KdeOutputOrderV1GlobalData>,
        D: Dispatch<KdeOutputOrderV1, ()>,
        D: 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let global_data = KdeOutputOrderV1GlobalData {
            filter: Box::new(filter),
        };
        display.create_global::<D, KdeOutputOrderV1, _>(PROTOCOL_VERSION, global_data);

        Self {
            instances: HashMap::new(),
            output_order: Vec::new(),
        }
    }
}

impl<D> GlobalDispatch<KdeOutputOrderV1, KdeOutputOrderV1GlobalData, D> for KdeOutputOrderV1State
where
    D: Dispatch<KdeOutputOrderV1, ()>,
    D: KdeOutputOrderV1Handler,
{
    fn bind(
        state: &mut D,
        _handle: &DisplayHandle,
        client: &Client,
        resource: New<KdeOutputOrderV1>,
        _global_data: &KdeOutputOrderV1GlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        let instance = data_init.init(resource, ());
        state
            .kde_output_order_v1_state()
            .add_instance(client.id(), instance);
    }

    fn can_view(client: Client, global_data: &KdeOutputOrderV1GlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> Dispatch<KdeOutputOrderV1, (), D> for KdeOutputOrderV1State
where
    D: KdeOutputOrderV1Handler,
{
    fn request(
        state: &mut D,
        client: &Client,
        _resource: &KdeOutputOrderV1,
        request: <KdeOutputOrderV1 as Resource>::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
             wayland_protocols_plasma::output_order::v1::server::kde_output_order_v1::Request::Destroy => {
                 state.kde_output_order_v1_state().remove_instance(client.id());
             },
             _ => warn!("unsupported call to KdeOutputOrderV1"),
        }
    }
}

pub trait KdeOutputOrderV1Handler {
    fn kde_output_order_v1_state(&mut self) -> &mut KdeOutputOrderV1State;
}
#[macro_export]
macro_rules! delegate_kde_output_order_v1 {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            wayland_protocols_plasma::output_order::v1::server::kde_output_order_v1::KdeOutputOrderV1: $crate::protocols::kde_outputorder::KdeOutputOrderV1GlobalData
        ] => $crate::protocols::kde_outputorder::KdeOutputOrderV1State);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            wayland_protocols_plasma::output_order::v1::server::kde_output_order_v1::KdeOutputOrderV1: ()
        ] => $crate::protocols::kde_outputorder::KdeOutputOrderV1State);
    };
}
