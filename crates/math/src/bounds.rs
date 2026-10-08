//! Axis-aligned bounding boxes.

use crate::{Vec3, vec3};

/// An axis-aligned bounding box.
///
/// An empty box is represented by `min > max` on at least one axis; use
/// [`Aabb::is_empty`] to test for that.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    /// The minimum corner.
    pub min: Vec3,
    /// The maximum corner.
    pub max: Vec3,
}

impl Aabb {
    /// A bounding box from its min and max corners. The corners are ordered
    /// component-wise so `min <= max` always holds.
    pub fn new(a: Vec3, b: Vec3) -> Self {
        Self {
            min: vec3(a.x.min(b.x), a.y.min(b.y), a.z.min(b.z)),
            max: vec3(a.x.max(b.x), a.y.max(b.y), a.z.max(b.z)),
        }
    }

    /// A box from a center and per-axis half extents.
    pub fn from_center_half_extents(center: Vec3, half_extents: Vec3) -> Self {
        Self {
            min: center - half_extents,
            max: center + half_extents,
        }
    }

    /// The tightest box containing all points. Returns `None` for an empty
    /// slice.
    pub fn from_points(points: &[Vec3]) -> Option<Self> {
        let mut iter = points.iter().copied();
        let first = iter.next()?;
        let mut box_ = Self {
            min: first,
            max: first,
        };
        for p in iter {
            box_.expand_point(p);
        }
        Some(box_)
    }

    /// `true` if the box is empty (inverted on some axis).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y || self.min.z > self.max.z
    }

    /// The center of the box.
    #[inline]
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    /// The size (max - min) of the box.
    #[inline]
    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }

    /// The per-axis half extents.
    #[inline]
    pub fn half_extents(&self) -> Vec3 {
        self.size() * 0.5
    }

    /// The eight corners of the box.
    pub fn corners(&self) -> [Vec3; 8] {
        [
            vec3(self.min.x, self.min.y, self.min.z),
            vec3(self.max.x, self.min.y, self.min.z),
            vec3(self.min.x, self.max.y, self.min.z),
            vec3(self.max.x, self.max.y, self.min.z),
            vec3(self.min.x, self.min.y, self.max.z),
            vec3(self.max.x, self.min.y, self.max.z),
            vec3(self.min.x, self.max.y, self.max.z),
            vec3(self.max.x, self.max.y, self.max.z),
        ]
    }

    /// `true` if the point lies within the box (inclusive).
    #[inline]
    pub fn contains_point(&self, p: Vec3) -> bool {
        p.x >= self.min.x
            && p.x <= self.max.x
            && p.y >= self.min.y
            && p.y <= self.max.y
            && p.z >= self.min.z
            && p.z <= self.max.z
    }

    /// `true` if the two boxes overlap (inclusive).
    #[inline]
    pub fn intersects(&self, other: &Aabb) -> bool {
        self.min.x <= other.max.x
            && self.max.x >= other.min.x
            && self.min.y <= other.max.y
            && self.max.y >= other.min.y
            && self.min.z <= other.max.z
            && self.max.z >= other.min.z
    }

    /// Grow the box to include a point.
    #[inline]
    pub fn expand_point(&mut self, p: Vec3) {
        self.min.x = self.min.x.min(p.x);
        self.min.y = self.min.y.min(p.y);
        self.min.z = self.min.z.min(p.z);
        self.max.x = self.max.x.max(p.x);
        self.max.y = self.max.y.max(p.y);
        self.max.z = self.max.z.max(p.z);
    }

    /// Grow the box to include another box.
    #[inline]
    pub fn expand_box(&mut self, other: &Aabb) {
        self.expand_point(other.min);
        self.expand_point(other.max);
    }

    /// The union of two boxes.
    pub fn union(&self, other: &Aabb) -> Aabb {
        let mut out = *self;
        out.expand_box(other);
        out
    }

    /// Grow the box outward by `amount` on every axis.
    pub fn expanded(&self, amount: f32) -> Aabb {
        let d = vec3(amount, amount, amount);
        Aabb {
            min: self.min - d,
            max: self.max + d,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vec3_approx_eq;

    #[test]
    fn new_orders_corners() {
        let b = Aabb::new(vec3(1.0, 2.0, 3.0), vec3(-1.0, -2.0, -3.0));
        assert!(vec3_approx_eq(b.min, vec3(-1.0, -2.0, -3.0), 1e-6));
        assert!(vec3_approx_eq(b.max, vec3(1.0, 2.0, 3.0), 1e-6));
    }

    #[test]
    fn center_size_half_extents() {
        let b = Aabb::new(vec3(-1.0, -1.0, -1.0), vec3(1.0, 3.0, 5.0));
        assert!(vec3_approx_eq(b.center(), vec3(0.0, 1.0, 2.0), 1e-6));
        assert!(vec3_approx_eq(b.size(), vec3(2.0, 4.0, 6.0), 1e-6));
        assert!(vec3_approx_eq(b.half_extents(), vec3(1.0, 2.0, 3.0), 1e-6));
    }

    #[test]
    fn contains_point_inclusive() {
        let b = Aabb::new(vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0));
        assert!(b.contains_point(vec3(0.5, 0.5, 0.5)));
        assert!(b.contains_point(vec3(0.0, 0.0, 0.0)));
        assert!(b.contains_point(vec3(1.0, 1.0, 1.0)));
        assert!(!b.contains_point(vec3(1.1, 0.5, 0.5)));
    }

    #[test]
    fn intersects_and_union() {
        let a = Aabb::new(vec3(0.0, 0.0, 0.0), vec3(2.0, 2.0, 2.0));
        let b = Aabb::new(vec3(1.0, 1.0, 1.0), vec3(3.0, 3.0, 3.0));
        let c = Aabb::new(vec3(5.0, 5.0, 5.0), vec3(6.0, 6.0, 6.0));
        assert!(a.intersects(&b));
        assert!(!a.intersects(&c));
        let u = a.union(&c);
        assert!(vec3_approx_eq(u.min, vec3(0.0, 0.0, 0.0), 1e-6));
        assert!(vec3_approx_eq(u.max, vec3(6.0, 6.0, 6.0), 1e-6));
    }

    #[test]
    fn from_points_is_tight() {
        let pts = [
            vec3(1.0, 2.0, 3.0),
            vec3(-4.0, 0.0, 2.0),
            vec3(0.0, 5.0, -1.0),
        ];
        let b = Aabb::from_points(&pts).unwrap();
        assert!(vec3_approx_eq(b.min, vec3(-4.0, 0.0, -1.0), 1e-6));
        assert!(vec3_approx_eq(b.max, vec3(1.0, 5.0, 3.0), 1e-6));
        assert!(Aabb::from_points(&[]).is_none());
    }

    #[test]
    fn from_center_half_extents_round_trips() {
        let b = Aabb::from_center_half_extents(vec3(1.0, 2.0, 3.0), vec3(0.5, 1.0, 2.0));
        assert!(vec3_approx_eq(b.center(), vec3(1.0, 2.0, 3.0), 1e-6));
        assert!(vec3_approx_eq(b.half_extents(), vec3(0.5, 1.0, 2.0), 1e-6));
    }

    #[test]
    fn corners_count_and_bounds() {
        let b = Aabb::new(vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0));
        let corners = b.corners();
        assert_eq!(corners.len(), 8);
        for c in corners {
            assert!(b.contains_point(c));
        }
    }
}
