use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use niri_ipc::PickedColor;
use zbus::fdo::{self, RequestNameFlags};
use zbus::zvariant::OwnedValue;
use zbus::{interface, zvariant};

use crate::dbus::fdbail;
use crate::niri::{ScreenshotOutput, ScreenshotPipe, ScreenshotTarget};
use crate::utils::LazyWriter;

use super::Start;

pub struct Screenshot {
    to_niri: calloop::channel::Sender<ScreenshotToNiri>,
}

pub enum ScreenshotToNiri {
    TakeScreenshot {
        include_pointer: bool,
        target: ScreenshotTarget,
        out: GnomeScreenshotOutput,
    },
    PickColor(async_oneshot::Sender<Option<PickedColor>>),
}

pub struct GnomeScreenshotOutput {
    filename: PathBuf,
    finish: async_oneshot::Sender<anyhow::Result<()>>,
}
pub struct GnomeScreenshotPipe {
    out: LazyWriter<png::StreamWriter<'static, File>>,
    finish: async_oneshot::Sender<anyhow::Result<()>>,
}

impl ScreenshotOutput for GnomeScreenshotOutput {
    type Pipe = GnomeScreenshotPipe;

    fn image_meta_failed(mut self, err: anyhow::Error) {
        let _ = self.finish.send(Err(err));
    }

    fn image_meta_success(
        self,
        _state: &mut crate::niri::Niri,
        data: crate::niri::ScreenshotData,
    ) -> anyhow::Result<Self::Pipe> {
        Ok(GnomeScreenshotPipe {
            out: LazyWriter::new(move || {
                let file = File::create(self.filename)?;
                Ok(png::Encoder::new(file, data.width, data.height)
                    .write_header()
                    .expect("msg")
                    .into_stream_writer()?)
            }),
            finish: self.finish,
        })
    }
}

impl Write for GnomeScreenshotPipe {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.out.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

impl ScreenshotPipe for GnomeScreenshotPipe {
    type Output = ();

    fn finish_success(mut self) -> anyhow::Result<Self::Output> {
        let _ = self.finish.send(Ok(()));
        Ok(())
    }

    fn finish_failure(mut self, e: anyhow::Error) {
        let _ = self.finish.send(Err(e));
    }
}

#[interface(name = "org.gnome.Shell.Screenshot")]
impl Screenshot {
    async fn screenshot(
        &self,
        include_cursor: bool,
        _flash: bool,
        filename: PathBuf,
    ) -> fdo::Result<(bool, PathBuf)> {
        let filename = if filename.is_absolute() {
            filename
        } else {
            let base = std::env::var_os("XDG_PICTURES_DIR")
                .or_else(|| std::env::var_os("HOME"))
                .unwrap_or_default();
            let base = PathBuf::from(base);
            base.join(filename)
        };

        let (finish, finished) = async_oneshot::oneshot();

        let out = GnomeScreenshotOutput {
            filename: filename.clone(),
            finish,
        };

        if let Err(err) = self.to_niri.send(ScreenshotToNiri::TakeScreenshot {
            include_pointer: include_cursor,
            target: ScreenshotTarget::AllOutputs,
            out,
        }) {
            warn!("error sending message to niri: {err:?}");
            fdbail!("internal error");
        }

        match finished.await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                warn!("error taking screenshot: {e:?}");
                fdbail!("internal error");
            }
            Err(e) => {
                warn!("error receiving message from niri: {e:?}");
                fdbail!("internal error");
            }
        }

        Ok((true, filename))
    }

    async fn pick_color(&self) -> fdo::Result<HashMap<String, OwnedValue>> {
        let (tx, rx) = async_oneshot::oneshot();
        if let Err(err) = self.to_niri.send(ScreenshotToNiri::PickColor(tx)) {
            warn!("error sending pick color message to niri: {err:?}");
            return Err(fdo::Error::Failed("internal error".to_owned()));
        }

        let color = match rx.await {
            Ok(Some(color)) => color,
            Ok(None) => {
                return Err(fdo::Error::Failed("no color picked".to_owned()));
            }
            Err(err) => {
                warn!("error receiving message from niri: {err:?}");
                return Err(fdo::Error::Failed("internal error".to_owned()));
            }
        };

        let mut result = HashMap::new();
        let [r, g, b] = color.rgb;
        result.insert(
            "color".to_string(),
            zvariant::OwnedValue::try_from(zvariant::Value::from((r, g, b))).unwrap(),
        );

        Ok(result)
    }
}

impl Screenshot {
    pub const fn new(to_niri: calloop::channel::Sender<ScreenshotToNiri>) -> Self {
        Self { to_niri }
    }
}

impl Start for Screenshot {
    fn start(self) -> anyhow::Result<zbus::blocking::Connection> {
        let conn = zbus::blocking::Connection::session()?;
        let flags = RequestNameFlags::AllowReplacement
            | RequestNameFlags::ReplaceExisting
            | RequestNameFlags::DoNotQueue;

        conn.object_server()
            .at("/org/gnome/Shell/Screenshot", self)?;
        conn.request_name_with_flags("org.gnome.Shell.Screenshot", flags)?;

        Ok(conn)
    }
}
