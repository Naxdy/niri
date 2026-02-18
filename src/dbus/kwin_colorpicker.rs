use zbus::fdo;
use zbus::fdo::RequestNameFlags;
use zbus::interface;

use crate::dbus::DbusInterface;
use crate::dbus::fdbail;
use crate::dbus::fdhow;

pub enum KwinColorpickerToNiri {
    PickColor(tokio::sync::oneshot::Sender<Option<niri_ipc::PickedColor>>),
}

pub struct KwinColorpicker {
    to_niri: calloop::channel::Sender<KwinColorpickerToNiri>,
}

#[interface(name = "org.kde.kwin.ColorPicker")]
impl KwinColorpicker {
    #[zbus(name = "pick")]
    async fn pick(&self) -> fdo::Result<u32> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        self.to_niri
            .send(KwinColorpickerToNiri::PickColor(tx))
            .map_err(|e| fdhow!("failed to request color pick: {e:?}"))?;

        let Some(color) = rx
            .await
            .map_err(|e| fdhow!("failed to wait for colorpick response: {e:?}"))?
        else {
            fdbail!("no color picked");
        };

        let color: [u8; 4] = [
            255, // alpha
            (color.rgb[0] * 255.) as u8,
            (color.rgb[1] * 255.) as u8,
            (color.rgb[2] * 255.) as u8,
        ];

        Ok(u32::from_be_bytes(color))
    }
}

impl DbusInterface for KwinColorpicker {
    type Message = KwinColorpickerToNiri;
    type InitArgs = ();

    fn start(self) -> anyhow::Result<zbus::blocking::Connection> {
        let conn = zbus::blocking::Connection::session()?;

        let flags = RequestNameFlags::AllowReplacement
            | RequestNameFlags::ReplaceExisting
            | RequestNameFlags::DoNotQueue;

        conn.object_server().at("/ColorPicker", self)?;
        conn.request_name_with_flags("org.kde.kwin.ColorPicker", flags)?;

        Ok(conn)
    }

    fn init_interface(to_niri: calloop::channel::Sender<Self::Message>, _: Self::InitArgs) -> Self {
        Self { to_niri }
    }

    fn on_callback(msg: Self::Message, state: &mut crate::niri::State) {
        match msg {
            KwinColorpickerToNiri::PickColor(sender) => {
                state.handle_pick_color(sender);
            }
        }
    }
}
