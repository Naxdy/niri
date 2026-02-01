use std::{collections::HashMap, fs::File};

use anyhow::bail;
use smithay::reexports::rustix;
use zbus::{
    fdo::{self, RequestNameFlags},
    interface,
    zvariant::{OwnedValue, Value},
};

use crate::{
    dbus::{Start, fdbail, fdhow},
    niri::{Niri, NoopScreenshotPipe, ScreenshotData, ScreenshotOutput, ScreenshotTarget},
    window::mapped::MappedId,
};

pub struct KwinScreenshot2 {
    to_niri: calloop::channel::Sender<KwinScreenshot2ToNiri>,
}

pub struct KwinScreenshotOutput {
    data_tx: async_oneshot::Sender<anyhow::Result<ScreenshotData>>,
    pipe: File,
}
impl ScreenshotOutput for KwinScreenshotOutput {
    type Pipe = NoopScreenshotPipe<File>;

    fn image_meta_failed(mut self, err: anyhow::Error) {
        // Receiver is dead
        let _ = self.data_tx.send(Err(err));
    }

    fn image_meta_success(
        mut self,
        _state: &mut Niri,
        data: ScreenshotData,
    ) -> anyhow::Result<Self::Pipe> {
        if let Err(e) = self.data_tx.send(Ok(data)) {
            bail!("client no longer waits on the image: {e:?}")
        }
        Ok(NoopScreenshotPipe(self.pipe))
    }
}

pub enum KwinScreenshot2ToNiri {
    Screenshot {
        target: ScreenshotTarget,
        include_pointer: bool,
        out: KwinScreenshotOutput,
    },
    PickWindow(async_oneshot::Sender<Option<MappedId>>),
    PickOutput(async_oneshot::Sender<Option<String>>),
}

const QIMAGE_FORMAT_RGBA8888: u32 = 17;

fn image_data_to_dbus(data: ScreenshotData) -> HashMap<String, OwnedValue> {
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
    if let Some(window_id) = data.window_id {
        out.insert(
            "windowId".to_owned(),
            OwnedValue::try_from(Value::from(window_id)).unwrap(),
        );
    }
    out
}

async fn capture(
    this: &KwinScreenshot2,
    target: ScreenshotTarget,
    options: HashMap<String, OwnedValue>,
    pipe: zbus::zvariant::OwnedFd,
) -> fdo::Result<HashMap<String, OwnedValue>> {
    let pipe = rustix::io::fcntl_dupfd_cloexec(pipe, 0)
        .map_err(|e| fdhow!("failed to prepare pipe: {e:?}"))?;
    let pipe = File::from(pipe);

    let (data_tx, data_rx) = async_oneshot::oneshot();

    let include_pointer = match options.get("include-cursor").map(bool::try_from) {
        Some(Ok(v)) => v,
        _ => false,
    };

    let out = KwinScreenshotOutput { data_tx, pipe };

    this.to_niri
        .send(KwinScreenshot2ToNiri::Screenshot {
            target,
            include_pointer,
            out,
        })
        .map_err(|e| fdhow!("failed to request screenshot: {e:?}"))?;

    let data = match data_rx.await {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => fdbail!("{e:?}"),
        Err(e) => fdbail!("failed to request screenshot: {e:?}"),
    };
    Ok(image_data_to_dbus(data))
}

enum InteractiveKind {
    Window,
    Output,
    Unknown,
}
impl From<u32> for InteractiveKind {
    fn from(value: u32) -> Self {
        match value {
            0 => Self::Window,
            1 => Self::Output,
            _ => Self::Unknown,
        }
    }
}

/// https://github.com/KDE/kwin/blob/b3d8b7085a5186744807300e122f2ef687e943fe/src/plugins/screenshot/org.kde.KWin.ScreenShot2.xml
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
        capture(self, ScreenshotTarget::CurrentOutput, options, pipe).await
    }
    async fn capture_screen(
        &self,
        name: String,
        options: HashMap<String, OwnedValue>,
        pipe: zbus::zvariant::OwnedFd,
    ) -> fdo::Result<HashMap<String, OwnedValue>> {
        capture(self, ScreenshotTarget::Output(name), options, pipe).await
    }
    async fn capture_active_window(
        &self,
        options: HashMap<String, OwnedValue>,
        pipe: zbus::zvariant::OwnedFd,
    ) -> fdo::Result<HashMap<String, OwnedValue>> {
        capture(self, ScreenshotTarget::CurrentWindow, options, pipe).await
    }
    async fn capture_interactive(
        &self,
        kind: u32,
        options: HashMap<String, OwnedValue>,
        pipe: zbus::zvariant::OwnedFd,
    ) -> fdo::Result<HashMap<String, OwnedValue>> {
        match InteractiveKind::from(kind) {
            InteractiveKind::Window => {
                let (tx, rx) = async_oneshot::oneshot();
                self.to_niri
                    .send(KwinScreenshot2ToNiri::PickWindow(tx))
                    .map_err(|e| fdhow!("failed to request window pick: {e:?}"))?;
                let Some(window) = rx
                    .await
                    .map_err(|e| fdhow!("compositor failed to pick window: {e:?}"))?
                else {
                    fdbail!("no window selected");
                };
                capture(self, ScreenshotTarget::Window(window), options, pipe).await
            }
            InteractiveKind::Output => {
                let (tx, rx) = async_oneshot::oneshot();
                self.to_niri
                    .send(KwinScreenshot2ToNiri::PickOutput(tx))
                    .map_err(|e| fdhow!("failed to request window pick: {e:?}"))?;
                let Some(output) = rx
                    .await
                    .map_err(|e| fdhow!("compositor failed to pick output: {e:?}"))?
                else {
                    fdbail!("no output selected");
                };
                capture(self, ScreenshotTarget::Output(output), options, pipe).await
            }
            _ => fdbail!("unsupported pick option"),
        }
    }

    // There is also a capture_workspace method, which is supposed to capture all screens, but it is not used by spectacle,
    // instead spectacle screenshots all outputs and glues them together itself, yay.
    //
    // There is also capture_area, which is being bypassed too.
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
