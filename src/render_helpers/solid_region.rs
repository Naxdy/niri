use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::GlesTexture;
use smithay::backend::renderer::sync::SyncPoint;
use smithay::utils::user_data::UserDataMap;
use smithay::{
    backend::renderer::{
        Color32F, Frame, Renderer,
        element::{Element, Id, RenderElement},
        gles::GlesRenderer,
        utils::CommitCounter,
    },
    utils::{Logical, Physical, Rectangle, Scale, Size, Transform},
};

use crate::render_helpers::render_to_texture;
use crate::utils::region::Region;

/// Renders the rectangles of a [`Region`] as blacked out content.
#[derive(Debug, Clone)]
pub struct SolidRegionRenderElement {
    id: Id,
    region: Region<f64, Logical>,
    commit: CommitCounter,
    scale: Scale<f64>,
}

impl SolidRegionRenderElement {
    pub fn new(region: Region<f64, Logical>, scale: Scale<f64>) -> Self {
        Self {
            id: Id::new(),
            region,
            commit: CommitCounter::default(),
            scale,
        }
    }
}

impl Element for SolidRegionRenderElement {
    fn id(&self) -> &smithay::backend::renderer::element::Id {
        &self.id
    }

    fn current_commit(&self) -> smithay::backend::renderer::utils::CommitCounter {
        self.commit
    }

    fn src(&self) -> smithay::utils::Rectangle<f64, smithay::utils::Buffer> {
        Rectangle::from_size(Size::from((1., 1.)))
    }

    fn geometry(
        &self,
        scale: smithay::utils::Scale<f64>,
    ) -> smithay::utils::Rectangle<i32, smithay::utils::Physical> {
        let mut rects = self.region.rects();

        let Some(first) = rects.next() else {
            return Rectangle::from_size(Size::from((1, 1)));
        };

        rects
            .fold(first, |acc, curr| acc.merge(curr))
            .to_physical_precise_round(scale)
    }
}

impl<R> RenderElement<R> for SolidRegionRenderElement
where
    R: Renderer,
{
    fn draw(
        &self,
        frame: &mut <R>::Frame<'_, '_>,
        _src: Rectangle<f64, smithay::utils::Buffer>,
        _dst: Rectangle<i32, smithay::utils::Physical>,
        damage: &[Rectangle<i32, smithay::utils::Physical>],
        _opaque_regions: &[Rectangle<i32, smithay::utils::Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), <R>::Error> {
        for rect in self.region.rects() {
            frame.draw_solid(
                rect.to_physical_precise_round(self.scale),
                damage,
                Color32F::BLACK,
            )?;
        }

        Ok(())
    }

    #[inline]
    fn underlying_storage(
        &self,
        _renderer: &mut R,
    ) -> Option<smithay::backend::renderer::element::UnderlyingStorage<'_>> {
        None
    }
}

pub fn render_region_to_texture(
    renderer: &mut GlesRenderer,
    region: Region<f64, Logical>,
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    transform: Transform,
    fourcc: Fourcc,
) -> anyhow::Result<(GlesTexture, SyncPoint)> {
    let elem = SolidRegionRenderElement::new(region, scale);

    render_to_texture(
        renderer,
        size,
        scale,
        transform,
        fourcc,
        std::iter::once(elem),
    )
}
