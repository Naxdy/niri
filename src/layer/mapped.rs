use std::cell::RefCell;

use niri_config::utils::MergeWith as _;
use niri_config::{Config, LayerRule};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::desktop::{LayerSurface, PopupManager};
use smithay::utils::{Logical, Point, Rectangle, Scale, Size, Transform};
use smithay::wayland::shell::wlr_layer::{ExclusiveZone, Layer};

use super::ResolvedLayerRules;
use crate::animation::{Animation, Clock};
use crate::layout::shadow::Shadow;
use crate::niri_render_elements;
use crate::render_helpers::blur::EffectsFramebuffersUserData;
use crate::render_helpers::blur::element::{
    Blur, BlurRenderContext, BlurRenderElement, CommitTracker,
};
use crate::render_helpers::clipped_surface::ClippedSurfaceRenderElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::shadow::ShadowRenderElement;
use crate::render_helpers::snapshot::RenderSnapshot;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::surface::push_elements_from_surface_tree;
use crate::render_helpers::{RenderTarget, render_to_texture};
use crate::utils::region::Region;
use crate::utils::render::{PushRenderElement, Render};
use crate::utils::{baba_is_float_offset, round_logical_in_physical};

type LayerRenderSnapshot = RenderSnapshot<
    LayerSurfaceRenderElement<GlesRenderer>,
    LayerSurfaceRenderElement<GlesRenderer>,
>;

#[derive(Clone, Debug)]
pub struct LayerSurfaceRenderContext {
    pub location: Point<f64, Logical>,
    pub target: RenderTarget,
    pub fx_buffers: Option<EffectsFramebuffersUserData>,
}

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

    /// Blur region as specified by the KDE blur / background effect protocols.
    blur_region: Option<Region<i32, Logical>>,
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
            alpha_cfg: config.animations.layer_open.anim,
            blur_region: None,
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

    pub fn geometry(&self) -> Rectangle<f64, Logical> {
        self.geo
    }

    pub const fn set_blurred(&mut self, new_blurred: bool) {
        if !self.rules.blur.off {
            self.rules.blur.on = new_blurred;
        }
    }

    pub fn set_blur_region(&mut self, region: Option<Region<i32, Logical>>) {
        self.blur_region = region;
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

        let mut contents = Vec::new();

        self.render(
            renderer,
            LayerSurfaceRenderContext {
                location: Point::default(),
                target: RenderTarget::Output,
                fx_buffers: None,
            },
            &mut contents,
        );

        let mut blocked_out_contents = Vec::new();

        self.render(
            renderer,
            LayerSurfaceRenderContext {
                location: Point::default(),
                target: RenderTarget::Screencast,
                fx_buffers: None,
            },
            &mut blocked_out_contents,
        );

        // Right before layer destruction, some shells may commit without any render elements, in
        // which case we do _not_ want to update our snapshot, since that would prevent us from
        // rendering a fade-out animation.
        if contents.is_empty() {
            None
        } else {
            Some(RenderSnapshot {
                contents,
                blocked_out_contents,
                block_out_from: self.rules().block_out_from,
                size: self.geo.size,
                texture: Default::default(),
                blocked_out_texture: Default::default(),
            })
        }
    }

    pub fn render_popups<R, C>(
        &self,
        renderer: &mut R,
        context: LayerSurfaceRenderContext,
        collector: &mut C,
    ) where
        R: NiriRenderer,
        C: PushRenderElement<LayerSurfaceRenderElement<R>, R>,
    {
        let LayerSurfaceRenderContext {
            location, target, ..
        } = context;

        let scale = Scale::from(self.scale);
        let alpha = self.rules.opacity.unwrap_or(1.).clamp(0., 1.);
        let location = location + self.bob_offset();

        if target.should_block_out(self.rules.block_out_from) {
            return;
        }

        // Layer surfaces don't have extra geometry like windows.

        let buf_pos = location;

        let surface = self.surface.wl_surface();

        for (popup, popup_offset) in PopupManager::popups_for_surface(surface) {
            // Layer surfaces don't have extra geometry like windows.

            let offset = popup_offset - popup.geometry().loc;

            push_elements_from_surface_tree(
                renderer,
                popup.wl_surface(),
                (buf_pos + offset.to_f64()).to_physical_precise_round(scale),
                scale,
                alpha,
                Kind::ScanoutCandidate,
                &mut collector.as_child(),
            );
        }
    }

    pub fn render_normal<R, C>(
        &self,
        renderer: &mut R,
        context: LayerSurfaceRenderContext,
        collector: &mut C,
    ) where
        R: NiriRenderer,
        C: PushRenderElement<LayerSurfaceRenderElement<R>, R>,
    {
        let LayerSurfaceRenderContext {
            location,
            target,
            fx_buffers,
        } = context;

        let scale = Scale::from(self.scale);
        let alpha = if let Some(alpha) = &self.alpha_animation {
            alpha.clamped_value() as f32
        } else {
            self.rules.opacity.unwrap_or(1.).clamp(0., 1.)
        };
        let location = location + self.bob_offset();

        // Normal surface elements used to render a texture for the ignore alpha pass inside the
        // blur shader.
        let mut gles_elems: Vec<LayerSurfaceRenderElement<GlesRenderer>> = vec![];
        let mut new_unmap_tracker = CommitTracker::new();
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
            new_unmap_tracker.insert_from_elem(&elem);
            collector.push_element(elem);
        } else {
            // Layer surfaces don't have extra geometry like windows.
            let buf_pos = location;

            let surface = self.surface.wl_surface();

            let mut our_tracker = CommitTracker::new();

            push_elements_from_surface_tree(
                renderer,
                surface,
                buf_pos.to_physical_precise_round(scale),
                scale,
                alpha,
                Kind::ScanoutCandidate,
                &mut |elem| {
                    our_tracker.insert_from_elem(&elem);
                    new_unmap_tracker.insert_from_elem(&elem);
                    collector.push_element(elem);
                },
            );

            // If there's been an update to our render elements, we need to render them again for
            // our blur ignore alpha pass.
            if ignore_alpha > 0.
                && self.blur_region.is_none()
                && self.blur.maybe_update_commit_tracker(our_tracker)
            {
                push_elements_from_surface_tree(
                    renderer.as_gles_renderer(),
                    surface,
                    buf_pos.to_physical_precise_round(scale),
                    scale,
                    // Elements for the alpha texture are always rendered at "final" opacity, so
                    // the blur doesn't just "pop into existence" at some point during the fade in
                    // animation.
                    self.rules.opacity.unwrap_or(1.),
                    Kind::ScanoutCandidate,
                    &mut gles_elems.as_child(),
                );
            } else {
                update_alpha_tex = false;
            }
        };

        if let Some(fx_buffers) = fx_buffers
            && (matches!(self.surface.layer(), Layer::Top | Layer::Overlay)
                && !target.should_block_out(self.rules.block_out_from))
        {
            let alpha_tex = (!gles_elems.is_empty())
                .then(|| {
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
                .flatten();

            if update_alpha_tex {
                if let Some((alpha_tex, sync_point)) = alpha_tex {
                    if let Err(e) = sync_point.wait() {
                        warn!("failed to wait for sync point: {e:?}");
                    }
                    self.blur.set_alpha_tex(alpha_tex);
                } else {
                    self.blur.clear_alpha_tex();
                }
            }

            let blur_sample_area = Rectangle::new(location, self.geo.size).to_i32_round();

            let geo = Rectangle::new(location, blur_sample_area.size.to_f64());

            let blur_region = self.blur_region.as_ref().map_or_else(
                || Region::from_rects(std::iter::once(blur_sample_area)),
                |r| r.with_offset(location.to_i32_round()),
            );

            self.blur.render(
                renderer,
                BlurRenderContext {
                    fx_buffers,
                    destination_region: &blur_region,
                    corner_radius: self.rules.geometry_corner_radius.unwrap_or_default(),
                    scale: self.scale,
                    geometry: geo,
                    true_blur: !self.rules.blur.x_ray.unwrap_or_default(),
                    render_loc: None,
                    overview_zoom: None,
                    alpha,
                },
                &mut |elem| {
                    new_unmap_tracker.insert_from_elem(&elem);
                    collector.push_element(elem);
                },
            );
        }

        let location = location.to_physical_precise_round(scale).to_logical(scale);
        self.shadow.render(renderer, location, &mut |elem| {
            new_unmap_tracker.insert_from_elem(&elem);
            collector.push_element(elem);
        });

        let mut tracker = self.unmap_tracker.borrow_mut();

        if should_try_update_snapshot
            && (*tracker != new_unmap_tracker
                || self.unmap_snapshot.borrow().is_none()
                || self.are_animations_ongoing())
        {
            *tracker = new_unmap_tracker;
            drop(tracker);

            self.try_update_unmap_snapshot(renderer.as_gles_renderer());
        }
    }
}

impl<R> Render<'_, R> for MappedLayer
where
    R: NiriRenderer,
{
    type RenderContext = LayerSurfaceRenderContext;

    type RenderElement = LayerSurfaceRenderElement<R>;

    fn render<C>(&self, renderer: &mut R, context: Self::RenderContext, collector: &mut C)
    where
        C: PushRenderElement<Self::RenderElement, R>,
    {
        self.render_popups(renderer, context.clone(), collector);
        self.render_normal(renderer, context, collector);
    }
}
