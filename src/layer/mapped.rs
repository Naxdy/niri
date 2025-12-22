use std::cell::RefCell;

use niri_config::utils::MergeWith as _;
use niri_config::{Config, LayerRule};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::desktop::{LayerSurface, PopupManager};
use smithay::utils::{Logical, Point, Rectangle, Scale, Size, Transform};
use smithay::wayland::shell::wlr_layer::{ExclusiveZone, Layer};

use super::ResolvedLayerRules;
use crate::animation::{Animation, Clock};
use crate::layout::shadow::Shadow;
use crate::niri_render_elements;
use crate::render_helpers::blur::EffectsFramebuffersUserData;
use crate::render_helpers::blur::element::{Blur, BlurRenderElement, CommitTracker};
use crate::render_helpers::clipped_surface::ClippedSurfaceRenderElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::shadow::ShadowRenderElement;
use crate::render_helpers::snapshot::RenderSnapshot;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::{RenderTarget, SplitElements, render_to_texture};
use crate::utils::{baba_is_float_offset, round_logical_in_physical};

type LayerRenderSnapshot = RenderSnapshot<
    LayerSurfaceRenderElement<GlesRenderer>,
    LayerSurfaceRenderElement<GlesRenderer>,
>;

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
    blur: Blur,

    /// Geometry of this layer.
    geo: Rectangle<f64, Logical>,

    /// The view size for the layer surface's output.
    view_size: Size<f64, Logical>,

    /// Scale of the output the layer surface is on (and rounds its sizes to).
    scale: f64,

    /// Clock for driving animations.
    clock: Clock,

    /// Snapshot for fade-out layer animation.
    unmap_snapshot: RefCell<Option<LayerRenderSnapshot>>,

    /// Commit tracker for when to update the unmap snapshot.
    unmap_tracker: RefCell<CommitTracker>,

    /// The alpha animation for this layer surface.
    alpha_animation: Option<Animation>,

    /// Configuration for the alpha animation.
    alpha_cfg: niri_config::Animation,
}

niri_render_elements! {
    LayerSurfaceRenderElement<R> => {
        Wayland = WaylandSurfaceRenderElement<R>,
        SolidColor = SolidColorRenderElement,
        Shadow = ShadowRenderElement,
        Blur = BlurRenderElement,
        ClippedBlur = ClippedSurfaceRenderElement<BlurRenderElement>,
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
            blur: Blur::new(blur_config),
            geo: Rectangle::default(),
            unmap_snapshot: RefCell::new(None),
            unmap_tracker: RefCell::new(CommitTracker::default()),
            alpha_animation: None,
            alpha_cfg: config.animations.window_open.anim,
        }
    }

    pub fn advance_animations(&mut self) {
        if let Some(alpha) = &mut self.alpha_animation
            && alpha.is_done()
        {
            self.alpha_animation = None;
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
        self.blur.update_config(blur_config);
    }

    pub fn update_shaders(&mut self) {
        self.shadow.update_shaders();
    }

    pub const fn update_sizes(&mut self, view_size: Size<f64, Logical>, scale: f64) {
        self.view_size = view_size;
        self.scale = scale;
    }

    pub fn update_render_elements(&mut self, geo: Rectangle<f64, Logical>) {
        // Round to physical pixels.
        let size = geo
            .size
            .to_physical_precise_round(self.scale)
            .to_logical(self.scale);

        self.geo = geo;

        self.block_out_buffer.resize(size);

        let radius = self.rules.geometry_corner_radius.unwrap_or_default();
        // FIXME: is_active based on keyboard focus?
        self.shadow
            .update_render_elements(size, true, radius, self.scale, 1.);

        self.blur.update_render_elements(self.rules.blur.on);
    }

    pub const fn are_animations_ongoing(&self) -> bool {
        self.rules.baba_is_float || self.alpha_animation.is_some()
    }

    pub const fn surface(&self) -> &LayerSurface {
        &self.surface
    }

    pub const fn rules(&self) -> &ResolvedLayerRules {
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

    pub fn start_fade_in_animation(&mut self) {
        self.alpha_animation = Some(Animation::new(
            self.clock.clone(),
            0.,
            self.rules.opacity.unwrap_or(1.) as f64,
            0.,
            self.alpha_cfg,
        ))
    }

    pub fn render<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        location: Point<f64, Logical>,
        target: RenderTarget,
        fx_buffers: Option<EffectsFramebuffersUserData>,
    ) -> SplitElements<LayerSurfaceRenderElement<R>> {
        let mut rv = SplitElements::default();

        let scale = Scale::from(self.scale);
        let alpha = if let Some(alpha) = &self.alpha_animation {
            alpha.clamped_value() as f32
        } else {
            self.rules.opacity.unwrap_or(1.).clamp(0., 1.)
        };
        let location = location + self.bob_offset();

        // Normal surface elements used to render a texture for the ignore alpha pass inside the
        // blur shader.
        let mut gles_elems: Option<Vec<LayerSurfaceRenderElement<GlesRenderer>>> = None;
        let ignore_alpha = self.rules.blur.ignore_alpha.unwrap_or_default().0;
        let mut update_alpha_tex = ignore_alpha > 0.;

        // We only want to update the layer texture snapshot if we are in the main render pass.
        // Currently, we can verify this by checking the presence of `fx_buffers`, since they are
        // only passed for rendering blur, which is currently only rendered on output and output
        // screencasts.
        let should_try_update_snapshot = fx_buffers.is_some();

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

            // If there's been an update to our render elements, we need to render them again for
            // our blur ignore alpha pass.
            if ignore_alpha > 0.
                && self
                    .blur
                    .maybe_update_commit_tracker(CommitTracker::from_elements(rv.normal.iter()))
            {
                gles_elems = Some(render_elements_from_surface_tree(
                    renderer.as_gles_renderer(),
                    surface,
                    buf_pos.to_physical_precise_round(scale),
                    scale,
                    alpha,
                    Kind::ScanoutCandidate,
                ));
            } else {
                update_alpha_tex = false;
            }
        };

        let blur_elem = (matches!(self.surface.layer(), Layer::Top | Layer::Overlay)
            && !target.should_block_out(self.rules.block_out_from))
        .then(|| {
            let fx_buffers = fx_buffers?;

            // TODO: respect sync point?
            let alpha_tex = gles_elems
                .and_then(|gles_elems| {
                    let fx_buffers = fx_buffers.borrow();

                    let transform = fx_buffers.transform();

                    render_to_texture(
                        renderer.as_gles_renderer(),
                        transform.transform_size(fx_buffers.output_size()),
                        self.scale.into(),
                        Transform::Normal,
                        Fourcc::Abgr8888,
                        gles_elems.into_iter(),
                    )
                    .inspect_err(|e| warn!("failed to render alpha tex for layer surface: {e:?}"))
                    .ok()
                })
                .map(|r| r.0);

            if update_alpha_tex {
                if let Some(alpha_tex) = alpha_tex {
                    self.blur.set_alpha_tex(alpha_tex);
                } else {
                    self.blur.clear_alpha_tex();
                }
            }

            let blur_sample_area = Rectangle::new(location, self.geo.size).to_i32_round();

            let geo = Rectangle::new(location, blur_sample_area.size.to_f64());

            Some(
                self.blur
                    .render(
                        renderer.as_gles_renderer(),
                        fx_buffers,
                        blur_sample_area,
                        self.rules.geometry_corner_radius.unwrap_or_default(),
                        self.scale,
                        geo,
                        !self.rules.blur.x_ray.unwrap_or_default(),
                        blur_sample_area.loc.to_f64(),
                        None,
                        alpha,
                    )
                    .map(Into::into),
            )
        })
        .flatten()
        .flatten()
        .into_iter();

        let location = location.to_physical_precise_round(scale).to_logical(scale);
        rv.normal
            .extend(self.shadow.render(renderer, location).map(Into::into));

        {
            let mut tracker = self.unmap_tracker.borrow_mut();
            let elem_tracker = CommitTracker::from_elements(rv.normal.iter());

            if should_try_update_snapshot
                && (*tracker != elem_tracker || self.unmap_snapshot.borrow().is_none())
            {
                *tracker = elem_tracker;
                drop(tracker);

                self.try_update_unmap_snapshot(renderer.as_gles_renderer());
            }
        }

        rv.normal.extend(blur_elem);

        rv
    }

    pub fn geometry(&self) -> Rectangle<f64, Logical> {
        self.geo
    }

    pub const fn set_blurred(&mut self, new_blurred: bool) {
        if !self.rules.blur.off {
            self.rules.blur.on = new_blurred;
        }
    }

    fn try_update_unmap_snapshot(&self, renderer: &mut GlesRenderer) {
        if let Some(snapshot) = self.render_snapshot(renderer) {
            let mut cell = self.unmap_snapshot.borrow_mut();
            *cell = Some(snapshot);
        }
    }

    pub fn take_unmap_snapshot(&mut self) -> Option<LayerRenderSnapshot> {
        self.unmap_snapshot.take()
    }

    fn render_snapshot(&self, renderer: &mut GlesRenderer) -> Option<LayerRenderSnapshot> {
        let _span = tracy_client::span!("MappedLayer::render_snapshot");

        let contents = self.render(renderer, Point::default(), RenderTarget::Output, None);

        let blocked_out_contents = self.render(
            renderer,
            Point::from((0., 0.)),
            RenderTarget::Screencast,
            None,
        );

        // Right before layer destruction, some shells may commit without any render elements, in
        // which case we do _not_ want to update our snapshot, since that would prevent us from
        // rendering a fade-out animation.
        if contents.normal.is_empty() {
            None
        } else {
            Some(RenderSnapshot {
                contents: contents.normal,
                blocked_out_contents: blocked_out_contents.normal,
                block_out_from: self.rules().block_out_from,
                size: self.geo.size,
                texture: Default::default(),
                blocked_out_texture: Default::default(),
            })
        }
    }
}
