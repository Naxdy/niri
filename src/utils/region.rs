use std::ops::{Add, AddAssign, Sub};

use smithay::{
    utils::{Coordinate, Logical, Point, Rectangle},
    wayland::compositor::{RectangleKind, RegionAttributes},
};

/// A region described using a list of additive [`Rectangle`]s.
#[derive(Debug, Clone)]
pub struct Region<N, Kind>
where
    N: Coordinate + Default + PartialOrd + Add + Sub + Copy + AddAssign,
{
    rects: RegionInner<N, Kind>,
}

impl<N, Kind> Region<N, Kind>
where
    N: Coordinate + Default + PartialOrd + Add + Sub + Copy + AddAssign,
{
    pub fn new() -> Self {
        Self {
            rects: RegionInner::Empty,
        }
    }

    pub fn with_offset(&self, point: Point<N, Kind>) -> Self {
        let mut this = Self {
            rects: self
                .rects
                .iter()
                .map(|mut r| {
                    r.loc.x += point.x;
                    r.loc.y += point.y;
                    r
                })
                .collect(),
        };

        this.rects.sort_rects();

        this
    }

    pub fn rects(&self) -> impl Iterator<Item = Rectangle<N, Kind>> {
        self.rects.iter()
    }

    pub fn subtract_rect(&mut self, rect: Rectangle<N, Kind>) {
        self.rects = Rectangle::subtract_rects_many(self.rects.iter(), std::iter::once(rect))
            .into_iter()
            .collect();

        self.rects.sort_rects();
    }

    pub fn add_rect(&mut self, rect: Rectangle<N, Kind>) {
        if !self.rects.iter().any(|r| r.intersection(rect).is_some()) {
            // nothing intersects, so we can just add this rectangle as-is
            self.rects.push(rect);
        } else if self.rects.iter().any(|r| r.contains_rect(rect)) {
            // we are not adding any new rects
            return;
        } else {
            // something intersects, so we only add the new portions to our list
            self.rects.append(rect.subtract_rects(self.rects.iter()));
        }

        self.rects.sort_rects();
    }

    pub fn from_rects(rects: impl IntoIterator<Item = Rectangle<N, Kind>>) -> Self {
        let mut this = Self {
            rects: rects.into_iter().collect(),
        };

        this.rects.sort_rects();

        this
    }

    pub fn len(&self) -> usize {
        self.rects.len()
    }

    pub fn is_empty(&self) -> bool {
        matches!(self.rects, RegionInner::Empty)
    }
}

impl Region<i32, Logical> {
    pub fn from_region_attributes(value: RegionAttributes) -> Self {
        value.rects.into_iter().fold(
            Self {
                rects: RegionInner::new(),
            },
            |mut acc, (kind, rect)| {
                match kind {
                    RectangleKind::Add => acc.add_rect(rect),
                    RectangleKind::Subtract => acc.subtract_rect(rect),
                }
                acc
            },
        )
    }
}

impl From<RegionAttributes> for Region<i32, Logical> {
    fn from(value: RegionAttributes) -> Self {
        Self::from_region_attributes(value)
    }
}

impl<N, Kind> FromIterator<Rectangle<N, Kind>> for Region<N, Kind>
where
    N: Coordinate + Default + PartialOrd + Add + Sub + Copy + AddAssign,
{
    fn from_iter<T: IntoIterator<Item = Rectangle<N, Kind>>>(iter: T) -> Self {
        iter.into_iter().fold(Self::new(), |mut acc, curr| {
            acc.add_rect(curr);
            acc
        })
    }
}

#[derive(Debug, Clone)]
enum RegionInner<N, Kind>
where
    N: Coordinate + Default + PartialOrd + Add + Sub + Copy + AddAssign,
{
    Empty,
    Single(Rectangle<N, Kind>),
    Multiple(Vec<Rectangle<N, Kind>>),
}

impl<N, Kind> RegionInner<N, Kind>
where
    N: Coordinate + Default + PartialOrd + Add + Sub + Copy + AddAssign,
{
    fn new() -> Self {
        Self::Empty
    }

    fn iter(&self) -> RegionIterator<'_, N, Kind> {
        RegionIterator {
            inner: self,
            idx: 0,
        }
    }

    fn sort_rects(&mut self) {
        if let Self::Multiple(rects) = self {
            rects.sort_by(|a, b| {
                a.loc.x.partial_cmp(&b.loc.x).unwrap_or(
                    a.loc
                        .y
                        .partial_cmp(&b.loc.y)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
            });
        }
    }

    fn push(&mut self, rect: Rectangle<N, Kind>) {
        match self {
            Self::Empty => {
                *self = Self::Single(rect);
            }
            Self::Single(rectangle) => {
                *self = Self::Multiple(vec![*rectangle, rect]);
            }
            Self::Multiple(rectangles) => {
                rectangles.push(rect);
            }
        }
    }

    fn append(&mut self, rects: impl IntoIterator<Item = Rectangle<N, Kind>>) {
        rects.into_iter().for_each(|rect| self.push(rect));
    }

    fn len(&self) -> usize {
        match self {
            RegionInner::Empty => 0,
            RegionInner::Single(_) => 1,
            RegionInner::Multiple(rectangles) => rectangles.len(),
        }
    }
}

impl<N, Kind> FromIterator<Rectangle<N, Kind>> for RegionInner<N, Kind>
where
    N: Coordinate + Default + PartialOrd + Add + Sub + Copy + AddAssign,
{
    fn from_iter<T: IntoIterator<Item = Rectangle<N, Kind>>>(iter: T) -> Self {
        let mut iter = iter.into_iter();

        let Some(first) = iter.next() else {
            return Self::Empty;
        };

        if let Some(second) = iter.next() {
            let mut v = vec![first, second];
            v.append(&mut iter.collect());
            Self::Multiple(v)
        } else {
            Self::Single(first)
        }
    }
}

pub struct RegionIterator<'a, N, Kind>
where
    N: Coordinate + Default + PartialOrd + Add + Sub + Copy + AddAssign,
{
    inner: &'a RegionInner<N, Kind>,
    idx: usize,
}

impl<'a, N, Kind> Iterator for RegionIterator<'a, N, Kind>
where
    N: Coordinate + Default + PartialOrd + Add + Sub + Copy + AddAssign,
{
    type Item = Rectangle<N, Kind>;

    fn next(&mut self) -> Option<Self::Item> {
        let item = match self.inner {
            RegionInner::Empty => return None,
            RegionInner::Single(rectangle) => {
                if self.idx > 0 {
                    return None;
                }

                Some(rectangle)
            }
            RegionInner::Multiple(rectangles) => rectangles.get(self.idx),
        };

        self.idx += 1;

        item.copied()
    }
}
