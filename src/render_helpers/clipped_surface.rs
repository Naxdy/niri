use glam::{Mat3, Vec2};
use niri_config::CornerRadius;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, ffi,
};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform};

use super::damage::ExtraDamage;
use super::renderer::AsGlesFrame as _;
use super::shaders::mat3_uniform;
use crate::backend::tty::{TtyFrame, TtyRenderer, TtyRendererError};

#[derive(Debug)]
pub struct ClippedSurfaceRenderElement<E>
where
    E: Element,
{
    inner: E,
    program: GlesTexProgram,
    corner_radius: CornerRadius,
    geometry: Rectangle<f64, Logical>,
    uniforms: Vec<Uniform<'static>>,
    alpha_tex: Option<GlesTexture>,
}

#[derive(Debug, Default, Clone)]
pub struct RoundedCornerDamage {
    damage: ExtraDamage,
    corner_radius: CornerRadius,
}

impl<E> ClippedSurfaceRenderElement<E>
where
    E: Element,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        elem: E,
        view_src: Rectangle<f64, Logical>,
        buf_size: Size<i32, Logical>,
        scale: Scale<f64>,
        geometry: Rectangle<f64, Logical>,
        program: GlesTexProgram,
        corner_radius: CornerRadius,
        alpha_tex: Option<GlesTexture>,
        ignore_alpha: f64,
    ) -> Self {
        let elem_geo = elem.geometry(scale);

        let elem_geo_loc = Vec2::new(elem_geo.loc.x as f32, elem_geo.loc.y as f32);
        let elem_geo_size = Vec2::new(elem_geo.size.w as f32, elem_geo.size.h as f32);

        let geo = geometry.to_physical_precise_round(scale);
        let geo_loc = Vec2::new(geo.loc.x, geo.loc.y);
        let geo_size = Vec2::new(geo.size.w, geo.size.h);

        let buf_size = Vec2::new(buf_size.w as f32, buf_size.h as f32);

        let src_loc = Vec2::new(view_src.loc.x as f32, view_src.loc.y as f32);
        let src_size = Vec2::new(view_src.size.w as f32, view_src.size.h as f32);

        let transform = elem.transform();
        // HACK: ??? for some reason flipped ones are fine.
        let transform = match transform {
            Transform::_90 => Transform::_270,
            Transform::_270 => Transform::_90,
            x => x,
        };
        let transform_matrix = Mat3::from_translation(Vec2::new(0.5, 0.5))
            * Mat3::from_cols_array(transform.matrix().as_ref())
            * Mat3::from_translation(-Vec2::new(0.5, 0.5));

        // FIXME: y_inverted
        let input_to_geo = transform_matrix * Mat3::from_scale(elem_geo_size / geo_size)
            * Mat3::from_translation((elem_geo_loc - geo_loc) / elem_geo_size)
            // Apply viewporter src.
            * Mat3::from_scale(buf_size / src_size)
            * Mat3::from_translation(-src_loc / buf_size);

        let mut uniforms = vec![
            Uniform::new("niri_scale", scale.x as f32),
            Uniform::new("geo_size", (geometry.size.w as f32, geometry.size.h as f32)),
            Uniform::new("corner_radius", <[f32; 4]>::from(corner_radius)),
            mat3_uniform("input_to_geo", input_to_geo),
        ];

        if alpha_tex.is_some() && ignore_alpha > 0. {
            uniforms.push(Uniform::new("alpha_tex", 1));
            uniforms.push(Uniform::new("ignore_alpha", ignore_alpha as f32));
        }

        Self {
            inner: elem,
            program,
            corner_radius,
            geometry,
            uniforms,
            alpha_tex,
        }
    }

    pub fn will_clip(
        elem: &E,
        scale: Scale<f64>,
        geometry: Rectangle<f64, Logical>,
        corner_radius: CornerRadius,
    ) -> bool {
        let elem_geo = elem.geometry(scale);
        let geo = geometry.to_physical_precise_round(scale);

        if corner_radius == CornerRadius::default() {
            !geo.contains_rect(elem_geo)
        } else {
            let corners = Self::rounded_corners(geometry, corner_radius);
            let corners = corners
                .into_iter()
                .map(|rect| rect.to_physical_precise_up(scale));
            let geo = Rectangle::subtract_rects_many([geo], corners);
            !Rectangle::subtract_rects_many([elem_geo], geo).is_empty()
        }
    }

    fn rounded_corners(
        geo: Rectangle<f64, Logical>,
        corner_radius: CornerRadius,
    ) -> [Rectangle<f64, Logical>; 4] {
        let top_left = corner_radius.top_left as f64;
        let top_right = corner_radius.top_right as f64;
        let bottom_right = corner_radius.bottom_right as f64;
        let bottom_left = corner_radius.bottom_left as f64;

        [
            Rectangle::new(geo.loc, Size::from((top_left, top_left))),
            Rectangle::new(
                Point::from((geo.loc.x + geo.size.w - top_right, geo.loc.y)),
                Size::from((top_right, top_right)),
            ),
            Rectangle::new(
                Point::from((
                    geo.loc.x + geo.size.w - bottom_right,
                    geo.loc.y + geo.size.h - bottom_right,
                )),
                Size::from((bottom_right, bottom_right)),
            ),
            Rectangle::new(
                Point::from((geo.loc.x, geo.loc.y + geo.size.h - bottom_left)),
                Size::from((bottom_left, bottom_left)),
            ),
        ]
    }
}

impl<E> Element for ClippedSurfaceRenderElement<E>
where
    E: Element,
{
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        // FIXME: radius changes need to cause damage.
        let damage = self.inner.damage_since(scale, commit);

        // Intersect with geometry, since we're clipping by it.
        let mut geo = self.geometry.to_physical_precise_round(scale);
        geo.loc -= self.geometry(scale).loc;
        damage
            .into_iter()
            .filter_map(|rect| rect.intersection(geo))
            .collect()
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        let regions = self.inner.opaque_regions(scale);

        // Intersect with geometry, since we're clipping by it.
        let mut geo = self.geometry.to_physical_precise_round(scale);
        geo.loc -= self.geometry(scale).loc;
        let regions = regions
            .into_iter()
            .filter_map(|rect| rect.intersection(geo));

        // Subtract the rounded corners.
        if self.corner_radius == CornerRadius::default() {
            regions.collect()
        } else {
            let corners = Self::rounded_corners(self.geometry, self.corner_radius);

            let elem_loc = self.geometry(scale).loc;
            let corners = corners.into_iter().map(|rect| {
                let mut rect = rect.to_physical_precise_up(scale);
                rect.loc -= elem_loc;
                rect
            });

            OpaqueRegions::from_slice(&Rectangle::subtract_rects_many(regions, corners))
        }
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl<E> RenderElement<GlesRenderer> for ClippedSurfaceRenderElement<E>
where
    E: RenderElement<GlesRenderer>,
{
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        frame.override_default_tex_program(self.program.clone(), self.uniforms.clone());

        if let Some(alpha_tex) = &self.alpha_tex {
            frame.with_context(|gl| unsafe {
                gl.ActiveTexture(ffi::TEXTURE1);
                gl.BindTexture(ffi::TEXTURE_2D, alpha_tex.tex_id());
                gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, ffi::LINEAR as i32);
                gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MAG_FILTER, ffi::LINEAR as i32);
            })?;
        }

        RenderElement::<GlesRenderer>::draw(&self.inner, frame, src, dst, damage, opaque_regions)?;
        frame.clear_tex_program_override();
        Ok(())
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        // If scanout for things other than Wayland buffers is implemented, this will need to take
        // the target GPU into account.
        None
    }
}

impl<'render, E> RenderElement<TtyRenderer<'render>> for ClippedSurfaceRenderElement<E>
where
    E: RenderElement<TtyRenderer<'render>>,
{
    fn draw(
        &self,
        frame: &mut TtyFrame<'render, '_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), TtyRendererError<'render>> {
        frame
            .as_gles_frame()
            .override_default_tex_program(self.program.clone(), self.uniforms.clone());

        if let Some(alpha_tex) = &self.alpha_tex {
            frame.as_gles_frame().with_context(|gl| unsafe {
                gl.ActiveTexture(ffi::TEXTURE1);
                gl.BindTexture(ffi::TEXTURE_2D, alpha_tex.tex_id());
                gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, ffi::LINEAR as i32);
                gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MAG_FILTER, ffi::LINEAR as i32);
            })?;
        }

        RenderElement::draw(&self.inner, frame, src, dst, damage, opaque_regions)?;
        frame.as_gles_frame().clear_tex_program_override();
        Ok(())
    }

    fn underlying_storage(
        &self,
        _renderer: &mut TtyRenderer<'render>,
    ) -> Option<UnderlyingStorage<'_>> {
        // If scanout for things other than Wayland buffers is implemented, this will need to take
        // the target GPU into account.
        None
    }
}

impl RoundedCornerDamage {
    pub fn set_size(&mut self, size: Size<f64, Logical>) {
        self.damage.set_size(size);
    }

    pub fn set_corner_radius(&mut self, corner_radius: CornerRadius) {
        if self.corner_radius == corner_radius {
            return;
        }

        // FIXME: make the damage granular.
        self.corner_radius = corner_radius;
        self.damage.damage_all();
    }

    pub fn element(&self) -> ExtraDamage {
        self.damage.clone()
    }
}
