// Originally ported from https://github.com/nferhat/fht-compositor/blob/main/src/renderer/blur/element.rs

use std::cell::RefCell;
use std::collections::HashMap;

use glam::{Mat3, Vec2};
use niri_config::CornerRadius;

use pango::glib::property::PropertySet;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    ffi, GlesError, GlesFrame, GlesRenderer, GlesTexture, Uniform,
};
use smithay::backend::renderer::utils::{CommitCounter, OpaqueRegions};
use smithay::backend::renderer::Texture;
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform};

use crate::backend::tty::{TtyFrame, TtyRenderer, TtyRendererError};
use crate::render_helpers::blur::EffectsFramebufffersUserData;
use crate::render_helpers::render_data::RendererData;
use crate::render_helpers::renderer::AsGlesFrame;
use crate::render_helpers::shaders::{mat3_uniform, Shaders};

use super::{CurrentBuffer, EffectsFramebuffers};

#[derive(Debug, Clone)]
enum BlurVariant {
    Optimized {
        texture: GlesTexture,
    },
    True {
        fx_buffers: EffectsFramebufffersUserData,
        config: niri_config::Blur,
    },
}

/// Used for tracking commit counters of a collection of elements.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct CommitTracker(HashMap<Id, CommitCounter>);

impl CommitTracker {
    pub fn new() -> Self {
        Self(Default::default())
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
pub struct Blur {
    config: niri_config::Blur,
    inner: RefCell<Option<BlurRenderElement>>,
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
            self.inner.set(None);
        }

        self.config = config;
    }

    pub fn clear_alpha_tex(&self) {
        if let Some(inner) = self.inner.borrow_mut().as_mut() {
            if self.alpha_tex.borrow().is_some() {
                inner.damage_all();
            }
        }

        self.alpha_tex.set(None);
    }

    pub fn set_alpha_tex(&self, alpha_tex: GlesTexture) {
        self.alpha_tex.set(Some(alpha_tex));
        self.inner.set(None);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        fx_buffers: EffectsFramebufffersUserData,
        sample_area: Rectangle<i32, Logical>,
        corner_radius: CornerRadius,
        scale: f64,
        geometry: Rectangle<f64, Logical>,
        mut true_blur: bool,
        render_loc: Point<f64, Logical>,
    ) -> Option<BlurRenderElement> {
        if !self.config.on {
            return None;
        }

        // FIXME: true blur is broken on 90/270 transformed monitors
        if !matches!(
            fx_buffers.borrow().transform(),
            Transform::Normal | Transform::Flipped180,
        ) {
            true_blur = false;
        }

        let mut inner = self.inner.borrow_mut();

        let Some(inner) = inner.as_mut() else {
            let elem = BlurRenderElement::new(
                &fx_buffers.borrow(),
                sample_area,
                corner_radius,
                scale,
                self.config,
                geometry,
                self.alpha_tex.borrow().clone(),
                if true_blur {
                    BlurVariant::True {
                        fx_buffers: fx_buffers.clone(),
                        config: self.config,
                    }
                } else {
                    BlurVariant::Optimized {
                        texture: fx_buffers.borrow().optimized_blur.clone(),
                    }
                },
                render_loc,
            );

            *inner = Some(elem.clone());

            return Some(elem);
        };

        if true_blur != matches!(&inner.variant, BlurVariant::True { .. }) {
            inner.variant = if true_blur {
                BlurVariant::True {
                    fx_buffers: fx_buffers.clone(),
                    config: self.config,
                }
            } else {
                BlurVariant::Optimized {
                    texture: fx_buffers.borrow().optimized_blur.clone(),
                }
            };

            inner.damage_all();
        }

        let fx_buffers = fx_buffers.borrow();

        if inner.sample_area == sample_area
            && inner.geometry == geometry
            && inner.scale == scale
            && inner.corner_radius == corner_radius
            && inner.render_loc == render_loc
        {
            if !matches!(&inner.variant, BlurVariant::Optimized { texture }
                    if texture.size().w == fx_buffers.output_size().w
                        && texture.size().h == fx_buffers.output_size().h)
            {
                // If we are true blur, or if our output size changed, we need to re-render.
                // PERF: is there a better solution for true blur?
                inner.damage_all();
            }

            return Some(inner.clone());
        }

        inner.render_loc = render_loc;
        inner.sample_area = sample_area;
        inner.alpha_tex = self.alpha_tex.borrow().clone();
        inner.scale = scale;
        inner.geometry = geometry;
        inner.damage_all();
        inner.update_uniforms(&fx_buffers, &self.config);

        Some(inner.clone())
    }
}

#[derive(Clone, Debug)]
pub struct BlurRenderElement {
    id: Id,
    uniforms: Vec<Uniform<'static>>,
    sample_area: Rectangle<i32, Logical>,
    alpha_tex: Option<GlesTexture>,
    scale: f64,
    commit: CommitCounter,
    corner_radius: CornerRadius,
    geometry: Rectangle<f64, Logical>,
    variant: BlurVariant,
    render_loc: Point<f64, Logical>,
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
        corner_radius: CornerRadius,
        scale: f64,
        config: niri_config::Blur,
        geometry: Rectangle<f64, Logical>,
        alpha_tex: Option<GlesTexture>,
        variant: BlurVariant,
        render_loc: Point<f64, Logical>,
    ) -> Self {
        let mut this = Self {
            id: Id::new(),
            uniforms: Vec::with_capacity(7),
            alpha_tex,
            sample_area,
            scale,
            corner_radius,
            geometry,
            commit: CommitCounter::default(),
            variant,
            render_loc,
        };

        this.update_uniforms(fx_buffers, &config);

        this
    }

    fn update_uniforms(&mut self, fx_buffers: &EffectsFramebuffers, config: &niri_config::Blur) {
        let transform = Transform::Normal;

        let elem_geo: Rectangle<i32, _> = self.sample_area.to_physical_precise_round(self.scale);
        let elem_geo_loc = Vec2::new(elem_geo.loc.x as f32, elem_geo.loc.y as f32);
        let elem_geo_size = Vec2::new(elem_geo.size.w as f32, elem_geo.size.h as f32);

        let view_src = self.sample_area; // CORRECT
        let buf_size = fx_buffers.output_size().to_f64().to_logical(self.scale); // CORRECT
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
            Uniform::new("corner_radius", <[f32; 4]>::from(self.corner_radius)),
            Uniform::new("geo_size", geo_size.to_array()),
            Uniform::new("niri_scale", self.scale as f32),
            Uniform::new("noise", config.noise.0 as f32),
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
            self.sample_area
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

        let downscaled_dst = Rectangle::new(
            dst.loc,
            Size::from((
                (dst.size.w as f64 / self.scale) as i32,
                (dst.size.h as f64 / self.scale) as i32,
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
                downscaled_dst,
                damage,
                opaque_regions,
                Transform::Normal,
                1.,
                Some(&program),
                &self.uniforms,
            ),
            BlurVariant::True { fx_buffers, config } => {
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
                let blurred_texture = gles_frame.with_context(|gl| unsafe {
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
                        downscaled_dst,
                        self.alpha_tex.as_ref(),
                    )
                })??;

                gles_frame.render_texture_from_to(
                    &blurred_texture,
                    src,
                    downscaled_dst,
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

        <BlurRenderElement as RenderElement<GlesRenderer>>::draw(
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
