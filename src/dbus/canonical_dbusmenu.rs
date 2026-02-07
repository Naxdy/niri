use zbus::{
    fdo::{self, RequestNameFlags},
    interface,
    zvariant::{ObjectPath, OwnedObjectPath},
};

use crate::{
    dbus::{Start, fdbail, fdhow},
    protocols::kde_appmenu::AppmenuPath,
};

pub enum AppmenuToNiri {
    QueryAppmenu(u32, async_oneshot::Sender<Option<AppmenuPath>>),
}

pub struct CanonicalDbusmenu {
    to_niri: calloop::channel::Sender<AppmenuToNiri>,
}
#[interface(name = "com.canonical.AppMenu.Registrar")]
impl CanonicalDbusmenu {
    fn register_window(&self, _window_id: u32, _menu_object_path: ObjectPath) -> fdo::Result<()> {
        fdbail!("xwayland is not supported");
    }
    fn unregister_window(&self, _window_id: u32) -> fdo::Result<()> {
        fdbail!("xwayland is not supported");
    }

    async fn get_menu_for_window(&self, window_id: u32) -> fdo::Result<(String, OwnedObjectPath)> {
        let (tx, rx) = async_oneshot::oneshot();
        self.to_niri
            .send(AppmenuToNiri::QueryAppmenu(window_id, tx))
            .map_err(|e| fdhow!("failed to send appmenu request: {e:?}"))?;
        let appmenu_path = rx
            .await
            .map_err(|e| fdhow!("failed to query appmenu: {e:?}"))?
            .unwrap_or_else(|| AppmenuPath::not_found());

        Ok((appmenu_path.service_name, appmenu_path.path))
    }
}
impl CanonicalDbusmenu {
    pub fn new(to_niri: calloop::channel::Sender<AppmenuToNiri>) -> Self {
        Self { to_niri }
    }
}

impl Start for CanonicalDbusmenu {
    fn start(self) -> anyhow::Result<zbus::blocking::Connection> {
        let conn = zbus::blocking::Connection::session()?;
        let flags = RequestNameFlags::AllowReplacement
            | RequestNameFlags::ReplaceExisting
            | RequestNameFlags::DoNotQueue;

        conn.object_server()
            .at("/com/canonical/AppMenu/Registrar", self)?;
        conn.request_name_with_flags("com.canonical.AppMenu.Registrar", flags)?;

        Ok(conn)
    }
}
