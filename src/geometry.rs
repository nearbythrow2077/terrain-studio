use crate::math::Vec3;
use crate::objects::{Object, Shape};
use crate::scene::Terrain;

#[derive(Clone, Copy)]
pub struct Ray {
    pub origin: Vec3,
    pub direction: Vec3,
}

impl Ray {
    pub fn at(self, t: f32) -> Vec3 {
        self.origin + self.direction * t
    }
}

pub const EPSILON: f32 = 0.00015;

#[derive(Clone, Copy)]
pub struct Hit {
    pub distance: f32,
    pub normal: Vec3,
    pub local: Vec3,
}

pub struct Placed<'a> {
    pub object: &'a Object,
    center: Vec3,
    basis: [Vec3; 3],
    scale: Vec3,
    radius_squared: f32,
    inverse: Option<[Vec3; 3]>,
    terrain: Option<Heightfield<'a>>,
}

impl<'a> Placed<'a> {
    pub fn new(object: &'a Object) -> Self {
        let [x, y, z] = object.rotation.map(f32::to_radians);
        let ((sx, cx), (sy, cy), (sz, cz)) = (x.sin_cos(), y.sin_cos(), z.sin_cos());
        let rotate = |Vec3(x, y, z)| {
            let (y, z) = (cx * y - sx * z, sx * y + cx * z);
            let (x, z) = (cy * x + sy * z, -sy * x + cy * z);
            Vec3(cz * x - sz * y, sz * x + cz * y, z)
        };
        let [x, y, z] = object.position;
        let [a, b, c] = object.scale;
        let basis = [
            rotate(Vec3(1.0, 0.0, 0.0)),
            rotate(Vec3(0.0, 1.0, 0.0)),
            rotate(Vec3(0.0, 0.0, 1.0)),
        ];
        let mut radius_squared = a * a + b * b + c * c;
        let inverse = object.model_basis.map(|cols| {
            let [x, y, z] = cols.map(Vec3::from_array);
            let [x, y, z] = [rotate(x) * a, rotate(y) * b, rotate(z) * c];
            radius_squared = (x.length() + y.length() + z.length()).powi(2);
            let determinant = x.dot(y.cross(z));
            [
                y.cross(z) / determinant,
                z.cross(x) / determinant,
                x.cross(y) / determinant,
            ]
        });
        Self {
            object,
            center: Vec3(x, y, z),
            basis,
            scale: Vec3(a, b, c),
            radius_squared,
            inverse,
            terrain: object
                .heightmap
                .as_ref()
                .map(|map| Heightfield::new(map, 1.0)),
        }
    }

    pub fn hit(&self, ray: Ray, end: f32) -> Option<Hit> {
        let relative = ray.origin - self.center;
        let t = (-relative.dot(ray.direction)).clamp(0.0, end);
        let nearest = relative + ray.direction * t;
        if nearest.dot(nearest) > self.radius_squared {
            return None;
        }
        let local = |p: Vec3| {
            if let Some([x, y, z]) = self.inverse {
                return Vec3(p.dot(x), p.dot(y), p.dot(z));
            }
            Vec3(
                p.dot(self.basis[0]) / self.scale.0,
                p.dot(self.basis[1]) / self.scale.1,
                p.dot(self.basis[2]) / self.scale.2,
            )
        };
        let local_ray = Ray {
            origin: local(relative),
            direction: local(ray.direction),
        };
        let (distance, normal) = match self.object.shape {
            Shape::Sphere => {
                let (a, b) = ball(local_ray, 1.0)?;
                let t = if a > EPSILON { a } else { b };
                (t, local_ray.at(t))
            }
            Shape::Box | Shape::Image => convex(
                local_ray,
                &[
                    (Vec3(1., 0., 0.), 1.),
                    (Vec3(-1., 0., 0.), 1.),
                    (Vec3(0., 1., 0.), 1.),
                    (Vec3(0., -1., 0.), 1.),
                    (Vec3(0., 0., 1.), 1.),
                    (Vec3(0., 0., -1.), 1.),
                ],
            )?,
            Shape::Pyramid => convex(
                local_ray,
                &[
                    (Vec3(1., 0.5, 0.), 0.5),
                    (Vec3(-1., 0.5, 0.), 0.5),
                    (Vec3(0., 0.5, 1.), 0.5),
                    (Vec3(0., 0.5, -1.), 0.5),
                    (Vec3(0., -1., 0.), 1.),
                ],
            )?,
            Shape::Cone => round_sides(local_ray, true, end)?,
            Shape::Cylinder => round_sides(local_ray, false, end)?,
            Shape::Torus => torus(local_ray, end)?,
            Shape::Terrain => {
                let hit = self.terrain.as_ref()?.hit(local_ray, end)?;
                (hit.distance, hit.normal)
            }
            Shape::Mesh => {
                let hit =
                    self.object
                        .mesh
                        .as_ref()?
                        .hit(local_ray, end, self.object.smooth_mesh)?;
                (hit.distance, hit.normal)
            }
        };
        if !(EPSILON..end).contains(&distance) {
            return None;
        }
        let world_normal = if let Some([x, y, z]) = self.inverse {
            x * normal.0 + y * normal.1 + z * normal.2
        } else {
            self.basis[0] * (normal.0 / self.scale.0)
                + self.basis[1] * (normal.1 / self.scale.1)
                + self.basis[2] * (normal.2 / self.scale.2)
        };
        Some(Hit {
            distance,
            normal: world_normal.normalized(),
            local: local_ray.at(distance),
        })
    }
}

fn roots(a: f32, b: f32, c: f32) -> Option<(f32, f32)> {
    if a.abs() < 1e-8 {
        let t = -c / b;
        return t.is_finite().then_some((t, t));
    }
    let determinant = b.mul_add(b, -4.0 * a * c);
    if determinant < 0.0 {
        return None;
    }
    let q = -0.5 * (b + determinant.sqrt().copysign(b));
    let (t, u) = if q == 0.0 { (0.0, 0.0) } else { (q / a, c / q) };
    Some((t.min(u), t.max(u)))
}

fn ball(ray: Ray, radius: f32) -> Option<(f32, f32)> {
    roots(
        ray.direction.dot(ray.direction),
        2.0 * ray.direction.dot(ray.origin),
        ray.origin.dot(ray.origin) - radius * radius,
    )
}

fn convex(ray: Ray, planes: &[(Vec3, f32)]) -> Option<(f32, Vec3)> {
    let mut near = (f32::NEG_INFINITY, Vec3::default());
    let mut far = (f32::INFINITY, Vec3::default());
    for &(normal, bound) in planes {
        let distance = bound - normal.dot(ray.origin);
        let speed = normal.dot(ray.direction);
        if speed.abs() < 1e-8 {
            if distance < 0.0 {
                return None;
            }
            continue;
        }
        let t = distance / speed;
        if speed < 0.0 && t > near.0 {
            near = (t, normal);
        }
        if speed > 0.0 && t < far.0 {
            far = (t, normal);
        }
        if near.0 > far.0 {
            return None;
        }
    }
    Some(if near.0 > EPSILON { near } else { far })
}

fn round_sides(ray: Ray, cone: bool, end: f32) -> Option<(f32, Vec3)> {
    let Vec3(x, y, z) = ray.origin;
    let Vec3(u, v, w) = ray.direction;
    let (r, dr) = if cone {
        ((1.0 - y) * 0.5, -v * 0.5)
    } else {
        (1.0, 0.0)
    };
    let intersections = roots(
        u * u + w * w - dr * dr,
        2.0 * (x * u + z * w - r * dr),
        x * x + z * z - r * r,
    );
    let mut closest = (end, Vec3::default());
    if let Some((a, b)) = intersections {
        for t in [a, b] {
            let p = ray.at(t);
            if (EPSILON..closest.0).contains(&t) && (-1.0..=1.0).contains(&p.1) {
                let ny = if cone { (1.0 - p.1) * 0.25 } else { 0.0 };
                closest = (t, Vec3(p.0, ny, p.2));
            }
        }
    }
    for h in if cone { &[-1.0][..] } else { &[-1.0, 1.0] } {
        let t = (h - y) / v;
        let p = ray.at(t);
        if (EPSILON..closest.0).contains(&t) && p.0 * p.0 + p.2 * p.2 <= 1.0 {
            closest = (t, Vec3(0.0, *h, 0.0));
        }
    }
    (closest.0 < end).then_some(closest)
}

fn torus(ray: Ray, end: f32) -> Option<(f32, Vec3)> {
    let (start, stop) = ball(ray, 1.0)?;
    let mut t = start.max(EPSILON);
    let limit = stop.min(end);
    let inverse_speed = ray.direction.dot(ray.direction).sqrt().recip();
    for _ in 0..70 {
        if t >= limit {
            break;
        }
        let p = ray.at(t);
        let radial = p.0.hypot(p.2);
        let d = (radial - 0.72).hypot(p.1) - 0.27;
        if d.abs() < 0.00025 && t > EPSILON {
            let inner = 1.0 - 0.72 / radial.max(0.0001);
            return Some((t, Vec3(p.0 * inner, p.1, p.2 * inner)));
        }
        t += (d.abs() * inverse_speed).max(0.00005);
    }
    None
}

pub struct Heightfield<'a> {
    terrain: &'a Terrain,
    relief: f32,
    step_x: f32,
    step_z: f32,
    heights: Vec<Cell>,
    normals: Vec<Vec3>,
}

struct Cell {
    low: f32,
    high: f32,
    triangles: [(f32, f32, f32); 2],
}

impl<'a> Heightfield<'a> {
    pub fn new(terrain: &'a Terrain, relief: f32) -> Self {
        let (width, depth) = (terrain.width, terrain.depth);
        let (step_x, step_z) = (2.0 / (width - 1) as f32, 2.0 / (depth - 1) as f32);
        let h = &terrain.heights;
        let mut heights = Vec::with_capacity((width - 1) * (depth - 1));
        let mut normals = Vec::with_capacity(width * depth);
        for z in 0..depth {
            for x in 0..width {
                let (left, right) = (x.saturating_sub(1), (x + 1).min(width - 1));
                let (top, bottom) = (z.saturating_sub(1), (z + 1).min(depth - 1));
                let dx = (h[z * width + right] - h[z * width + left]) * relief
                    / ((right - left) as f32 * step_x);
                let dz = (h[bottom * width + x] - h[top * width + x]) * relief
                    / ((bottom - top) as f32 * step_z);
                normals.push(Vec3(-dx, 1.0, -dz));

                if x < width - 1 && z < depth - 1 {
                    let i = z * width + x;
                    let [a, b, c, d] = [h[i], h[i + 1], h[i + width], h[i + width + 1]];
                    let triangles =
                        [(a, b - a, c - a), (b + c - d, d - c, d - b)].map(|(base, hs, ht)| {
                            (base * relief, hs * relief / step_x, ht * relief / step_z)
                        });
                    heights.push(Cell {
                        low: a.min(b).min(c).min(d) * relief,
                        high: a.max(b).max(c).max(d) * relief,
                        triangles,
                    });
                }
            }
        }
        Self {
            terrain,
            relief,
            step_x,
            step_z,
            heights,
            normals,
        }
    }

    pub fn hit(&self, ray: Ray, limit: f32) -> Option<Hit> {
        let mut enter = EPSILON;
        let mut exit = limit;
        for (origin, direction, min, max) in [
            (ray.origin.0, ray.direction.0, -1., 1.),
            (ray.origin.1, ray.direction.1, 0., self.relief),
            (ray.origin.2, ray.direction.2, -1., 1.),
        ] {
            if direction.abs() < 1e-8 {
                if origin < min || origin > max {
                    return None;
                }
                continue;
            }
            let a = (min - origin) / direction;
            let b = (max - origin) / direction;
            enter = enter.max(a.min(b));
            exit = exit.min(a.max(b));
        }
        if enter >= exit {
            return None;
        }
        let p = ray.at(enter);
        let mut x = (((p.0 + 1.0) / self.step_x) as i32).clamp(0, self.terrain.width as i32 - 2);
        let mut z = (((p.2 + 1.0) / self.step_z) as i32).clamp(0, self.terrain.depth as i32 - 2);
        let mut along_x = GridAxis::new(x, self.step_x, ray.origin.0, ray.direction.0);
        let mut along_z = GridAxis::new(z, self.step_z, ray.origin.2, ray.direction.2);
        while x >= 0
            && z >= 0
            && x < self.terrain.width as i32 - 1
            && z < self.terrain.depth as i32 - 1
        {
            let end = along_x.next.min(along_z.next).min(exit);
            if let Some(hit) = self.cell(ray, x as usize, z as usize, enter, end) {
                return Some(hit);
            }
            if end >= exit {
                break;
            }
            if along_x.next <= along_z.next {
                x += along_x.step;
                along_x.next += along_x.interval;
            } else {
                z += along_z.step;
                along_z.next += along_z.interval;
            }
            enter = end;
        }
        None
    }

    fn cell(&self, ray: Ray, x: usize, z: usize, start: f32, end: f32) -> Option<Hit> {
        let cell = &self.heights[z * (self.terrain.width - 1) + x];
        let yl = ray.at(start).1;
        let yr = ray.at(end).1;
        if yl.min(yr) > cell.high || yl.max(yr) < cell.low {
            return None;
        }

        let x0 = x as f32 * self.step_x - 1.0;
        let z0 = z as f32 * self.step_z - 1.0;
        let mut best = None;
        for (index, &(height, dx, dz)) in cell.triangles.iter().enumerate() {
            let t = (height + dx * (ray.origin.0 - x0) + dz * (ray.origin.2 - z0) - ray.origin.1)
                / (ray.direction.1 - dx * ray.direction.0 - dz * ray.direction.2);
            if t > EPSILON
                && t >= start - 0.00003
                && t <= end + 0.00003
                && best.is_none_or(|hit: Hit| t < hit.distance)
            {
                let p = ray.at(t);
                let diagonal = (p.0 - x0) / self.step_x + (p.2 - z0) / self.step_z;
                if (index == 0 && diagonal <= 1.0002) || (index == 1 && diagonal >= 0.9998) {
                    best = Some(Hit {
                        distance: t,
                        normal: self.normal(x, z, p),
                        local: p,
                    });
                }
            }
        }
        best
    }
    fn normal(&self, x: usize, z: usize, p: Vec3) -> Vec3 {
        let width = self.terrain.width;
        let vertex = |x: usize, z: usize| self.normals[z * width + x];
        let u = ((p.0 + 1.0) / self.step_x - x as f32).clamp(0.0, 1.0);
        let v = ((p.2 + 1.0) / self.step_z - z as f32).clamp(0.0, 1.0);
        vertex(x, z)
            .mix(vertex(x + 1, z), u)
            .mix(vertex(x, z + 1).mix(vertex(x + 1, z + 1), u), v)
            .normalized()
    }
}

struct GridAxis {
    next: f32,
    interval: f32,
    step: i32,
}

impl GridAxis {
    fn new(cell: i32, size: f32, origin: f32, direction: f32) -> Self {
        if direction.abs() < 1e-8 {
            return Self {
                next: f32::INFINITY,
                interval: f32::INFINITY,
                step: 0,
            };
        }
        let positive = direction > 0.0;
        let edge = (cell + i32::from(positive)) as f32 * size - 1.0;
        Self {
            next: (edge - origin) / direction,
            interval: size / direction.abs(),
            step: if positive { 1 } else { -1 },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heightfield_hits_both_triangles_with_front_facing_normals() {
        let terrain = Terrain {
            width: 2,
            depth: 2,
            heights: vec![0.25, 0.5, 0.5, 0.75],
        };
        let field = Heightfield::new(&terrain, 1.0);
        for (axis, expected) in [(-0.5, 0.375), (0.5, 0.625)] {
            let ray = Ray {
                origin: Vec3(axis, 2.0, axis),
                direction: Vec3(0.0, -1.0, 0.0),
            };
            let hit = field.hit(ray, 10.0).unwrap();
            assert!((ray.at(hit.distance).1 - expected).abs() < 1e-5);
            assert!(hit.normal.1 > 0.9);
        }
    }

    #[test]
    fn affine_terrain_intersects_at_the_saved_world_location() {
        let mut object = Object::new(Shape::Terrain).at([0.0; 3], [1.0; 3]);
        object.heightmap = Some(std::sync::Arc::new(Terrain {
            width: 2,
            depth: 2,
            heights: vec![0.5; 4],
        }));
        object.model_basis = Some([[2.0, 1.0, 0.0], [0.0, 3.0, 0.0], [0.0, 0.0, -1.0]]);
        let ray = Ray {
            origin: Vec3(1.0, 10.0, 0.0),
            direction: Vec3(0.0, -1.0, 0.0),
        };
        let hit = Placed::new(&object).hit(ray, 20.0).unwrap();
        assert!((hit.distance - 8.0).abs() < 1e-5);
        assert!((hit.local.1 - 0.5).abs() < 1e-5);
        assert!(hit.normal.dot(Vec3(-0.5, 1.0, 0.0).normalized()) > 0.99999);
    }

    #[test]
    fn torus_preserves_hole_and_scaled_box_distance_is_in_world_units() {
        let ring = Object::new(Shape::Torus).at([0.0; 3], [1.0; 3]);
        let ray = Ray {
            origin: Vec3(0.0, 2.0, 0.0),
            direction: Vec3(0.0, -1.0, 0.0),
        };
        assert!(Placed::new(&ring).hit(ray, 10.0).is_none());
        let cuboid = Object::new(Shape::Box).at([0.0; 3], [0.5, 0.25, 0.5]);
        let hit = Placed::new(&cuboid).hit(ray, 10.0).unwrap();
        assert!((hit.distance - 1.75).abs() < 1e-5);
    }
}
