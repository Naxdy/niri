use serde::{Deserialize, Serialize};

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
