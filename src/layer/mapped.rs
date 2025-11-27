use niri_config::utils::MergeWith as _;
use niri_config::{Blur, Config, LayerRule};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::desktop::{LayerSurface, PopupManager};
use smithay::utils::{Logical, Point, Rectangle, Scale, Size, Transform};
use smithay::wayland::shell::wlr_layer::{ExclusiveZone, Layer};

use super::ResolvedLayerRules;
use crate::animation::Clock;
use crate::layout::shadow::Shadow;
use crate::niri_render_elements;
use crate::render_helpers::blur::element::BlurRenderElement;
use crate::render_helpers::blur::EffectsFramebufffersUserData;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::shadow::ShadowRenderElement;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::{render_to_texture, RenderTarget, SplitElements};
use crate::utils::{baba_is_float_offset, round_logical_in_physical};

#[derive(Debug)]
pub struct MappedLayer {
    /// The surface itself.
    surface: LayerSurface,

    /// Up-to-date rules.
    rules: ResolvedLayerRules,

    /// Buffer to draw instead of the surface when it should be blocked out.
    block_out_buffer: SolidColorBuffer,

    /// The shadow around the surface.
    shadow: Shadow,

    /// Configuration for this layer's blur.
    blur_config: Blur,

    /// Size (used for blur).
    // TODO: move to standalone blur struct
    size: Size<f64, Logical>,

    /// The view size for the layer surface's output.
    view_size: Size<f64, Logical>,

    /// Scale of the output the layer surface is on (and rounds its sizes to).
    scale: f64,

    /// Clock for driving animations.
    clock: Clock,
}

niri_render_elements! {
    LayerSurfaceRenderElement<R> => {
        Wayland = WaylandSurfaceRenderElement<R>,
        SolidColor = SolidColorRenderElement,
        Shadow = ShadowRenderElement,
        Blur = BlurRenderElement,
    }
}

impl MappedLayer {
    pub fn new(
        surface: LayerSurface,
        rules: ResolvedLayerRules,
        view_size: Size<f64, Logical>,
        scale: f64,
        clock: Clock,
        config: &Config,
    ) -> Self {
        // Shadows and blur for layer surfaces need to be explicitly enabled.
        let mut shadow_config = config.layout.shadow;
        shadow_config.on = false;
        shadow_config.merge_with(&rules.shadow);

        let mut blur_config = config.layout.blur;
        blur_config.on = false;
        blur_config.merge_with(&rules.blur);

        Self {
            surface,
            rules,
            block_out_buffer: SolidColorBuffer::new((0., 0.), [0., 0., 0., 1.]),
            view_size,
            scale,
            shadow: Shadow::new(shadow_config),
            clock,
            blur_config,
            size: Size::default(),
        }
    }

    pub fn update_config(&mut self, config: &Config) {
        // Shadows and blur for layer surfaces need to be explicitly enabled.
        let mut shadow_config = config.layout.shadow;
        shadow_config.on = false;
        shadow_config.merge_with(&self.rules.shadow);
        self.shadow.update_config(shadow_config);

        let mut blur_config = config.layout.blur;
        blur_config.on = false;
        blur_config.merge_with(&self.rules.blur);
        self.blur_config = blur_config;
    }

    pub fn update_shaders(&mut self) {
        self.shadow.update_shaders();
    }

    pub fn update_sizes(&mut self, view_size: Size<f64, Logical>, scale: f64) {
        self.view_size = view_size;
        self.scale = scale;
    }

    pub fn update_render_elements(&mut self, size: Size<f64, Logical>) {
        // Round to physical pixels.
        let size = size
            .to_physical_precise_round(self.scale)
            .to_logical(self.scale);

        self.size = size;

        self.block_out_buffer.resize(size);

        let radius = self.rules.geometry_corner_radius.unwrap_or_default();
        // FIXME: is_active based on keyboard focus?
        self.shadow
            .update_render_elements(size, true, radius, self.scale, 1.);
    }

    pub fn are_animations_ongoing(&self) -> bool {
        self.rules.baba_is_float
    }

    pub fn surface(&self) -> &LayerSurface {
        &self.surface
    }

    pub fn rules(&self) -> &ResolvedLayerRules {
        &self.rules
    }

    /// Recomputes the resolved layer rules and returns whether they changed.
    pub fn recompute_layer_rules(&mut self, rules: &[LayerRule], is_at_startup: bool) -> bool {
        let new_rules = ResolvedLayerRules::compute(rules, &self.surface, is_at_startup);
        if new_rules == self.rules {
            return false;
        }

        self.rules = new_rules;
        true
    }

    pub fn place_within_backdrop(&self) -> bool {
        if !self.rules.place_within_backdrop {
            return false;
        }

        if self.surface.layer() != Layer::Background {
            return false;
        }

        let state = self.surface.cached_state();
        if state.exclusive_zone != ExclusiveZone::DontCare {
            return false;
        }

        true
    }

    pub fn bob_offset(&self) -> Point<f64, Logical> {
        if !self.rules.baba_is_float {
            return Point::from((0., 0.));
        }

        let y = baba_is_float_offset(self.clock.now(), self.view_size.h);
        let y = round_logical_in_physical(self.scale, y);
        Point::from((0., y))
    }

    pub fn render<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        location: Point<f64, Logical>,
        target: RenderTarget,
        fx_buffers: Option<EffectsFramebufffersUserData>,
    ) -> SplitElements<LayerSurfaceRenderElement<R>> {
        let mut rv = SplitElements::default();

        let scale = Scale::from(self.scale);
        let alpha = self.rules.opacity.unwrap_or(1.).clamp(0., 1.);
        let location = location + self.bob_offset();

        let mut gles_elems: Option<Vec<LayerSurfaceRenderElement<GlesRenderer>>> = None;

        if target.should_block_out(self.rules.block_out_from) {
            // Round to physical pixels.
            let location = location.to_physical_precise_round(scale).to_logical(scale);

            // FIXME: take geometry-corner-radius into account.
            let elem = SolidColorRenderElement::from_buffer(
                &self.block_out_buffer,
                location,
                alpha,
                Kind::Unspecified,
            );
            rv.normal.push(elem.into());
        } else {
            // Layer surfaces don't have extra geometry like windows.
            let buf_pos = location;

            let surface = self.surface.wl_surface();
            for (popup, popup_offset) in PopupManager::popups_for_surface(surface) {
                // Layer surfaces don't have extra geometry like windows.
                let offset = popup_offset - popup.geometry().loc;

                rv.popups.extend(render_elements_from_surface_tree(
                    renderer,
                    popup.wl_surface(),
                    (buf_pos + offset.to_f64()).to_physical_precise_round(scale),
                    scale,
                    alpha,
                    Kind::ScanoutCandidate,
                ));
            }

            rv.normal = render_elements_from_surface_tree(
                renderer,
                surface,
                buf_pos.to_physical_precise_round(scale),
                scale,
                alpha,
                Kind::ScanoutCandidate,
            );

            gles_elems = Some(render_elements_from_surface_tree(
                renderer.as_gles_renderer(),
                surface,
                buf_pos.to_physical_precise_round(scale),
                scale,
                alpha,
                Kind::ScanoutCandidate,
            ));
        };

        let blur_elem = (self.blur_config.on
            && matches!(self.surface.layer(), Layer::Top | Layer::Overlay))
        .then(|| {
            let fx_buffers = fx_buffers?;
            let fx_buffers = fx_buffers.borrow();

            // TODO: respect sync point?
            let alpha_tex = gles_elems
                .and_then(|gles_elems| {
                    let transform = fx_buffers.transform();

                    render_to_texture(
                        renderer.as_gles_renderer(),
                        transform.transform_size(fx_buffers.output_size()),
                        self.scale.into(),
                        Transform::Normal,
                        Fourcc::Abgr8888,
                        gles_elems.into_iter(),
                    )
                    .inspect_err(|e| warn!("failed to render alpha tex: {e:?}"))
                    .ok()
                })
                .map(|r| r.0);

            Some(
                BlurRenderElement::new_optimized(
                    renderer,
                    &fx_buffers,
                    Rectangle::new(location, self.size).to_i32_round(),
                    location.to_physical_precise_round(self.scale),
                    self.rules
                        .geometry_corner_radius
                        .unwrap_or_default()
                        .top_left,
                    self.scale,
                    self.blur_config,
                    alpha_tex,
                )
                .into(),
            )
        })
        .flatten()
        .into_iter();

        let location = location.to_physical_precise_round(scale).to_logical(scale);
        rv.normal
            .extend(self.shadow.render(renderer, location).map(Into::into));

        rv.normal.extend(blur_elem);

        rv
    }
}
