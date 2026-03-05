use crate::BlurRule;
use crate::appearance::{BlockOutFrom, CornerRadius, ShadowRule};
use crate::utils::RegexEq;

#[derive(knus::Decode, Debug, Default, Clone, PartialEq)]
pub struct LayerRule {
    #[knus(children(name = "match"))]
    pub matches: Vec<Match>,
    #[knus(children(name = "exclude"))]
    pub excludes: Vec<Match>,

    #[knus(child, unwrap(argument))]
    pub opacity: Option<f32>,
    #[knus(child, unwrap(argument))]
    pub block_out_from: Option<BlockOutFrom>,
    #[knus(child, default)]
    pub shadow: ShadowRule,
    #[knus(child, default)]
    pub blur: BlurRule,
    #[knus(child)]
    pub geometry_corner_radius: Option<CornerRadius>,
    #[knus(child, unwrap(argument))]
    pub place_within_backdrop: Option<bool>,
    #[knus(child, unwrap(argument))]
    pub baba_is_float: Option<bool>,
}

#[derive(knus::Decode, Debug, Default, Clone, PartialEq, Eq)]
pub struct Match {
    #[knus(property, str)]
    pub namespace: Option<RegexEq>,
    #[knus(property)]
    pub at_startup: Option<bool>,
}
