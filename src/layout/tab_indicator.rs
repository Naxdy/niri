use std::cell::RefCell;
use std::cmp::min;
use std::iter::zip;
use std::mem;

use anyhow::ensure;
use itertools::izip;
use niri_config::{CornerRadius, Gradient, GradientRelativeTo, TabIndicatorPosition};
use pango::FontDescription;
use pango::glib::property::PropertySet;
use pangocairo::cairo::{self, ImageSurface};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::utils::{Logical, Physical, Point, Rectangle, Size, Transform};

use super::LayoutElement;
use crate::animation::{Animation, Clock};
use crate::niri_render_elements;
use crate::render_helpers::border::BorderRenderElement;
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::utils::render::{PushRenderElement, Render};
use crate::utils::{
    floor_logical_in_physical_max1, round_logical_in_physical, round_logical_in_physical_max1,
    to_physical_precise_round,
};

const MIN_DIST_TO_EDGES: f64 = 20.;

/// Fixed distance between the font and the tab bar
const GAP_TO_BAR: f64 = 2.;

#[derive(Debug)]
pub struct TabIndicator {
    shader_locs: Vec<Point<f64, Logical>>,
    shaders: Vec<BorderRenderElement>,
    open_anim: Option<Animation>,
    tabs: Vec<TabInfo>,
    title_textures: Vec<TitleTexture>,
    config: niri_config::TabIndicator,
}

#[derive(Debug)]
pub struct TabInfo {
    /// Gradient for the tab indicator.
    pub gradient: Gradient,
    /// Tab geometry in the same coordinate system as the area.
    pub geometry: Rectangle<f64, Logical>,
    /// The title for this tab.
    pub title: String,
}

niri_render_elements! {
    TabIndicatorRenderElement => {
        Gradient = BorderRenderElement,
        Title = PrimaryGpuTextureRenderElement,
    }
}

#[derive(Debug, Default)]
struct TitleTexture {
    title: String,
    scale: f64,
    max_size: Size<f64, Logical>,
    // cached result of the rendered title texture
    texture: RefCell<Option<TextureBuffer<GlesTexture>>>,
    // the maximum size wanted by the title texture if it had infinite space
    wanted_size: RefCell<Option<Size<i32, Physical>>>,
    font_size: u32,
}

impl TabIndicator {
    pub const fn new(config: niri_config::TabIndicator) -> Self {
        Self {
            shader_locs: Vec::new(),
            shaders: Vec::new(),
            tabs: Vec::new(),
            title_textures: Vec::new(),
            open_anim: None,
            config,
        }
    }

    pub const fn update_config(&mut self, config: niri_config::TabIndicator) {
        self.config = config;
    }

    pub fn update_shaders(&mut self) {
        for elem in &mut self.shaders {
            elem.damage_all();
        }
    }

    pub fn advance_animations(&mut self) {
        if let Some(anim) = &mut self.open_anim
            && anim.is_done()
        {
            self.open_anim = None;
        }
    }

    pub const fn are_animations_ongoing(&self) -> bool {
        self.open_anim.is_some()
    }

    pub fn start_open_animation(&mut self, clock: Clock, config: niri_config::Animation) {
        self.open_anim = Some(Animation::new(clock, 0., 1., 0., config));
    }

    fn tab_rects(
        &self,
        area: Rectangle<f64, Logical>,
        count: usize,
        scale: f64,
    ) -> impl Iterator<Item = Rectangle<f64, Logical>> + use<> {
        let round = |logical: f64| round_logical_in_physical(scale, logical);
        let round_max1 = |logical: f64| round_logical_in_physical_max1(scale, logical);

        let progress = self.open_anim.as_ref().map_or(1., |a| a.value().max(0.));

        let width = round_max1(self.config.width);
        let gaps_between = round_max1(self.config.gaps_between_tabs);

        let position = self.config.position;
        let side = area.size.w;
        let total_prop = self.config.length.total_proportion.unwrap_or(0.5);
        let min_length = round(side * total_prop.clamp(0., 2.));

        // Compute px_per_tab before applying the animation to gaps_between in order to avoid it
        // growing and shrinking over the duration of the animation.
        let pixel = 1. / scale;
        let shortest_length = (count as f64).mul_add(pixel + gaps_between, -gaps_between);
        let length = f64::max(min_length, shortest_length);
        let px_per_tab = (length + gaps_between) / count as f64 - gaps_between;

        let gaps_between = round(self.config.gaps_between_tabs * progress);

        let length = ((count - 1) as f64).mul_add(px_per_tab + gaps_between, px_per_tab * progress);
        let px_per_tab = floor_logical_in_physical_max1(scale, px_per_tab);
        let floored_length =
            ((count - 1) as f64).mul_add(px_per_tab + gaps_between, px_per_tab * progress);
        let mut ones_left = ((length - floored_length) / pixel).round() as usize;

        let mut shader_loc = Point::from((0., round((side - length) / 2.)));
        match position {
            TabIndicatorPosition::Top => mem::swap(&mut shader_loc.x, &mut shader_loc.y),
            TabIndicatorPosition::Bottom => {
                shader_loc.x = shader_loc.y;
                shader_loc.y = area.size.h - width;
            }
        }
        shader_loc += area.loc;

        (0..count).map(move |idx| {
            let mut px_per_tab = px_per_tab;
            if ones_left > 0 {
                ones_left -= 1;
                px_per_tab += pixel;
            }

            let loc = shader_loc;

            shader_loc.x += px_per_tab + gaps_between;

            let size = Size::from((
                px_per_tab * if idx == count - 1 { progress } else { 1. },
                width,
            ));

            Rectangle::new(loc, size)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_render_elements(
        &mut self,
        tabs: Vec<TabInfo>,
        enabled: bool,
        // Geometry of the tabs area.
        area: Rectangle<f64, Logical>,
        // View rect relative to the tabs area.
        area_view_rect: Rectangle<f64, Logical>,
        is_active: bool,
        scale: f64,
    ) {
        self.tabs = tabs;
        let tab_count = self.tabs.len();

        if !enabled || self.config.off {
            self.shader_locs.clear();
            self.shaders.clear();
            return;
        }

        let count = tab_count;
        if self.config.hide_when_single_tab && count == 1 {
            self.shader_locs.clear();
            self.shaders.clear();
            return;
        }

        self.shaders.resize_with(count, Default::default);
        self.shader_locs.resize_with(count, Default::default);

        let radius = self.config.corner_radius as f32;
        let shared_rounded_corners = self.config.gaps_between_tabs == 0.;
        let mut tabs_left = tab_count;

        let rects = self.tab_rects(area, count, scale).collect::<Vec<_>>();

        if self.title_textures.len() != self.tabs.len() {
            self.title_textures = zip(self.tabs.iter(), rects.iter())
                .map(|(t, rect)| {
                    TitleTexture::new(
                        t.title.clone(),
                        scale,
                        Size::new((rect.size.w - 20.).max(0.), 24.),
                        self.config.title_font_size,
                    )
                })
                .collect();
        } else {
            izip!(
                self.title_textures.iter_mut(),
                self.tabs.iter(),
                rects.iter(),
            )
            .for_each(|(tex, t, rect)| {
                tex.update_config(
                    Some(t.title.clone()),
                    Some(scale),
                    Some(Size::new((rect.size.w - MIN_DIST_TO_EDGES).max(0.), 16384.)),
                    Some(self.config.title_font_size),
                );
            });
        }

        for (shader, loc, tab, rect) in izip!(
            &mut self.shaders,
            &mut self.shader_locs,
            &self.tabs,
            rects.iter(),
        ) {
            *loc = rect.loc;

            let mut gradient_area = match tab.gradient.relative_to {
                GradientRelativeTo::Window => tab.geometry,
                GradientRelativeTo::WorkspaceView => area_view_rect,
            };
            gradient_area.loc -= *loc;

            let mut color_from = tab.gradient.from;
            let mut color_to = tab.gradient.to;
            if !is_active {
                color_from *= 0.5;
                color_to *= 0.5;
            }

            let radius = if shared_rounded_corners && tab_count > 1 {
                if tabs_left == tab_count {
                    // First tab.
                    CornerRadius {
                        top_left: radius,
                        top_right: 0.,
                        bottom_right: 0.,
                        bottom_left: radius,
                    }
                } else if tabs_left == 1 {
                    // Last tab.
                    CornerRadius {
                        top_left: 0.,
                        top_right: radius,
                        bottom_right: radius,
                        bottom_left: 0.,
                    }
                } else {
                    // Tab in the middle.
                    CornerRadius::default()
                }
            } else {
                // Separate tabs, or the only tab.
                CornerRadius::from(radius)
            };
            let radius = radius.fit_to(rect.size.w as f32, rect.size.h as f32);
            tabs_left -= 1;

            shader.update(
                rect.size,
                gradient_area,
                tab.gradient.in_,
                color_from,
                color_to,
                ((tab.gradient.angle as f32) - 90.).to_radians(),
                Rectangle::from_size(rect.size),
                0.,
                radius,
                scale as f32,
                1.,
            );
        }
    }

    fn font_height(&self) -> f64 {
        if !self.config.hide_titles {
            // we need an initial approximate value here, because when we first spawn the tab
            // indicator, the textures are not yet rendered, but the tile resize animation plays
            // immediately.
            self.title_textures
                .iter()
                .fold(self.config.title_font_size as f64, |acc, curr| {
                    if let Some(texture) = curr.texture.borrow().as_ref() {
                        texture.logical_size().h.max(acc)
                    } else {
                        acc
                    }
                })
        } else {
            0.
        }
    }

    pub fn hit(
        &self,
        area: Rectangle<f64, Logical>,
        tab_count: usize,
        scale: f64,
        point: Point<f64, Logical>,
    ) -> Option<usize> {
        if self.config.off {
            return None;
        }

        let count = tab_count;
        if self.config.hide_when_single_tab && count == 1 {
            return None;
        }

        let font_height = self.font_height();

        self.tab_rects(area, count, scale)
            .map(|mut rect| {
                if font_height > 0. {
                    match self.config.position {
                        TabIndicatorPosition::Top => {
                            rect.loc.y -= GAP_TO_BAR;
                        }
                        TabIndicatorPosition::Bottom => {
                            rect.loc.y -= font_height + GAP_TO_BAR + self.config.gap;
                        }
                    }

                    rect.size.h += font_height + GAP_TO_BAR + self.config.gap;
                }
                rect
            })
            .enumerate()
            .find_map(|(idx, rect)| rect.contains(point).then_some(idx))
    }

    /// Extra size occupied by the tab indicator.
    pub fn extra_size(&self, tab_count: usize, scale: f64) -> Size<f64, Logical> {
        if self.config.off || (self.config.hide_when_single_tab && tab_count == 1) {
            return Size::from((0., 0.));
        }

        let round = |logical: f64| round_logical_in_physical(scale, logical);
        let width = round(self.config.width);
        let gap = round(self.config.gap);
        let font_height = self.font_height()
            + (if !self.config.hide_titles {
                GAP_TO_BAR
            } else {
                0.
            });

        // No, I am *not* falling into the rabbit hole of "what if the tab indicator is wide enough
        // that it peeks from the other side of the window".
        let size = f64::max(0., width + gap + font_height);

        Size::from((0., size))
    }

    /// Offset of the tabbed content due to space occupied by the tab indicator.
    pub fn content_offset(&self, tab_count: usize, scale: f64) -> Point<f64, Logical> {
        match self.config.position {
            TabIndicatorPosition::Top => self.extra_size(tab_count, scale).to_point(),
            TabIndicatorPosition::Bottom => Point::from((0., 0.)),
        }
    }

    pub const fn config(&self) -> niri_config::TabIndicator {
        self.config
    }
}

impl TabInfo {
    #[allow(clippy::too_many_arguments)]
    pub fn new<W: LayoutElement>(
        window: &W,
        focus_ring_config: &niri_config::FocusRing,
        border_config: &niri_config::FocusRing,
        is_active: bool,
        is_urgent: bool,
        config: &niri_config::TabIndicator,
        tile_size: Size<f64, Logical>,
    ) -> Self {
        let rules = window.rules();
        let rule = rules.tab_indicator;

        let gradient_from_rule = || {
            let (color, gradient) = if is_urgent {
                (rule.urgent_color, rule.urgent_gradient)
            } else if is_active {
                (rule.active_color, rule.active_gradient)
            } else {
                (rule.inactive_color, rule.inactive_gradient)
            };
            let color = color.map(Gradient::from);
            gradient.or(color)
        };

        let gradient_from_config = || {
            let (color, gradient) = if is_urgent {
                (config.urgent_color, config.urgent_gradient)
            } else if is_active {
                (config.active_color, config.active_gradient)
            } else {
                (config.inactive_color, config.inactive_gradient)
            };
            let color = color.map(Gradient::from);
            gradient.or(color)
        };

        let gradient_from_border = || {
            // Come up with tab indicator gradient matching the focus ring or the border, whichever
            // one is enabled.
            let config = if focus_ring_config.off {
                border_config
            } else {
                focus_ring_config
            };

            let (color, gradient) = if is_urgent {
                (config.urgent_color, config.urgent_gradient)
            } else if is_active {
                (config.active_color, config.active_gradient)
            } else {
                (config.inactive_color, config.inactive_gradient)
            };
            gradient.unwrap_or_else(|| Gradient::from(color))
        };

        let gradient = gradient_from_rule()
            .or_else(gradient_from_config)
            .unwrap_or_else(gradient_from_border);

        let geometry = Rectangle::new(Point::default(), tile_size);

        Self {
            gradient,
            geometry,
            title: window.title().unwrap_or_default(),
        }
    }
}

impl TitleTexture {
    fn new(title: String, scale: f64, max_size: Size<f64, Logical>, font_size: u32) -> Self {
        Self {
            title,
            scale,
            texture: Default::default(),
            max_size,
            wanted_size: Default::default(),
            font_size,
        }
    }

    fn update_config(
        &mut self,
        new_title: Option<String>,
        new_scale: Option<f64>,
        new_max_size: Option<Size<f64, Logical>>,
        new_font_size: Option<u32>,
    ) {
        if let Some(new_font_size) = new_font_size {
            if new_font_size != self.font_size {
                self.texture.set(None);
                self.wanted_size.set(None);
            }
            self.font_size = new_font_size;
        }
        if let Some(new_title) = new_title {
            if new_title != self.title {
                self.texture.set(None);
                self.wanted_size.set(None);
            }
            self.title = new_title;
        }

        if let Some(new_scale) = new_scale {
            if new_scale != self.scale {
                self.texture.set(None);
                self.wanted_size.set(None);
            }
            self.scale = new_scale;
        }

        if let Some(new_max_size) = new_max_size {
            if new_max_size != self.max_size {
                // if we have a size adjustment, we only need to re-render if either:
                //  a) the texture's wanted size is larger than the current max size _and_ the new
                //  max size is larger
                //  b) the new max size is smaller than the current texture
                //
                // otherwise the texture will still fully fit within the current allocated space
                let should_dirty = if let (Some(texture), Some(wanted_size)) = (
                    self.texture.borrow().as_ref(),
                    self.wanted_size.borrow().as_ref(),
                ) {
                    let wanted_size = wanted_size.to_f64().to_logical(self.scale);
                    let tex_size = texture.logical_size();
                    (wanted_size.w >= self.max_size.w && new_max_size.w > self.max_size.w)
                        || (wanted_size.h >= self.max_size.h && new_max_size.h > self.max_size.h)
                        || (tex_size.w > new_max_size.w)
                        || (tex_size.h > new_max_size.h)
                } else {
                    true
                };

                if should_dirty {
                    self.texture.set(None);
                    self.wanted_size.set(None);
                }
            }
            self.max_size = new_max_size;
        }
    }

    fn get(&self, renderer: &mut GlesRenderer) -> anyhow::Result<TextureBuffer<GlesTexture>> {
        let mut tex = self.texture.borrow_mut();

        if self.title.is_empty() {
            return Err(anyhow::anyhow!(
                "cannot render title texture if title is empty"
            ));
        }

        match &*tex {
            Some(texture) => Ok(texture.clone()),
            None => {
                let (new_tex, wanted_size) = render_title_texture(
                    renderer,
                    &self.title,
                    self.scale,
                    self.max_size,
                    self.font_size,
                )?;
                *tex = Some(new_tex.clone());
                self.wanted_size.set(Some(wanted_size));
                Ok(new_tex)
            }
        }
    }
}

fn render_title_texture(
    renderer: &mut GlesRenderer,
    title: &str,
    scale: f64,
    max_size: Size<f64, Logical>,
    font_size: u32,
) -> anyhow::Result<(TextureBuffer<GlesTexture>, Size<i32, Physical>)> {
    let _span = tracy_client::span!("tab_indicator::render_title_texture");

    // TODO: expose in config
    let mut font = FontDescription::from_string(&format!("sans {font_size}px"));
    font.set_absolute_size(to_physical_precise_round(scale, font.size()));

    let surface = ImageSurface::create(cairo::Format::ARgb32, 0, 0)?;
    let cr = cairo::Context::new(&surface)?;
    let layout = pangocairo::functions::create_layout(&cr);
    layout.context().set_round_glyph_positions(false);

    layout.set_single_paragraph_mode(true);
    layout.set_font_description(Some(&font));
    layout.set_text(title);

    let (width, height) = layout.pixel_size();
    let wanted_size = Size::new(width, height);

    // Guard against overly long window titles.
    let max_size = max_size.to_physical_precise_round(scale);
    let width = min(width, max_size.w);
    let height = min(height, max_size.h);

    ensure!(width > 0 && height > 0);

    let surface = ImageSurface::create(cairo::Format::ARgb32, width, height)?;
    let cr = cairo::Context::new(&surface)?;
    cr.set_source_rgb(1., 1., 1.);
    pangocairo::functions::show_layout(&cr, &layout);

    drop(cr);

    let data = surface.take_data().unwrap();
    let buffer = TextureBuffer::from_memory(
        renderer,
        &data,
        Fourcc::Argb8888,
        (width, height),
        false,
        scale,
        Transform::Normal,
        Vec::new(),
    )?;

    Ok((buffer, wanted_size))
}

impl<R> Render<'_, R> for TabIndicator
where
    R: NiriRenderer,
{
    type RenderContext = Point<f64, Logical>;
    type RenderElement = TabIndicatorRenderElement;

    fn render<C>(&self, renderer: &mut R, render_context: Self::RenderContext, collector: &mut C)
    where
        C: PushRenderElement<TabIndicatorRenderElement, R>,
    {
        let pos = render_context;

        let has_border_shader = BorderRenderElement::has_shader(renderer);
        if !has_border_shader {
            return;
        }

        (!self.config.hide_titles).then(|| {
            zip(&self.title_textures, &self.shader_locs)
                .filter_map(|(tex, loc)| match tex.get(renderer.as_gles_renderer()) {
                    Ok(texture) => {
                        let pos_x = (tex.max_size.w + MIN_DIST_TO_EDGES) / 2.
                            - texture.logical_size().w / 2.;

                        let pos_y = match self.config.position {
                            TabIndicatorPosition::Top => -GAP_TO_BAR,
                            TabIndicatorPosition::Bottom => GAP_TO_BAR - texture.logical_size().h,
                        };

                        Some(PrimaryGpuTextureRenderElement(
                            TextureRenderElement::from_texture_buffer(
                                texture,
                                pos + *loc + Point::new(pos_x, pos_y),
                                1.,
                                None,
                                None,
                                Kind::Unspecified,
                            ),
                        ))
                    }
                    Err(_) => {
                        // silent fail is ok, we just won't show the title
                        None
                    }
                })
                .for_each(|e| collector.push_element(e))
        });

        let font_height = self.font_height();

        zip(&self.shaders, &self.shader_locs)
            .map(move |(shader, loc)| {
                let offset = if !self.config.hide_titles {
                    match self.config.position {
                        TabIndicatorPosition::Top => Point::new(0., font_height + GAP_TO_BAR),
                        TabIndicatorPosition::Bottom => Point::new(0., -font_height - GAP_TO_BAR),
                    }
                } else {
                    Point::default()
                };

                shader.clone().with_location(pos + *loc + offset)
            })
            .for_each(|e| collector.push_element(e));
    }
}
