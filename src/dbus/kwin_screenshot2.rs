use std::{collections::HashMap, fs::File};

use smithay::reexports::rustix;
use zbus::{
    fdo::{self, RequestNameFlags},
    interface,
    zvariant::{OwnedValue, Value},
};

use crate::dbus::Start;

pub struct KwinScreenshot2 {
    to_niri: calloop::channel::Sender<KwinScreenshot2ToNiri>,
}

pub struct KwinImageData {
    pub width: u32,
    pub height: u32,
    pub screen: Option<String>,
    pub scale: f64,
}

pub enum KwinScreenshot2ToNiri {
    CaptureScreen {
        // None for current
        name: Option<String>,
        include_cursor: bool,
        data_tx: async_oneshot::Sender<anyhow::Result<KwinImageData>>,
        pipe: File,
    },
}

const QIMAGE_FORMAT_RGBA8888: u32 = 17;

fn image_data_to_dbus(data: KwinImageData) -> HashMap<String, OwnedValue> {
    let mut out = HashMap::new();
    out.insert(
        "type".to_owned(),
        OwnedValue::try_from(Value::from("raw")).unwrap(),
    );
    out.insert(
        "width".to_owned(),
        OwnedValue::try_from(Value::from(data.width)).unwrap(),
    );
    out.insert(
        "height".to_owned(),
        OwnedValue::try_from(Value::from(data.height)).unwrap(),
    );
    out.insert(
        "scale".to_owned(),
        OwnedValue::try_from(Value::from(data.scale)).unwrap(),
    );
    out.insert(
        "format".to_owned(),
        OwnedValue::try_from(Value::from(QIMAGE_FORMAT_RGBA8888)).unwrap(),
    );
    if let Some(screen) = data.screen {
        out.insert(
            "screen".to_owned(),
            OwnedValue::try_from(Value::from(screen)).unwrap(),
        );
    }
    out
}

async fn capture_screen(
    this: &KwinScreenshot2,
    name: Option<String>,
    options: HashMap<String, OwnedValue>,
    pipe: zbus::zvariant::OwnedFd,
) -> fdo::Result<HashMap<String, OwnedValue>> {
    let pipe = rustix::io::fcntl_dupfd_cloexec(pipe, 0)
        .map_err(|e| fdo::Error::Failed(format!("failed to prepare pipe: {e:?}")))?;
    let pipe = File::from(pipe);

    let (data_tx, data_rx) = async_oneshot::oneshot();

    let include_cursor = match options.get("include-cursor").map(bool::try_from) {
        Some(Ok(v)) => v,
        _ => false,
    };

    this.to_niri
        .send(KwinScreenshot2ToNiri::CaptureScreen {
            name,
            include_cursor,
            data_tx,
            pipe,
        })
        .map_err(|e| fdo::Error::Failed(format!("failed to request screenshot: {e:?}")))?;

    let data = match data_rx.await {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return Err(fdo::Error::Failed(e.to_string())),
        Err(e) => {
            return Err(fdo::Error::Failed(format!(
                "failed to request screenshot: {e:?}"
            )));
        }
    };
    Ok(image_data_to_dbus(data))
}

#[interface(name = "org.kde.KWin.ScreenShot2")]
impl KwinScreenshot2 {
    #[zbus(property)]
    fn version(&self) -> u32 {
        4
    }
    async fn capture_active_screen(
        &self,
        options: HashMap<String, OwnedValue>,
        pipe: zbus::zvariant::OwnedFd,
    ) -> fdo::Result<HashMap<String, OwnedValue>> {
        capture_screen(self, None, options, pipe).await
    }
    async fn capture_screen(
        &self,
        name: String,
        options: HashMap<String, OwnedValue>,
        pipe: zbus::zvariant::OwnedFd,
    ) -> fdo::Result<HashMap<String, OwnedValue>> {
        capture_screen(self, Some(name), options, pipe).await
    }
}

impl KwinScreenshot2 {
    pub const fn new(to_niri: calloop::channel::Sender<KwinScreenshot2ToNiri>) -> Self {
        Self { to_niri }
    }
}

impl Start for KwinScreenshot2 {
    fn start(self) -> anyhow::Result<zbus::blocking::Connection> {
        let conn = zbus::blocking::Connection::session()?;
        let flags = RequestNameFlags::AllowReplacement
            | RequestNameFlags::ReplaceExisting
            | RequestNameFlags::DoNotQueue;

        conn.object_server().at("/org/kde/KWin/ScreenShot2", self)?;
        conn.request_name_with_flags("org.kde.KWin.ScreenShot2", flags)?;

        Ok(conn)
    }
}
