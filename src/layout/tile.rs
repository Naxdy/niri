use core::f64;
use std::rc::Rc;
use std::sync::atomic::Ordering;

use niri_config::utils::MergeWith as _;
use niri_config::{Color, CornerRadius, GradientInterpolation};
use niri_ipc::WindowLayout;
use portable_atomic::AtomicU8;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::{Element, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Logical, Point, Rectangle, Scale, Size};

use super::focus_ring::{FocusRing, FocusRingRenderElement};
use super::opening_window::{OpenAnimation, OpeningWindowRenderElement};
use super::shadow::Shadow;
use super::{
    HitType, LayoutElement, LayoutElementRenderElement, LayoutElementRenderSnapshot, Options,
    RESIZE_ANIMATION_THRESHOLD, SizeFrac,
};
use crate::animation::{Animation, Clock};
use crate::layout::SizingMode;
use crate::layout::tab_indicator::{TabIndicator, TabIndicatorRenderElement, TabInfo};
use crate::niri_render_elements;
use crate::render_helpers::RenderTarget;
use crate::render_helpers::blur::EffectsFramebuffersUserData;
use crate::render_helpers::blur::element::{Blur, BlurRenderElement};
use crate::render_helpers::border::BorderRenderElement;
use crate::render_helpers::clipped_surface::{ClippedSurfaceRenderElement, RoundedCornerDamage};
use crate::render_helpers::damage::ExtraDamage;
use crate::render_helpers::offscreen::{OffscreenBuffer, OffscreenRenderElement};
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::resize::ResizeRenderElement;
use crate::render_helpers::shaders::Shaders;
use crate::render_helpers::shadow::ShadowRenderElement;
use crate::render_helpers::snapshot::RenderSnapshot;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::utils::transaction::Transaction;
use crate::utils::{
    baba_is_float_offset, round_logical_in_physical, round_logical_in_physical_max1,
};

#[derive(Debug)]
struct WindowSizeOverride {
    size_override: Option<Size<i32, Logical>>,
    rendered_frames: AtomicU8,
}

impl WindowSizeOverride {
    const fn new() -> Self {
        Self {
            size_override: None,
            rendered_frames: AtomicU8::new(0),
        }
    }

    fn set(&mut self, size_override: Size<i32, Logical>) {
        self.size_override = Some(size_override);
        self.rendered_frames.store(0, Ordering::SeqCst);
    }

    fn get(&self) -> Option<Size<i32, Logical>> {
        if self.rendered_frames.load(Ordering::SeqCst) < 3 {
            self.size_override
        } else {
            None
        }
    }

    fn increment(&self) {
        if self.rendered_frames.load(Ordering::SeqCst) < 3 {
            self.rendered_frames.add(1, Ordering::SeqCst);
        }
    }
}

#[derive(Debug)]
enum WindowInner<W: LayoutElement> {
    // needs to be an `Option`, so we can `take` in `WindowInner::group`
    // this will never be `None` in real life though, so unwrapping is safe
    Single(Option<W>),
    Multiple { windows: Vec<W>, focus_idx: usize },
}

impl<W: LayoutElement> WindowInner<W> {
    /// Turn this [`WindowInner::Single`] into a [`WindowInner::Multiple`] with one member. Has no
    /// effect if this is already a [`WindowInner::Multiple`].
    fn group(&mut self) {
        if let Self::Single(w) = self {
            *self = Self::Multiple {
                windows: vec![w.take().expect("should always have a window")],
                focus_idx: 0,
            };
        }
    }

    /// Attempt to focus a window by id. Returns `true` if a window was found & focused, `false`
    /// otherwise.
    fn focus_window(&mut self, id: &W::Id) -> bool {
        match self {
            Self::Single(w) => w.as_ref().unwrap().id() == id,
            Self::Multiple { windows, focus_idx } => {
                if let Some(new_focus_idx) = windows.iter().position(|w| w.id() == id) {
                    *focus_idx = new_focus_idx;
                    true
                } else {
                    false
                }
            }
        }
    }

    fn focused_window(&self) -> &W {
        match self {
            Self::Single(w) => w.as_ref().unwrap(),
            Self::Multiple { windows, focus_idx } => windows
                .get(*focus_idx)
                .expect("should have correct focus_idx"),
        }
    }

    fn focused_window_mut(&mut self) -> &mut W {
        match self {
            Self::Single(w) => w.as_mut().unwrap(),
            Self::Multiple { windows, focus_idx } => windows
                .get_mut(*focus_idx)
                .expect("should have correct focus_idx"),
        }
    }

    fn focus_next(&mut self) -> &W {
        match self {
            Self::Single(w) => w.as_ref().unwrap(),
            Self::Multiple { windows, focus_idx } => {
                *focus_idx = (*focus_idx + 1) % windows.len();
                windows
                    .get(*focus_idx)
                    .expect("should have correct focus_idx")
            }
        }
    }

    fn focus_prev(&mut self) -> &W {
        match self {
            Self::Single(w) => w.as_ref().unwrap(),
            Self::Multiple { windows, focus_idx } => {
                *focus_idx = (*focus_idx - 1) % windows.len();
                windows
                    .get(*focus_idx)
                    .expect("should have correct focus_idx")
            }
        }
    }

    /// Removes a window. Returns the number of remaining windows in this [`WindowInner`].
    fn remove_window(&mut self, id: &W::Id) -> usize {
        match self {
            Self::Single(w) => {
                if w.as_ref().unwrap().id() == id {
                    0
                } else {
                    1
                }
            }
            Self::Multiple { windows, focus_idx } => {
                // If we only have a single window remaining, don't actually remove it yet, as the
                // tile itself will be removed instead, and we still need the window for things
                // like closing animations.
                if windows.len() == 1 {
                    return 0;
                }

                let current_focus_id = windows
                    .get(*focus_idx)
                    .expect("should have correct focus_idx")
                    .id()
                    .clone();

                if let Some(to_remove_idx) = windows.iter().position(|w| w.id() == id) {
                    windows.remove(to_remove_idx);

                    *focus_idx = windows
                        .iter()
                        .position(|w| *w.id() == current_focus_id)
                        .unwrap_or(0);
                }

                windows.len()
            }
        }
    }

    /// Ungroup a given window from this tile, and return the removed window.
    fn ungroup_single(&mut self, id: &W::Id) -> Option<W> {
        match self {
            Self::Single(_) => None,
            Self::Multiple { windows, focus_idx } => {
                if windows.len() == 1 {
                    let window = windows.pop().expect("should have exactly 1 element");

                    *self = Self::Single(Some(window));

                    None
                } else if let Some(remove_idx) = windows.iter().position(|w| w.id() == id) {
                    let prev_focus_id = windows[*focus_idx].id().clone();
                    let ungrouped = windows.remove(remove_idx);

                    *focus_idx = windows
                        .iter()
                        .position(|w| w.id() == &prev_focus_id)
                        .unwrap_or(0);

                    Some(ungrouped)
                } else {
                    None
                }
            }
        }
    }

    /// Ungroup all windows, returning a list of [`Tile`]s containing the ungrouped windows, and
    /// turning this into an instance of [`WindowInner::Single`].
    fn ungroup_all(
        &mut self,
        view_size: Size<f64, Logical>,
        scale: f64,
        clock: Clock,
        options: Rc<Options>,
    ) -> Vec<Tile<W>> {
        match self {
            Self::Single(_) => Vec::new(),
            Self::Multiple { windows, focus_idx } => {
                let focused_window = windows.remove(*focus_idx);
                let new_tiles = windows
                    .drain(..)
                    .map(|w| Tile::new(w, view_size, scale, clock.clone(), options.clone()))
                    .collect();

                *self = Self::Single(Some(focused_window));

                new_tiles
            }
        }
    }

    /// Add a window to this [`WindowInner`], and focus it.
    fn add_window(&mut self, window: W) {
        match self {
            Self::Single(_) => {
                error!("attempted to add window to non-grouped tile");
            }
            Self::Multiple { windows, focus_idx } => {
                windows.push(window);
                *focus_idx = windows.len().checked_sub(1).expect("should never overflow");
            }
        }
    }

    fn iter(&self) -> impl Iterator<Item = &W> {
        WindowInnerIter {
            inner: self,
            count_idx: 0,
        }
    }

    fn iter_mut(&mut self) -> impl Iterator<Item = &mut W> {
        match self {
            Self::Single(w) => WindowInnerIterMut::Single(Some(w.as_mut().unwrap())),
            Self::Multiple {
                windows,
                focus_idx: _,
            } => WindowInnerIterMut::Multiple(windows.iter_mut()),
        }
    }
}

pub enum WindowInnerIterMut<'a, W: LayoutElement> {
    Single(Option<&'a mut W>),
    Multiple(std::slice::IterMut<'a, W>),
}

impl<'a, W: LayoutElement> Iterator for WindowInnerIterMut<'a, W> {
    type Item = &'a mut W;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Single(w) => w.take(),
            Self::Multiple(iter) => iter.next(),
        }
    }
}

pub struct WindowInnerIter<'a, W: LayoutElement> {
    inner: &'a WindowInner<W>,
    count_idx: usize,
}

impl<'a, W: LayoutElement> Iterator for WindowInnerIter<'a, W> {
    type Item = &'a W;

    fn next(&mut self) -> Option<Self::Item> {
        match &self.inner {
            WindowInner::Single(window) => {
                if self.count_idx == 0 {
                    self.count_idx += 1;
                    Some(window.as_ref().unwrap())
                } else {
                    None
                }
            }
            WindowInner::Multiple {
                windows,
                focus_idx: _,
            } => {
                let r = windows.get(self.count_idx);
                self.count_idx += 1;
                r
            }
        }
    }
}

impl<W: LayoutElement> WindowInner<W> {}

/// Toplevel window with decorations.
#[derive(Debug)]
pub struct Tile<W: LayoutElement> {
    /// The toplevel window (or group) itself.
    window: WindowInner<W>,

    /// The border around the window.
    border: FocusRing,

    /// The focus ring around the window.
    focus_ring: FocusRing,

    /// The shadow around the window.
    shadow: Shadow,

    /// This tile's current sizing mode.
    ///
    /// This will update only when the `window` actually goes maximized or fullscreen, rather than
    /// right away, to avoid black backdrop flicker before the window has had a chance to resize.
    sizing_mode: SizingMode,

    /// The black backdrop for fullscreen windows.
    fullscreen_backdrop: SolidColorBuffer,

    /// Whether the tile should float upon unfullscreening.
    pub(super) restore_to_floating: bool,

    /// The size that the window should assume when going floating.
    ///
    /// This is generally the last size the window had when it was floating. It can be unknown if
    /// the window starts out in the tiling layout or fullscreen.
    pub(super) floating_window_size: Option<Size<i32, Logical>>,

    /// The position that the tile should assume when going floating, relative to the floating
    /// space working area.
    ///
    /// This is generally the last position the tile had when it was floating. It can be unknown if
    /// the window starts out in the tiling layout.
    pub(super) floating_pos: Option<Point<f64, SizeFrac>>,

    /// Currently selected preset width index when this tile is floating.
    pub(super) floating_preset_width_idx: Option<usize>,

    /// Currently selected preset height index when this tile is floating.
    pub(super) floating_preset_height_idx: Option<usize>,

    /// The animation upon opening a window.
    open_animation: Option<OpenAnimation>,

    /// The animation of the window resizing.
    resize_animation: Option<ResizeAnimation>,

    /// The animation of a tile visually moving horizontally.
    move_x_animation: Option<MoveAnimation>,

    /// The animation of a tile visually moving vertically.
    move_y_animation: Option<MoveAnimation>,

    /// The animation of the tile's opacity.
    pub(super) alpha_animation: Option<AlphaAnimation>,

    /// Offset during the initial interactive move rubberband.
    pub(super) interactive_move_offset: Point<f64, Logical>,

    /// Snapshot of the last render for use in the close animation.
    unmap_snapshot: Option<TileRenderSnapshot>,

    /// Extra damage for clipped surface corner radius changes.
    rounded_corner_damage: RoundedCornerDamage,

    /// The view size for the tile's workspace.
    ///
    /// Used as the fullscreen target size.
    view_size: Size<f64, Logical>,

    /// Scale of the output the tile is on (and rounds its sizes to).
    scale: f64,

    /// Tab indicator for when this tile is grouped.
    tab_indicator: TabIndicator,

    /// HACK: Temporary size override, since after switching tabs, there will be a few frames of the new
    /// window still having to adjust, which causes a jerking visual without this compensation.
    window_size_override: WindowSizeOverride,

    /// This tile's blur settings.
    blur: Blur,

    /// Clock for driving animations.
    pub(super) clock: Clock,

    /// Configurable properties of the layout.
    pub(super) options: Rc<Options>,
}

niri_render_elements! {
    TileRenderElement<R> => {
        LayoutElement = LayoutElementRenderElement<R>,
        FocusRing = FocusRingRenderElement,
        SolidColor = SolidColorRenderElement,
        Opening = OpeningWindowRenderElement,
        Resize = ResizeRenderElement,
        Border = BorderRenderElement,
        Shadow = ShadowRenderElement,
        Blur = BlurRenderElement,
        BlurClippedSurface = ClippedSurfaceRenderElement<BlurRenderElement>,
        ClippedSurface = ClippedSurfaceRenderElement<WaylandSurfaceRenderElement<R>>,
        Offscreen = OffscreenRenderElement,
        ExtraDamage = ExtraDamage,
        TabIndicator = TabIndicatorRenderElement,
    }
}

pub type TileRenderSnapshot =
    RenderSnapshot<TileRenderElement<GlesRenderer>, TileRenderElement<GlesRenderer>>;

#[derive(Debug)]
struct ResizeAnimation {
    anim: Animation,
    size_from: Size<f64, Logical>,
    snapshot: LayoutElementRenderSnapshot,
    offscreen: OffscreenBuffer,
    tile_size_from: Size<f64, Logical>,
    // If the resize involved the fullscreen state at some point, this is the progress toward the
    // fullscreen state. Used for things like fullscreen backdrop alpha.
    //
    // Note that this can be set even if this specific resize is between two non-fullscreen states,
    // for example when issuing a new resize during an unfullscreen resize.
    fullscreen_progress: Option<Animation>,
    // Similar to above but for fullscreen-or-maximized.
    expanded_progress: Option<Animation>,
}

#[derive(Debug)]
struct MoveAnimation {
    anim: Animation,
    from: f64,
}

#[derive(Debug)]
pub(super) struct AlphaAnimation {
    pub(super) anim: Animation,
    /// Whether the animation should persist after it's done.
    ///
    /// This is used by things like interactive move which need to animate alpha to
    /// semitransparent, then hold it at semitransparent for a while, until the operation
    /// completes.
    pub(super) hold_after_done: bool,
    offscreen: OffscreenBuffer,
}

impl<W: LayoutElement> Tile<W> {
    pub fn new(
        window: W,
        view_size: Size<f64, Logical>,
        scale: f64,
        clock: Clock,
        options: Rc<Options>,
    ) -> Self {
        let rules = window.rules();
        let border_config = options.layout.border.merged_with(&rules.border);
        let focus_ring_config = options.layout.focus_ring.merged_with(&rules.focus_ring);
        let shadow_config = options.layout.shadow.merged_with(&rules.shadow);
        let sizing_mode = window.sizing_mode();
        let tab_indicator_config = options.layout.tab_indicator;

        // Blur needs to be enabled explicitly
        let mut blur_config = options.layout.blur;
        blur_config.on = false;
        blur_config.merge_with(&rules.blur);

        Self {
            window: WindowInner::Single(Some(window)),
            border: FocusRing::new(border_config.into()),
            focus_ring: FocusRing::new(focus_ring_config),
            shadow: Shadow::new(shadow_config),
            blur: Blur::new(blur_config),
            sizing_mode,
            fullscreen_backdrop: SolidColorBuffer::new((0., 0.), [0., 0., 0., 1.]),
            restore_to_floating: false,
            floating_window_size: None,
            floating_pos: None,
            floating_preset_width_idx: None,
            floating_preset_height_idx: None,
            open_animation: None,
            resize_animation: None,
            move_x_animation: None,
            move_y_animation: None,
            alpha_animation: None,
            interactive_move_offset: Point::from((0., 0.)),
            unmap_snapshot: None,
            rounded_corner_damage: Default::default(),
            view_size,
            scale,
            clock,
            options,
            window_size_override: WindowSizeOverride::new(),
            tab_indicator: TabIndicator::new(tab_indicator_config),
        }
    }

    pub fn update_config(
        &mut self,
        view_size: Size<f64, Logical>,
        scale: f64,
        options: Rc<Options>,
    ) {
        // If preset widths or heights changed, clear our stored preset index.
        if self.options.layout.preset_column_widths != options.layout.preset_column_widths {
            self.floating_preset_width_idx = None;
        }
        if self.options.layout.preset_window_heights != options.layout.preset_window_heights {
            self.floating_preset_height_idx = None;
        }

        self.view_size = view_size;
        self.scale = scale;
        self.options = options;

        let round_max1 = |logical| round_logical_in_physical_max1(self.scale, logical);

        let rules = self.window.focused_window().rules();

        let mut border_config = self.options.layout.border.merged_with(&rules.border);
        border_config.width = round_max1(border_config.width);
        self.border.update_config(border_config.into());

        let mut focus_ring_config = self
            .options
            .layout
            .focus_ring
            .merged_with(&rules.focus_ring);
        focus_ring_config.width = round_max1(focus_ring_config.width);
        self.focus_ring.update_config(focus_ring_config);

        let shadow_config = self.options.layout.shadow.merged_with(&rules.shadow);
        self.shadow.update_config(shadow_config);

        self.tab_indicator
            .update_config(self.options.layout.tab_indicator);

        // Blur needs to be enabled explicitly
        let mut blur_config = self.options.layout.blur;
        blur_config.on = false;
        blur_config.merge_with(&rules.blur);
        self.blur.update_config(blur_config);
    }

    pub fn update_shaders(&mut self) {
        self.border.update_shaders();
        self.focus_ring.update_shaders();
        self.shadow.update_shaders();
    }

    pub fn update_window(&mut self) {
        let prev_sizing_mode = self.sizing_mode;
        self.sizing_mode = self.window.focused_window().sizing_mode();

        if let Some(animate_from) = self.window.focused_window_mut().take_animation_snapshot() {
            let params = match self.resize_animation.take() {
                Some(resize) => {
                    // Compute like in animated_window_size(), but using the snapshot geometry (since
                    // the current one is already overwritten).
                    let mut size = animate_from.size;

                    let val = resize.anim.value();
                    let size_from = resize.size_from;
                    let tile_size_from = resize.tile_size_from;

                    size.w = (size.w - size_from.w).mul_add(val, size_from.w);
                    size.h = (size.h - size_from.h).mul_add(val, size_from.h);

                    let mut tile_size = animate_from.size;
                    if prev_sizing_mode.is_fullscreen() {
                        tile_size.w = f64::max(tile_size.w, self.view_size.w);
                        tile_size.h = f64::max(tile_size.h, self.view_size.h);
                    } else if prev_sizing_mode.is_normal() && !self.border.is_off() {
                        let width = self.border.width();
                        tile_size.w += width * 2.;
                        tile_size.h += width * 2.;
                    }

                    tile_size.w = (tile_size.w - tile_size_from.w).mul_add(val, tile_size_from.w);
                    tile_size.h = (tile_size.h - tile_size_from.h).mul_add(val, tile_size_from.h);

                    let fullscreen_from = resize
                        .fullscreen_progress
                        .map(|anim| anim.clamped_value().clamp(0., 1.))
                        .unwrap_or(if prev_sizing_mode.is_fullscreen() {
                            1.
                        } else {
                            0.
                        });

                    let expanded_from = resize
                        .expanded_progress
                        .map(|anim| anim.clamped_value().clamp(0., 1.))
                        .unwrap_or(if prev_sizing_mode.is_normal() { 0. } else { 1. });

                    // Also try to reuse the existing offscreen buffer if we have one.
                    (
                        size,
                        tile_size,
                        fullscreen_from,
                        expanded_from,
                        resize.offscreen,
                    )
                }
                _ => {
                    let size = animate_from.size;

                    // Compute like in tile_size().
                    let mut tile_size = size;
                    if prev_sizing_mode.is_fullscreen() {
                        tile_size.w = f64::max(tile_size.w, self.view_size.w);
                        tile_size.h = f64::max(tile_size.h, self.view_size.h);
                    } else if prev_sizing_mode.is_normal() && !self.border.is_off() {
                        let width = self.border.width();
                        tile_size.w += width * 2.;
                        tile_size.h += width * 2.;
                    }

                    let fullscreen_from = if prev_sizing_mode.is_fullscreen() {
                        1.
                    } else {
                        0.
                    };

                    let expanded_from = if prev_sizing_mode.is_normal() { 0. } else { 1. };

                    (
                        size,
                        tile_size,
                        fullscreen_from,
                        expanded_from,
                        OffscreenBuffer::default(),
                    )
                }
            };
            let (size_from, tile_size_from, fullscreen_from, expanded_from, offscreen) = params;

            let change =
                self.window.focused_window().size().to_f64().to_point() - size_from.to_point();
            let change = f64::max(change.x.abs(), change.y.abs());
            let tile_change = self.tile_size().to_f64().to_point() - tile_size_from.to_point();
            let tile_change = f64::max(tile_change.x.abs(), tile_change.y.abs());
            let change = f64::max(change, tile_change);
            if self.window_size_override.get().is_none() && change > RESIZE_ANIMATION_THRESHOLD {
                let anim = Animation::new(
                    self.clock.clone(),
                    0.,
                    1.,
                    0.,
                    self.options.animations.window_resize.anim,
                );

                let fullscreen_to = if self.sizing_mode.is_fullscreen() {
                    1.
                } else {
                    0.
                };
                let expanded_to = if self.sizing_mode.is_normal() { 0. } else { 1. };
                let fullscreen_progress = (fullscreen_from != fullscreen_to)
                    .then(|| anim.restarted(fullscreen_from, fullscreen_to, 0.));
                let expanded_progress = (expanded_from != expanded_to)
                    .then(|| anim.restarted(expanded_from, expanded_to, 0.));

                self.resize_animation = Some(ResizeAnimation {
                    anim,
                    size_from,
                    snapshot: animate_from,
                    offscreen,
                    tile_size_from,
                    fullscreen_progress,
                    expanded_progress,
                });
            } else {
                self.resize_animation = None;
            }
        }

        let round_max1 = |logical| round_logical_in_physical_max1(self.scale, logical);

        let rules = self.focused_window().rules().clone();
        let mut border_config = self.options.layout.border.merged_with(&rules.border);
        border_config.width = round_max1(border_config.width);
        self.border.update_config(border_config.into());

        let mut focus_ring_config = self
            .options
            .layout
            .focus_ring
            .merged_with(&rules.focus_ring);
        focus_ring_config.width = round_max1(focus_ring_config.width);
        self.focus_ring.update_config(focus_ring_config);

        let shadow_config = self.options.layout.shadow.merged_with(&rules.shadow);
        self.shadow.update_config(shadow_config);

        let window_size = self.window_size();
        let radius = rules
            .geometry_corner_radius
            .unwrap_or_default()
            .fit_to(window_size.w as f32, window_size.h as f32);
        self.rounded_corner_damage.set_corner_radius(radius);
        self.rounded_corner_damage.set_size(window_size);
    }

    pub fn advance_animations(&mut self) {
        if let Some(open) = &mut self.open_animation
            && open.is_done()
        {
            self.open_animation = None;
        }

        if let Some(resize) = &mut self.resize_animation
            && resize.anim.is_done()
        {
            self.resize_animation = None;
        }

        if let Some(move_) = &mut self.move_x_animation
            && move_.anim.is_done()
        {
            self.move_x_animation = None;
        }
        if let Some(move_) = &mut self.move_y_animation
            && move_.anim.is_done()
        {
            self.move_y_animation = None;
        }

        if let Some(alpha) = &mut self.alpha_animation
            && !alpha.hold_after_done
            && alpha.anim.is_done()
        {
            self.alpha_animation = None;
        }

        self.tab_indicator.advance_animations();
    }

    pub fn are_animations_ongoing(&self) -> bool {
        self.are_transitions_ongoing()
            || self.window.focused_window().rules().baba_is_float == Some(true)
    }

    pub fn are_transitions_ongoing(&self) -> bool {
        self.open_animation.is_some()
            || self.resize_animation.is_some()
            || self.move_x_animation.is_some()
            || self.move_y_animation.is_some()
            || self
                .alpha_animation
                .as_ref()
                .is_some_and(|alpha| !alpha.anim.is_done())
            || self.tab_indicator.are_animations_ongoing()
    }

    pub fn update_render_elements(&mut self, is_active: bool, view_rect: Rectangle<f64, Logical>) {
        let rules = self.window.focused_window().rules();
        let animated_tile_size = self.animated_tile_size();
        let expanded_progress = self.expanded_progress();

        let draw_border_with_background = rules
            .draw_border_with_background
            .unwrap_or_else(|| !self.window.focused_window().has_ssd());
        let border_width = self.visual_border_width().unwrap_or(0.);

        // Do the inverse of tile_size() in order to handle the unfullscreen animation for windows
        // that were smaller than the fullscreen size, and therefore their animated_window_size() is
        // currently much smaller than the tile size.
        let mut border_window_size = animated_tile_size;
        border_window_size.w -= border_width * 2.;
        border_window_size.h -= border_width * 2.;

        let radius = rules
            .geometry_corner_radius
            .map_or(CornerRadius::default(), |radius| {
                radius.expanded_by(border_width as f32)
            })
            .scaled_by(1. - expanded_progress as f32);
        self.border.update_render_elements(
            border_window_size,
            is_active,
            !draw_border_with_background,
            self.window.focused_window().is_urgent(),
            Rectangle::new(
                view_rect.loc - Point::from((border_width, border_width)),
                view_rect.size,
            ),
            radius,
            self.scale,
            1. - expanded_progress as f32,
        );

        let radius = if self.visual_border_width().is_some() {
            radius
        } else {
            rules
                .geometry_corner_radius
                .unwrap_or_default()
                .scaled_by(1. - expanded_progress as f32)
        };
        self.shadow.update_render_elements(
            animated_tile_size,
            is_active,
            radius,
            self.scale,
            1. - expanded_progress as f32,
        );

        let draw_focus_ring_with_background = if self.border.is_off() {
            draw_border_with_background
        } else {
            false
        };
        let radius = radius.expanded_by(self.focus_ring.width() as f32);
        self.focus_ring.update_render_elements(
            animated_tile_size,
            is_active,
            !draw_focus_ring_with_background,
            self.window.focused_window().is_urgent(),
            view_rect,
            radius,
            self.scale,
            1. - expanded_progress as f32,
        );

        self.fullscreen_backdrop.resize(animated_tile_size);

        self.blur
            .update_render_elements(self.focused_window().wants_blur());

        match &self.window {
            WindowInner::Single(_) => {
                self.tab_indicator.update_render_elements(
                    vec![],
                    false,
                    Rectangle::new(Point::default(), self.animated_bounding_box()),
                    view_rect,
                    is_active,
                    self.scale,
                );
            }
            WindowInner::Multiple { windows, focus_idx } => {
                let tabs = windows
                    .iter()
                    .enumerate()
                    .map(|(idx, w)| {
                        TabInfo::new(
                            w,
                            self.focus_ring().config(),
                            self.border().config(),
                            idx == *focus_idx,
                            w.is_urgent(),
                            &self.tab_indicator.config(),
                            self.animated_tile_size(),
                        )
                    })
                    .collect();

                self.tab_indicator.update_render_elements(
                    tabs,
                    true,
                    Rectangle::new(Point::default(), self.animated_bounding_box()),
                    view_rect,
                    is_active,
                    self.scale,
                );
            }
        }
    }

    pub const fn scale(&self) -> f64 {
        self.scale
    }

    pub fn render_offset(&self) -> Point<f64, Logical> {
        let mut offset = Point::from((0., 0.));

        if let Some(move_) = &self.move_x_animation {
            offset.x += move_.from * move_.anim.value();
        }
        if let Some(move_) = &self.move_y_animation {
            offset.y += move_.from * move_.anim.value();
        }

        offset += self.interactive_move_offset;

        offset
    }

    pub fn start_open_animation(&mut self) {
        self.open_animation = Some(OpenAnimation::new(Animation::new(
            self.clock.clone(),
            0.,
            1.,
            0.,
            self.options.animations.window_open.anim,
        )));
    }

    pub fn resize_animation(&self) -> Option<&Animation> {
        self.resize_animation.as_ref().map(|resize| &resize.anim)
    }

    pub fn animate_move_from(&mut self, from: Point<f64, Logical>) {
        self.animate_move_x_from(from.x);
        self.animate_move_y_from(from.y);
    }

    pub fn animate_move_x_from(&mut self, from: f64) {
        self.animate_move_x_from_with_config(from, self.options.animations.window_movement.0);
    }

    pub fn animate_move_x_from_with_config(&mut self, from: f64, config: niri_config::Animation) {
        let current_offset = self.render_offset().x;

        // Preserve the previous config if ongoing.
        let anim = self.move_x_animation.take().map(|move_| move_.anim);
        let anim = anim
            .map(|anim| anim.restarted(1., 0., 0.))
            .unwrap_or_else(|| Animation::new(self.clock.clone(), 1., 0., 0., config));

        self.move_x_animation = Some(MoveAnimation {
            anim,
            from: from + current_offset,
        });
    }

    pub fn animate_move_y_from(&mut self, from: f64) {
        self.animate_move_y_from_with_config(from, self.options.animations.window_movement.0);
    }

    pub fn animate_move_y_from_with_config(&mut self, from: f64, config: niri_config::Animation) {
        let current_offset = self.render_offset().y;

        // Preserve the previous config if ongoing.
        let anim = self.move_y_animation.take().map(|move_| move_.anim);
        let anim = anim
            .map(|anim| anim.restarted(1., 0., 0.))
            .unwrap_or_else(|| Animation::new(self.clock.clone(), 1., 0., 0., config));

        self.move_y_animation = Some(MoveAnimation {
            anim,
            from: from + current_offset,
        });
    }

    pub fn offset_move_y_anim_current(&mut self, offset: f64) {
        if let Some(move_) = self.move_y_animation.as_mut() {
            // If the anim is almost done, there's little point trying to offset it; we can let
            // things jump. If it turns out like a bad idea, we could restart the anim instead.
            let value = move_.anim.value();
            if value > 0.001 {
                move_.from += offset / value;
            }
        }
    }

    pub fn stop_move_animations(&mut self) {
        self.move_x_animation = None;
        self.move_y_animation = None;
    }

    pub fn animate_alpha(&mut self, from: f64, to: f64, config: niri_config::Animation) {
        let from = from.clamp(0., 1.);
        let to = to.clamp(0., 1.);

        let (current, offscreen) = match self.alpha_animation.take() {
            Some(alpha) => (alpha.anim.clamped_value(), alpha.offscreen),
            _ => (from, OffscreenBuffer::default()),
        };

        self.alpha_animation = Some(AlphaAnimation {
            anim: Animation::new(self.clock.clone(), current, to, 0., config),
            hold_after_done: false,
            offscreen,
        });
    }

    pub fn ensure_alpha_animates_to_1(&mut self) {
        if let Some(alpha) = &self.alpha_animation
            && alpha.anim.to() != 1.
        {
            // Cancel animation instead of starting a new one because the user likely wants to
            // see the tile right away.
            self.alpha_animation = None;
        }
    }

    pub const fn hold_alpha_animation_after_done(&mut self) {
        if let Some(alpha) = &mut self.alpha_animation {
            alpha.hold_after_done = true;
        }
    }

    pub const fn is_grouped_tile(&self) -> bool {
        matches!(
            self.window,
            WindowInner::Multiple {
                windows: _,
                focus_idx: _
            }
        )
    }

    pub fn remove_window(&mut self, id: &W::Id) -> usize {
        let current_size = self.focused_window().size();
        let current_mode = self.focused_window().sizing_mode();
        self.window_size_override.set(current_size);

        let out = self.window.remove_window(id);

        self.focused_window_mut()
            .request_size(current_size, current_mode, false, None);

        out
    }

    pub fn add_window(&mut self, window: W) {
        self.window.add_window(window);
    }

    pub fn take_window(self) -> W {
        match self.window {
            WindowInner::Single(w) => w.expect("should have a single window"),
            WindowInner::Multiple {
                mut windows,
                focus_idx,
            } => windows.remove(focus_idx),
        }
    }

    pub fn focus_next(&mut self) -> &W {
        let current_size = self.focused_window().size();
        let current_mode = self.focused_window().sizing_mode();
        self.window_size_override.set(current_size);
        self.window.focus_next();

        let new_window = self.focused_window_mut();

        new_window.request_size(current_size, current_mode, false, None);

        new_window
    }

    pub fn focus_prev(&mut self) -> &W {
        let current_size = self.focused_window().size();
        let current_mode = self.focused_window().sizing_mode();
        self.window_size_override.set(current_size);
        self.window.focus_prev();

        let new_window = self.focused_window_mut();

        new_window.request_size(current_size, current_mode, false, None);

        new_window
    }

    pub fn ungroup_single(&mut self, id: &W::Id) -> Option<W> {
        let current_size = self.focused_window().size();
        let current_mode = self.focused_window().sizing_mode();
        let extra_size = self.tab_indicator_extra_size();

        let out = self.window.ungroup_single(id);

        if matches!(&self.window, WindowInner::Single(_)) && extra_size.h > 0. {
            self.animate_move_from(self.tab_indicator_content_offset());
        }

        self.focused_window_mut()
            .request_size(current_size, current_mode, false, None);

        out
    }

    pub fn group(&mut self) {
        let will_group = matches!(&self.window, WindowInner::Single(_));
        self.window.group();

        if will_group {
            self.animate_move_from(Point::new(0., 0.) - self.tab_indicator_content_offset());
        }
    }

    pub fn start_tab_indicator_open_animation(&mut self) {
        self.tab_indicator.start_open_animation(
            self.clock.clone(),
            self.options.animations.window_movement.0,
        );
    }

    pub fn tab_indicator_content_offset(&self) -> Point<f64, Logical> {
        if self.focused_window().sizing_mode() != SizingMode::Normal {
            return Point::new(0., 0.);
        }

        match &self.window {
            WindowInner::Single(_) => Point::default(),
            WindowInner::Multiple {
                windows,
                focus_idx: _,
            } => self.tab_indicator.content_offset(windows.len(), self.scale),
        }
    }

    pub fn tab_indicator_extra_size(&self) -> Size<f64, Logical> {
        if self.focused_window().sizing_mode() != SizingMode::Normal {
            return Size::new(0., 0.);
        }

        match &self.window {
            WindowInner::Single(_) => Size::default(),
            WindowInner::Multiple {
                windows,
                focus_idx: _,
            } => self.tab_indicator.extra_size(windows.len(), self.scale),
        }
    }

    pub fn ungroup_all(&mut self) -> Vec<Self> {
        let extra_size = self.tab_indicator_extra_size();

        if extra_size.h > 0. {
            self.animate_move_from(self.tab_indicator_content_offset());
        }

        self.window.ungroup_all(
            self.view_size,
            self.scale,
            self.clock.clone(),
            self.options.clone(),
        )
    }

    pub fn has_window(&self, id: &W::Id) -> bool {
        match &self.window {
            WindowInner::Single(w) => w.as_ref().unwrap().id() == id,
            WindowInner::Multiple {
                windows,
                focus_idx: _,
            } => windows.iter().any(|w| w.id() == id),
        }
    }

    pub fn focused_window(&self) -> &W {
        self.window.focused_window()
    }

    pub fn focused_window_mut(&mut self) -> &mut W {
        self.window.focused_window_mut()
    }

    pub fn focus_window(&mut self, id: &W::Id) -> bool {
        if self.focused_window().id() == id {
            return true;
        }

        let current_size = self.focused_window().size();
        let current_mode = self.focused_window().sizing_mode();

        if self.window.focus_window(id) {
            self.window_size_override.set(current_size);

            let new_window = self.focused_window_mut();

            new_window.request_size(current_size, current_mode, false, None);

            true
        } else {
            false
        }
    }

    pub fn windows(&self) -> impl Iterator<Item = &W> {
        self.window.iter()
    }

    pub fn windows_mut(&mut self) -> impl Iterator<Item = &mut W> {
        self.window.iter_mut()
    }

    pub const fn sizing_mode(&self) -> SizingMode {
        self.sizing_mode
    }

    fn fullscreen_progress(&self) -> f64 {
        if let Some(resize) = &self.resize_animation
            && let Some(anim) = &resize.fullscreen_progress
        {
            return anim.clamped_value().clamp(0., 1.);
        }

        if self.sizing_mode.is_fullscreen() {
            1.
        } else {
            0.
        }
    }

    fn expanded_progress(&self) -> f64 {
        if let Some(resize) = &self.resize_animation
            && let Some(anim) = &resize.expanded_progress
        {
            return anim.clamped_value().clamp(0., 1.);
        }

        if self.sizing_mode.is_normal() { 0. } else { 1. }
    }

    /// Returns `None` if the border is hidden and `Some(width)` if it should be shown.
    pub const fn effective_border_width(&self) -> Option<f64> {
        if !self.sizing_mode.is_normal() {
            return None;
        }

        if self.border.is_off() {
            return None;
        }

        Some(self.border.width())
    }

    fn visual_border_width(&self) -> Option<f64> {
        if self.border.is_off() {
            return None;
        }

        let expanded_progress = self.expanded_progress();

        // Only hide the border when fully expanded to avoid jarring border appearance.
        if expanded_progress == 1. {
            return None;
        }

        // FIXME: would be cool to, like, gradually resize the border from full width to 0 during
        // fullscreening, but the rest of the code isn't quite ready for that yet. It needs to
        // handle things like computing intermediate tile size when an animated resize starts during
        // an animated unfullscreen resize.
        Some(self.border.width())
    }

    /// Returns the location of the window's visual geometry within this Tile.
    pub fn window_loc(&self) -> Point<f64, Logical> {
        let mut loc = Point::from((0., 0.));

        let window_size = self.animated_window_size();
        let target_size = self.animated_tile_size();

        // Center the window within its tile.
        //
        // - Without borders, the sizes match, so this difference is zero.
        // - Borders always match from all sides, so this difference is pre-rounded to physical.
        // - In fullscreen, if the window is smaller than the tile, then it gets centered, otherwise
        //   the tile size matches the window.
        // - During animations, the window remains centered within the tile; this is important for
        //   the to/from fullscreen animation.
        loc.x += (target_size.w - window_size.w) / 2.;
        loc.y += (target_size.h - window_size.h) / 2.;

        // Round to physical pixels.
        loc = loc
            .to_physical_precise_round(self.scale)
            .to_logical(self.scale);

        loc += self.tab_indicator_content_offset();

        loc
    }

    pub fn tile_size(&self) -> Size<f64, Logical> {
        let mut size = self.window_size();

        if self.sizing_mode.is_fullscreen() {
            // Normally we'd just return the fullscreen size here, but this makes things a bit
            // nicer if a fullscreen window is bigger than the fullscreen size for some reason.
            size.w = f64::max(size.w, self.view_size.w);
            size.h = f64::max(size.h, self.view_size.h);
            return size;
        }

        if let Some(width) = self.effective_border_width() {
            size.w += width * 2.;
            size.h += width * 2.;
        }

        size
    }

    pub fn tile_bounding_box(&self) -> Size<f64, Logical> {
        self.tile_size() + self.tab_indicator_extra_size()
    }

    pub fn animated_bounding_box(&self) -> Size<f64, Logical> {
        self.animated_tile_size() + self.tab_indicator_extra_size()
    }

    pub fn tile_expected_or_current_size(&self) -> Size<f64, Logical> {
        let mut size = self.window_expected_or_current_size();

        if self.sizing_mode.is_fullscreen() {
            // Normally we'd just return the fullscreen size here, but this makes things a bit
            // nicer if a fullscreen window is bigger than the fullscreen size for some reason.
            size.w = f64::max(size.w, self.view_size.w);
            size.h = f64::max(size.h, self.view_size.h);
            return size;
        }

        if let Some(width) = self.effective_border_width() {
            size.w += width * 2.;
            size.h += width * 2.;
        }

        size
    }

    pub fn window_size(&self) -> Size<f64, Logical> {
        let mut size = self
            .window_size_override
            .get()
            .unwrap_or_else(|| self.window.focused_window().size())
            .to_f64();
        size = size
            .to_physical_precise_round(self.scale)
            .to_logical(self.scale);

        size
    }

    pub fn window_expected_or_current_size(&self) -> Size<f64, Logical> {
        let size = self.window.focused_window().expected_size();
        let mut size = size
            .unwrap_or_else(|| self.window.focused_window().size())
            .to_f64();
        size = size
            .to_physical_precise_round(self.scale)
            .to_logical(self.scale);
        size
    }

    pub fn animated_window_size(&self) -> Size<f64, Logical> {
        let mut size = self.window_size();

        if let Some(resize) = &self.resize_animation {
            let val = resize.anim.value();
            let size_from = resize.size_from.to_f64();

            size.w = f64::max(1., (size.w - size_from.w).mul_add(val, size_from.w));
            size.h = f64::max(1., (size.h - size_from.h).mul_add(val, size_from.h));
            size = size
                .to_physical_precise_round(self.scale)
                .to_logical(self.scale);
        }

        size
    }

    pub fn animated_tile_size(&self) -> Size<f64, Logical> {
        let mut size = self.tile_size();

        if let Some(resize) = &self.resize_animation {
            let val = resize.anim.value();
            let size_from = resize.tile_size_from.to_f64();

            size.w = f64::max(1., (size.w - size_from.w).mul_add(val, size_from.w));
            size.h = f64::max(1., (size.h - size_from.h).mul_add(val, size_from.h));
            size = size
                .to_physical_precise_round(self.scale)
                .to_logical(self.scale);
        }

        size
    }

    pub fn buf_loc(&self) -> Point<f64, Logical> {
        let mut loc = Point::from((0., 0.));
        loc += self.window_loc();
        loc += self.window.focused_window().buf_loc().to_f64();
        loc
    }

    /// Returns a partially-filled [`WindowLayout`].
    ///
    /// Only the sizing properties that a [`Tile`] can fill are filled.
    pub fn ipc_layout_template(&self) -> WindowLayout {
        WindowLayout {
            pos_in_scrolling_layout: None,
            tile_size: self.tile_size().into(),
            window_size: self.window.focused_window().size().into(),
            tile_pos_in_workspace_view: None,
            window_offset_in_tile: self.window_loc().into(),
        }
    }

    fn is_in_input_region(&self, mut point: Point<f64, Logical>) -> bool {
        point -= self.window_loc().to_f64();
        self.window.focused_window().is_in_input_region(point)
    }

    fn is_in_activation_region(&self, point: Point<f64, Logical>) -> bool {
        let activation_region = Rectangle::from_size(self.tile_bounding_box());
        activation_region.contains(point)
    }

    pub fn hit(&self, point: Point<f64, Logical>) -> Option<(Option<&W>, HitType)> {
        let offset = self.bob_offset();
        let point = point - offset;

        if self.is_in_input_region(point) {
            let win_pos = self.buf_loc() + offset;
            Some((Some(self.focused_window()), HitType::Input { win_pos }))
        } else if self.is_in_activation_region(point) {
            if let WindowInner::Multiple {
                windows,
                focus_idx: _,
            } = &self.window
                && let Some(hit_idx) = self.tab_indicator.hit(
                    Rectangle::from_size(self.tile_bounding_box()),
                    windows.len(),
                    self.scale,
                    point,
                )
            {
                return Some((
                    windows.get(hit_idx),
                    HitType::Activate {
                        is_tab_indicator: true,
                    },
                ));
            }

            Some((
                None,
                HitType::Activate {
                    is_tab_indicator: false,
                },
            ))
        } else {
            None
        }
    }

    pub fn request_tile_size(
        &mut self,
        mut size: Size<f64, Logical>,
        animate: bool,
        transaction: Option<Transaction>,
    ) {
        // Can't go through effective_border_width() because we might be fullscreen.
        if !self.border.is_off() {
            let width = self.border.width();
            size.w = f64::max(1., width.mul_add(-2., size.w));
            size.h = f64::max(1., width.mul_add(-2., size.h));
        }

        size -= self.tab_indicator_extra_size();

        // The size request has to be i32 unfortunately, due to Wayland. We floor here instead of
        // round to avoid situations where proportionally-sized columns don't fit on the screen
        // exactly.
        self.focused_window_mut().request_size(
            size.to_i32_floor(),
            SizingMode::Normal,
            animate,
            transaction,
        );
    }

    pub fn tile_width_for_window_width(&self, size: f64) -> f64 {
        (if self.border.is_off() {
            size
        } else {
            self.border.width().mul_add(2., size)
        }) + self.tab_indicator_extra_size().w
    }

    pub fn tile_height_for_window_height(&self, size: f64) -> f64 {
        (if self.border.is_off() {
            size
        } else {
            self.border.width().mul_add(2., size)
        }) + self.tab_indicator_extra_size().h
    }

    pub fn window_width_for_tile_width(&self, size: f64) -> f64 {
        (if self.border.is_off() {
            size
        } else {
            self.border.width().mul_add(-2., size)
        }) - self.tab_indicator_extra_size().w
    }

    pub fn window_height_for_tile_height(&self, size: f64) -> f64 {
        (if self.border.is_off() {
            size
        } else {
            self.border.width().mul_add(-2., size)
        }) - self.tab_indicator_extra_size().h
    }

    pub fn request_maximized(
        &mut self,
        size: Size<f64, Logical>,
        animate: bool,
        transaction: Option<Transaction>,
    ) {
        let desired_size = size - self.tab_indicator_extra_size();

        self.focused_window_mut().request_size(
            desired_size.to_i32_round(),
            SizingMode::Maximized,
            animate,
            transaction,
        );
    }

    pub fn request_fullscreen(&mut self, animate: bool, transaction: Option<Transaction>) {
        let desired_size = self.view_size - self.tab_indicator_extra_size();

        self.focused_window_mut().request_size(
            desired_size.to_i32_round(),
            SizingMode::Fullscreen,
            animate,
            transaction,
        );
    }

    pub fn min_size_nonfullscreen(&self) -> Size<f64, Logical> {
        let mut size = self.window.focused_window().min_size().to_f64();

        // Can't go through effective_border_width() because we might be fullscreen.
        if !self.border.is_off() {
            let width = self.border.width();

            size.w = f64::max(1., size.w);
            size.h = f64::max(1., size.h);

            size.w += width * 2.;
            size.h += width * 2.;
        }

        size
    }

    pub fn max_size_nonfullscreen(&self) -> Size<f64, Logical> {
        let mut size = self.window.focused_window().max_size().to_f64();

        // Can't go through effective_border_width() because we might be fullscreen.
        if !self.border.is_off() {
            let width = self.border.width();

            if size.w > 0. {
                size.w += width * 2.;
            }
            if size.h > 0. {
                size.h += width * 2.;
            }
        }

        size
    }

    pub fn bob_offset(&self) -> Point<f64, Logical> {
        if self.window.focused_window().rules().baba_is_float != Some(true) {
            return Point::from((0., 0.));
        }

        let y = baba_is_float_offset(self.clock.now(), self.view_size.h);
        let y = round_logical_in_physical(self.scale, y);
        Point::from((0., y))
    }

    #[allow(clippy::too_many_arguments)]
    fn render_inner<'a, R: NiriRenderer + 'a>(
        &'a self,
        renderer: &mut R,
        location: Point<f64, Logical>,
        real_location: Point<f64, Logical>,
        focus_ring: bool,
        target: RenderTarget,
        fx_buffers: Option<EffectsFramebuffersUserData>,
        overview_zoom: Option<f64>,
    ) -> impl Iterator<Item = TileRenderElement<R>> + 'a + use<'a, R, W> {
        let _span = tracy_client::span!("Tile::render_inner");

        let scale = Scale::from(self.scale);
        let fullscreen_progress = self.fullscreen_progress();
        let expanded_progress = self.expanded_progress();

        let win_alpha = if self
            .window
            .focused_window()
            .is_ignoring_opacity_window_rule()
        {
            1.
        } else {
            let alpha = self
                .window
                .focused_window()
                .rules()
                .opacity
                .unwrap_or(1.)
                .clamp(0., 1.);

            // Interpolate towards alpha = 1. at fullscreen.
            let p = fullscreen_progress as f32;
            alpha * (1. - p) + 1. * p
        };

        let tab_indicator_offset = self.tab_indicator_content_offset();

        let tab_indicator_loc = location + self.bob_offset();

        // This is here rather than in render_offset() because render_offset() is currently assumed
        // by the code to be temporary. So, for example, interactive move will try to "grab" the
        // tile at its current render offset and reset the render offset to zero by cancelling the
        // tile move animations. On the other hand, bob_offset() is not resettable, so adding it in
        // render_offset() would cause obvious animation glitches.
        //
        // This isn't to say that adding it here is perfect; indeed, it kind of breaks view_rect
        // passed to update_render_elements(). But, it works well enough for what it is.
        let location = location + self.bob_offset() + tab_indicator_offset;

        // `window_loc` already has the tab indicator offset applied to it for things like hit
        // detection, so here we need to undo it for the rest of the render function to make sense
        let window_loc = self.window_loc() - tab_indicator_offset;

        let window_size = self.window_size().to_f64();
        let animated_window_size = self.animated_window_size();
        let window_render_loc = location + window_loc;
        let area = Rectangle::new(window_render_loc, animated_window_size);

        // In certain passes, `location` is set to (0, 0), e.g. to "anchor" the tile to the cursor
        // during interactive move. However, this messes with our blur texture, so we need the
        // _actual_ render location to know where to sample the blur from.
        let blur_sample_area = Rectangle::new(
            real_location + window_loc + tab_indicator_offset,
            animated_window_size,
        );

        self.window_size_override.increment();

        let rules = self.window.focused_window().rules();

        // Clip to geometry including during the fullscreen animation to help with buggy clients
        // that submit a full-sized buffer before acking the fullscreen state (Firefox).
        let clip_to_geometry = fullscreen_progress < 1. && rules.clip_to_geometry == Some(true);
        let radius = rules
            .geometry_corner_radius
            .unwrap_or_default()
            .scaled_by(1. - expanded_progress as f32);

        // If we're resizing, try to render a shader, or a fallback.
        let mut resize_shader = None;
        let mut resize_popups = None;
        let mut resize_fallback = None;

        if let Some(resize) = &self.resize_animation {
            resize_popups = Some(
                self.window
                    .focused_window()
                    .render_popups(renderer, window_render_loc, scale, win_alpha, target)
                    .into_iter()
                    .map(Into::into),
            );

            if ResizeRenderElement::has_shader(renderer) {
                let gles_renderer = renderer.as_gles_renderer();

                if let Some(texture_from) = resize.snapshot.texture(gles_renderer, scale, target) {
                    let window_elements = self.window.focused_window().render_normal(
                        gles_renderer,
                        Point::from((0., 0.)),
                        scale,
                        1.,
                        target,
                    );

                    let current = resize
                        .offscreen
                        .render(gles_renderer, scale, &window_elements)
                        .map_err(|err| warn!("error rendering window to texture: {err:?}"))
                        .ok();

                    // Clip blocked-out resizes unconditionally because they use solid color render
                    // elements.
                    let clip_to_geometry = if target
                        .should_block_out(resize.snapshot.block_out_from)
                        && target.should_block_out(rules.block_out_from)
                    {
                        true
                    } else {
                        clip_to_geometry
                    };

                    if let Some((elem_current, _sync_point, mut data)) = current {
                        let texture_current = elem_current.texture().clone();
                        // The offset and size are computed in physical pixels and converted to
                        // logical with the same `scale`, so converting them back with rounding
                        // inside the geometry() call gives us the same physical result back.
                        let texture_current_geo = elem_current.geometry(scale);

                        let elem = ResizeRenderElement::new(
                            area,
                            scale,
                            texture_from.clone(),
                            resize.snapshot.size,
                            (texture_current, texture_current_geo),
                            window_size,
                            resize.anim.value() as f32,
                            resize.anim.clamped_value().clamp(0., 1.) as f32,
                            radius,
                            clip_to_geometry,
                            win_alpha,
                        );

                        // We're drawing the resize shader, not the offscreen directly.
                        data.id = elem.id().clone();

                        // This is not a problem for split popups as the code will look for them by
                        // original id when it doesn't find them on the offscreen.
                        self.window.focused_window().set_offscreen_data(Some(data));
                        resize_shader = Some(elem.into());
                    }
                }
            }

            if resize_shader.is_none() {
                let fallback_buffer = SolidColorBuffer::new(area.size, [1., 0., 0., 1.]);
                resize_fallback = Some(
                    SolidColorRenderElement::from_buffer(
                        &fallback_buffer,
                        area.loc,
                        win_alpha,
                        Kind::Unspecified,
                    )
                    .into(),
                );
            }
        }

        // If we're not resizing, render the window itself.
        let mut window_surface = None;
        let mut window_popups = None;
        let mut rounded_corner_damage = None;
        let has_border_shader = BorderRenderElement::has_shader(renderer);
        let geo = Rectangle::new(window_render_loc, window_size);
        let animated_geo = Rectangle::new(window_render_loc, animated_window_size);
        let clip_shader = clip_to_geometry
            .then(|| Shaders::get(renderer).clipped_surface.clone())
            .flatten();
        if resize_shader.is_none() && resize_fallback.is_none() {
            let window = self.window.focused_window().render(
                renderer,
                window_render_loc,
                scale,
                win_alpha,
                target,
            );

            let radius = radius.fit_to(window_size.w as f32, window_size.h as f32);

            if clip_to_geometry && clip_shader.is_some() {
                let damage = self.rounded_corner_damage.element();
                rounded_corner_damage = Some(damage.with_location(window_render_loc).into());
            }

            let clip_shader = clip_shader;

            window_surface = Some(window.normal.into_iter().map(move |elem| match elem {
                LayoutElementRenderElement::Wayland(elem) => {
                    // If we should clip to geometry, render a clipped window.
                    if clip_to_geometry
                        && let Some(shader) = clip_shader.clone()
                        && ClippedSurfaceRenderElement::will_clip(&elem, scale, geo, radius)
                    {
                        let view_src = elem.view().src;
                        let buf_size = elem.buffer_size();
                        return ClippedSurfaceRenderElement::new(
                            elem, view_src, buf_size, scale, geo, shader, radius, None, 0.,
                        )
                        .into();
                    }

                    // Otherwise, render it normally.
                    LayoutElementRenderElement::Wayland(elem).into()
                }
                LayoutElementRenderElement::SolidColor(elem) => {
                    // In this branch we're rendering a blocked-out window with a solid
                    // color. We need to render it with a rounded corner shader even if
                    // clip_to_geometry is false, because in this case we're assuming that
                    // the unclipped window CSD already has corners rounded to the
                    // user-provided radius, so our blocked-out rendering should match that
                    // radius.
                    if radius != CornerRadius::default() && has_border_shader {
                        return BorderRenderElement::new(
                            geo.size,
                            Rectangle::from_size(geo.size),
                            GradientInterpolation::default(),
                            Color::from_color32f(elem.color()),
                            Color::from_color32f(elem.color()),
                            0.,
                            Rectangle::from_size(geo.size),
                            0.,
                            radius,
                            scale.x as f32,
                            1.,
                        )
                        .with_location(geo.loc)
                        .into();
                    }

                    // Otherwise, render the solid color as is.
                    LayoutElementRenderElement::SolidColor(elem).into()
                }
            }));

            window_popups = Some(window.popups.into_iter().map(Into::into));
        }

        let rv = resize_popups
            .into_iter()
            .flatten()
            .chain(resize_shader)
            .chain(resize_fallback)
            .chain(window_popups.into_iter().flatten())
            .chain(rounded_corner_damage)
            .chain(window_surface.into_iter().flatten());

        let elem = if self.focused_window().sizing_mode() == SizingMode::Normal {
            self.tab_indicator
                .render(renderer, tab_indicator_loc)
                .map(Into::into)
                .into()
        } else {
            None
        };

        let rv = rv.chain(elem.into_iter().flatten());

        let elem = (fullscreen_progress > 0.).then(|| {
            let alpha = fullscreen_progress as f32;

            // During the un/fullscreen animation, render a border element in order to use the
            // animated corner radius.
            if fullscreen_progress < 1. && has_border_shader {
                let border_width = self.visual_border_width().unwrap_or(0.);
                let radius = rules
                    .geometry_corner_radius
                    .map_or(CornerRadius::default(), |radius| {
                        radius.expanded_by(border_width as f32)
                    })
                    .scaled_by(1. - expanded_progress as f32);

                let size = self.fullscreen_backdrop.size();
                let color = self.fullscreen_backdrop.color();
                BorderRenderElement::new(
                    size,
                    Rectangle::from_size(size),
                    GradientInterpolation::default(),
                    Color::from_color32f(color),
                    Color::from_color32f(color),
                    0.,
                    Rectangle::from_size(size),
                    0.,
                    radius,
                    scale.x as f32,
                    alpha,
                )
                .with_location(location)
                .into()
            } else {
                SolidColorRenderElement::from_buffer(
                    &self.fullscreen_backdrop,
                    location,
                    alpha,
                    Kind::Unspecified,
                )
                .into()
            }
        });
        let rv = rv.chain(elem);

        let elem = self.visual_border_width().map(|width| {
            self.border
                .render(renderer, location + Point::from((width, width)))
                .map(Into::into)
        });
        let rv = rv.chain(elem.into_iter().flatten());

        // Hide the focus ring when maximized/fullscreened. It's not normally visible anyway due to
        // being outside the monitor or obscured by a solid colored bar, but it is visible under
        // semitransparent bars in maximized state (which is a bit weird) and in the overview (also
        // a bit weird).
        let elem = (focus_ring && expanded_progress < 1.)
            .then(|| self.focus_ring.render(renderer, location).map(Into::into));
        let rv = rv.chain(elem.into_iter().flatten());

        let blur_elem = fx_buffers
            .and_then(|fx_buffers| {
                self.blur
                    .render(
                        renderer.as_gles_renderer(),
                        fx_buffers,
                        blur_sample_area.to_i32_round(),
                        radius,
                        self.scale,
                        animated_geo,
                        self.focused_window().is_floating()
                            && !self.focused_window().rules().blur.x_ray.unwrap_or_default(),
                        window_render_loc,
                        overview_zoom,
                        1.,
                    )
                    .map(Into::into)
            })
            .into_iter();

        let elem = (expanded_progress < 1.)
            .then(|| self.shadow.render(renderer, location).map(Into::into));
        let rv = rv.chain(elem.into_iter().flatten());

        rv.chain(blur_elem)
    }

    pub fn render<'a, R: NiriRenderer + 'a>(
        &'a self,
        renderer: &mut R,
        location: Point<f64, Logical>,
        focus_ring: bool,
        target: RenderTarget,
        fx_buffers: Option<EffectsFramebuffersUserData>,
        overview_zoom: Option<f64>,
    ) -> impl Iterator<Item = TileRenderElement<R>> + 'a + use<'a, R, W> {
        let _span = tracy_client::span!("Tile::render");

        let scale = Scale::from(self.scale);

        let tile_alpha = self
            .alpha_animation
            .as_ref()
            .map_or(1., |alpha| alpha.anim.clamped_value()) as f32;

        let mut open_anim_elem = None;
        let mut alpha_anim_elem = None;
        let mut window_elems = None;

        self.window.focused_window().set_offscreen_data(None);

        if let Some(open) = &self.open_animation {
            let renderer = renderer.as_gles_renderer();
            let elements = self.render_inner(
                renderer,
                Point::from((0., 0.)),
                location,
                focus_ring,
                target,
                fx_buffers.clone(),
                overview_zoom,
            );
            let elements = elements.collect::<Vec<TileRenderElement<_>>>();
            match open.render(
                renderer,
                &elements,
                self.animated_bounding_box(),
                location,
                scale,
                tile_alpha,
            ) {
                Ok((elem, data)) => {
                    self.window.focused_window().set_offscreen_data(Some(data));
                    open_anim_elem = Some(elem.into());
                }
                Err(err) => {
                    warn!("error rendering window opening animation: {err:?}");
                }
            }
        } else if let Some(alpha) = &self.alpha_animation {
            let renderer = renderer.as_gles_renderer();
            let elements = self.render_inner(
                renderer,
                Point::from((0., 0.)),
                location,
                focus_ring,
                target,
                fx_buffers.clone(),
                overview_zoom,
            );
            let elements = elements.collect::<Vec<TileRenderElement<_>>>();
            match alpha.offscreen.render(renderer, scale, &elements) {
                Ok((elem, _sync, data)) => {
                    let offset = elem.offset();
                    let elem = elem.with_alpha(tile_alpha).with_offset(location + offset);

                    self.window.focused_window().set_offscreen_data(Some(data));
                    alpha_anim_elem = Some(elem.into());
                }
                Err(err) => {
                    warn!("error rendering tile to offscreen for alpha animation: {err:?}");
                }
            }
        }

        if open_anim_elem.is_none() && alpha_anim_elem.is_none() {
            window_elems = Some(self.render_inner(
                renderer,
                location,
                location,
                focus_ring,
                target,
                fx_buffers,
                overview_zoom,
            ));
        }

        open_anim_elem
            .into_iter()
            .chain(alpha_anim_elem)
            .chain(window_elems.into_iter().flatten())
    }

    pub fn store_unmap_snapshot_if_empty(&mut self, renderer: &mut GlesRenderer) {
        if self.unmap_snapshot.is_some() {
            return;
        }

        self.unmap_snapshot = Some(self.render_snapshot(renderer));
    }

    fn render_snapshot(&self, renderer: &mut GlesRenderer) -> TileRenderSnapshot {
        let _span = tracy_client::span!("Tile::render_snapshot");

        let contents = self.render(
            renderer,
            Point::from((0., 0.)),
            false,
            RenderTarget::Output,
            None,
            None,
        );

        // A bit of a hack to render blocked out as for screencast, but I think it's fine here.
        let blocked_out_contents = self.render(
            renderer,
            Point::from((0., 0.)),
            false,
            RenderTarget::Screencast,
            None,
            None,
        );

        RenderSnapshot {
            contents: contents.collect(),
            blocked_out_contents: blocked_out_contents.collect(),
            block_out_from: self.window.focused_window().rules().block_out_from,
            size: self.animated_bounding_box(),
            texture: Default::default(),
            blocked_out_texture: Default::default(),
        }
    }

    pub const fn take_unmap_snapshot(&mut self) -> Option<TileRenderSnapshot> {
        self.unmap_snapshot.take()
    }

    pub const fn border(&self) -> &FocusRing {
        &self.border
    }

    pub const fn focus_ring(&self) -> &FocusRing {
        &self.focus_ring
    }

    pub const fn options(&self) -> &Rc<Options> {
        &self.options
    }

    #[cfg(test)]
    pub fn view_size(&self) -> Size<f64, Logical> {
        self.view_size
    }

    #[cfg(test)]
    pub fn verify_invariants(&self) {
        use approx::assert_abs_diff_eq;

        assert_eq!(self.sizing_mode, self.window.focused_window().sizing_mode());

        let scale = self.scale;
        let size = self.tile_size();
        let rounded = size.to_physical_precise_round(scale).to_logical(scale);
        assert_abs_diff_eq!(size.w, rounded.w, epsilon = 1e-5);
        assert_abs_diff_eq!(size.h, rounded.h, epsilon = 1e-5);
    }
}
