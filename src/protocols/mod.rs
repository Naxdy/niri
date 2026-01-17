pub mod ext_background_effect;
pub mod ext_workspace;
pub mod foreign_toplevel;
pub mod gamma_control;
pub mod kde_blur;
pub mod mutter_x11_interop;
pub mod output_management;
pub mod screencopy;
pub mod virtual_pointer;

// Doesn't make sense without dbus, as it is tied to dbusmenu
#[cfg(feature = "dbus")]
pub mod kde_appmenu;

pub mod raw;
