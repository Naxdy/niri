use std::{collections::HashMap, fs::File, io::Write, os::fd::OwnedFd, thread};

use anyhow::Context as _;
use smithay::reexports::rustix;
use zbus::{
    fdo::{self, RequestNameFlags},
    interface,
    zvariant::{OwnedValue, Value},
};

use crate::dbus::Start;

async fn spawn_blocking<R, F>(f: F) -> R
where
    F: 'static + Send + FnOnce() -> R,
    R: Send + 'static,
{
    // Why isn't there oneshot channels, at least?..
    // I haven't found threadpool in any of the libraries either, and thread::spawn looks dirty...
    let (tx, rx) = async_channel::bounded(1);
    thread::spawn(move || {
        tx.send_blocking(f())
            .expect("capacity == 1, reader is alive");
    });
    rx.recv().await.expect("thread panicked")
}

fn delayed_task<F>(f: F)
where
    F: FnOnce() -> anyhow::Result<()>,
    F: Send + 'static,
{
    thread::spawn(move || {
        if let Err(e) = f() {
            warn!("executing delayed task failed: {e:#}");
        }
    });
}

pub struct KwinScreenshot2 {
    to_niri: calloop::channel::Sender<KwinScreenshot2ToNiri>,

    // Spectacle screenshoting utility is obtaining output list from elsewhere,
    // and winit niri doesn't know about DP-1 etc outputs, this option overrides all the queries with winit output instead.
    fake_session: bool,
}

pub struct KwinImageData {
    pub width: u32,
    pub height: u32,
    // For proper region capture it also needs scaling and monitor positions,
    // but then rgba8888 format is not suitable
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
        "format".to_owned(),
        OwnedValue::try_from(Value::from(QIMAGE_FORMAT_RGBA8888)).unwrap(),
    );
    Ok(out)
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
        mut name: String,
        options: HashMap<String, OwnedValue>,
        pipe: zbus::zvariant::OwnedFd,
    ) -> fdo::Result<HashMap<String, OwnedValue>> {
        if self.fake_session {
            name = "winit".to_owned();
        }
        capture_screen(self, Some(name), options, pipe).await
    }
}

impl KwinScreenshot2 {
    pub const fn new(
        to_niri: calloop::channel::Sender<KwinScreenshot2ToNiri>,
        fake_session: bool,
    ) -> Self {
        Self {
            to_niri,
            fake_session,
        }
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
