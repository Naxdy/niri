use crate::render_helpers::renderer::NiriRenderer;

/// Helper trait implemented by structs that can render something into a [`NiriRenderer`].
///
/// This trait may be implemented multiple times, e.g. to separate out the rendering of surfaces
/// and popups.
pub trait Render<'a, R>
where
    R: NiriRenderer,
{
    /// Additional context required to render this struct's elements.
    ///
    /// For most simple elements, this is often a [`Point`](smithay::utils::Point) representing the
    /// desired render location.
    type RenderContext: 'a;

    /// The concrete type of the render element that will be pushed into the collector.
    type RenderElement;

    /// Render all elements and push them into the `collector`.
    ///
    /// A collector can be anything that implements the [`PushRenderElement`] trait.
    fn render<C>(&'a self, renderer: &mut R, context: Self::RenderContext, collector: &mut C)
    where
        C: PushRenderElement<Self::RenderElement, R>;
}

/// Helper trait designed to accept render elements being pushed into it.
///
/// By default, this is implemented for [`Vec`] and any [`FnMut`] that takes in a type `T` that
/// implements [`Into<E>`](std::convert::Into).
///
/// The underlying type should later output the elements in a LIFO manner, meaning that if `A` is
/// added first, and then `B` is added, `B` should be rendered first / "below" `A`, and `A` should
/// be rendered on top.
pub trait PushRenderElement<E, R>
where
    R: NiriRenderer,
{
    /// Add a new element to this [`PushRenderElement`].
    ///
    /// Note that elements
    fn push_element<T>(&mut self, element: T)
    where
        T: Into<E>;

    /// Create a new [`PushRenderElement`] that accepts any `T` that implement `Into<E>`.
    ///
    /// This is useful in nested hierarchies, where render elements are built from enums within
    /// enums, so one can simply pass a new [`PushRenderElement`] derived from this function
    /// instead of manually having to e.g. define a closure that performs the conversion.
    ///
    /// For example usage, have a look at `src/layout/tile.rs`.
    fn as_child<T>(&mut self) -> impl PushRenderElement<T, R>
    where
        T: Into<E>,
    {
        |elem: T| self.push_element(elem.into())
    }
}

impl<F, E, R> PushRenderElement<E, R> for F
where
    F: FnMut(E),
    R: NiriRenderer,
{
    fn push_element<T>(&mut self, element: T)
    where
        T: Into<E>,
    {
        (self)(element.into())
    }
}

impl<E, R> PushRenderElement<E, R> for Vec<E>
where
    R: NiriRenderer,
{
    fn push_element<T>(&mut self, element: T)
    where
        T: Into<E>,
    {
        self.push(element.into());
    }
}
