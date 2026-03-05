//! When the focused window is closed, some other window should be focused instead
//! This module describes selection of such next window.

use serde::{Deserialize, Serialize};

/// IDK
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
pub enum MruDirection {
    /// Most recently used to least.
    #[default]
    Forward,
    /// Least recently used to most.
    Backward,
}

/// When choosing a window to switch after closing the other window, which window scope should we
/// look up first
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[cfg_attr(feature = "knus", derive(knus::DecodeScalar))]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
pub enum MruScope {
    /// All windows.
    #[default]
    All,
    /// Windows on the active output.
    Output,
    /// Windows on the active workspace.
    Workspace,
}

/// When choosing a window to switch after closing the other window, which window should we look
/// up first
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[cfg_attr(feature = "knus", derive(knus::DecodeScalar))]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
pub enum MruFilter {
    /// All windows.
    #[default]
    #[cfg_attr(feature = "knus", knus(skip))]
    All,
    /// Windows with the same app id as the active window.
    AppId,
}
