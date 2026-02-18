use std::collections::HashMap;

use zbus::fdo::{self, RequestNameFlags};
use zbus::interface;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{SerializeDict, Type, Value};

use crate::dbus::{DbusInterface, fdhow};

pub struct Introspect {
    to_niri: calloop::channel::Sender<IntrospectToNiri>,
}

pub enum IntrospectToNiri {
    GetWindows(tokio::sync::oneshot::Sender<HashMap<u64, WindowProperties>>),
}

#[derive(Debug, SerializeDict, Type, Value)]
#[zvariant(signature = "dict")]
pub struct WindowProperties {
    /// Window title.
    pub title: String,
    /// Window app ID.
    ///
    /// This is actually the name of the .desktop file, and Shell does internal tracking to match
    /// Wayland app IDs to desktop files. We don't do that yet, which is the reason why
    /// xdg-desktop-portal-gnome's window list is missing icons.
    #[zvariant(rename = "app-id")]
    pub app_id: String,
}

#[interface(name = "org.gnome.Shell.Introspect")]
impl Introspect {
    async fn get_windows(&self) -> fdo::Result<HashMap<u64, WindowProperties>> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        if let Err(err) = self.to_niri.send(IntrospectToNiri::GetWindows(tx)) {
            warn!("error sending message to niri: {err:?}");
            return Err(fdo::Error::Failed("internal error".to_owned()));
        }

        rx.await
            .map_err(|e| fdhow!("error receiving message: {e:?}"))
    }

    // FIXME: call this upon window changes, once more of the infrastructure is there (will be
    // needed for the event stream IPC anyway).
    #[zbus(signal)]
    pub async fn windows_changed(ctxt: &SignalEmitter<'_>) -> zbus::Result<()>;
}

impl DbusInterface for Introspect {
    type InitArgs = ();

    type Message = IntrospectToNiri;

    fn start(self) -> anyhow::Result<zbus::blocking::Connection> {
        let conn = zbus::blocking::Connection::session()?;
        let flags = RequestNameFlags::AllowReplacement
            | RequestNameFlags::ReplaceExisting
            | RequestNameFlags::DoNotQueue;

        conn.object_server()
            .at("/org/gnome/Shell/Introspect", self)?;
        conn.request_name_with_flags("org.gnome.Shell.Introspect", flags)?;

        Ok(conn)
    }

    fn init_interface(
        to_niri: calloop::channel::Sender<Self::Message>,
        _init_args: Self::InitArgs,
    ) -> Self {
        Self { to_niri }
    }

    fn on_callback(msg: Self::Message, state: &mut crate::niri::State) {
        state.on_introspect_msg(msg);
    }
}
