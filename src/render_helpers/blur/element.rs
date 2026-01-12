// Originally ported from https://github.com/nferhat/fht-compositor/blob/main/src/renderer/blur/element.rs

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use glam::{Mat3, Vec2};
use niri_config::CornerRadius;

use pango::glib::property::PropertySet;
use smithay::backend::renderer::Texture;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesRenderer, GlesTexture, Uniform, ffi,
};
use smithay::backend::renderer::utils::{CommitCounter, OpaqueRegions};
use smithay::reexports::gbm::Format;
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform};

use crate::backend::tty::{TtyFrame, TtyRenderer, TtyRendererError};
use crate::render_helpers::blur::EffectsFramebuffersUserData;
use crate::render_helpers::render_data::RendererData;
use crate::render_helpers::renderer::{AsGlesFrame, NiriRenderer};
use crate::render_helpers::shaders::{Shaders, mat3_uniform};
use crate::utils::region::Region;
use crate::utils::render::{PushRenderElement, Render};

use super::{CurrentBuffer, EffectsFramebuffers};

#[derive(Debug, Clone)]
enum BlurVariant {
    Optimized {
        /// Reference to the globally cached optimized blur texture.
        texture: GlesTexture,
    },
    True {
        /// Individual cache of true blur texture.
        texture: GlesTexture,
        fx_buffers: EffectsFramebuffersUserData,
        config: niri_config::Blur,
        /// Timer to limit redraw rate of true blur. Currently set at 150ms fixed (~6.6 fps).
        rerender_at: Rc<RefCell<Option<Instant>>>,
    },
}

/// Used for tracking commit counters of a collection of elements.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct CommitTracker(HashMap<Id, CommitCounter>);

impl CommitTracker {
    pub fn new() -> Self {
        Self(Default::default())
    }

    pub fn insert_from_elem<'a, E: Element + 'a>(&mut self, elem: &'a E) {
        self.0.insert(elem.id().clone(), elem.current_commit());
    }

    pub fn from_elements<'a, E: Element + 'a>(elems: impl Iterator<Item = &'a E>) -> Self {
        Self(
            elems
                .map(|e| (e.id().clone(), e.current_commit()))
                .collect(),
        )
    }

    pub fn update<'a, E: Element + 'a>(&mut self, elems: impl Iterator<Item = &'a E>) {
        *self = Self::from_elements(elems);
    }
}

#[derive(Debug)]
pub struct BlurRenderContext<'a> {
    pub fx_buffers: EffectsFramebuffersUserData,
    pub destination_region: &'a Region<i32, Logical>,
    pub corner_radius: CornerRadius,
    pub scale: f64,
    pub geometry: Rectangle<f64, Logical>,
    pub true_blur: bool,
    /// used for elements that are rendered offscreen (e.g. tiles that are being dragged)
    pub render_loc: Option<Point<f64, Logical>>,
    pub overview_zoom: Option<f64>,
    pub alpha: f32,
}

#[derive(Debug)]
pub struct Blur {
    config: niri_config::Blur,
    inner: RefCell<Vec<BlurRenderElement>>,
    alpha_tex: RefCell<Option<GlesTexture>>,
    commit_tracker: RefCell<CommitTracker>,
}

impl Blur {
    pub fn new(config: niri_config::Blur) -> Self {
        Self {
            config,
            inner: Default::default(),
            alpha_tex: Default::default(),
            commit_tracker: Default::default(),
        }
    }

    pub fn maybe_update_commit_tracker(&self, other: CommitTracker) -> bool {
        if self.commit_tracker.borrow().eq(&other) {
            false
        } else {
            self.commit_tracker.set(other);
            true
        }
    }

    pub fn update_config(&mut self, config: niri_config::Blur) {
        if self.config != config {
            self.inner.set(vec![]);
        }

        self.config = config;
    }

    // TODO: the alpha tex methods can probably do better / without clearing `self.inner` entirely

    pub fn clear_alpha_tex(&self) {
        if self.alpha_tex.borrow().is_some() {
            self.inner
                .borrow_mut()
                .iter_mut()
                .for_each(BlurRenderElement::damage_all);
        }

        self.alpha_tex.set(None);
    }

    pub fn set_alpha_tex(&self, alpha_tex: GlesTexture) {
        self.alpha_tex.set(Some(alpha_tex));
        self.inner.set(vec![]);
    }

    pub const fn update_render_elements(&mut self, is_active: bool) {
        self.config.on = is_active;
    }
}

impl<'a, R> Render<'a, R> for Blur
where
    R: NiriRenderer,
{
    type RenderContext = BlurRenderContext<'a>;
    type RenderElement = BlurRenderElement;

    // TODO: separate some of this logic out to [`Blur::update_render_elements`]
    fn render<C>(&'a self, renderer: &mut R, render_context: Self::RenderContext, collector: &mut C)
    where
        C: PushRenderElement<BlurRenderElement, R>,
    {
        let BlurRenderContext {
            fx_buffers,
            destination_region,
            corner_radius,
            scale,
            geometry,
            mut true_blur,
            render_loc,
            overview_zoom,
            alpha,
        } = render_context;

        if !self.config.on || self.config.passes == 0 || self.config.radius.0 == 0. {
            return;
        }

        // FIXME: true blur is broken on 90/270 transformed monitors
        if !matches!(
            fx_buffers.borrow().transform(),
            Transform::Normal | Transform::Flipped180,
        ) {
            true_blur = false;
        }

        let sample_region = destination_region
            .rects()
            .iter()
            .map(|destination_area| {
                if let (Some(zoom), true) = (overview_zoom, true_blur) {
                    let mut sample_area = destination_area.to_f64().upscale(zoom);

                    let center = (fx_buffers.borrow().output_size.to_f64().to_logical(scale) / 2.)
                        .to_point();
                    sample_area.loc.x =
                        (center.x - destination_area.loc.x as f64).mul_add(-zoom, center.x);
                    sample_area.loc.y =
                        (center.y - destination_area.loc.y as f64).mul_add(-zoom, center.y);
                    sample_area.to_i32_round()
                } else {
                    *destination_area
                }
            })
            .collect::<Region<_, _>>();

        let mut tex_buffer = || {
            renderer
                .create_buffer(Format::Argb8888, fx_buffers.borrow().effects.size())
                .inspect_err(|e| {
                    warn!("failed to allocate buffer for cached true blur texture: {e:?}")
                })
                .ok()
        };

        let mut inner = self.inner.borrow_mut();

        let variant_needs_rerender = inner.iter().any(|inner| match &inner.variant {
            BlurVariant::Optimized { texture } => {
                let fx_buffers = fx_buffers.borrow();

                texture.size().w != fx_buffers.output_size().w
                    || texture.size().h != fx_buffers.output_size().h
            }
            BlurVariant::True { rerender_at, .. } => {
                // TODO: damage tracking of other render elements should happen here
                rerender_at.borrow().is_none_or(|r| r < Instant::now())
            }
        });

        let variant_needs_reconfigure = inner.iter().any(|inner| match &inner.variant {
            BlurVariant::Optimized { texture } => {
                texture.tex_id() != fx_buffers.borrow().optimized_blur.tex_id()
            }
            _ => false,
        });

        let changed_any = destination_region.rects().len() != inner.len()
            || inner
                .iter_mut()
                .zip(destination_region.rects())
                .zip(sample_region.rects())
                .any(|((inner, destination_area), sample_area)| {
                    // if nothing about our geometry changed, we don't need to re-render blur
                    if inner.sample_area == *sample_area
                        && inner.destination_area == *destination_area
                        && inner.geometry == geometry
                        && inner.scale == scale
                        && inner.corner_radius == corner_radius
                        && inner.render_loc
                            == render_loc.unwrap_or_else(|| destination_area.loc.to_f64())
                        && inner.alpha == alpha
                        && true_blur == matches!(&inner.variant, BlurVariant::True { .. })
                        && !variant_needs_reconfigure
                    {
                        if variant_needs_rerender {
                            // FIXME: currently, true blur only gets damaged on a fixed timer,
                            // which causes some artifacts for blur that is rendered above frequently
                            // updating surfaces (e.g. video, animated background). although this is preferable
                            // to re-rendering on every frame, the best solution would be to track "global
                            // output damage up to the point we're rendering", to find out whether or not we
                            // need to re-render true blur.
                            inner.damage_all();
                        }

                        false
                    } else {
                        true
                    }
                });

        if !changed_any {
            inner
                .iter()
                .cloned()
                .for_each(|e| collector.push_element(e));
            return;
        }

        *inner = destination_region
            .rects()
            .iter()
            .zip(sample_region.rects().iter())
            .filter_map(|(destination_area, sample_area)| {
                Some(BlurRenderElement::new(
                    &fx_buffers.borrow(),
                    *sample_area,
                    *destination_area,
                    corner_radius,
                    scale,
                    self.config,
                    geometry,
                    self.alpha_tex.borrow().clone(),
                    if true_blur {
                        BlurVariant::True {
                            fx_buffers: fx_buffers.clone(),
                            config: self.config,
                            texture: tex_buffer()?,
                            rerender_at: Default::default(),
                        }
                    } else {
                        BlurVariant::Optimized {
                            texture: fx_buffers.borrow().optimized_blur.clone(),
                        }
                    },
                    render_loc.unwrap_or_else(|| destination_area.loc.to_f64()),
                    alpha,
                ))
            })
            .collect();

        inner
            .iter()
            .cloned()
            .for_each(|e| collector.push_element(e));
    }
}

#[derive(Clone, Debug)]
pub struct BlurRenderElement {
    id: Id,
    uniforms: Vec<Uniform<'static>>,
    sample_area: Rectangle<i32, Logical>,
    destination_area: Rectangle<i32, Logical>,
    alpha_tex: Option<GlesTexture>,
    scale: f64,
    commit: CommitCounter,
    corner_radius: CornerRadius,
    geometry: Rectangle<f64, Logical>,
    variant: BlurVariant,
    render_loc: Point<f64, Logical>,
    alpha: f32,
}

impl BlurRenderElement {
    /// Create a new [`BlurElement`]. You are supposed to put this **below** the translucent surface
    /// that you want to blur. `area` is assumed to be relative to the `output` you are rendering
    /// in.
    ///
    /// If you don't update the blur optimized buffer
    /// [`EffectsFramebuffers::update_optimized_blur_buffer`] this element will either
    /// - Display outdated/wrong contents
    /// - Not display anything since the buffer will be empty.
    #[allow(clippy::too_many_arguments)]
    fn new(
        fx_buffers: &EffectsFramebuffers,
        sample_area: Rectangle<i32, Logical>,
        destination_area: Rectangle<i32, Logical>,
        corner_radius: CornerRadius,
        scale: f64,
        config: niri_config::Blur,
        geometry: Rectangle<f64, Logical>,
        alpha_tex: Option<GlesTexture>,
        variant: BlurVariant,
        render_loc: Point<f64, Logical>,
        alpha: f32,
    ) -> Self {
        let mut this = Self {
            id: Id::new(),
            uniforms: Vec::with_capacity(7),
            alpha_tex,
            sample_area,
            destination_area,
            scale,
            corner_radius,
            geometry,
            commit: CommitCounter::default(),
            variant,
            render_loc,
            alpha,
        };

        this.update_uniforms(fx_buffers, &config);

        this
    }

    fn update_uniforms(&mut self, fx_buffers: &EffectsFramebuffers, config: &niri_config::Blur) {
        let transform = Transform::Normal;

        let elem_geo: Rectangle<i32, _> =
            self.destination_area.to_physical_precise_round(self.scale);
        let elem_geo_loc = Vec2::new(elem_geo.loc.x as f32, elem_geo.loc.y as f32);
        let elem_geo_size = Vec2::new(elem_geo.size.w as f32, elem_geo.size.h as f32);

        let view_src = self.sample_area;
        let buf_size = fx_buffers.output_size().to_f64().to_logical(self.scale);
        let buf_size = Vec2::new(buf_size.w as f32, buf_size.h as f32);

        let geo = self.geometry.to_physical_precise_round(self.scale);
        let geo_loc = Vec2::new(geo.loc.x, geo.loc.y);
        let geo_size = Vec2::new(geo.size.w, geo.size.h);

        let src_loc = Vec2::new(view_src.loc.x as f32, view_src.loc.y as f32);
        let src_size = Vec2::new(view_src.size.w as f32, view_src.size.h as f32);

        let transform_matrix = Mat3::from_translation(Vec2::new(0.5, 0.5))
            * Mat3::from_cols_array(transform.matrix().as_ref())
            * Mat3::from_translation(-Vec2::new(0.5, 0.5));

        // FIXME: y_inverted
        let input_to_geo = transform_matrix * Mat3::from_scale(elem_geo_size / geo_size)
            * Mat3::from_translation((elem_geo_loc - geo_loc) / elem_geo_size)
            // Apply viewporter src.
            * Mat3::from_scale(buf_size / src_size)
            * Mat3::from_translation(-src_loc / buf_size);

        self.uniforms = vec![
            Uniform::new("alpha", self.alpha),
            Uniform::new("corner_radius", <[f32; 4]>::from(self.corner_radius)),
            Uniform::new("geo_size", geo_size.to_array()),
            Uniform::new("niri_scale", self.scale as f32),
            Uniform::new("noise", config.noise.0 as f32),
            Uniform::new("brightness", config.brightness.0 as f32),
            Uniform::new("contrast", config.contrast.0 as f32),
            Uniform::new("saturation", config.saturation.0 as f32),
            mat3_uniform("input_to_geo", input_to_geo),
            Uniform::new(
                "ignore_alpha",
                if self.alpha_tex.is_some() {
                    config.ignore_alpha.0 as f32
                } else {
                    0.
                },
            ),
            Uniform::new("alpha_tex", if self.alpha_tex.is_some() { 1 } else { 0 }),
        ];
    }

    fn damage_all(&mut self) {
        self.commit.increment()
    }
}

impl Element for BlurRenderElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.sample_area.to_f64().to_buffer(
            self.scale,
            Transform::Normal,
            &self.sample_area.size.to_f64(),
        )
    }

    fn transform(&self) -> Transform {
        Transform::Normal
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        if self.alpha_tex.is_some() || matches!(&self.variant, BlurVariant::True { .. }) {
            return OpaqueRegions::default();
        }

        let geometry = self.geometry(scale);

        let CornerRadius {
            top_left,
            top_right,
            bottom_right,
            bottom_left,
        } = self.corner_radius.scaled_by(scale.x as f32);

        let largest_radius = top_left.max(top_right).max(bottom_right).max(bottom_left);

        let rect = Rectangle::new(
            Point::new(top_left.ceil() as i32, top_left.ceil() as i32),
            (geometry.size.to_f64()
                - Size::new(largest_radius.ceil() as f64, largest_radius.ceil() as f64) * 2.)
                .to_i32_ceil(),
        );

        OpaqueRegions::from_slice(&[rect])
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        Rectangle::new(
            self.render_loc.to_physical_precise_round(scale),
            self.destination_area
                .to_f64()
                .to_physical_precise_round(scale)
                .size,
        )
    }

    fn alpha(&self) -> f32 {
        1.0
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for BlurRenderElement {
    fn draw(
        &self,
        gles_frame: &mut GlesFrame,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        let _span = trace_span!("blur_draw_gles").entered();

        let src = Rectangle::new(
            src.loc,
            Size::new(src.size.w * self.scale, src.size.h * self.scale),
        );

        let scaled_dst = Rectangle::new(
            dst.loc,
            Size::from((
                (dst.size.w as f64 * self.scale) as i32,
                (dst.size.h as f64 * self.scale) as i32,
            )),
        );

        let program = Shaders::get_from_frame(gles_frame)
            .blur_finish
            .clone()
            .expect("should be compiled");

        if let Some(alpha_tex) = &self.alpha_tex {
            gles_frame.with_context(|gl| unsafe {
                gl.ActiveTexture(ffi::TEXTURE1);
                gl.BindTexture(ffi::TEXTURE_2D, alpha_tex.tex_id());
                gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, ffi::LINEAR as i32);
                gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MAG_FILTER, ffi::LINEAR as i32);
            })?;
        }

        match &self.variant {
            BlurVariant::Optimized { texture } => gles_frame.render_texture_from_to(
                texture,
                src,
                scaled_dst,
                damage,
                opaque_regions,
                Transform::Normal,
                1.,
                Some(&program),
                &self.uniforms,
            ),
            BlurVariant::True {
                fx_buffers,
                config,
                texture,
                rerender_at,
            } => {
                let mut fx_buffers = fx_buffers.borrow_mut();

                fx_buffers.current_buffer = CurrentBuffer::Normal;

                let shaders = Shaders::get_from_frame(gles_frame).blur.clone();
                let vbos = RendererData::get_from_frame(gles_frame).vbos;
                let supports_instancing = gles_frame
                    .capabilities()
                    .contains(&smithay::backend::renderer::gles::Capability::Instancing);
                let debug = !gles_frame.debug_flags().is_empty();
                let projection_matrix = glam::Mat3::from_cols_array(gles_frame.projection());

                // Update the blur buffers.
                // We use gl ffi directly to circumvent some stuff done by smithay
                if rerender_at
                    .borrow()
                    .map(|r| r < Instant::now())
                    .unwrap_or(true)
                {
                    gles_frame.with_context(|gl| unsafe {
                        super::get_main_buffer_blur(
                            gl,
                            &mut fx_buffers,
                            &shaders,
                            *config,
                            projection_matrix,
                            self.scale as i32,
                            &vbos,
                            debug,
                            supports_instancing,
                            scaled_dst,
                            texture,
                            self.alpha_tex.as_ref(),
                        )
                    })??;

                    rerender_at.set(Some(
                        Instant::now()
                            + Duration::from_millis(config.draw_interval.0.round() as u64),
                    ));
                };

                gles_frame.render_texture_from_to(
                    texture,
                    src,
                    scaled_dst,
                    damage,
                    opaque_regions,
                    fx_buffers.transform(),
                    1.,
                    Some(&program),
                    &self.uniforms,
                )
            }
        }
    }

    fn underlying_storage(&self, _: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

impl<'render> RenderElement<TtyRenderer<'render>> for BlurRenderElement {
    fn draw(
        &self,
        frame: &mut TtyFrame<'_, '_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), TtyRendererError<'render>> {
        let _span = trace_span!("blur_draw_tty").entered();

        <Self as RenderElement<GlesRenderer>>::draw(
            self,
            frame.as_gles_frame(),
            src,
            dst,
            damage,
            opaque_regions,
        )?;

        Ok(())
    }

    fn underlying_storage(
        &'_ self,
        _renderer: &mut TtyRenderer<'render>,
    ) -> Option<UnderlyingStorage<'_>> {
        None
    }
}
