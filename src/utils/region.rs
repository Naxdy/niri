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
    rects: Vec<Rectangle<N, Kind>>,
}

impl<N, Kind> Region<N, Kind>
where
    N: Coordinate + Default + PartialOrd + Add + Sub + Copy + AddAssign,
{
    pub fn new() -> Self {
        Self { rects: Vec::new() }
    }

    pub fn with_offset(&self, point: Point<N, Kind>) -> Self {
        Self {
            rects: self
                .rects
                .iter()
                .copied()
                .map(|mut r| {
                    r.loc.x += point.x;
                    r.loc.y += point.y;
                    r
                })
                .collect(),
        }
    }

    pub fn rects(&self) -> &[Rectangle<N, Kind>] {
        &self.rects
    }

    pub fn subtract_rect(&mut self, rect: Rectangle<N, Kind>) {
        self.rects =
            Rectangle::subtract_rects_many(self.rects.iter().copied(), std::iter::once(rect));
    }

    pub fn add_rect(&mut self, rect: Rectangle<N, Kind>) {
        if !self.rects.iter().any(|r| r.intersection(rect).is_some()) {
            // nothing intersects, so we can just add this rectangle as-is
            self.rects.push(rect);
        } else if self.rects.iter().any(|r| r.contains_rect(rect)) {
            // we are not adding any new rects
        } else {
            // something intersects, so we only add the new portions to our list
            self.rects
                .append(&mut rect.subtract_rects(self.rects.clone()));
        }
    }

    pub fn from_rects(rects: impl IntoIterator<Item = Rectangle<N, Kind>>) -> Self {
        Self {
            rects: rects.into_iter().collect(),
        }
    }
}

impl Region<i32, Logical> {
    pub fn from_region_attributes(value: RegionAttributes) -> Self {
        let len = value.rects.len();

        value.rects.into_iter().fold(
            Self {
                rects: Vec::with_capacity(len),
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
