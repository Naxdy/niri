use calloop::channel::Event;
use zbus::blocking::Connection;
use zbus::object_server::Interface;

use crate::dbus::kwin_colorpicker::KwinColorpicker;
use crate::niri::{Niri, State};

pub mod freedesktop_a11y;
pub mod freedesktop_locale1;
pub mod freedesktop_login1;
pub mod freedesktop_screensaver;
pub mod gnome_shell_introspect;
pub mod kwin_colorpicker;
pub mod kwin_screenshot2;
pub mod mutter_display_config;
pub mod mutter_service_channel;

#[cfg(feature = "xdp-gnome-screencast")]
pub mod mutter_screen_cast;
#[cfg(feature = "xdp-gnome-screencast")]
use mutter_screen_cast::ScreenCast;

use self::freedesktop_a11y::KeyboardMonitor;
use self::freedesktop_screensaver::ScreenSaver;
use self::gnome_shell_introspect::Introspect;
use self::kwin_screenshot2::KwinScreenshot2;
use self::mutter_display_config::DisplayConfig;
use self::mutter_service_channel::ServiceChannel;

trait DbusInterface: Sized + Interface {
    type InitArgs;
    type Message;

    fn init_interface(
        to_niri: calloop::channel::Sender<Self::Message>,
        init_args: Self::InitArgs,
    ) -> Self;

    fn on_callback(msg: Self::Message, state: &mut State);

    fn start(self) -> anyhow::Result<zbus::blocking::Connection>;
}

#[derive(Default)]
pub struct DBusServers {
    pub conn_service_channel: Option<Connection>,
    pub conn_display_config: Option<Connection>,
    pub conn_screen_saver: Option<Connection>,
    pub conn_introspect: Option<Connection>,
    #[cfg(feature = "xdp-gnome-screencast")]
    pub conn_screen_cast: Option<Connection>,
    pub conn_login1: Option<Connection>,
    pub conn_locale1: Option<Connection>,
    pub conn_keyboard_monitor: Option<Connection>,
    pub conn_kwin_screenshot2: Option<Connection>,
    pub conn_kwin_colorpicker: Option<Connection>,
}

impl DBusServers {
    pub fn start(state: &mut State, is_session_instance: bool) {
        let _span = tracy_client::span!("DBusServers::start");

        let backend = &state.backend;
        let niri = &mut state.niri;

        let mut dbus = Self::default();

        if is_session_instance {
            dbus.conn_service_channel = start_interface::<ServiceChannel>(niri, ()).unwrap();
        }

        if is_session_instance
            || niri
                .config
                .borrow()
                .debug
                .dbus_interfaces_in_non_session_instances
        {
            dbus.conn_kwin_colorpicker = start_interface::<KwinColorpicker>(niri, ()).unwrap();

            dbus.conn_kwin_screenshot2 = start_interface::<KwinScreenshot2>(niri, ()).unwrap();

            dbus.conn_display_config =
                start_interface::<DisplayConfig>(niri, backend.ipc_outputs()).unwrap();

            dbus.conn_screen_saver =
                try_start(ScreenSaver::new(niri.is_fdo_idle_inhibited.clone()));

            dbus.conn_introspect = start_interface::<Introspect>(niri, ()).unwrap();

            #[cfg(feature = "xdp-gnome-screencast")]
            {
                dbus.conn_screen_cast =
                    start_interface::<ScreenCast>(niri, backend.ipc_outputs()).unwrap();
            }

            let keyboard_monitor = KeyboardMonitor::new();
            if let Some(x) = try_start(keyboard_monitor.clone()) {
                dbus.conn_keyboard_monitor = Some(x);
                niri.a11y_keyboard_monitor = Some(keyboard_monitor);
            }
        }

        let (to_niri, from_login1) = calloop::channel::channel();
        niri.event_loop
            .insert_source(from_login1, move |event, _, state| match event {
                calloop::channel::Event::Msg(msg) => state.on_login1_msg(msg),
                calloop::channel::Event::Closed => (),
            })
            .unwrap();

        match freedesktop_login1::start(to_niri) {
            Ok(conn) => {
                dbus.conn_login1 = Some(conn);
            }
            Err(err) => {
                warn!("error starting login1 watcher: {err:?}");
            }
        }

        let (to_niri, from_locale1) = calloop::channel::channel();
        niri.event_loop
            .insert_source(from_locale1, move |event, _, state| match event {
                calloop::channel::Event::Msg(msg) => state.on_locale1_msg(msg),
                calloop::channel::Event::Closed => (),
            })
            .unwrap();
        match freedesktop_locale1::start(to_niri) {
            Ok(conn) => {
                dbus.conn_locale1 = Some(conn);
            }
            Err(err) => {
                warn!("error starting locale1 watcher: {err:?}");
            }
        }

        niri.dbus = Some(dbus);
    }
}

fn start_interface<T>(niri: &mut Niri, init_args: T::InitArgs) -> anyhow::Result<Option<Connection>>
where
    T: DbusInterface,
{
    let (to_niri, from_interface) = calloop::channel::channel();
    let interface = T::init_interface(to_niri, init_args);

    niri.event_loop
        .insert_source(from_interface, move |event, _, state| match event {
            Event::Msg(msg) => {
                T::on_callback(msg, state);
            }
            Event::Closed => (),
        })
        .map_err(|e| anyhow::anyhow!("failed to register event source: {e:?}"))?;

    Ok(try_start(interface))
}

fn try_start<T>(interface: T) -> Option<Connection>
where
    T: DbusInterface,
{
    match interface.start() {
        Ok(conn) => Some(conn),
        Err(e) => {
            warn!("error starting {}: {e:?}", T::name());
            None
        }
    }
}

/// Like bail!(), but for fdo
macro_rules! fdbail {
    ($($tt:tt)*) => {
    	return Err(::zbus::fdo::Error::Failed(format!($($tt)*)))
    };
}
pub(crate) use fdbail;

/// Like anyhow!(), but for fdo
macro_rules! fdhow {
    ($($tt:tt)*) => {
    	::zbus::fdo::Error::Failed(format!($($tt)*))
    };
}
pub(crate) use fdhow;
